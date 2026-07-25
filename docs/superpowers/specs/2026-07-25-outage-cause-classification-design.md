# Outage cause classification — design

## Why

`crates/store/src/diagnostics.rs` (shipped 2026-07-24) already has a
sample-density heuristic (`classify_real_outage`) that tells a real internet
outage apart from a sleep/shutdown gap — but it's only used to compute the
Diagnostics tab's "frequent disconnects" flag. Every other surface still
treats every row in `outages` as a real drop:

- The Overview tab's "Disconnects" stat (`reliability_since`'s `disconnects`
  field) counts every outage unconditionally.
- The Incident Log (History tab) lists every outage with no distinction —
  a multi-hour "outage" that was actually the laptop being asleep or off
  reads exactly like a real ISP failure.
- The Event Timeline logs a `critical`-severity "disconnect" event the
  instant an outage opens, before there's any way to know if it's real.
- The route-instability check counts every `route_change` event, including
  the one a reboot/wake almost always fires on its own.

This work makes the classification a first-class, persisted fact about each
outage — computed once, at close time — instead of a Diagnostics-tab-only
side calculation, and uses it everywhere downtime is counted or displayed.

## Scope

1. Persist REAL vs. GAP classification into the existing `outages.cause`
   column at outage-close time (no migration — the column already exists).
2. `reliability_since`'s `disconnects` count and the Diagnostics tab's
   "frequent disconnects" check both read the persisted cause instead of
   (in the diagnostics case) recomputing it per call.
3. The Event Timeline's `disconnect`/`reconnect` events move from
   open-time to close-time, so severity can reflect the real classification.
4. The route-instability check excludes `route_change` events that fall
   within a buffer window around a GAP-classified outage.
5. The Incident Log UI shows GAP outages distinctly rather than hiding or
   miscounting them.
6. A one-time reset of the `outages` and `events` tables (both the local
   dev DB and the live daemon's production DB), since we're not writing a
   historical backfill — going forward, the corrected logic captures
   everything from a clean slate.

Explicitly out of scope: distinguishing *why* a gap happened (suspend vs.
shutdown vs. crash vs. service restart) — all of these produce the same
"no samples during this span" signature and get one shared label. Also out
of scope: a historical backfill/reclassification pass over existing rows
(superseded by the reset in point 6) and any new settings/configurability
for the thresholds already established in the prior spec.

## Backend (`crates/store`)

### Constants

Two shared string constants (in `crates/store/src/lib.rs`, alongside the
`Outage` type they classify):

```rust
pub const OUTAGE_CAUSE_REAL: &str = "all internet targets unreachable";
pub const OUTAGE_CAUSE_GAP: &str = "device was asleep/off";
```

`OUTAGE_CAUSE_REAL` is the same string `open_outage` already writes today —
choosing it keeps any outage that closes before this ships correctly
classified as real by default (moot in practice once the tables are reset,
but keeps the constant meaningful).

### Close-time classification

New method in `crates/store/src/diagnostics.rs` (co-located with
`classify_real_outage`/`sample_count_between`, which it reuses):

```rust
pub async fn close_outage_classified(
    &self,
    id: i64,
    ts_start: i64,
    ts_end: i64,
    reconnect_ms: Option<i64>,
) -> crate::Result<()>
```

Computes `actual = self.sample_count_between(ts_start, ts_end).await?`,
`duration_ms = ts_end - ts_start`, classifies via `classify_real_outage`,
and runs one `UPDATE outages SET ts_end = ?, duration_ms = ? - ts_start,
reconnect_ms = ?, cause = ? WHERE id = ?`. The existing unclassified
`close_outage` stays as-is (used directly by existing tests as a raw
fixture-building primitive).

### `crates/monitor` wiring

`update_outage` (`crates/monitor/src/lib.rs`) changes in two ways:

1. `open_outage`'s cause argument becomes `Some(tracium_store::OUTAGE_CAUSE_REAL)`
   (an explicit placeholder — corrected at close) instead of the inline
   string literal it uses today.
2. The close branch calls `close_outage_classified(o.id, o.ts_start, now,
   Some(reconnect))` instead of `close_outage(o.id, now, Some(reconnect))`.
3. The `insert_event(now, "disconnect", "critical", ...)` call currently in
   the *open* branch is removed. Both the `"disconnect"` and `"reconnect"`
   events are written in the *close* branch instead, once classification is
   known:
   - `disconnect`: `ts = o.ts_start`, severity `"critical"` if REAL, `"info"`
     if GAP.
   - `reconnect`: `ts = now`, severity stays `"info"` either way (unchanged
     from today), `duration_ms = reconnect`.

   Practical effect: a Timeline entry for an outage now appears once it
   resolves, not the instant it's detected. The live "Online/Offline" header
   pill is unaffected — that reads `StatusUpdate.online`, computed fresh
   every cycle, independent of the `events`/`outages` tables. An outage that
   never closes (app quits while still down) simply produces no Timeline
   entry for it, matching how it already produces no *closed* `outages` row
   either.

### `reliability_since` (`crates/store/src/lib.rs`)

The `disconnects` query changes from
`SELECT count(*) FROM outages WHERE ts_start >= ?` to:

```sql
SELECT count(*) FROM outages
WHERE ts_start >= ? AND (cause IS NULL OR cause <> ?)
```

bound to `OUTAGE_CAUSE_GAP`. This is the one function behind every reliability
window — the Overview tab's 24h stat and the CLI's `traciumd report --window
7d`/`30d` — so the fix applies everywhere that reads it, not just one view.

### Diagnostics tab simplification (`crates/store/src/diagnostics.rs`)

`real_outage_count` currently loops over closed outages and recomputes
`classify_real_outage` per row via `sample_count_between`. It simplifies to
a single query:

```sql
SELECT count(*) FROM outages
WHERE ts_start >= ? AND ts_end IS NOT NULL AND cause = ?
```

bound to `OUTAGE_CAUSE_REAL`. This is safe now that classification is
reliably persisted at close time for every outage going forward (and the
table reset means there's no stale historical data to worry about).

### Route-instability gap exclusion

`check_route_instability`'s event-count input changes from a raw count of
`route_change` events in the last 6h to one that excludes events near a
GAP-classified outage. New constant:

```rust
const ROUTE_CHANGE_GAP_BUFFER_MS: i64 = 2 * 60 * 1000; // 2 minutes
```

`Store::diagnostics()` fetches GAP-classified outages overlapping the 6h
route-change window (`cause = OUTAGE_CAUSE_GAP AND ts_end IS NOT NULL AND
ts_end >= since`), and a route_change event is excluded if its `ts` falls
within `[outage.ts_start - BUFFER, outage.ts_end + BUFFER]` for any of them.
Small data volumes (a handful of gaps/events at most) — filtering in Rust
after two simple queries, no complex SQL join needed.

## Frontend (`src/App.tsx`)

The `Outage` interface already has `cause: string | null` (present since
this field was originally added, just unused in rendering). No interface
change needed.

**Incident Log** (`data-tab="history"`, the "Incident log" card): each `<li>`
checks `o.cause === "device was asleep/off"`:
- GAP: dot class `dot--info` (muted) instead of `dot--critical`/`dot--warn`;
  main text "Device was asleep/off" instead of `fmtDur(o.duration_ms)`; the
  "recovered in Xs" line is omitted (not meaningful for a gap).
- REAL: unchanged from today.

The summary line above the list (`longest outage: X · N total`) computes
its max-duration and count from REAL outages only
(`outages.filter(o => o.cause !== "device was asleep/off")`).

## Data reset

There is only one real database to reset: Tauri's `app_data_dir()` is keyed
by the bundle identifier (`com.tracium.app`) alone, not by dev vs. release,
so the headless `traciumd` daemon and any build of the GUI app all read and
write the same file — `~/.local/share/com.tracium.app/tracium.db`. (Rust
unit/integration tests are unaffected either way: they already use a fresh
`Store::open_in_memory()` per test, which starts empty every run.)

Once the above is implemented and tested, clear the `outages` and `events`
tables in that one file. This is a real, irreversible wipe of live history
on the system this daemon runs on — it needs an explicit, separate
confirmation immediately before it's run, not bundled into the rest of this
work.

Nothing else is touched — raw `connectivity_samples`, DNS stats, speedtests,
security snapshots, etc. all stay intact.

## Testing

- Rust: `classify_real_outage`'s existing unit tests are untouched (pure
  function, unaffected by this change).
- The existing Task 2 integration test
  (`diagnostics_excludes_sleep_gapped_outages_from_frequent_disconnects`)
  builds its fixtures with raw `open_outage`/`close_outage` calls today,
  which would leave every fixture's `cause` at the REAL default — breaking
  once `real_outage_count` trusts the persisted value instead of
  recomputing. It's updated to close its fixtures via
  `close_outage_classified` instead, so the persisted `cause` ends up
  correct for both the 3 "real" and 1 "gap" fixture, and the test continues
  to assert the same outcome (3 real outages counted).
- New unit/integration tests: `close_outage_classified` writes the correct
  `cause` for both a real and a gap fixture; `reliability_since`'s
  `disconnects` excludes GAP outages; the route-instability check excludes
  a `route_change` event landing inside a GAP outage's buffer window but
  still counts one landing outside it.
- Frontend: no test harness for this app (per the prior spec) — a human
  pass in a running build confirms the Incident Log rendering.

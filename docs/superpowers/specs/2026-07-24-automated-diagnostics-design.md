# Automated network diagnostics flags — design

## Why

Investigating a user-reported "browser pages sometimes take 10s to load"
symptom surfaced clear evidence already sitting in Tracium's own database:
packet loss on traceroute hops past the router, frequent `route_change`
events, and a handful of outages whose duration didn't match how many probe
samples actually existed in that window. All of this was found by hand with
the `traciumd` CLI. The point of this feature is to make that kind of
diagnosis automatic and visible in the GUI — no AI involved, just threshold
checks over data already being collected.

## Scope

Four independent, deterministic checks, each producing zero or one
`Diagnostic`:

1. Upstream route instability
2. Real vs. sleep-inflated outages → "frequent disconnects"
3. Bufferbloat / high jitter
4. DNS resolver degraded

Explicitly out of scope for this pass: Wi-Fi signal/roaming flags, user-
configurable thresholds, and any historical/trend view of diagnostics (only
the current state is computed, on demand, like `reliability_since`).

## Backend (`crates/store`)

No new tables. A new `Diagnostic` type:

```rust
pub struct Diagnostic {
    pub key: String,      // stable id, e.g. "route_instability"
    pub severity: String, // "warn" | "bad"
    pub title: String,    // "Upstream route instability"
    pub summary: String,  // one-line, for the header popover
    pub detail: String,   // fuller explanation + the actual numbers
}
```

`Store::diagnostics(&self, now: i64) -> Result<Vec<Diagnostic>>` orchestrates
four independent checks. Each check is implemented as a pure function taking
already-fetched rows and returning `Option<Diagnostic>`, so each is unit-
testable without a database. A missing precondition (e.g. no traceroute has
run yet) means that check silently produces `None` — never an error.

### Check 1: Upstream route instability

Inputs: `latest_traceroute()`, count of `recent_events` with
`kind == "route_change"` in the last 6 hours.

Triggers when **both**:
- some hop with `hop_no > 1` (past the LAN gateway) has `loss_pct >= 20.0`
- `>= 4` route_change events occurred in the last 6h

Requiring both avoids flagging a single noisy hop or a one-off route change
in isolation.

### Check 2: Real vs. sleep-inflated outages

For each `Outage` in the last 24h, compute:
- `expected = duration_ms / 15000` (the fixed 15s probe cadence)
- `actual` = count of `connectivity_samples` rows with `ts` between
  `ts_start` and `ts_end` (new `Store` query needed:
  `sample_count_between(from, to)`)

If `actual < expected * 0.5`, classify the outage as a sleep/resume gap
(the process wasn't sampling for most of that window) rather than a real
outage, and exclude it.

Triggers "frequent disconnects" when `>= 3` *real* outages remain in the
last 24h.

### Check 3: Bufferbloat / high jitter

Inputs: latest `SpeedtestRow` (via `speedtest_history(1)`),
`reliability_since` over the last 1h.

Triggers if **either**:
- latest `bufferbloat_grade` is `"D"` or `"F"`
- `avg_jitter_ms > 20.0`

### Check 4: DNS resolver degraded

Input: `dns_comparison` over the last 1h.

Triggers if **either**:
- any `DnsResolverStat` has `failures > 0`
- every resolver's `avg_ms > 100.0`

### Thresholds

All four thresholds above are plain Rust constants, not user-configurable in
this pass. They're expected to need tuning after real-world use; changing
them is a one-line change per constant.

### Tauri command

`diagnostics()` in `src-tauri/src/lib.rs`, following the exact shape of
`reliability`/`dns_comparison`: takes `State<'_, AppState>`, returns
`Result<Vec<Diagnostic>, String>`.

## Frontend (`src/App.tsx`)

- Fetched via `invoke("diagnostics")` from the same polling effect that
  already fetches `target_status`/`recent_events`/etc. Stored in one new
  `diagnostics` state array — no new polling loop.
- **Header pill**: shown next to the existing theme toggle only when
  `diagnostics.length > 0`. Text: `⚠ 2 issues` (singular for 1). Hovering or
  clicking opens a small popover listing each `summary`; clicking a row (or
  the pill) switches to the new Diagnostics tab.
- **New "Diagnostics" tab**: added to the `TABS` array (7th tab, after
  History). Each active `Diagnostic` renders as a card: title, summary, and
  the fuller `detail` text with the concrete numbers baked in (e.g. "4 route
  changes in the last 6h, 40% loss on hop 6 (1.1.1.1)"), colored by
  severity. Empty state: "No issues detected."

## Testing

- Rust: unit tests for each of the four classifier functions in
  `crates/store`'s existing `mod tests`, using fixture rows and asserting
  the expected `Option<Diagnostic>` — including a fixture that exercises the
  sleep-gap exclusion (an outage with a plausible duration but almost no
  samples in range).
- Frontend: this environment has no attached display to drive the actual
  Tauri window, so there's no automated visual verification here. The tab/
  badge logic will be checked for sensible behavior at the code level; a
  human pass in a running build is needed to confirm the popover layout and
  interactions look right.

## Non-goals / follow-ups

- Wi-Fi signal/roaming flag (deferred, own follow-up).
- User-configurable thresholds.
- Historical diagnostics (trend over time, not just current state).

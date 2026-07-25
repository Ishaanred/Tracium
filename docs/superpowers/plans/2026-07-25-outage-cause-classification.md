# Outage Cause Classification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist a REAL-vs-GAP (sleep/shutdown) classification on every outage at close time, and use it everywhere downtime is counted or displayed — the Disconnects stat, the Diagnostics tab, the Event Timeline, and the route-instability check — instead of only the Diagnostics tab's existing on-the-fly heuristic.

**Architecture:** Two new string constants (`OUTAGE_CAUSE_REAL`/`OUTAGE_CAUSE_GAP`) reuse the existing `outages.cause` column. A new `Store::close_outage_classified` reuses the already-shipped `classify_real_outage`/`sample_count_between` (from `crates/store/src/diagnostics.rs`) to decide and persist the classification once, when an outage closes. Every consumer — `reliability_since`, the Diagnostics tab's frequent-disconnects check, the route-instability check, and the frontend Incident Log — reads that persisted value instead of recomputing or ignoring it.

**Tech Stack:** Rust (sqlx/SQLite) in `tracium-store` and `tracium-monitor`; React/TypeScript in `src/App.tsx`.

## Global Constraints

- Reference spec: `docs/superpowers/specs/2026-07-25-outage-cause-classification-design.md`.
- No new database tables or migrations — `outages.cause` already exists.
- `OUTAGE_CAUSE_REAL = "all internet targets unreachable"`, `OUTAGE_CAUSE_GAP = "device was asleep/off"` — exact strings, used verbatim on both the Rust and TypeScript sides (no shared enum/codegen between them; both sides hardcode the same literal).
- `ROUTE_CHANGE_GAP_BUFFER_MS = 2 * 60 * 1000` (2 minutes) — the window around a GAP outage in which a `route_change` event is excluded from the route-instability count.
- Out of scope: distinguishing *why* a gap happened (suspend vs. shutdown vs. crash), a historical backfill/reclassification pass (superseded by a one-time manual reset of the `outages`/`events` tables, done separately and only after this plan is fully implemented, tested, and deployed — not part of any task here), and any new user-facing settings for thresholds.

---

### Task 1: Constants + `close_outage_classified`

**Files:**
- Modify: `crates/store/src/lib.rs` (add two constants before the `Outage` struct, ~line 1288)
- Modify: `crates/store/src/diagnostics.rs` (add `close_outage_classified` method + tests)

**Interfaces:**
- Consumes: `crate::Store` (existing), `Store::sample_count_between` (existing, `crates/store/src/diagnostics.rs:171`), `classify_real_outage` (existing, `crates/store/src/diagnostics.rs:37`).
- Produces: `pub const OUTAGE_CAUSE_REAL: &str` and `pub const OUTAGE_CAUSE_GAP: &str` (consumed by Tasks 2, 3, 4), `pub async fn Store::close_outage_classified(&self, id: i64, ts_start: i64, ts_end: i64, reconnect_ms: Option<i64>) -> crate::Result<bool>` returning `true` if classified real, `false` if a gap (consumed by Task 2).

- [ ] **Step 1: Write the failing tests**

In `crates/store/src/diagnostics.rs`, add these two tests to the existing `#[cfg(test)] mod tests { ... }` block (after `sample_count_between_counts_only_rows_in_range`, before `diagnostics_excludes_sleep_gapped_outages_from_frequent_disconnects`):

```rust
    #[tokio::test]
    async fn close_outage_classified_marks_real_outage() {
        let store = crate::Store::open_in_memory().await.unwrap();
        store.seed_default_targets(0).await.unwrap();
        let id = store.open_outage(1000, Some(crate::OUTAGE_CAUSE_REAL)).await.unwrap();
        // Two samples across the 30s span => classified real.
        store
            .insert_connectivity_sample(crate::NewConnectivitySample {
                ts: 1000,
                target_id: 1,
                ip_version: 4,
                sent: 1,
                received: 0,
                loss_pct: 100.0,
                rtt_min: None,
                rtt_avg: None,
                rtt_max: None,
                rtt_jitter: None,
                up: false,
            })
            .await
            .unwrap();
        store
            .insert_connectivity_sample(crate::NewConnectivitySample {
                ts: 16_000,
                target_id: 1,
                ip_version: 4,
                sent: 1,
                received: 0,
                loss_pct: 100.0,
                rtt_min: None,
                rtt_avg: None,
                rtt_max: None,
                rtt_jitter: None,
                up: false,
            })
            .await
            .unwrap();

        let is_real = store.close_outage_classified(id, 1000, 31_000, Some(0)).await.unwrap();
        assert!(is_real);

        let outages = store.recent_outages(1).await.unwrap();
        assert_eq!(outages[0].cause.as_deref(), Some(crate::OUTAGE_CAUSE_REAL));
    }

    #[tokio::test]
    async fn close_outage_classified_marks_gap_outage() {
        let store = crate::Store::open_in_memory().await.unwrap();
        store.seed_default_targets(0).await.unwrap();
        let id = store.open_outage(1000, Some(crate::OUTAGE_CAUSE_REAL)).await.unwrap();
        // No samples at all across a 2-hour span => classified a gap.
        let is_real =
            store.close_outage_classified(id, 1000, 1000 + 7_200_000, Some(0)).await.unwrap();
        assert!(!is_real);

        let outages = store.recent_outages(1).await.unwrap();
        assert_eq!(outages[0].cause.as_deref(), Some(crate::OUTAGE_CAUSE_GAP));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p tracium-store close_outage_classified`
Expected: FAIL — compile error, `close_outage_classified` and the two constants don't exist yet

- [ ] **Step 3: Add the constants**

In `crates/store/src/lib.rs`, right before the `/// A recorded (possibly ongoing) internet outage.` comment (~line 1288, immediately after the `Event` struct's closing `}`), insert:

```rust
/// An outage where every internet target was genuinely unreachable —
/// the default `cause` an outage opens with, corrected at close time if it
/// turns out to be a sleep/shutdown gap instead.
pub const OUTAGE_CAUSE_REAL: &str = "all internet targets unreachable";
/// An outage that was actually the device being asleep or off — detected by
/// [`crate::diagnostics`]'s sample-density check at close time, not a real
/// ISP drop.
pub const OUTAGE_CAUSE_GAP: &str = "device was asleep/off";

```

- [ ] **Step 4: Implement `close_outage_classified`**

In `crates/store/src/diagnostics.rs`, add this method to the existing `impl crate::Store { ... }` block, right after `sample_count_between` (after its closing `}`, before `count_events_since`):

```rust
    /// Close an outage, classifying it as real ([`crate::OUTAGE_CAUSE_REAL`])
    /// or a sleep/shutdown gap ([`crate::OUTAGE_CAUSE_GAP`]) based on how
    /// many samples actually exist between `ts_start` and `ts_end`, and
    /// persist that classification into `cause`. Returns `true` if
    /// classified real, `false` if classified a gap.
    pub async fn close_outage_classified(
        &self,
        id: i64,
        ts_start: i64,
        ts_end: i64,
        reconnect_ms: Option<i64>,
    ) -> crate::Result<bool> {
        let actual = self.sample_count_between(ts_start, ts_end).await?;
        let duration_ms = ts_end - ts_start;
        let is_real = classify_real_outage(duration_ms, actual);
        let cause = if is_real { crate::OUTAGE_CAUSE_REAL } else { crate::OUTAGE_CAUSE_GAP };
        sqlx::query(
            "UPDATE outages SET ts_end = ?, duration_ms = ? - ts_start, reconnect_ms = ?, \
             cause = ? WHERE id = ?",
        )
        .bind(ts_end)
        .bind(ts_end)
        .bind(reconnect_ms)
        .bind(cause)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(is_real)
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p tracium-store close_outage_classified`
Expected: both tests PASS

- [ ] **Step 6: Run the full store suite to check for regressions**

Run: `cargo test -p tracium-store`
Expected: all tests PASS (this task only adds code — nothing existing calls `close_outage_classified` yet)

- [ ] **Step 7: Commit**

```bash
git add crates/store/src/lib.rs crates/store/src/diagnostics.rs
git commit -m "Add OUTAGE_CAUSE constants and Store::close_outage_classified"
```

---

### Task 2: `crates/monitor` wiring

**Files:**
- Modify: `crates/monitor/src/lib.rs`

**Interfaces:**
- Consumes: `tracium_store::{OUTAGE_CAUSE_REAL, OUTAGE_CAUSE_GAP}` and `Store::close_outage_classified` (Task 1).
- Produces: nothing new consumed by later tasks — this task only changes *when* and *with what severity* `disconnect`/`reconnect` events are written, and what cause an outage opens/closes with.

- [ ] **Step 1: Write the failing test**

In `crates/monitor/src/lib.rs`'s `#[cfg(test)] mod tests { ... }` block, add this test right after `outage_opens_when_down_and_closes_on_recovery`:

```rust
    #[tokio::test]
    async fn outage_closes_as_gap_when_mostly_no_samples_in_range() {
        // A port nothing listens on -> every probe fails.
        let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead_port = dead.local_addr().unwrap().port();
        drop(dead);

        let store = Store::open_in_memory().await.unwrap();
        add_local_target(&store, "127.0.0.1").await;

        // Cycle 1: down at ts=1000 -> outage opens.
        let down = Monitor::new(store.clone(), cfg(dead_port));
        let u1 = down.tick(1000).await.unwrap();
        assert!(u1.outage_ongoing);

        // Cycle 2: up, but 2 hours later with no cycles in between — simulates
        // a suspend/shutdown gap, not a real multi-hour outage.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let live_port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { while listener.accept().await.is_ok() {} });
        let up = Monitor::new(store.clone(), cfg(live_port));
        let u2 = up.tick(1000 + 7_200_000).await.unwrap();
        assert!(u2.online);

        let outages = store.recent_outages(1).await.unwrap();
        assert_eq!(outages[0].cause.as_deref(), Some(OUTAGE_CAUSE_GAP));

        // Not counted as a real disconnect.
        let r = store.reliability_since(0).await.unwrap();
        assert_eq!(r.disconnects, 0);

        // The Timeline's disconnect event reflects the gap, not a critical alarm.
        let events = store.recent_events(10).await.unwrap();
        let disconnect = events.iter().find(|e| e.kind == "disconnect").expect("disconnect event");
        assert_eq!(disconnect.severity, "info");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p tracium-monitor outage_closes_as_gap_when_mostly_no_samples_in_range`
Expected: FAIL — with the current code, this outage closes with `cause` still `"all internet targets unreachable"` (the hardcoded open-time string) and a `disconnect` event with severity `"critical"` was already written at open time

- [ ] **Step 3: Update the import list**

In `crates/monitor/src/lib.rs`, change the `use tracium_store::{...}` import (~line 22):

```rust
use tracium_store::{
    NewConnectivitySample, SecuritySnapshot, Store, StoreError, TracerouteHop, WifiSample,
};
```

to:

```rust
use tracium_store::{
    NewConnectivitySample, SecuritySnapshot, Store, StoreError, TracerouteHop, WifiSample,
    OUTAGE_CAUSE_GAP, OUTAGE_CAUSE_REAL,
};
```

(`OUTAGE_CAUSE_GAP` isn't used in `update_outage` itself, only in the test above — but the test module does `use super::*;`, which brings this top-level import into its scope, so it's not dead. Only `OUTAGE_CAUSE_REAL` is used in the non-test code path.)

- [ ] **Step 4: Rewrite `update_outage`**

Replace the existing `update_outage` function (~line 414):

```rust
    /// Open an outage when everything drops, close it when anything recovers.
    async fn update_outage(&self, now: i64, online: bool, all_down: bool) -> Result<(), StoreError> {
        let open = self.store.current_open_outage().await?;
        match (open, all_down, online) {
            (None, true, _) => {
                self.store.open_outage(now, Some("all internet targets unreachable")).await?;
                self.store.insert_event(now, "disconnect", "critical", None, None).await?;
            }
            (Some(o), _, true) => {
                let reconnect = now - o.ts_start;
                self.store.close_outage(o.id, now, Some(reconnect)).await?;
                self.store.insert_event(now, "reconnect", "info", Some(reconnect), None).await?;
            }
            _ => {}
        }
        Ok(())
    }
```

with:

```rust
    /// Open an outage when everything drops, close it when anything recovers.
    /// Classification (real vs. sleep/shutdown gap) can only be known once
    /// the outage closes, so both the `disconnect` and `reconnect` Timeline
    /// events are written at close time, not open time.
    async fn update_outage(&self, now: i64, online: bool, all_down: bool) -> Result<(), StoreError> {
        let open = self.store.current_open_outage().await?;
        match (open, all_down, online) {
            (None, true, _) => {
                self.store.open_outage(now, Some(OUTAGE_CAUSE_REAL)).await?;
            }
            (Some(o), _, true) => {
                let reconnect = now - o.ts_start;
                let is_real = self
                    .store
                    .close_outage_classified(o.id, o.ts_start, now, Some(reconnect))
                    .await?;
                let disconnect_severity = if is_real { "critical" } else { "info" };
                self.store
                    .insert_event(o.ts_start, "disconnect", disconnect_severity, None, None)
                    .await?;
                self.store.insert_event(now, "reconnect", "info", Some(reconnect), None).await?;
            }
            _ => {}
        }
        Ok(())
    }
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p tracium-monitor outage_closes_as_gap_when_mostly_no_samples_in_range`
Expected: PASS

- [ ] **Step 6: Run the full monitor suite to check for regressions**

Run: `cargo test -p tracium-monitor`
Expected: all tests PASS, including the existing `outage_opens_when_down_and_closes_on_recovery` (that outage's duration is 3000ms with 2 samples present, which `classify_real_outage` still resolves as real, so `r.disconnects == 1` continues to hold)

- [ ] **Step 7: Commit**

```bash
git add crates/monitor/src/lib.rs
git commit -m "Classify outages at close time; move Timeline events to close time"
```

---

### Task 3: `reliability_since` filters out GAP outages

**Files:**
- Modify: `crates/store/src/lib.rs`

**Interfaces:**
- Consumes: `OUTAGE_CAUSE_GAP` (Task 1, same file so no import needed).
- Produces: nothing new consumed by later tasks — this task only changes what `Reliability.disconnects` counts.

- [ ] **Step 1: Write the failing test**

In `crates/store/src/lib.rs`'s `mod tests { ... }` block, add this test right after `outage_open_and_close` (~line 1873):

```rust
    #[tokio::test]
    async fn reliability_disconnects_excludes_gap_outages() {
        let store = Store::open_in_memory().await.unwrap();
        let real_id = store.open_outage(1000, Some(OUTAGE_CAUSE_REAL)).await.unwrap();
        store.close_outage(real_id, 2000, Some(1000)).await.unwrap();

        let gap_id = store.open_outage(3000, Some(OUTAGE_CAUSE_GAP)).await.unwrap();
        store.close_outage(gap_id, 4000, Some(1000)).await.unwrap();

        let r = store.reliability_since(0).await.unwrap();
        assert_eq!(r.disconnects, 1, "only the real outage should count");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p tracium-store reliability_disconnects_excludes_gap_outages`
Expected: FAIL — `assertion failed: r.disconnects == 1` (currently counts both, so it's `2`)

- [ ] **Step 3: Update the `disconnects` query**

In `crates/store/src/lib.rs`'s `reliability_since` (~line 852), replace:

```rust
        let disconnects: i64 =
            sqlx::query_scalar("SELECT count(*) FROM outages WHERE ts_start >= ?")
                .bind(since)
                .fetch_one(&self.pool)
                .await?;
```

with:

```rust
        let disconnects: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM outages WHERE ts_start >= ? AND (cause IS NULL OR cause <> ?)",
        )
        .bind(since)
        .bind(OUTAGE_CAUSE_GAP)
        .fetch_one(&self.pool)
        .await?;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p tracium-store reliability_disconnects_excludes_gap_outages`
Expected: PASS

- [ ] **Step 5: Run the full store suite to check for regressions**

Run: `cargo test -p tracium-store`
Expected: all tests PASS, including `outage_open_and_close` (its outage's `cause` is `"all targets down"`, which is `<> OUTAGE_CAUSE_GAP`, so it still counts) and `outage_opens_when_down_and_closes_on_recovery` in the monitor crate (unaffected by this store-only change, but re-run it too as a sanity check: `cargo test -p tracium-monitor`)

- [ ] **Step 6: Commit**

```bash
git add crates/store/src/lib.rs
git commit -m "Exclude GAP-classified outages from the disconnects count"
```

---

### Task 4: Diagnostics tab reads persisted cause; route-change gap exclusion

**Files:**
- Modify: `crates/store/src/diagnostics.rs`

**Interfaces:**
- Consumes: `OUTAGE_CAUSE_REAL`/`OUTAGE_CAUSE_GAP` (Task 1), `Store::close_outage_classified` (Task 1, used to rewrite the existing sleep-gap test's fixtures).
- Produces: nothing new consumed by later tasks.

- [ ] **Step 1: Write the failing tests**

Add this test to `crates/store/src/diagnostics.rs`'s `mod tests { ... }` block, right after `sample_count_between_counts_only_rows_in_range` (before `diagnostics_excludes_sleep_gapped_outages_from_frequent_disconnects`):

```rust
    #[tokio::test]
    async fn route_change_excludes_events_near_a_gap_outage() {
        let store = crate::Store::open_in_memory().await.unwrap();
        // A gap-classified outage from ts=100_000 to ts=200_000 (no samples
        // inserted in that range, so it classifies as a gap).
        let id = store.open_outage(100_000, Some(crate::OUTAGE_CAUSE_REAL)).await.unwrap();
        store.close_outage_classified(id, 100_000, 200_000, Some(0)).await.unwrap();

        // Just inside the gap's buffer window -> excluded.
        store.insert_event(260_000, "route_change", "warn", None, None).await.unwrap();
        // Clearly outside it -> still counts.
        store.insert_event(500_000, "route_change", "warn", None, None).await.unwrap();

        let n = store.route_change_count_excluding_gaps(0).await.unwrap();
        assert_eq!(n, 1);
    }
```

Then replace the existing `diagnostics_excludes_sleep_gapped_outages_from_frequent_disconnects` test (its fixtures currently use raw `open_outage`/`close_outage`, which won't correctly persist `cause` once `real_outage_count` trusts the persisted value instead of recomputing it) with:

```rust
    #[tokio::test]
    async fn diagnostics_excludes_sleep_gapped_outages_from_frequent_disconnects() {
        let store = crate::Store::open_in_memory().await.unwrap();
        store.seed_default_targets(0).await.unwrap();

        // Three real outages: each ~30s with samples throughout (2 samples each).
        for base in [1_000_i64, 100_000, 200_000] {
            let id = store.open_outage(base, Some(crate::OUTAGE_CAUSE_REAL)).await.unwrap();
            store
                .insert_connectivity_sample(crate::NewConnectivitySample {
                    ts: base,
                    target_id: 1,
                    ip_version: 4,
                    sent: 1,
                    received: 0,
                    loss_pct: 100.0,
                    rtt_min: None,
                    rtt_avg: None,
                    rtt_max: None,
                    rtt_jitter: None,
                    up: false,
                })
                .await
                .unwrap();
            store
                .insert_connectivity_sample(crate::NewConnectivitySample {
                    ts: base + 15_000,
                    target_id: 1,
                    ip_version: 4,
                    sent: 1,
                    received: 0,
                    loss_pct: 100.0,
                    rtt_min: None,
                    rtt_avg: None,
                    rtt_max: None,
                    rtt_jitter: None,
                    up: false,
                })
                .await
                .unwrap();
            store.close_outage_classified(id, base, base + 30_000, Some(0)).await.unwrap();
        }

        // A fourth "outage" that's actually a multi-hour sleep gap: huge
        // duration, but only one sample exists in that whole window.
        let sleep_id = store.open_outage(300_000, Some(crate::OUTAGE_CAUSE_REAL)).await.unwrap();
        store
            .insert_connectivity_sample(crate::NewConnectivitySample {
                ts: 300_000,
                target_id: 1,
                ip_version: 4,
                sent: 1,
                received: 0,
                loss_pct: 100.0,
                rtt_min: None,
                rtt_avg: None,
                rtt_max: None,
                rtt_jitter: None,
                up: false,
            })
            .await
            .unwrap();
        store
            .close_outage_classified(sleep_id, 300_000, 300_000 + 7_200_000, Some(0))
            .await
            .unwrap();

        let diagnostics = store.diagnostics(400_000).await.unwrap();
        let frequent = diagnostics
            .iter()
            .find(|d| d.key == "frequent_disconnects")
            .expect("3 real outages in 24h should trigger frequent_disconnects");
        assert!(frequent.detail.contains('3'));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p tracium-store route_change_excludes_events_near_a_gap_outage`
Expected: FAIL — compile error, `route_change_count_excluding_gaps` doesn't exist yet

- [ ] **Step 3: Add the gap-buffer constant**

In `crates/store/src/diagnostics.rs`, add this constant right after `const DNS_SLOW_THRESHOLD_MS: f64 = 100.0;` (~line 24):

```rust

const ROUTE_CHANGE_GAP_BUFFER_MS: i64 = 2 * 60 * 1000; // 2 minutes
```

- [ ] **Step 4: Replace `count_events_since` with `route_change_count_excluding_gaps`**

Replace the existing `count_events_since` method (in the `impl crate::Store { ... }` block):

```rust
    async fn count_events_since(&self, since: i64, kind: &str) -> crate::Result<i64> {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM events WHERE kind = ? AND ts >= ?")
            .bind(kind)
            .bind(since)
            .fetch_one(&self.pool)
            .await?;
        Ok(n)
    }
```

with:

```rust
    /// Count of `route_change` events since `since`, excluding any that fall
    /// within [`ROUTE_CHANGE_GAP_BUFFER_MS`] of a GAP-classified outage — a
    /// reboot/wake almost always fires one spurious route change on its own.
    async fn route_change_count_excluding_gaps(&self, since: i64) -> crate::Result<i64> {
        let events: Vec<i64> = sqlx::query_scalar(
            "SELECT ts FROM events WHERE kind = 'route_change' AND ts >= ?",
        )
        .bind(since)
        .fetch_all(&self.pool)
        .await?;

        let gaps: Vec<(i64, Option<i64>)> = sqlx::query_as(
            "SELECT ts_start, ts_end FROM outages \
             WHERE cause = ? AND ts_end IS NOT NULL AND ts_end >= ?",
        )
        .bind(crate::OUTAGE_CAUSE_GAP)
        .bind(since)
        .fetch_all(&self.pool)
        .await?;
        let gap_ranges: Vec<(i64, i64)> =
            gaps.into_iter().filter_map(|(start, end)| end.map(|e| (start, e))).collect();

        let count = events
            .into_iter()
            .filter(|ts| {
                !gap_ranges.iter().any(|(gap_start, gap_end)| {
                    *ts >= gap_start - ROUTE_CHANGE_GAP_BUFFER_MS
                        && *ts <= gap_end + ROUTE_CHANGE_GAP_BUFFER_MS
                })
            })
            .count() as i64;
        Ok(count)
    }
```

- [ ] **Step 5: Update `diagnostics()`'s call site**

In `Store::diagnostics()`, replace:

```rust
        let route_change_count = self
            .count_events_since(now - ROUTE_CHANGE_WINDOW_MS, "route_change")
            .await?;
```

with:

```rust
        let route_change_count = self
            .route_change_count_excluding_gaps(now - ROUTE_CHANGE_WINDOW_MS)
            .await?;
```

- [ ] **Step 6: Simplify `real_outage_count`**

Replace the existing `real_outage_count` method:

```rust
    /// Count of outages starting at or after `since` that are closed and
    /// classified as real (not a sleep/resume gap) via
    /// [`classify_real_outage`].
    async fn real_outage_count(&self, since: i64) -> crate::Result<i64> {
        let outages = sqlx::query_as::<_, crate::Outage>(
            "SELECT id, ts_start, ts_end, duration_ms, reconnect_ms, cause \
             FROM outages WHERE ts_start >= ? AND ts_end IS NOT NULL ORDER BY ts_start DESC",
        )
        .bind(since)
        .fetch_all(&self.pool)
        .await?;

        let mut real = 0;
        for o in &outages {
            let (Some(duration_ms), Some(ts_end)) = (o.duration_ms, o.ts_end) else { continue };
            let actual = self.sample_count_between(o.ts_start, ts_end).await?;
            if classify_real_outage(duration_ms, actual) {
                real += 1;
            }
        }
        Ok(real)
    }
```

with:

```rust
    /// Count of outages starting at or after `since` that are closed and
    /// persisted as real (not a sleep/shutdown gap) — reads the
    /// classification [`crate::Store::close_outage_classified`] already
    /// wrote, rather than recomputing it.
    async fn real_outage_count(&self, since: i64) -> crate::Result<i64> {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM outages WHERE ts_start >= ? AND ts_end IS NOT NULL AND cause = ?",
        )
        .bind(since)
        .bind(crate::OUTAGE_CAUSE_REAL)
        .fetch_one(&self.pool)
        .await?;
        Ok(n)
    }
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p tracium-store diagnostics::tests`
Expected: all tests PASS, including the two new/rewritten ones

- [ ] **Step 8: Run the full store suite to check for regressions**

Run: `cargo test -p tracium-store`
Expected: all tests PASS

- [ ] **Step 9: Commit**

```bash
git add crates/store/src/diagnostics.rs
git commit -m "Diagnostics tab reads persisted outage cause; exclude route_change events near a gap"
```

---

### Task 5: Frontend Incident Log shows GAP outages distinctly

**Files:**
- Modify: `src/App.tsx`

**Interfaces:**
- Consumes: `Outage.cause` (already present in the `Outage` TypeScript interface, `src/App.tsx:57` — no interface change needed).
- Produces: nothing consumed by later tasks — this is the last task.

- [ ] **Step 1: Add the shared cause constant**

In `src/App.tsx`, right after `const QOE_WINDOW_SECS = 30 * 60; // smooth QoE over the last 30 minutes` (~line 173), add:

```typescript
const OUTAGE_CAUSE_GAP = "device was asleep/off";
```

- [ ] **Step 2: Rewrite the Incident Log section**

Replace the existing Incident Log JSX (the block starting `{outages.length === 0 ? (` through its matching `)}`, ~lines 1025-1046):

```tsx
        {outages.length === 0 ? (
          <p className="status status--ok">No outages recorded. 🎉</p>
        ) : (
          <>
            <p className="status" style={{ marginBottom: 10, fontSize: 12 }}>
              longest outage:{" "}
              {fmtDur(Math.max(...outages.map((o) => o.duration_ms ?? 0)))} · {outages.length} total
            </p>
            <ul className="events">
              {outages.map((o) => (
                <li key={o.id}>
                  <span className={`dot dot--${o.ts_end == null ? "critical" : "warn"}`} aria-hidden />
                  <span className="events__kind">{fmtDur(o.duration_ms)}</span>
                  {o.reconnect_ms != null && (
                    <span className="events__dur">recovered in {fmtDur(o.reconnect_ms)}</span>
                  )}
                  <span className="events__time">{new Date(o.ts_start).toLocaleString()}</span>
                </li>
              ))}
            </ul>
          </>
        )}
```

with:

```tsx
        {outages.length === 0 ? (
          <p className="status status--ok">No outages recorded. 🎉</p>
        ) : (
          <>
            {(() => {
              const real = outages.filter((o) => o.cause !== OUTAGE_CAUSE_GAP);
              return real.length === 0 ? (
                <p className="status status--ok" style={{ marginBottom: 10, fontSize: 12 }}>
                  No real outages — {outages.length} sleep/shutdown gap
                  {outages.length === 1 ? "" : "s"} excluded.
                </p>
              ) : (
                <p className="status" style={{ marginBottom: 10, fontSize: 12 }}>
                  longest outage:{" "}
                  {fmtDur(Math.max(...real.map((o) => o.duration_ms ?? 0)))} · {real.length} total
                </p>
              );
            })()}
            <ul className="events">
              {outages.map((o) => {
                const isGap = o.cause === OUTAGE_CAUSE_GAP;
                return (
                  <li key={o.id}>
                    <span
                      className={`dot dot--${isGap ? "info" : o.ts_end == null ? "critical" : "warn"}`}
                      aria-hidden
                    />
                    <span className="events__kind">
                      {isGap ? "Device was asleep/off" : fmtDur(o.duration_ms)}
                    </span>
                    {!isGap && o.reconnect_ms != null && (
                      <span className="events__dur">recovered in {fmtDur(o.reconnect_ms)}</span>
                    )}
                    <span className="events__time">{new Date(o.ts_start).toLocaleString()}</span>
                  </li>
                );
              })}
            </ul>
          </>
        )}
```

- [ ] **Step 3: Verify the frontend type-checks**

Run: `npx tsc --noEmit`
Expected: no errors

- [ ] **Step 4: Commit**

```bash
git add src/App.tsx
git commit -m "Show sleep/shutdown gap outages distinctly in the Incident Log"
```

---

## After all tasks land

The controller (not a task/subagent) handles the live-database reset separately, only after this plan is fully implemented, reviewed, merged, and the app has been rebuilt and redeployed (so the new classification logic is actually running before any data starts accumulating again):

1. Rebuild and redeploy the app (`pnpm tauri build`, restart `tracium.service`) so `close_outage_classified` is actually the code path in use.
2. Get explicit, separate confirmation before touching the live database.
3. Clear the `outages` and `events` tables in `~/.local/share/com.tracium.app/tracium.db` — nothing else.

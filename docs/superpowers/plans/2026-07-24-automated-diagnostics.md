# Automated Network Diagnostics Flags Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface four deterministic, threshold-based diagnostic flags (upstream route instability, real-vs-sleep-filtered frequent disconnects, bufferbloat/high jitter, DNS degradation) in Tracium's GUI, computed from data already in the store — no AI, no new tables.

**Architecture:** A new `crates/store/src/diagnostics.rs` module holds a `Diagnostic` type, four pure/unit-testable classifier functions, and a `Store::diagnostics(now)` orchestration method that feeds them from existing (plus two new) store queries. A new `diagnostics` Tauri command exposes it. The frontend polls it alongside existing status calls, shows a header pill when flags are active, and lists full detail on a new "Diagnostics" tab.

**Tech Stack:** Rust (sqlx/SQLite) in `tracium-store` and `src-tauri`; React/TypeScript in `src/App.tsx`; plain CSS in `src/styles.css`.

## Global Constraints

- Reference spec: `docs/superpowers/specs/2026-07-24-automated-diagnostics-design.md`.
- Probe cadence is a fixed 15000ms (`crates/monitor/src/lib.rs:73`) — `tracium-store` cannot depend on `tracium-monitor` (dependency runs the other way), so this is a documented constant in the new module, not an import.
- Thresholds are hardcoded Rust constants for this pass (not user-configurable): hop loss `>= 20%`, `>= 4` route_change events / 6h, `>= 3` real outages / 24h, sleep-gap ratio `< 0.5` of expected samples, jitter `> 20ms` / 1h, DNS resolver `avg_ms > 100ms` or any `failures > 0` / 1h.
- No new database tables or migrations.
- Every check must degrade to `None` silently on missing data (e.g. no traceroute yet) — never an error.

---

### Task 1: Diagnostic type + pure classifier functions

**Files:**
- Create: `crates/store/src/diagnostics.rs`
- Modify: `crates/store/src/lib.rs` (add `mod diagnostics;` and re-export)

**Interfaces:**
- Consumes: `crate::{TracerouteView, TracerouteHopRow, SpeedtestRow, DnsResolverStat}` (all already defined in `crates/store/src/lib.rs`, all-`pub` fields).
- Produces (used by Task 2 and the frontend via serde):
  - `pub struct Diagnostic { pub key: String, pub severity: String, pub title: String, pub summary: String, pub detail: String }`
  - `pub(crate) fn classify_real_outage(duration_ms: i64, actual_samples: i64) -> bool`
  - `pub(crate) fn check_route_instability(trace: Option<&TracerouteView>, route_change_count: i64) -> Option<Diagnostic>`
  - `pub(crate) fn check_frequent_disconnects(real_outage_count: i64) -> Option<Diagnostic>`
  - `pub(crate) fn check_bufferbloat_jitter(latest_speedtest: Option<&SpeedtestRow>, avg_jitter_ms: Option<f64>) -> Option<Diagnostic>`
  - `pub(crate) fn check_dns_degraded(stats: &[DnsResolverStat]) -> Option<Diagnostic>`

- [ ] **Step 1: Write the failing tests**

Create `crates/store/src/diagnostics.rs` with just the type, constants, function signatures (bodies `todo!()`), and this test module:

```rust
//! Automated, threshold-based network diagnostics — no AI, just arithmetic
//! over data `tracium-store` already collects.

use crate::{DnsResolverStat, SpeedtestRow, TracerouteView};

/// Probe cadence used by `tracium-monitor` (`MonitorConfig::default().interval`).
/// `tracium-store` can't depend on `tracium-monitor` (dependency runs the other
/// way), so this is duplicated here — keep it in sync with
/// `crates/monitor/src/lib.rs`'s `MonitorConfig::default()`.
const PROBE_INTERVAL_MS: i64 = 15_000;

const ROUTE_CHANGE_WINDOW_MS: i64 = 6 * 60 * 60 * 1000;
const ROUTE_CHANGE_MIN_COUNT: i64 = 4;
const HOP_LOSS_THRESHOLD_PCT: f64 = 20.0;

const OUTAGE_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;
const REAL_OUTAGE_MIN_COUNT: i64 = 3;
const SLEEP_GAP_SAMPLE_RATIO: f64 = 0.5;

const JITTER_WINDOW_MS: i64 = 60 * 60 * 1000;
const JITTER_THRESHOLD_MS: f64 = 20.0;

const DNS_WINDOW_MS: i64 = 60 * 60 * 1000;
const DNS_SLOW_THRESHOLD_MS: f64 = 100.0;

/// A threshold-triggered flag surfaced in the GUI's Diagnostics tab.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Diagnostic {
    pub key: String,
    pub severity: String,
    pub title: String,
    pub summary: String,
    pub detail: String,
}

pub(crate) fn classify_real_outage(duration_ms: i64, actual_samples: i64) -> bool {
    todo!()
}

pub(crate) fn check_route_instability(
    trace: Option<&TracerouteView>,
    route_change_count: i64,
) -> Option<Diagnostic> {
    todo!()
}

pub(crate) fn check_frequent_disconnects(real_outage_count: i64) -> Option<Diagnostic> {
    todo!()
}

pub(crate) fn check_bufferbloat_jitter(
    latest_speedtest: Option<&SpeedtestRow>,
    avg_jitter_ms: Option<f64>,
) -> Option<Diagnostic> {
    todo!()
}

pub(crate) fn check_dns_degraded(stats: &[DnsResolverStat]) -> Option<Diagnostic> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TracerouteHopRow;

    fn hop(hop_no: i64, ip: &str, loss_pct: Option<f64>) -> TracerouteHopRow {
        TracerouteHopRow {
            hop_no,
            ip: Some(ip.to_string()),
            hostname: None,
            rtt_ms: Some(5.0),
            loss_pct,
            asn: None,
            as_name: None,
        }
    }

    fn trace(hops: Vec<TracerouteHopRow>) -> TracerouteView {
        TracerouteView {
            id: 1,
            ts: 0,
            target: "1.1.1.1".into(),
            hop_count: hops.len() as i64,
            route_hash: "hash".into(),
            hops,
        }
    }

    #[test]
    fn classify_real_outage_true_when_samples_match_cadence() {
        // 60s outage at 15s cadence => 4 expected samples; 4 actual => real.
        assert!(classify_real_outage(60_000, 4));
    }

    #[test]
    fn classify_real_outage_false_when_mostly_a_sample_gap() {
        // 2.1h outage (7_560_000ms) but almost no samples => sleep/resume gap.
        assert!(!classify_real_outage(7_560_000, 2));
    }

    #[test]
    fn classify_real_outage_true_for_zero_duration() {
        assert!(classify_real_outage(0, 0));
    }

    #[test]
    fn route_instability_silent_with_no_traceroute() {
        assert_eq!(check_route_instability(None, 10), None);
    }

    #[test]
    fn route_instability_silent_below_thresholds() {
        // Loss present but too few route changes.
        let t = trace(vec![hop(1, "192.168.1.1", Some(0.0)), hop(6, "1.1.1.1", Some(40.0))]);
        assert_eq!(check_route_instability(Some(&t), 1), None);

        // Enough route changes but no hop loss past the gateway.
        let t2 = trace(vec![hop(1, "192.168.1.1", Some(0.0)), hop(6, "1.1.1.1", Some(0.0))]);
        assert_eq!(check_route_instability(Some(&t2), 10), None);
    }

    #[test]
    fn route_instability_fires_when_both_conditions_met() {
        let t = trace(vec![
            hop(1, "192.168.1.1", Some(0.0)),
            hop(6, "1.1.1.1", Some(40.0)),
        ]);
        let d = check_route_instability(Some(&t), 4).expect("should fire");
        assert_eq!(d.key, "route_instability");
        assert_eq!(d.severity, "warn");
        assert!(d.detail.contains("1.1.1.1"));
        assert!(d.detail.contains("40"));
    }

    #[test]
    fn route_instability_ignores_the_gateway_hop() {
        // 100% loss but on hop_no == 1 (the LAN gateway) — should not count.
        let t = trace(vec![hop(1, "192.168.1.1", Some(100.0))]);
        assert_eq!(check_route_instability(Some(&t), 10), None);
    }

    #[test]
    fn frequent_disconnects_silent_below_threshold() {
        assert_eq!(check_frequent_disconnects(2), None);
    }

    #[test]
    fn frequent_disconnects_fires_at_threshold() {
        let d = check_frequent_disconnects(3).expect("should fire");
        assert_eq!(d.key, "frequent_disconnects");
        assert!(d.detail.contains('3'));
    }

    #[test]
    fn bufferbloat_jitter_silent_when_clean() {
        let speed = SpeedtestRow { bufferbloat_grade: Some("A".into()), ..Default::default() };
        assert_eq!(check_bufferbloat_jitter(Some(&speed), Some(5.0)), None);
        assert_eq!(check_bufferbloat_jitter(None, None), None);
    }

    #[test]
    fn bufferbloat_jitter_fires_on_poor_grade() {
        let speed = SpeedtestRow { bufferbloat_grade: Some("F".into()), ..Default::default() };
        let d = check_bufferbloat_jitter(Some(&speed), Some(5.0)).expect("should fire");
        assert_eq!(d.key, "bufferbloat_jitter");
        assert!(d.summary.contains('F'));
    }

    #[test]
    fn bufferbloat_jitter_fires_on_high_jitter_alone() {
        let d = check_bufferbloat_jitter(None, Some(25.0)).expect("should fire");
        assert_eq!(d.key, "bufferbloat_jitter");
        assert!(d.summary.contains("25"));
    }

    #[test]
    fn dns_degraded_silent_when_fast_and_clean() {
        let stats = vec![DnsResolverStat {
            resolver: "1.1.1.1".into(),
            avg_ms: Some(5.0),
            count: 10,
            failures: 0,
        }];
        assert_eq!(check_dns_degraded(&stats), None);
    }

    #[test]
    fn dns_degraded_fires_on_any_failures() {
        let stats = vec![
            DnsResolverStat { resolver: "1.1.1.1".into(), avg_ms: Some(5.0), count: 10, failures: 0 },
            DnsResolverStat { resolver: "8.8.8.8".into(), avg_ms: Some(30.0), count: 10, failures: 2 },
        ];
        let d = check_dns_degraded(&stats).expect("should fire");
        assert_eq!(d.key, "dns_degraded");
    }

    #[test]
    fn dns_degraded_fires_when_all_resolvers_slow() {
        let stats = vec![
            DnsResolverStat { resolver: "1.1.1.1".into(), avg_ms: Some(150.0), count: 10, failures: 0 },
            DnsResolverStat { resolver: "8.8.8.8".into(), avg_ms: Some(200.0), count: 10, failures: 0 },
        ];
        assert!(check_dns_degraded(&stats).is_some());
    }

    #[test]
    fn dns_degraded_silent_when_only_one_of_several_is_slow() {
        let stats = vec![
            DnsResolverStat { resolver: "1.1.1.1".into(), avg_ms: Some(5.0), count: 10, failures: 0 },
            DnsResolverStat { resolver: "8.8.8.8".into(), avg_ms: Some(200.0), count: 10, failures: 0 },
        ];
        assert_eq!(check_dns_degraded(&stats), None);
    }
}
```

Add the module to the crate in `crates/store/src/lib.rs` — insert right after the existing top-of-file doc comment/imports block (after line 17, `pub static MIGRATOR...`):

```rust
mod diagnostics;
pub use diagnostics::Diagnostic;
```

- [ ] **Step 2: Run tests to verify they fail (compile error from `todo!()`)**

Run: `cargo test -p tracium-store diagnostics::tests -- --list`
Expected: builds fail to link/run because `todo!()` panics — run one test directly instead to confirm panic:
Run: `cargo test -p tracium-store classify_real_outage_true_when_samples_match_cadence`
Expected: test panics with `not yet implemented`

- [ ] **Step 3: Implement the classifier functions**

Replace each `todo!()` body:

```rust
pub(crate) fn classify_real_outage(duration_ms: i64, actual_samples: i64) -> bool {
    if duration_ms <= 0 {
        return true;
    }
    let expected = duration_ms as f64 / PROBE_INTERVAL_MS as f64;
    actual_samples as f64 >= expected * SLEEP_GAP_SAMPLE_RATIO
}

pub(crate) fn check_route_instability(
    trace: Option<&TracerouteView>,
    route_change_count: i64,
) -> Option<Diagnostic> {
    let trace = trace?;
    let worst = trace
        .hops
        .iter()
        .filter(|h| h.hop_no > 1)
        .filter_map(|h| h.loss_pct.map(|l| (h, l)))
        .filter(|(_, l)| *l >= HOP_LOSS_THRESHOLD_PCT)
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())?;
    if route_change_count < ROUTE_CHANGE_MIN_COUNT {
        return None;
    }
    let (hop, loss) = worst;
    let hop_ip = hop.ip.as_deref().unwrap_or("unknown");
    Some(Diagnostic {
        key: "route_instability".into(),
        severity: "warn".into(),
        title: "Upstream route instability".into(),
        summary: format!(
            "{route_change_count} route changes in the last 6h, {loss:.0}% loss on hop {}",
            hop.hop_no
        ),
        detail: format!(
            "Your traceroute to {target} shows {loss:.0}% packet loss on hop {hop_no} ({hop_ip}), \
             and there have been {route_change_count} route changes in the last 6 hours. This \
             points to instability upstream of your router, past your ISP's edge — not something \
             fixable locally.",
            target = trace.target,
            hop_no = hop.hop_no,
        ),
    })
}

pub(crate) fn check_frequent_disconnects(real_outage_count: i64) -> Option<Diagnostic> {
    if real_outage_count < REAL_OUTAGE_MIN_COUNT {
        return None;
    }
    Some(Diagnostic {
        key: "frequent_disconnects".into(),
        severity: "warn".into(),
        title: "Frequent short disconnects".into(),
        summary: format!("{real_outage_count} real outages in the last 24h"),
        detail: format!(
            "{real_outage_count} internet outages in the last 24 hours were confirmed as real \
             drops (continuous failed probes throughout), not just your device sleeping. This is \
             more than the occasional blip and is worth raising with your ISP.",
        ),
    })
}

pub(crate) fn check_bufferbloat_jitter(
    latest_speedtest: Option<&SpeedtestRow>,
    avg_jitter_ms: Option<f64>,
) -> Option<Diagnostic> {
    let poor_grade = latest_speedtest
        .and_then(|s| s.bufferbloat_grade.as_deref())
        .filter(|g| *g == "D" || *g == "F")
        .map(|g| g.to_string());
    let high_jitter = avg_jitter_ms.filter(|j| *j > JITTER_THRESHOLD_MS);
    if poor_grade.is_none() && high_jitter.is_none() {
        return None;
    }
    let mut parts = Vec::new();
    if let Some(g) = &poor_grade {
        parts.push(format!("bufferbloat grade {g}"));
    }
    if let Some(j) = high_jitter {
        parts.push(format!("{j:.0}ms average jitter over the last hour"));
    }
    let summary = parts.join(", ");
    Some(Diagnostic {
        key: "bufferbloat_jitter".into(),
        severity: "warn".into(),
        title: "Bufferbloat / high jitter".into(),
        summary: summary.clone(),
        detail: format!(
            "{summary}. This causes choppy calls and rubber-banding in games even when the \
             connection is technically \"up\" — often fixable with QoS/SQM on your router.",
        ),
    })
}

pub(crate) fn check_dns_degraded(stats: &[DnsResolverStat]) -> Option<Diagnostic> {
    if stats.is_empty() {
        return None;
    }
    let any_failures = stats.iter().any(|s| s.failures > 0);
    let all_slow = stats
        .iter()
        .all(|s| s.avg_ms.map(|v| v > DNS_SLOW_THRESHOLD_MS).unwrap_or(false));
    if !any_failures && !all_slow {
        return None;
    }
    let total_failures: i64 = stats.iter().map(|s| s.failures).sum();
    let summary = if any_failures {
        format!("{total_failures} DNS lookup failures in the last hour")
    } else {
        "All compared DNS resolvers are slow (>100ms) in the last hour".to_string()
    };
    let breakdown = stats
        .iter()
        .map(|s| {
            let avg = s.avg_ms.map(|v| format!("{v:.1}ms")).unwrap_or_else(|| "n/a".into());
            format!("{} avg {} ({} failures)", s.resolver, avg, s.failures)
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(Diagnostic {
        key: "dns_degraded".into(),
        severity: "warn".into(),
        title: "DNS resolver degraded".into(),
        summary,
        detail: format!(
            "Over the last hour: {breakdown}. Slow or failing DNS lookups add delay before every \
             new connection, which can look like general page-load lag.",
        ),
    })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p tracium-store diagnostics::tests`
Expected: all tests in the list above `PASS` (16 tests)

- [ ] **Step 5: Commit**

```bash
git add crates/store/src/diagnostics.rs crates/store/src/lib.rs
git commit -m "Add pure classifier functions for automated diagnostics flags"
```

---

### Task 2: `Store::diagnostics()` orchestration + new queries

**Files:**
- Modify: `crates/store/src/diagnostics.rs` (append `impl Store` block + queries + integration tests)

**Interfaces:**
- Consumes: `Store` (from `crate::Store`, `self.pool` accessible — private field, but `diagnostics` is a child module of the crate root so it can see it), `Store::latest_traceroute`, `Store::speedtest_history`, `Store::reliability_since`, `Store::dns_comparison` (all pre-existing, `crates/store/src/lib.rs`), and Task 1's `Diagnostic` + four `check_*`/`classify_real_outage` functions.
- Produces: `pub async fn Store::diagnostics(&self, now: i64) -> crate::Result<Vec<Diagnostic>>` (consumed by Task 3), `pub async fn Store::sample_count_between(&self, from: i64, to: i64) -> crate::Result<i64>` (standalone public query, useful on its own).

- [ ] **Step 1: Write the failing integration test**

Append to the `#[cfg(test)] mod tests` block in `crates/store/src/diagnostics.rs` (same file, so add `use crate::{NewConnectivitySample, Store};` to the test module's imports and these tests at the end of the `mod tests` block, before its closing `}`):

```rust
    #[tokio::test]
    async fn diagnostics_empty_on_fresh_store() {
        let store = Store::open_in_memory().await.unwrap();
        assert!(store.diagnostics(0).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn sample_count_between_counts_only_rows_in_range() {
        let store = Store::open_in_memory().await.unwrap();
        store.seed_default_targets(0).await.unwrap();
        for ts in [100, 200, 300, 999] {
            store
                .insert_connectivity_sample(NewConnectivitySample {
                    ts,
                    target_id: 1,
                    ip_version: 4,
                    sent: 1,
                    received: 1,
                    loss_pct: 0.0,
                    rtt_min: Some(1.0),
                    rtt_avg: Some(1.0),
                    rtt_max: Some(1.0),
                    rtt_jitter: Some(0.0),
                    up: true,
                })
                .await
                .unwrap();
        }
        assert_eq!(store.sample_count_between(100, 300).await.unwrap(), 3);
        assert_eq!(store.sample_count_between(0, 999).await.unwrap(), 4);
        assert_eq!(store.sample_count_between(500, 900).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn diagnostics_excludes_sleep_gapped_outages_from_frequent_disconnects() {
        let store = Store::open_in_memory().await.unwrap();
        store.seed_default_targets(0).await.unwrap();

        // Three real outages: each ~30s with samples throughout (2 samples each).
        for base in [1_000_i64, 100_000, 200_000] {
            let id = store.open_outage(base, None).await.unwrap();
            store
                .insert_connectivity_sample(NewConnectivitySample {
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
                .insert_connectivity_sample(NewConnectivitySample {
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
            store.close_outage(id, base + 30_000, Some(0)).await.unwrap();
        }

        // A fourth "outage" that's actually a multi-hour sleep gap: huge
        // duration, but only one sample exists in that whole window.
        let sleep_id = store.open_outage(300_000, None).await.unwrap();
        store
            .insert_connectivity_sample(NewConnectivitySample {
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
        store.close_outage(sleep_id, 300_000 + 7_200_000, Some(0)).await.unwrap();

        let diagnostics = store.diagnostics(400_000).await.unwrap();
        let frequent = diagnostics
            .iter()
            .find(|d| d.key == "frequent_disconnects")
            .expect("3 real outages in 24h should trigger frequent_disconnects");
        assert!(frequent.detail.contains('3'));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p tracium-store diagnostics::tests::diagnostics_empty_on_fresh_store`
Expected: FAIL — `Store::diagnostics` and `Store::sample_count_between` don't exist yet (compile error)

- [ ] **Step 3: Implement `sample_count_between` and `Store::diagnostics`**

Append to `crates/store/src/diagnostics.rs` (after the `check_dns_degraded` function, before the `#[cfg(test)]` module):

```rust
impl crate::Store {
    /// Count of `connectivity_samples` rows with `ts` in `[from, to]`
    /// (inclusive). Used to distinguish a real outage (continuous failed
    /// probing) from a sleep/resume gap (almost no samples at all).
    pub async fn sample_count_between(&self, from: i64, to: i64) -> crate::Result<i64> {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM connectivity_samples WHERE ts >= ? AND ts <= ?",
        )
        .bind(from)
        .bind(to)
        .fetch_one(&self.pool)
        .await?;
        Ok(n)
    }

    async fn count_events_since(&self, since: i64, kind: &str) -> crate::Result<i64> {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM events WHERE kind = ? AND ts >= ?")
            .bind(kind)
            .bind(since)
            .fetch_one(&self.pool)
            .await?;
        Ok(n)
    }

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

    /// Run all automated diagnostics checks against the current state of the
    /// store. Each check independently returns zero or one flag; missing
    /// preconditions (e.g. no traceroute yet) simply produce no flag, never
    /// an error.
    pub async fn diagnostics(&self, now: i64) -> crate::Result<Vec<Diagnostic>> {
        let mut out = Vec::new();

        let trace = self.latest_traceroute().await?;
        let route_change_count = self
            .count_events_since(now - ROUTE_CHANGE_WINDOW_MS, "route_change")
            .await?;
        if let Some(d) = check_route_instability(trace.as_ref(), route_change_count) {
            out.push(d);
        }

        let real_outages = self.real_outage_count(now - OUTAGE_WINDOW_MS).await?;
        if let Some(d) = check_frequent_disconnects(real_outages) {
            out.push(d);
        }

        let latest_speedtest = self.speedtest_history(1).await?;
        let jitter_rel = self.reliability_since(now - JITTER_WINDOW_MS).await?;
        if let Some(d) = check_bufferbloat_jitter(latest_speedtest.first(), jitter_rel.avg_jitter_ms)
        {
            out.push(d);
        }

        let dns_stats = self.dns_comparison(now - DNS_WINDOW_MS).await?;
        if let Some(d) = check_dns_degraded(&dns_stats) {
            out.push(d);
        }

        Ok(out)
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p tracium-store diagnostics::tests`
Expected: all tests (unit + the 3 new integration tests) `PASS`

- [ ] **Step 5: Run the full store test suite to check for regressions**

Run: `cargo test -p tracium-store`
Expected: all tests `PASS` (no existing test touches `diagnostics`, so this should be a pure addition)

- [ ] **Step 6: Commit**

```bash
git add crates/store/src/diagnostics.rs
git commit -m "Add Store::diagnostics() orchestration with sleep-gap-aware outage filtering"
```

---

### Task 3: Tauri `diagnostics()` command

**Files:**
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: `tracium_store::Diagnostic` (from Task 1's re-export), `Store::diagnostics` (Task 2), `now_ms()` (already imported from `tracium_monitor` at the top of this file).
- Produces: Tauri command `diagnostics` returning `Result<Vec<Diagnostic>, String>` — the exact invoke name (`"diagnostics"`) the frontend will call in Task 4.

- [ ] **Step 1: Add `Diagnostic` to the existing `tracium_store` import**

In `src-tauri/src/lib.rs`, the import block currently reads (around line 11):

```rust
use tracium_store::{
    BandwidthNow, BandwidthTotals, ConnectivitySample, Device, DnsResolverStat, Event, NewTarget,
    GatewaySample, InterfaceErrorsRow, Outage, QoeAverage, Reliability, Rollup, SecuritySnapshot,
    SpeedtestRow, Store, Target, TargetStatus, TracerouteView, WifiSample,
};
```

Change it to:

```rust
use tracium_store::{
    BandwidthNow, BandwidthTotals, ConnectivitySample, Device, Diagnostic, DnsResolverStat, Event,
    NewTarget, GatewaySample, InterfaceErrorsRow, Outage, QoeAverage, Reliability, Rollup,
    SecuritySnapshot, SpeedtestRow, Store, Target, TargetStatus, TracerouteView, WifiSample,
};
```

- [ ] **Step 2: Add the command**

Insert this function right after `recent_outages` (which ends around line 323, just before the `export_csv` doc comment):

```rust
/// Automated diagnostics: threshold-based flags computed from data already
/// in the store (route instability, real-vs-sleep-filtered disconnects,
/// bufferbloat/jitter, DNS degradation). No AI involved.
#[tauri::command]
async fn diagnostics(state: State<'_, AppState>) -> Result<Vec<Diagnostic>, String> {
    state.store.diagnostics(now_ms()).await.map_err(|e| e.to_string())
}
```

- [ ] **Step 3: Register it in the invoke handler**

In the `tauri::generate_handler![...]` list (around line 389), add `diagnostics,` right after `recent_outages,`:

```rust
            recent_events,
            recent_outages,
            diagnostics,
            export_csv
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p tracium`
Expected: compiles with no errors

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/lib.rs
git commit -m "Expose diagnostics() as a Tauri command"
```

---

### Task 4: Frontend — fetch diagnostics + header pill

**Files:**
- Modify: `src/App.tsx`

**Interfaces:**
- Consumes: Tauri command `"diagnostics"` (Task 3), returning `Diagnostic[]`.
- Produces: `diagnostics` state (`Diagnostic[]`), used by Task 5's Diagnostics tab panel.

- [ ] **Step 1: Add the `Diagnostic` interface**

In `src/App.tsx`, add this interface right after the `Reliability` interface (around line 159, before `interface TargetStatus`):

```typescript
interface Diagnostic {
  key: string;
  severity: string;
  title: string;
  summary: string;
  detail: string;
}
```

- [ ] **Step 2: Add state and the fetch call**

Add state right after `const [outages, setOutages] = useState<Outage[]>([]);` (line 253):

```typescript
  const [diagnostics, setDiagnostics] = useState<Diagnostic[]>([]);
```

Add the fetch inside `refreshDerived` (in the function body, right after the `recent_outages` invoke call, i.e. after line 376's `.catch(() => {});`):

```typescript
    invoke<Diagnostic[]>("diagnostics").then(setDiagnostics).catch(() => {});
```

- [ ] **Step 3: Add the header pill component**

Add this component near the other small components (`Info`, `CardTitle`), right after the `Info` function definition (around line 1182, after its closing `}`):

```tsx
/** Header badge shown only when automated diagnostics are active. */
function DiagnosticsPill({
  diagnostics,
  onOpen,
}: {
  diagnostics: Diagnostic[];
  onOpen: () => void;
}) {
  if (diagnostics.length === 0) return null;
  return (
    <span className="diag-pill" tabIndex={0}>
      <button className="diag-pill__trigger" onClick={onOpen}>
        ⚠ {diagnostics.length} issue{diagnostics.length === 1 ? "" : "s"}
      </button>
      <span className="diag-pill__list" role="tooltip">
        {diagnostics.map((d) => (
          <button key={d.key} className="diag-pill__item" onClick={onOpen}>
            {d.summary}
          </button>
        ))}
      </span>
    </span>
  );
}
```

- [ ] **Step 4: Render the pill in the header**

In the header JSX, the theme toggle button is currently (around line 462-470):

```tsx
        </span>
        <button
          className="icon-btn"
          title={`Switch to ${theme === "dark" ? "light" : "dark"} theme`}
          onClick={() => setTheme((t) => (t === "dark" ? "light" : "dark"))}
        >
          {theme === "dark" ? "☀" : "☾"}
        </button>
      </header>
```

Change it to insert the pill right before the theme toggle:

```tsx
        </span>
        <DiagnosticsPill diagnostics={diagnostics} onOpen={() => setTab("diagnostics")} />
        <button
          className="icon-btn"
          title={`Switch to ${theme === "dark" ? "light" : "dark"} theme`}
          onClick={() => setTheme((t) => (t === "dark" ? "light" : "dark"))}
        >
          {theme === "dark" ? "☀" : "☾"}
        </button>
      </header>
```

(`setTab("diagnostics")` will be valid once Task 5 adds `"diagnostics"` to the `Tab` type — if building this task standalone before Task 5, TypeScript will error here; that's expected and resolved by Task 5.)

- [ ] **Step 5: Add pill styles**

In `src/styles.css`, insert right after the `.icon-btn:hover { ... }` block (around line 494), before the `/* Tab navigation */` comment:

```css
.diag-pill {
  position: relative;
  display: inline-flex;
}
.diag-pill__trigger {
  flex: none;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  background: color-mix(in srgb, var(--warn) 16%, transparent);
  border: 1px solid var(--warn);
  color: var(--warn);
  border-radius: 999px;
  padding: 5px 12px;
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
}
.diag-pill__trigger:hover {
  background: color-mix(in srgb, var(--warn) 26%, transparent);
}
.diag-pill__list {
  position: absolute;
  top: calc(100% + 8px);
  right: 0;
  width: 260px;
  display: flex;
  flex-direction: column;
  background: var(--elev);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 6px;
  opacity: 0;
  pointer-events: none;
  transition: opacity 0.12s;
  z-index: 30;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
}
.diag-pill:hover .diag-pill__list,
.diag-pill:focus-within .diag-pill__list {
  opacity: 1;
  pointer-events: auto;
}
.diag-pill__item {
  display: block;
  width: 100%;
  text-align: left;
  background: none;
  border: none;
  border-radius: 6px;
  padding: 7px 8px;
  font-size: 12px;
  color: var(--text);
  cursor: pointer;
}
.diag-pill__item:hover {
  background: var(--card);
}
```

- [ ] **Step 6: Verify the frontend type-checks**

This step will still fail until Task 5 adds `"diagnostics"` to the `Tab` union — run it anyway to confirm the *only* error is that one:

Run: `npx tsc --noEmit`
Expected: exactly one error, referencing `setTab("diagnostics")` and `Tab` not including `"diagnostics"` — no other errors

- [ ] **Step 7: Commit**

```bash
git add src/App.tsx src/styles.css
git commit -m "Fetch diagnostics and add header issues pill (frontend)"
```

---

### Task 5: Frontend — Diagnostics tab

**Files:**
- Modify: `src/App.tsx`
- Modify: `src/styles.css`

**Interfaces:**
- Consumes: `diagnostics` state (Task 4), `Diagnostic` interface (Task 4).
- Produces: nothing consumed by later tasks — this is the last task.

- [ ] **Step 1: Add `"diagnostics"` to the `Tab` type and `TABS`**

Change (around line 222):

```typescript
type Tab = "overview" | "connectivity" | "lan" | "routing" | "security" | "history";
const TABS: { id: Tab; label: string }[] = [
  { id: "overview", label: "Overview" },
  { id: "connectivity", label: "Connectivity" },
  { id: "lan", label: "Wi-Fi & LAN" },
  { id: "routing", label: "DNS & Routing" },
  { id: "security", label: "Security" },
  { id: "history", label: "History" },
];
```

to:

```typescript
type Tab = "overview" | "connectivity" | "lan" | "routing" | "security" | "history" | "diagnostics";
const TABS: { id: Tab; label: string }[] = [
  { id: "overview", label: "Overview" },
  { id: "connectivity", label: "Connectivity" },
  { id: "lan", label: "Wi-Fi & LAN" },
  { id: "routing", label: "DNS & Routing" },
  { id: "security", label: "Security" },
  { id: "history", label: "History" },
  { id: "diagnostics", label: "Diagnostics" },
];
```

- [ ] **Step 2: Add the CSS visibility rule for the new tab**

In `src/styles.css`, the tab-panel visibility rule currently reads (around line 533):

```css
.panels[data-active="overview"] > [data-tab~="overview"],
.panels[data-active="connectivity"] > [data-tab~="connectivity"],
.panels[data-active="lan"] > [data-tab~="lan"],
.panels[data-active="routing"] > [data-tab~="routing"],
.panels[data-active="security"] > [data-tab~="security"],
.panels[data-active="history"] > [data-tab~="history"] {
  display: block;
}
```

Add the `diagnostics` line:

```css
.panels[data-active="overview"] > [data-tab~="overview"],
.panels[data-active="connectivity"] > [data-tab~="connectivity"],
.panels[data-active="lan"] > [data-tab~="lan"],
.panels[data-active="routing"] > [data-tab~="routing"],
.panels[data-active="security"] > [data-tab~="security"],
.panels[data-active="history"] > [data-tab~="history"],
.panels[data-active="diagnostics"] > [data-tab~="diagnostics"] {
  display: block;
}
```

- [ ] **Step 3: Add card-layout CSS for multi-line diagnostic entries**

In `src/styles.css`, right after the `.events__time { ... }` block (around line 355), before `.row`:

```css
.events li.diag-card {
  align-items: flex-start;
}
.diag-card__body {
  display: flex;
  flex-direction: column;
  gap: 3px;
}
.diag-card__title {
  font-weight: 600;
}
.diag-card__detail {
  color: var(--muted);
  font-size: 13px;
  line-height: 1.5;
}
```

- [ ] **Step 4: Add the Diagnostics tab panel**

In `src/App.tsx`, insert this new `<section>` right after the Incident log section closes (after line 1035's `</section>`, before the `<section className="card" data-tab="lan">` that starts the Router/SNMP card):

```tsx
      <section className="card" data-tab="diagnostics">
        <CardTitle
          title="Diagnostics"
          info="Automatic checks over data Tracium already collects — route stability, real vs. sleep-filtered disconnects, bufferbloat/jitter, and DNS health. No AI involved, just thresholds."
        />
        {diagnostics.length === 0 ? (
          <p className="status status--ok">No issues detected. 🎉</p>
        ) : (
          <ul className="events">
            {diagnostics.map((d) => (
              <li key={d.key} className="diag-card">
                <span
                  className={`dot dot--${d.severity === "bad" ? "critical" : "warn"}`}
                  aria-hidden
                />
                <span className="diag-card__body">
                  <strong className="diag-card__title">{d.title}</strong>
                  <span className="diag-card__detail">{d.detail}</span>
                </span>
              </li>
            ))}
          </ul>
        )}
      </section>
```

- [ ] **Step 5: Verify the frontend type-checks clean**

Run: `npx tsc --noEmit`
Expected: no errors (the `Tab`/`setTab("diagnostics")` mismatch from Task 4 is now resolved)

- [ ] **Step 6: Manual verification (requires a display)**

This environment has no attached display to drive an actual Tauri window, so this step must be done by a human on a machine with one:

Run: `npm run tauri dev`
Expected: app launches; when diagnostics are active (or after temporarily lowering a threshold constant in `crates/store/src/diagnostics.rs` to force one), the header pill appears next to the theme toggle, hovering/clicking it shows the summary list, clicking switches to the Diagnostics tab, and the tab shows full detail cards. With no diagnostics active, the tab shows "No issues detected. 🎉" and the pill is absent.

- [ ] **Step 7: Commit**

```bash
git add src/App.tsx src/styles.css
git commit -m "Add Diagnostics tab with full flag detail cards"
```

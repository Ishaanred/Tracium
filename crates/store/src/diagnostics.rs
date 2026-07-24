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

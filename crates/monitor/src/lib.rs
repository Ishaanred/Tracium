//! NetPulse monitoring orchestration.
//!
//! Drives the connectivity probe on a schedule, persists each cycle to the
//! store, and detects outages (internet considered *down* only when **every**
//! enabled internet target is unreachable in a cycle). Emits a [`StatusUpdate`]
//! after each tick so the UI can react live.
//!
//! The per-cycle work is [`Monitor::tick`], which takes an explicit `now` and
//! is fully deterministic/testable. [`Monitor::run`] is the thin real-time
//! wrapper around it.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use netpulse_probe::{dns_lookup, probe, public_ip, BandwidthSampler, ProbeConfig};
use netpulse_store::{NewConnectivitySample, Store, StoreError};

/// Resolvers compared on the DNS cadence (label, IP).
const DNS_RESOLVERS: &[(&str, &str)] =
    &[("Cloudflare", "1.1.1.1"), ("Google", "8.8.8.8"), ("Quad9", "9.9.9.9")];
/// Hostnames rotated through so we don't hammer one name (and dodge caching).
const DNS_HOSTS: &[&str] = &["example.com", "wikipedia.org", "github.com", "cloudflare.com"];

pub mod qoe;
pub use qoe::Qoe;

/// How the monitor samples.
#[derive(Debug, Clone)]
pub struct MonitorConfig {
    /// Time between probe cycles.
    pub interval: Duration,
    /// TCP port to connect to on each target (default 443).
    pub port: u16,
    /// Per-target probe settings (count, timeout, gap).
    pub probe: ProbeConfig,
    /// How often to run rollups + retention pruning.
    pub maintenance_interval: Duration,
    /// How often to sample DNS resolvers.
    pub dns_interval: Duration,
    /// Per-lookup DNS timeout.
    pub dns_timeout: Duration,
    /// How often to check the public IP.
    pub public_ip_interval: Duration,
    /// Public-IP HTTP timeout.
    pub public_ip_timeout: Duration,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(15),
            port: 443,
            probe: ProbeConfig::default(),
            maintenance_interval: Duration::from_secs(3600),
            dns_interval: Duration::from_secs(60),
            dns_timeout: Duration::from_secs(3),
            public_ip_interval: Duration::from_secs(600),
            public_ip_timeout: Duration::from_secs(5),
        }
    }
}

/// Live status emitted after each cycle.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StatusUpdate {
    pub ts: i64,
    pub online: bool,
    pub targets_up: i64,
    pub targets_total: i64,
    /// Lowest average RTT across targets that responded (ms).
    pub best_latency_ms: Option<f64>,
    /// Mean loss across probed targets this cycle.
    pub avg_loss_pct: Option<f64>,
    pub outage_ongoing: bool,
    /// Quality-of-experience scores for this cycle; `None` while offline.
    pub qoe: Option<Qoe>,
}

pub struct Monitor {
    store: Store,
    config: MonitorConfig,
}

impl Monitor {
    pub fn new(store: Store, config: MonitorConfig) -> Self {
        Self { store, config }
    }

    /// Run one probe cycle at time `now` (unix ms): probe every enabled internet
    /// target, persist samples, update outage state, and return live status.
    pub async fn tick(&self, now: i64) -> Result<StatusUpdate, StoreError> {
        let targets: Vec<_> = self
            .store
            .list_targets()
            .await?
            .into_iter()
            .filter(|t| t.enabled && t.kind == "internet")
            .collect();

        let mut targets_up = 0i64;
        let mut best_latency: Option<f64> = None;
        let mut loss_sum = 0.0f64;
        let mut jitter_sum = 0.0f64;
        let mut jitter_n = 0i64;

        for t in &targets {
            let ipv = t.ip_version.unwrap_or(4) as u8;
            let cfg = ProbeConfig { ip_version: Some(ipv), ..self.config.probe.clone() };
            let out = probe(&t.host, self.config.port, &cfg).await;

            self.store
                .insert_connectivity_sample(NewConnectivitySample {
                    ts: now,
                    target_id: t.id,
                    ip_version: ipv as i64,
                    sent: out.sent as i64,
                    received: out.received as i64,
                    loss_pct: out.loss_pct,
                    rtt_min: out.rtt_min,
                    rtt_avg: out.rtt_avg,
                    rtt_max: out.rtt_max,
                    rtt_jitter: out.rtt_jitter,
                    up: out.up,
                })
                .await?;

            loss_sum += out.loss_pct;
            if out.up {
                targets_up += 1;
                if let Some(avg) = out.rtt_avg {
                    best_latency = Some(best_latency.map_or(avg, |b| b.min(avg)));
                }
                if let Some(j) = out.rtt_jitter {
                    jitter_sum += j;
                    jitter_n += 1;
                }
            }
        }

        let total = targets.len() as i64;
        let online = targets_up > 0;
        // Only meaningful when we actually have targets to judge by.
        let all_down = total > 0 && targets_up == 0;

        self.update_outage(now, online, all_down).await?;

        let avg_loss = (total > 0).then(|| loss_sum / total as f64);

        // Quality-of-experience scores from this cycle's best latency + mean
        // jitter + mean loss. Persisted for trending; None while offline.
        let qoe = if online {
            let jitter = if jitter_n > 0 { jitter_sum / jitter_n as f64 } else { 0.0 };
            let q = Qoe::score(best_latency.unwrap_or(0.0), jitter, avg_loss.unwrap_or(0.0));
            self.store
                .insert_qoe(now, q.gaming, q.video_call, q.streaming, q.web, q.voip)
                .await?;
            Some(q)
        } else {
            None
        };

        Ok(StatusUpdate {
            ts: now,
            online,
            targets_up,
            targets_total: total,
            best_latency_ms: best_latency,
            avg_loss_pct: avg_loss,
            outage_ongoing: all_down,
            qoe,
        })
    }

    /// Probe each comparison resolver once for a rotating hostname, persisting
    /// results to `dns_samples`. Cheap enough to run on its own slow cadence.
    pub async fn sample_dns(&self, now: i64) -> Result<(), StoreError> {
        let host = DNS_HOSTS[((now / 60_000) as usize) % DNS_HOSTS.len()];
        for (_, ip) in DNS_RESOLVERS {
            let Ok(addr) = ip.parse() else { continue };
            let r = dns_lookup(addr, host, self.config.dns_timeout).await;
            self.store
                .insert_dns_sample(now, &r.resolver, &r.query_host, r.lookup_ms, r.success, None)
                .await?;
        }
        Ok(())
    }

    /// Persist one bandwidth reading: the aggregate rate plus per-interface
    /// byte deltas (for usage totals). The sampler is owned by the caller
    /// because it holds counter state across cycles.
    pub async fn record_bandwidth(
        &self,
        now: i64,
        sample: netpulse_probe::BandwidthSample,
    ) -> Result<(), StoreError> {
        self.store
            .insert_bandwidth_sample(now, sample.rx_bps as i64, sample.tx_bps as i64)
            .await?;
        for iface in &sample.per_iface {
            self.store
                .insert_interface_bytes(now, &iface.iface, iface.rx_bytes as i64, iface.tx_bytes as i64)
                .await?;
        }
        Ok(())
    }

    /// Check the public IP (blocking HTTP off the async runtime), persist a
    /// snapshot, and log a `public_ip_change` event when it differs from the
    /// last known value.
    pub async fn sample_public_ip(&self, now: i64) -> Result<(), StoreError> {
        let timeout = self.config.public_ip_timeout;
        let ip = tokio::task::spawn_blocking(move || public_ip(timeout)).await.unwrap_or(None);
        let Some(ip) = ip else { return Ok(()) };

        let previous = self.store.latest_public_ip().await?;
        self.store.insert_public_ip(now, Some(&ip)).await?;
        if let Some(prev) = previous {
            if prev != ip {
                let payload = format!(r#"{{"from":"{prev}","to":"{ip}"}}"#);
                self.store
                    .insert_event(now, "public_ip_change", "warn", None, Some(&payload))
                    .await?;
            }
        }
        Ok(())
    }

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

    /// Real-time loop: tick every `interval`, forwarding each status to `sink`.
    /// Runs until the sink is closed (receiver dropped) or forever otherwise.
    pub async fn run(&self, sink: Option<tokio::sync::mpsc::Sender<StatusUpdate>>) {
        // Roll up any backlog + prune once at startup.
        if let Err(e) = self.store.maintain(now_ms()).await {
            eprintln!("netpulse initial maintenance failed: {e}");
        }
        let mut last_maint = now_ms();
        let mut last_dns = 0i64; // 0 => sample on the first cycle
        let mut last_pubip = 0i64;
        let mut bandwidth = BandwidthSampler::new();

        let mut ticker = tokio::time::interval(self.config.interval);
        let maint_ms = self.config.maintenance_interval.as_millis() as i64;
        let dns_ms = self.config.dns_interval.as_millis() as i64;
        let pubip_ms = self.config.public_ip_interval.as_millis() as i64;
        loop {
            ticker.tick().await;
            let now = now_ms();
            match self.tick(now).await {
                Ok(update) => {
                    if let Some(tx) = &sink {
                        if tx.send(update).await.is_err() {
                            break; // receiver gone; stop.
                        }
                    }
                }
                Err(e) => {
                    // A transient DB error shouldn't kill monitoring.
                    eprintln!("netpulse monitor tick failed: {e}");
                }
            }
            // Bandwidth every cycle (cheap; rate is over the tick interval).
            if let Err(e) = self.record_bandwidth(now, bandwidth.sample()).await {
                eprintln!("netpulse bandwidth sample failed: {e}");
            }
            if now - last_dns >= dns_ms {
                if let Err(e) = self.sample_dns(now).await {
                    eprintln!("netpulse dns sample failed: {e}");
                }
                last_dns = now;
            }
            if now - last_pubip >= pubip_ms {
                if let Err(e) = self.sample_public_ip(now).await {
                    eprintln!("netpulse public-ip sample failed: {e}");
                }
                last_pubip = now;
            }
            if now - last_maint >= maint_ms {
                if let Err(e) = self.store.maintain(now).await {
                    eprintln!("netpulse maintenance failed: {e}");
                }
                last_maint = now;
            }
        }
    }
}

/// Current unix time in milliseconds.
pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use netpulse_store::NewTarget;
    use tokio::net::TcpListener;

    fn cfg(port: u16) -> MonitorConfig {
        MonitorConfig {
            interval: Duration::from_millis(1),
            port,
            probe: ProbeConfig {
                count: 3,
                timeout: Duration::from_millis(500),
                gap: Duration::ZERO,
                ip_version: None,
            },
            maintenance_interval: Duration::from_secs(3600),
            dns_interval: Duration::from_secs(60),
            dns_timeout: Duration::from_millis(300),
            public_ip_interval: Duration::from_secs(600),
            public_ip_timeout: Duration::from_millis(300),
        }
    }

    async fn add_local_target(store: &Store, host: &str) {
        store
            .add_target(NewTarget {
                label: "local".into(),
                host: host.into(),
                kind: "internet".into(),
                ip_version: Some(4),
                enabled: true,
                created_at: 0,
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn tick_online_persists_and_reports() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { while listener.accept().await.is_ok() {} });

        let store = Store::open_in_memory().await.unwrap();
        add_local_target(&store, "127.0.0.1").await;

        let mon = Monitor::new(store.clone(), cfg(port));
        let update = mon.tick(1000).await.unwrap();

        assert!(update.online);
        assert_eq!(update.targets_up, 1);
        assert_eq!(update.targets_total, 1);
        assert!(update.best_latency_ms.is_some());
        assert!(!update.outage_ongoing);
        // Local loopback is pristine -> QoE computed and near-perfect.
        let qoe = update.qoe.expect("qoe present when online");
        assert!(qoe.gaming > 90.0, "gaming {}", qoe.gaming);

        assert_eq!(store.recent_connectivity(10).await.unwrap().len(), 1);
        assert!(store.current_open_outage().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn outage_opens_when_down_and_closes_on_recovery() {
        // A port nothing listens on -> every probe fails.
        let dead = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead_port = dead.local_addr().unwrap().port();
        drop(dead);

        let store = Store::open_in_memory().await.unwrap();
        add_local_target(&store, "127.0.0.1").await;

        // Cycle 1: down -> outage opens.
        let down = Monitor::new(store.clone(), cfg(dead_port));
        let u1 = down.tick(1000).await.unwrap();
        assert!(!u1.online);
        assert!(u1.outage_ongoing);
        let open = store.current_open_outage().await.unwrap();
        assert!(open.is_some(), "outage should be open");

        // Cycle 2: bring up a listener on a fresh port -> recovery closes outage.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let live_port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { while listener.accept().await.is_ok() {} });

        let up = Monitor::new(store.clone(), cfg(live_port));
        let u2 = up.tick(4000).await.unwrap();
        assert!(u2.online);
        assert!(store.current_open_outage().await.unwrap().is_none(), "outage should close");

        let r = store.reliability_since(0).await.unwrap();
        assert_eq!(r.disconnects, 1);
        assert_eq!(r.samples, 2);
    }
}

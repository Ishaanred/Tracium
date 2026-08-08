//! Tracium monitoring orchestration.
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

use tracium_probe::security::{
    check_doh, check_dot, detect_nat, detect_upnp, detect_vpn, firewall_active, scan_local_ports,
    COMMON_PORTS,
};
use tracium_probe::{
    default_gateway_ip, discover_devices, dns_cache_stats, dns_lookup, get_wifi, interface_errors,
    ping, probe, public_ip, traceroute, BandwidthSampler, ProbeConfig,
};
use tracium_store::{
    NewConnectivitySample, SecuritySnapshot, Store, StoreError, TracerouteHop, WifiSample,
};

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
    /// How often to run the security-posture snapshot.
    pub security_interval: Duration,
    /// Timeout for individual security probes (DoH/DoT).
    pub security_timeout: Duration,
    /// How often to sample the Wi-Fi link.
    pub wifi_interval: Duration,
    /// How often to enumerate LAN devices.
    pub devices_interval: Duration,
    /// How often to run a traceroute.
    pub traceroute_interval: Duration,
    /// Traceroute target + limits.
    pub traceroute_target: String,
    pub traceroute_max_hops: u8,
    pub traceroute_timeout: Duration,
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
            security_interval: Duration::from_secs(300),
            security_timeout: Duration::from_secs(4),
            wifi_interval: Duration::from_secs(30),
            devices_interval: Duration::from_secs(60),
            traceroute_interval: Duration::from_secs(600),
            traceroute_target: "1.1.1.1".to_string(),
            traceroute_max_hops: 30,
            traceroute_timeout: Duration::from_secs(40),
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
    /// Mean jitter across reachable targets this cycle (ms).
    pub avg_jitter_ms: Option<f64>,
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

            // Quality aggregates (loss/latency/jitter) count only *reachable*
            // targets. A fully-down target — e.g. an IPv6 host on a v4-only
            // network — isn't "100% packet loss on your connection", it's an
            // unavailable path; including it would wreck loss and QoE.
            if out.up {
                targets_up += 1;
                loss_sum += out.loss_pct;
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

        let avg_loss = (targets_up > 0).then(|| loss_sum / targets_up as f64);
        let avg_jitter = (jitter_n > 0).then(|| jitter_sum / jitter_n as f64);

        // Quality-of-experience scores from this cycle's best latency + mean
        // jitter + mean loss. Persisted for trending; None while offline.
        let qoe = if online {
            let q = Qoe::score(best_latency.unwrap_or(0.0), avg_jitter.unwrap_or(0.0), avg_loss.unwrap_or(0.0));
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
            avg_jitter_ms: avg_jitter,
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

        // Cache hit rate (Linux/systemd-resolved only). OFF BY DEFAULT: reading
        // `resolvectl statistics` can trigger a polkit auth prompt on some
        // systems, so we only collect it when the user opts in via the
        // `dns.cache_stats` setting (=1). Otherwise we never shell out to it.
        let cache_stats_on = self.store.get_setting_i64("dns.cache_stats").await?.unwrap_or(0) == 1;
        if let Some(cur) = cache_stats_on.then(dns_cache_stats).flatten() {
            let prev_h = self.store.get_setting_f64("dns.cache_hits").await?;
            let prev_m = self.store.get_setting_f64("dns.cache_misses").await?;
            if let (Some(ph), Some(pm)) = (prev_h, prev_m) {
                let dh = (cur.hits as f64 - ph).max(0.0);
                let dm = (cur.misses as f64 - pm).max(0.0);
                if dh + dm > 0.0 {
                    let rate = dh / (dh + dm) * 100.0;
                    self.store.set_setting("dns.cache_hit_rate", &rate.to_string(), now).await?;
                }
            }
            self.store.set_setting("dns.cache_hits", &cur.hits.to_string(), now).await?;
            self.store.set_setting("dns.cache_misses", &cur.misses.to_string(), now).await?;
        }
        Ok(())
    }

    /// Persist one bandwidth reading: the aggregate rate plus per-interface
    /// byte deltas (for usage totals). The sampler is owned by the caller
    /// because it holds counter state across cycles.
    pub async fn record_bandwidth(
        &self,
        now: i64,
        sample: tracium_probe::BandwidthSample,
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

    /// Sample the Wi-Fi link and persist it; emit a `roam` event when the BSSID
    /// changes (the device jumped to a different AP).
    pub async fn sample_wifi(&self, now: i64) -> Result<(), StoreError> {
        let Some(info) = get_wifi().await else { return Ok(()) };
        let previous = self.store.latest_wifi().await?;

        let sample = WifiSample {
            ts: now,
            ssid: info.ssid,
            bssid: info.bssid,
            rssi_dbm: info.rssi_dbm,
            quality_pct: info.quality_pct,
            link_speed_mbps: info.link_mbps,
            band: info.band,
            channel: info.channel,
        };
        self.store.insert_wifi_sample(&sample).await?;

        if let (Some(prev), Some(now_bssid)) = (previous.and_then(|p| p.bssid), &sample.bssid) {
            if &prev != now_bssid {
                let payload = format!(r#"{{"from":"{prev}","to":"{now_bssid}"}}"#);
                self.store.insert_event(now, "roam", "info", None, Some(&payload)).await?;
            }
        }
        Ok(())
    }

    /// Ping the default gateway (ICMP via the OS `ping`) for LAN latency + loss.
    pub async fn sample_gateway(&self, now: i64) -> Result<(), StoreError> {
        let Some(ip) = default_gateway_ip() else { return Ok(()) };
        if let Some(p) = ping(&ip, 3, Duration::from_secs(3)).await {
            self.store.insert_gateway_sample(now, p.rtt_avg, p.loss_pct).await?;
        }
        Ok(())
    }

    /// Enumerate LAN devices from the ARP cache and upsert them.
    pub async fn sample_devices(&self, now: i64) -> Result<(), StoreError> {
        for entry in discover_devices().await {
            self.store.upsert_device(&entry.mac, &entry.ip, now).await?;
        }
        Ok(())
    }

    /// Run a traceroute to the configured target, persist it, and emit a
    /// `route_change` event if the path differs from the previous run.
    pub async fn sample_traceroute(&self, now: i64) -> Result<(), StoreError> {
        let target = &self.config.traceroute_target;
        let Some(trace) = traceroute(
            target,
            self.config.traceroute_max_hops,
            self.config.traceroute_timeout,
        )
        .await
        else {
            return Ok(()); // tool missing / timed out — skip this round
        };

        let previous = self.store.last_route_hash(target).await?;

        // Resolve AS info per hop via Team Cymru, deduping repeated IPs.
        let mut asn_cache: std::collections::HashMap<String, Option<(String, Option<String>)>> =
            std::collections::HashMap::new();
        let mut hops: Vec<TracerouteHop> = Vec::with_capacity(trace.hops.len());
        for h in &trace.hops {
            let (asn, as_name) = match &h.ip {
                Some(ip) => {
                    if !asn_cache.contains_key(ip) {
                        asn_cache.insert(ip.clone(), tracium_probe::lookup_asn(ip).await);
                    }
                    match asn_cache.get(ip).and_then(|o| o.clone()) {
                        Some((a, n)) => (Some(a), n),
                        None => (None, None),
                    }
                }
                None => (None, None),
            };
            hops.push(TracerouteHop {
                hop_no: h.hop_no as i64,
                ip: h.ip.clone(),
                hostname: None,
                rtt_ms: h.rtt_ms,
                loss_pct: h.loss_pct,
                asn,
                as_name,
            });
        }
        self.store.save_traceroute(now, target, &trace.route_hash, &hops).await?;

        if let Some(prev) = previous {
            if prev != trace.route_hash {
                let payload = format!(r#"{{"target":"{target}","hops":{}}}"#, trace.hops.len());
                self.store
                    .insert_event(now, "route_change", "warn", None, Some(&payload))
                    .await?;
            }
        }
        Ok(())
    }

    /// Gather a security-posture snapshot (VPN heuristic, DoH/DoT reachability,
    /// firewall status, locally-listening ports) and persist it. Blocking
    /// probes run off the async runtime.
    pub async fn sample_security(&self, now: i64) -> Result<(), StoreError> {
        let to = self.config.security_timeout;
        let doh = tokio::task::spawn_blocking(move || check_doh(to)).await.unwrap_or(false);
        let dot = tokio::task::spawn_blocking(move || check_dot(to)).await.unwrap_or(false);
        let firewall = tokio::task::spawn_blocking(firewall_active).await.unwrap_or(None);
        let vpn = tokio::task::spawn_blocking(detect_vpn).await.ok();
        let nat = tokio::task::spawn_blocking(move || detect_nat(to)).await.unwrap_or(None);
        let upnp = detect_upnp(to).await;
        let open = scan_local_ports(COMMON_PORTS, Duration::from_millis(200)).await;

        let snapshot = SecuritySnapshot {
            ts: now,
            public_ip: self.store.latest_public_ip().await?,
            nat_type: nat,
            upnp_enabled: upnp,
            firewall_active: firewall,
            vpn_detected: vpn.map(|v| v.active),
            doh_active: Some(doh),
            dot_active: Some(dot),
            open_ports: serde_json::to_string(&open).ok(),
        };
        self.store.insert_security_snapshot(&snapshot).await?;
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
            eprintln!("tracium initial maintenance failed: {e}");
        }
        let mut last_maint = now_ms();
        let mut last_dns = 0i64; // 0 => sample on the first cycle
        let mut last_pubip = 0i64;
        let mut last_security = 0i64;
        let mut last_trace = 0i64;
        let mut last_devices = 0i64;
        let mut last_wifi = 0i64;
        let mut bandwidth = BandwidthSampler::new();

        let mut ticker = tokio::time::interval(self.config.interval);
        let maint_ms = self.config.maintenance_interval.as_millis() as i64;
        let dns_ms = self.config.dns_interval.as_millis() as i64;
        let pubip_ms = self.config.public_ip_interval.as_millis() as i64;
        let security_ms = self.config.security_interval.as_millis() as i64;
        let trace_ms = self.config.traceroute_interval.as_millis() as i64;
        let devices_ms = self.config.devices_interval.as_millis() as i64;
        let wifi_ms = self.config.wifi_interval.as_millis() as i64;
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
                    eprintln!("tracium monitor tick failed: {e}");
                }
            }
            // Bandwidth every cycle (cheap; rate is over the tick interval).
            if let Err(e) = self.record_bandwidth(now, bandwidth.sample()).await {
                eprintln!("tracium bandwidth sample failed: {e}");
            }
            // Gateway/LAN ping every cycle (local, fast).
            if let Err(e) = self.sample_gateway(now).await {
                eprintln!("tracium gateway sample failed: {e}");
            }
            // NIC error/drop counters (cheap /proc read).
            if let Some(er) = interface_errors() {
                if let Err(e) = self
                    .store
                    .insert_interface_errors(now, er.rx_errors, er.rx_drops, er.tx_errors, er.tx_drops)
                    .await
                {
                    eprintln!("tracium interface-errors sample failed: {e}");
                }
            }
            if now - last_dns >= dns_ms {
                if let Err(e) = self.sample_dns(now).await {
                    eprintln!("tracium dns sample failed: {e}");
                }
                last_dns = now;
            }
            if now - last_pubip >= pubip_ms {
                if let Err(e) = self.sample_public_ip(now).await {
                    eprintln!("tracium public-ip sample failed: {e}");
                }
                last_pubip = now;
            }
            if now - last_security >= security_ms {
                if let Err(e) = self.sample_security(now).await {
                    eprintln!("tracium security sample failed: {e}");
                }
                last_security = now;
            }
            if now - last_trace >= trace_ms {
                if let Err(e) = self.sample_traceroute(now).await {
                    eprintln!("tracium traceroute failed: {e}");
                }
                last_trace = now;
            }
            if now - last_devices >= devices_ms {
                if let Err(e) = self.sample_devices(now).await {
                    eprintln!("tracium device discovery failed: {e}");
                }
                last_devices = now;
            }
            if now - last_wifi >= wifi_ms {
                if let Err(e) = self.sample_wifi(now).await {
                    eprintln!("tracium wifi sample failed: {e}");
                }
                last_wifi = now;
            }
            if now - last_maint >= maint_ms {
                if let Err(e) = self.store.maintain(now).await {
                    eprintln!("tracium maintenance failed: {e}");
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
    use tracium_store::NewTarget;
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
            security_interval: Duration::from_secs(300),
            security_timeout: Duration::from_millis(300),
            wifi_interval: Duration::from_secs(30),
            devices_interval: Duration::from_secs(60),
            traceroute_interval: Duration::from_secs(600),
            traceroute_target: "1.1.1.1".to_string(),
            traceroute_max_hops: 30,
            traceroute_timeout: Duration::from_secs(25),
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

//! NetPulse connectivity probing.
//!
//! Measures reachability + round-trip latency with **unprivileged TCP-connect
//! timing** rather than ICMP echo. ICMP requires raw sockets (root or
//! `CAP_NET_RAW`) on Linux, which is unacceptable for a zero-setup desktop app.
//! Timing the TCP handshake to a known-open port (e.g. 443 on 1.1.1.1) needs no
//! privileges, works identically on Windows and Linux, and is a stable,
//! consistent signal for latency/jitter/loss trends.
//!
//! A "successful" probe = a completed TCP connection. A refused connection or a
//! timeout counts as loss. This crate has no database or GUI dependency.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use tokio::net::TcpStream;
use tokio::time::timeout;

/// How to run one probe cycle against a target.
#[derive(Debug, Clone)]
pub struct ProbeConfig {
    /// Number of connection attempts in the cycle.
    pub count: u16,
    /// Per-attempt connection timeout.
    pub timeout: Duration,
    /// Gap between attempts within the cycle (keeps a cycle from hammering).
    pub gap: Duration,
    /// Restrict resolution to this IP version (4 or 6). `None` = either.
    pub ip_version: Option<u8>,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            count: 5,
            timeout: Duration::from_secs(2),
            gap: Duration::from_millis(200),
            ip_version: None,
        }
    }
}

/// Aggregate result of one probe cycle. Latency fields are `None` on 100% loss.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeOutcome {
    pub sent: u16,
    pub received: u16,
    pub loss_pct: f64,
    pub rtt_min: Option<f64>,
    pub rtt_avg: Option<f64>,
    pub rtt_max: Option<f64>,
    /// Mean absolute difference between consecutive RTTs (ms). `None` if < 2 hits.
    pub rtt_jitter: Option<f64>,
    /// True if at least one attempt connected.
    pub up: bool,
}

/// Probe `host:port` `count` times and aggregate. `host` may be an IP or a
/// DNS name; resolution failures are reported as a fully-down outcome (never an
/// error), so the sampler records "down" rather than dropping the cycle.
pub async fn probe(host: &str, port: u16, cfg: &ProbeConfig) -> ProbeOutcome {
    let addrs = match resolve(host, port, cfg.ip_version).await {
        Some(addrs) if !addrs.is_empty() => addrs,
        _ => return down(cfg.count),
    };

    let mut rtts: Vec<f64> = Vec::with_capacity(cfg.count as usize);
    for i in 0..cfg.count {
        if i > 0 && !cfg.gap.is_zero() {
            tokio::time::sleep(cfg.gap).await;
        }
        if let Some(ms) = attempt(&addrs, cfg.timeout).await {
            rtts.push(ms);
        }
    }

    summarize(cfg.count, rtts)
}

/// One connection attempt against the first address that accepts. Returns the
/// elapsed milliseconds on success, `None` on refused/timeout.
async fn attempt(addrs: &[SocketAddr], to: Duration) -> Option<f64> {
    let start = Instant::now();
    // Try each resolved address until one connects; a target usually has one.
    for addr in addrs {
        match timeout(to, TcpStream::connect(addr)).await {
            Ok(Ok(_stream)) => {
                return Some(start.elapsed().as_secs_f64() * 1000.0);
            }
            // Refused / other error: try the next address.
            Ok(Err(_)) => continue,
            // Timed out on this address; the whole attempt is lost.
            Err(_) => return None,
        }
    }
    None
}

async fn resolve(host: &str, port: u16, ip_version: Option<u8>) -> Option<Vec<SocketAddr>> {
    let iter = tokio::net::lookup_host((host, port)).await.ok()?;
    let addrs: Vec<SocketAddr> = iter
        .filter(|a| match ip_version {
            Some(4) => a.is_ipv4(),
            Some(6) => a.is_ipv6(),
            _ => true,
        })
        .collect();
    Some(addrs)
}

fn down(count: u16) -> ProbeOutcome {
    ProbeOutcome {
        sent: count,
        received: 0,
        loss_pct: 100.0,
        rtt_min: None,
        rtt_avg: None,
        rtt_max: None,
        rtt_jitter: None,
        up: false,
    }
}

fn summarize(sent: u16, rtts: Vec<f64>) -> ProbeOutcome {
    let received = rtts.len() as u16;
    if received == 0 {
        return down(sent);
    }
    let min = rtts.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = rtts.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let avg = rtts.iter().sum::<f64>() / received as f64;

    // Jitter = mean of absolute differences between consecutive RTTs.
    let jitter = if received >= 2 {
        let diffs: f64 = rtts.windows(2).map(|w| (w[1] - w[0]).abs()).sum();
        Some(diffs / (received as f64 - 1.0))
    } else {
        None
    };

    let loss_pct = (sent - received) as f64 / sent as f64 * 100.0;
    ProbeOutcome {
        sent,
        received,
        loss_pct,
        rtt_min: Some(min),
        rtt_avg: Some(avg),
        rtt_max: Some(max),
        rtt_jitter: jitter,
        up: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    fn fast_cfg(count: u16) -> ProbeConfig {
        ProbeConfig { count, timeout: Duration::from_millis(500), gap: Duration::ZERO, ip_version: None }
    }

    #[tokio::test]
    async fn connects_to_open_port_and_measures_rtt() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Accept loop so connects complete.
        tokio::spawn(async move {
            loop {
                if listener.accept().await.is_err() {
                    break;
                }
            }
        });

        let out = probe(&addr.ip().to_string(), addr.port(), &fast_cfg(4)).await;
        assert!(out.up);
        assert_eq!(out.sent, 4);
        assert_eq!(out.received, 4);
        assert_eq!(out.loss_pct, 0.0);
        assert!(out.rtt_avg.unwrap() >= 0.0);
        assert!(out.rtt_min.unwrap() <= out.rtt_max.unwrap());
        assert!(out.rtt_jitter.is_some());
    }

    #[tokio::test]
    async fn closed_port_is_full_loss() {
        // Bind then drop to get a port nothing listens on.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let out = probe(&addr.ip().to_string(), addr.port(), &fast_cfg(3)).await;
        assert!(!out.up);
        assert_eq!(out.received, 0);
        assert_eq!(out.loss_pct, 100.0);
        assert!(out.rtt_avg.is_none());
    }

    #[tokio::test]
    async fn unresolvable_host_reports_down_not_error() {
        let out = probe("no.such.host.invalid.", 443, &fast_cfg(2)).await;
        assert!(!out.up);
        assert_eq!(out.loss_pct, 100.0);
    }

    #[tokio::test]
    #[ignore = "hits the real internet; run with --ignored"]
    async fn real_cloudflare_smoke() {
        let out = probe("1.1.1.1", 443, &ProbeConfig::default()).await;
        println!("cloudflare probe: {out:?}");
        assert!(out.up, "expected 1.1.1.1:443 to be reachable");
        assert!(out.rtt_avg.unwrap() > 0.0);
    }

    #[test]
    fn jitter_is_mean_consecutive_diff() {
        // rtts 10, 14, 12 -> diffs |4|,|-2| -> mean 3.0
        let out = summarize(3, vec![10.0, 14.0, 12.0]);
        assert_eq!(out.rtt_jitter, Some(3.0));
        assert_eq!(out.rtt_min, Some(10.0));
        assert_eq!(out.rtt_max, Some(14.0));
        assert_eq!(out.rtt_avg, Some(12.0));
    }
}

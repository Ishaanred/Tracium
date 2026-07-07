//! ICMP ping by wrapping the OS `ping` tool (Linux/macOS) or `ping` on Windows,
//! and parsing its summary.
//!
//! Used for the **gateway / LAN** where TCP-connect is useless (routers refuse
//! TCP, so a connect probe reads as 100% loss even when the gateway is healthy).
//! The system `ping` binary is setuid/capability-enabled for normal users, so
//! this stays unprivileged — same wrap-the-OS-tool approach as traceroute.

use std::time::Duration;

use tokio::process::Command;
use tokio::time::timeout;

/// Aggregate result of a ping run. Latency fields are `None` on 100% loss.
#[derive(Debug, Clone, PartialEq)]
pub struct PingResult {
    pub sent: u32,
    pub received: u32,
    pub loss_pct: f64,
    pub rtt_min: Option<f64>,
    pub rtt_avg: Option<f64>,
    pub rtt_max: Option<f64>,
}

/// Ping `host` `count` times. Returns `None` if the tool is missing/times out.
pub async fn ping(host: &str, count: u16, to: Duration) -> Option<PingResult> {
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("ping");
        c.args(["-n", &count.to_string(), host]);
        c
    };
    #[cfg(not(target_os = "windows"))]
    let mut cmd = {
        let mut c = Command::new("ping");
        // -n numeric, -c count, -W 1s per-reply wait.
        c.args(["-n", "-c", &count.to_string(), "-W", "1", host]);
        c
    };

    let output = timeout(to, cmd.output()).await.ok()?.ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    #[cfg(target_os = "windows")]
    let r = parse_windows_ping(&text);
    #[cfg(not(target_os = "windows"))]
    let r = parse_unix_ping(&text);
    r
}

/// Parse Linux/BSD `ping` summary output.
pub fn parse_unix_ping(text: &str) -> Option<PingResult> {
    let mut sent = 0;
    let mut received = 0;
    let mut loss = 100.0;
    let (mut min, mut avg, mut max) = (None, None, None);

    for line in text.lines() {
        // "3 packets transmitted, 3 received, 0% packet loss, time 2003ms"
        if line.contains("packets transmitted") {
            let nums: Vec<&str> = line.split(',').collect();
            sent = nums.first().and_then(|s| s.split_whitespace().next()).and_then(|s| s.parse().ok()).unwrap_or(0);
            received = nums.get(1).and_then(|s| s.split_whitespace().next()).and_then(|s| s.parse().ok()).unwrap_or(0);
            if let Some(l) = nums.iter().find(|s| s.contains("packet loss")) {
                loss = l.trim().split('%').next().and_then(|s| s.trim().parse().ok()).unwrap_or(100.0);
            }
        }
        // "rtt min/avg/max/mdev = 0.300/0.340/0.400/0.040 ms"
        if let Some(rest) = line.split(" = ").nth(1) {
            if line.contains("min/avg/max") {
                let parts: Vec<&str> = rest.split('/').collect();
                min = parts.first().and_then(|s| s.trim().parse().ok());
                avg = parts.get(1).and_then(|s| s.trim().parse().ok());
                max = parts.get(2).and_then(|s| s.trim().parse().ok());
            }
        }
    }
    if sent == 0 {
        return None;
    }
    Some(PingResult { sent, received, loss_pct: loss, rtt_min: min, rtt_avg: avg, rtt_max: max })
}

/// Parse Windows `ping` summary output.
pub fn parse_windows_ping(text: &str) -> Option<PingResult> {
    let mut sent = 0;
    let mut received = 0;
    let mut loss = 100.0;
    let (mut min, mut avg, mut max) = (None, None, None);

    for line in text.lines() {
        let l = line.trim();
        // "Packets: Sent = 3, Received = 3, Lost = 0 (0% loss),"
        if l.starts_with("Packets:") {
            for part in l.trim_start_matches("Packets:").split(',') {
                let p = part.trim();
                if let Some(v) = p.strip_prefix("Sent = ") {
                    sent = v.trim().parse().unwrap_or(0);
                } else if let Some(v) = p.strip_prefix("Received = ") {
                    received = v.trim().parse().unwrap_or(0);
                } else if p.contains('%') {
                    if let Some(pct) = p.split('(').nth(1) {
                        loss = pct.split('%').next().and_then(|s| s.trim().parse().ok()).unwrap_or(100.0);
                    }
                }
            }
        }
        // "Minimum = 0ms, Maximum = 1ms, Average = 0ms"
        if l.starts_with("Minimum") {
            for part in l.split(',') {
                let p = part.trim();
                let val = |s: &str| s.split('=').nth(1).and_then(|v| v.trim().trim_end_matches("ms").trim().parse::<f64>().ok());
                if p.starts_with("Minimum") {
                    min = val(p);
                } else if p.starts_with("Maximum") {
                    max = val(p);
                } else if p.starts_with("Average") {
                    avg = val(p);
                }
            }
        }
    }
    if sent == 0 {
        return None;
    }
    Some(PingResult { sent, received, loss_pct: loss, rtt_min: min, rtt_avg: avg, rtt_max: max })
}

/// The default gateway's IP (IPv4 preferred), via `netdev`.
pub fn default_gateway_ip() -> Option<String> {
    let gw = netdev::get_default_gateway().ok()?;
    gw.ipv4.first().map(|ip| ip.to_string()).or_else(|| gw.ipv6.first().map(|ip| ip.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unix_ping() {
        let sample = "\
PING 192.168.1.1 (192.168.1.1) 56(84) bytes of data.
64 bytes from 192.168.1.1: icmp_seq=1 ttl=64 time=0.340 ms
64 bytes from 192.168.1.1: icmp_seq=2 ttl=64 time=0.300 ms

--- 192.168.1.1 ping statistics ---
3 packets transmitted, 2 received, 33% packet loss, time 2003ms
rtt min/avg/max/mdev = 0.300/0.340/0.400/0.040 ms";
        let r = parse_unix_ping(sample).unwrap();
        assert_eq!(r.sent, 3);
        assert_eq!(r.received, 2);
        assert_eq!(r.loss_pct, 33.0);
        assert_eq!(r.rtt_min, Some(0.300));
        assert_eq!(r.rtt_avg, Some(0.340));
        assert_eq!(r.rtt_max, Some(0.400));
    }

    #[test]
    fn parses_windows_ping() {
        let sample = "\
Pinging 192.168.1.1 with 32 bytes of data:
Reply from 192.168.1.1: bytes=32 time=1ms TTL=64

Ping statistics for 192.168.1.1:
    Packets: Sent = 4, Received = 4, Lost = 0 (0% loss),
Approximate round trip times in milli-seconds:
    Minimum = 0ms, Maximum = 2ms, Average = 1ms";
        let r = parse_windows_ping(sample).unwrap();
        assert_eq!(r.sent, 4);
        assert_eq!(r.received, 4);
        assert_eq!(r.loss_pct, 0.0);
        assert_eq!(r.rtt_min, Some(0.0));
        assert_eq!(r.rtt_max, Some(2.0));
        assert_eq!(r.rtt_avg, Some(1.0));
    }

    #[tokio::test]
    #[ignore = "requires ping + a reachable host; run with --ignored"]
    async fn real_ping_localhost() {
        let r = ping("127.0.0.1", 2, Duration::from_secs(5)).await.unwrap();
        println!("{r:?}");
        assert!(r.received > 0);
    }
}

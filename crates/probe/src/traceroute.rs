//! Traceroute by wrapping the OS tool (`traceroute` on Linux, `tracert` on
//! Windows) and parsing its output.
//!
//! Why not a raw-socket crate (e.g. trippy)? Real traceroute must send low-TTL
//! packets and read the ICMP "time exceeded" replies, which needs root /
//! `CAP_NET_RAW` on Linux — the same privilege wall that made us pick
//! TCP-connect over ICMP for pinging. The OS tools already hold that privilege
//! (setuid / built-in), so wrapping them keeps NetPulse unprivileged. The
//! parsing is the interesting part and is fully unit-tested below.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Duration;

use tokio::process::Command;
use tokio::time::timeout;

/// One hop in a traceroute. `ip`/`rtt_ms` are `None` for a non-responding hop.
#[derive(Debug, Clone, PartialEq)]
pub struct Hop {
    pub hop_no: u32,
    pub ip: Option<String>,
    pub rtt_ms: Option<f64>,
}

/// A completed traceroute.
#[derive(Debug, Clone, PartialEq)]
pub struct TraceResult {
    pub target: String,
    pub hops: Vec<Hop>,
    /// Stable hash of the ordered responding-hop IPs — used to detect route
    /// changes between runs.
    pub route_hash: String,
}

/// Run a traceroute to `target`, parsing the platform tool's output. Returns
/// `None` if the tool is missing, times out, or produces no hops.
pub async fn traceroute(target: &str, max_hops: u8, to: Duration) -> Option<TraceResult> {
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("tracert");
        c.args(["-d", "-h", &max_hops.to_string(), target]);
        c
    };
    #[cfg(not(target_os = "windows"))]
    let mut cmd = {
        let mut c = Command::new("traceroute");
        // -n: numeric (skip rDNS, faster); -m: max hops; -q 1: one query/hop.
        c.args(["-n", "-q", "1", "-m", &max_hops.to_string(), target]);
        c
    };

    let output = timeout(to, cmd.output()).await.ok()?.ok()?;
    let text = String::from_utf8_lossy(&output.stdout);

    #[cfg(target_os = "windows")]
    let hops = parse_tracert(&text);
    #[cfg(not(target_os = "windows"))]
    let hops = parse_traceroute(&text);

    if hops.is_empty() {
        return None;
    }
    let route_hash = hash_route(&hops);
    Some(TraceResult { target: target.to_string(), hops, route_hash })
}

/// Hash the ordered responding-hop IPs (non-responders included as empty) so a
/// changed path yields a different value.
pub fn hash_route(hops: &[Hop]) -> String {
    let mut h = DefaultHasher::new();
    for hop in hops {
        hop.ip.as_deref().unwrap_or("*").hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

/// Parse GNU/BSD `traceroute -n` output.
pub fn parse_traceroute(text: &str) -> Vec<Hop> {
    let mut hops = Vec::new();
    for line in text.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        let Some(hop_no) = toks.first().and_then(|t| t.parse::<u32>().ok()) else {
            continue; // header / blank line
        };
        let rest = &toks[1..];
        hops.push(Hop { hop_no, ip: first_ip(rest), rtt_ms: first_rtt(rest) });
    }
    hops
}

/// Parse Windows `tracert -d` output.
pub fn parse_tracert(text: &str) -> Vec<Hop> {
    let mut hops = Vec::new();
    for line in text.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        let Some(hop_no) = toks.first().and_then(|t| t.parse::<u32>().ok()) else {
            continue;
        };
        let rest = &toks[1..];
        // tracert puts the IP last; RTT columns look like "5", "<1", or "*".
        hops.push(Hop { hop_no, ip: first_ip(rest), rtt_ms: first_rtt_tracert(rest) });
    }
    hops
}

/// First token that parses as an IPv4/IPv6 address.
fn first_ip(toks: &[&str]) -> Option<String> {
    toks.iter()
        .map(|t| t.trim_matches(|c| c == '(' || c == ')' || c == '[' || c == ']'))
        .find(|t| t.parse::<std::net::IpAddr>().is_ok())
        .map(|t| t.to_string())
}

/// First "<num> ms" pair in GNU traceroute output.
fn first_rtt(toks: &[&str]) -> Option<f64> {
    toks.windows(2).find_map(|w| {
        if w[1] == "ms" {
            w[0].parse::<f64>().ok()
        } else {
            None
        }
    })
}

/// First RTT column in tracert output ("5", "<1", or "*" followed by "ms").
fn first_rtt_tracert(toks: &[&str]) -> Option<f64> {
    toks.windows(2).find_map(|w| {
        if w[1] == "ms" {
            w[0].trim_start_matches('<').parse::<f64>().ok()
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gnu_traceroute() {
        let sample = "\
traceroute to 1.1.1.1 (1.1.1.1), 30 hops max, 60 byte packets
 1  192.168.1.1  1.234 ms
 2  * * *
 3  10.0.0.1  5.20 ms
 4  1.1.1.1  9.80 ms";
        let hops = parse_traceroute(sample);
        assert_eq!(hops.len(), 4);
        assert_eq!(hops[0], Hop { hop_no: 1, ip: Some("192.168.1.1".into()), rtt_ms: Some(1.234) });
        assert_eq!(hops[1], Hop { hop_no: 2, ip: None, rtt_ms: None });
        assert_eq!(hops[3].ip.as_deref(), Some("1.1.1.1"));
    }

    #[test]
    fn parses_windows_tracert() {
        let sample = "\
Tracing route to one.one.one.one [1.1.1.1]
over a maximum of 30 hops:

  1    <1 ms    <1 ms    <1 ms  192.168.1.1
  2     5 ms     4 ms     6 ms  10.0.0.1
  3     *        *        *     Request timed out.
  4     9 ms     8 ms     9 ms  1.1.1.1

Trace complete.";
        let hops = parse_tracert(sample);
        assert_eq!(hops.len(), 4);
        assert_eq!(hops[0], Hop { hop_no: 1, ip: Some("192.168.1.1".into()), rtt_ms: Some(1.0) });
        assert_eq!(hops[1].rtt_ms, Some(5.0));
        assert_eq!(hops[2], Hop { hop_no: 3, ip: None, rtt_ms: None });
        assert_eq!(hops[3].ip.as_deref(), Some("1.1.1.1"));
    }

    #[test]
    fn route_hash_changes_with_path() {
        let a = vec![Hop { hop_no: 1, ip: Some("10.0.0.1".into()), rtt_ms: Some(1.0) }];
        let b = vec![Hop { hop_no: 1, ip: Some("10.0.0.2".into()), rtt_ms: Some(1.0) }];
        assert_ne!(hash_route(&a), hash_route(&b));
        assert_eq!(hash_route(&a), hash_route(&a));
    }

    #[tokio::test]
    #[ignore = "requires traceroute/tracert + network; run with --ignored"]
    async fn real_traceroute() {
        let r = traceroute("1.1.1.1", 20, Duration::from_secs(20)).await;
        println!("{r:#?}");
        assert!(r.is_some());
    }
}

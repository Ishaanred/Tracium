//! Network-identity probes. Currently: discovering the public IP address.
//!
//! Uses a blocking HTTP client (`ureq`); callers on an async runtime should
//! invoke this via `tokio::task::spawn_blocking`.

use std::time::Duration;

/// Fetch the machine's public IP as seen from the internet. Tries Cloudflare's
/// trace endpoint first, then ipify as a fallback. Returns `None` on failure.
pub fn public_ip(timeout: Duration) -> Option<String> {
    if let Some(ip) = cloudflare_trace(timeout) {
        return Some(ip);
    }
    ipify(timeout)
}

fn cloudflare_trace(timeout: Duration) -> Option<String> {
    let body = ureq::get("https://1.1.1.1/cdn-cgi/trace")
        .timeout(timeout)
        .call()
        .ok()?
        .into_string()
        .ok()?;
    // Lines look like `ip=203.0.113.4`.
    body.lines()
        .find_map(|l| l.strip_prefix("ip="))
        .map(|ip| ip.trim().to_string())
        .filter(|ip| !ip.is_empty())
}

fn ipify(timeout: Duration) -> Option<String> {
    let ip = ureq::get("https://api.ipify.org")
        .timeout(timeout)
        .call()
        .ok()?
        .into_string()
        .ok()?
        .trim()
        .to_string();
    (!ip.is_empty()).then_some(ip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "hits the real internet; run with --ignored"]
    fn fetches_a_public_ip() {
        let ip = public_ip(Duration::from_secs(5)).expect("should discover a public IP");
        println!("public ip: {ip}");
        // Either an IPv4 (contains '.') or IPv6 (contains ':') address.
        assert!(ip.contains('.') || ip.contains(':'), "not an IP: {ip}");
    }
}

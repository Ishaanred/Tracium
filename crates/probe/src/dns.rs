//! DNS probing: time a lookup against a *specific* resolver.
//!
//! Uses `hickory-resolver` pointed at one nameserver (UDP/53) with caching
//! disabled, so each probe measures a real query — enabling side-by-side
//! resolver comparison (Cloudflare vs Google vs Quad9 vs the system resolver).

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use hickory_resolver::config::{NameServerConfig, Protocol, ResolverConfig, ResolverOpts};
use hickory_resolver::TokioAsyncResolver;

/// Result of one DNS lookup against a chosen resolver.
#[derive(Debug, Clone, PartialEq)]
pub struct DnsResult {
    pub resolver: String,
    pub query_host: String,
    pub lookup_ms: Option<f64>,
    pub success: bool,
}

/// Resolve `host` via the resolver at `resolver_ip:53`, timing the query.
/// Never errors — a failure is reported as `success = false`.
pub async fn dns_lookup(resolver_ip: IpAddr, host: &str, timeout: Duration) -> DnsResult {
    let mut config = ResolverConfig::new();
    config.add_name_server(NameServerConfig {
        socket_addr: SocketAddr::new(resolver_ip, 53),
        protocol: Protocol::Udp,
        tls_dns_name: None,
        trust_negative_responses: true,
        bind_addr: None,
    });

    let mut opts = ResolverOpts::default();
    opts.timeout = timeout;
    opts.attempts = 1;
    opts.cache_size = 0; // measure real lookups, not cache hits
    opts.use_hosts_file = false;

    let resolver = TokioAsyncResolver::tokio(config, opts);
    let start = Instant::now();
    match resolver.lookup_ip(host).await {
        Ok(res) if res.iter().next().is_some() => DnsResult {
            resolver: resolver_ip.to_string(),
            query_host: host.to_string(),
            lookup_ms: Some(start.elapsed().as_secs_f64() * 1000.0),
            success: true,
        },
        _ => DnsResult {
            resolver: resolver_ip.to_string(),
            query_host: host.to_string(),
            lookup_ms: None,
            success: false,
        },
    }
}

/// Cumulative DNS cache hit/miss counters (systemd-resolved, Linux only).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DnsCacheStats {
    pub hits: i64,
    pub misses: i64,
}

/// Read systemd-resolved's cache counters via `resolvectl statistics`.
/// `None` on non-Linux, or if resolvectl/systemd-resolved isn't present.
pub fn dns_cache_stats() -> Option<DnsCacheStats> {
    #[cfg(target_os = "linux")]
    {
        let out = std::process::Command::new("resolvectl").arg("statistics").output().ok()?;
        if !out.status.success() {
            return None;
        }
        parse_resolvectl(&String::from_utf8_lossy(&out.stdout))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Parse the `Cache Hits:` / `Cache Misses:` lines from `resolvectl statistics`.
pub fn parse_resolvectl(text: &str) -> Option<DnsCacheStats> {
    let mut hits = None;
    let mut misses = None;
    for line in text.lines() {
        let l = line.trim();
        if let Some(v) = l.strip_prefix("Cache Hits:") {
            hits = v.trim().parse().ok();
        } else if let Some(v) = l.strip_prefix("Cache Misses:") {
            misses = v.trim().parse().ok();
        }
    }
    Some(DnsCacheStats { hits: hits?, misses: misses? })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_resolvectl_statistics() {
        let sample = "\
Transactions
              Current Transactions: 0
                Total Transactions: 182475
Cache
                Current Cache Size: 57
                        Cache Hits: 3000
                      Cache Misses: 179475";
        let s = parse_resolvectl(sample).unwrap();
        assert_eq!(s.hits, 3000);
        assert_eq!(s.misses, 179475);
    }


    #[tokio::test]
    #[ignore = "hits the real internet; run with --ignored"]
    async fn real_lookup_via_cloudflare() {
        let r = dns_lookup("1.1.1.1".parse().unwrap(), "example.com", Duration::from_secs(3)).await;
        println!("dns: {r:?}");
        assert!(r.success);
        assert!(r.lookup_ms.unwrap() > 0.0);
    }

    #[tokio::test]
    async fn unreachable_resolver_fails_cleanly() {
        // 192.0.2.1 is TEST-NET-1 (RFC 5737) — guaranteed unroutable.
        let r = dns_lookup(
            "192.0.2.1".parse().unwrap(),
            "example.com",
            Duration::from_millis(300),
        )
        .await;
        assert!(!r.success);
        assert!(r.lookup_ms.is_none());
    }
}

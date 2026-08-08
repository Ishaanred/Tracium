//! IP → ASN / AS-name lookup via Team Cymru's free DNS service.
//!
//! Cymru answers TXT queries: `<reversed-ip>.origin.asn.cymru.com` gives the
//! origin ASN, and `AS<n>.asn.cymru.com` gives the AS name. No account or
//! dataset needed. IPv4 only for now; private/reserved IPs are skipped.

use std::net::Ipv4Addr;
use std::sync::OnceLock;

use hickory_resolver::config::{ResolverConfig, ResolverOpts};
use hickory_resolver::proto::rr::rdata::TXT;
use hickory_resolver::TokioAsyncResolver;

/// Shared system resolver, built once (Cymru answers over normal DNS).
fn resolver() -> &'static TokioAsyncResolver {
    static R: OnceLock<TokioAsyncResolver> = OnceLock::new();
    R.get_or_init(|| TokioAsyncResolver::tokio(ResolverConfig::default(), ResolverOpts::default()))
}

/// Look up `(AS number string, AS name)` for an IPv4 address. `None` for
/// private/reserved IPs or on any failure.
pub async fn lookup_asn(ip: &str) -> Option<(String, Option<String>)> {
    let resolver = resolver();
    let v4: Ipv4Addr = ip.parse().ok()?;
    if v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified() {
        return None;
    }
    let o = v4.octets();
    let q = format!("{}.{}.{}.{}.origin.asn.cymru.com.", o[3], o[2], o[1], o[0]);
    let txt = resolver.txt_lookup(q).await.ok()?;
    // "15169 | 8.8.8.0/24 | US | arin | ..." — first field is the ASN.
    let raw = txt_string(txt.iter().next()?);
    let asn = raw.split('|').next()?.trim().split_whitespace().next()?.to_string();
    if asn.is_empty() {
        return None;
    }
    let name = asn_name(resolver, &asn).await;
    Some((format!("AS{asn}"), name))
}

async fn asn_name(resolver: &TokioAsyncResolver, asn: &str) -> Option<String> {
    let q = format!("AS{asn}.asn.cymru.com.");
    let txt = resolver.txt_lookup(q).await.ok()?;
    // "15169 | US | arin | date | GOOGLE, US" — name is the last field.
    let raw = txt_string(txt.iter().next()?);
    raw.rsplit('|').next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn txt_string(t: &TXT) -> String {
    t.txt_data().iter().map(|b| String::from_utf8_lossy(b)).collect::<Vec<_>>().join("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "hits Team Cymru DNS; run with --ignored"]
    async fn lookup_google_dns() {
        let asn = lookup_asn("8.8.8.8").await;
        println!("8.8.8.8 -> {asn:?}");
        assert!(asn.is_some());
        assert!(lookup_asn("192.168.1.1").await.is_none()); // private -> None
    }
}

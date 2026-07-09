//! Security-posture probes: VPN/interface detection, encrypted-DNS capability,
//! firewall status, and a local open-port scan.
//!
//! These are best-effort and platform-aware; each degrades to "unknown"
//! (`None`) rather than failing. Nothing here needs elevated privileges:
//! interface enumeration and TCP-connect scanning are unprivileged, and
//! firewall *status* reads use native CLIs (rule reads that need admin are not
//! attempted).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time::timeout;

/// VPN heuristic result.
#[derive(Debug, Clone, PartialEq)]
pub struct VpnStatus {
    pub active: bool,
    pub interfaces: Vec<String>,
}

/// Interface-name prefixes that indicate a VPN/virtual tunnel.
const VPN_PREFIXES: &[&str] = &["wg", "tun", "tap", "ppp", "utun", "ipsec", "nordlynx", "proton"];

/// Detect a likely-active VPN by scanning interface names for tunnel prefixes.
/// Heuristic by nature — reports which interfaces matched.
pub fn detect_vpn() -> VpnStatus {
    let mut interfaces = Vec::new();
    for iface in netdev::get_interfaces() {
        let name = iface.name.to_lowercase();
        if VPN_PREFIXES.iter().any(|p| name.starts_with(p)) {
            interfaces.push(iface.name);
        }
    }
    VpnStatus { active: !interfaces.is_empty(), interfaces }
}

/// Can we reach a DNS-over-HTTPS endpoint (Cloudflare JSON API)? Blocking HTTP;
/// call via `spawn_blocking` from an async runtime.
pub fn check_doh(to: Duration) -> bool {
    ureq::get("https://cloudflare-dns.com/dns-query")
        .query("name", "example.com")
        .query("type", "A")
        .set("accept", "application/dns-json")
        .timeout(to)
        .call()
        .map(|r| r.status() == 200)
        .unwrap_or(false)
}

/// Is the DNS-over-TLS port (853) reachable on Cloudflare? A TCP-connect probe —
/// a reachability signal, not a full DoT handshake. Blocking; use `spawn_blocking`.
pub fn check_dot(to: Duration) -> bool {
    let addr: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 853);
    std::net::TcpStream::connect_timeout(&addr, to).is_ok()
}

/// Best-effort NAT type via STUN — a passive check (no changes made). Queries
/// two public STUN servers from one socket and compares the mapped address:
/// a stable mapping = cone NAT ("moderate", or "open" if the port is preserved),
/// a changing mapping = symmetric NAT ("strict"). `None`/"blocked" if STUN can't
/// be reached. Full RFC5780 classification (inbound reachability) isn't attempted.
pub fn detect_nat(timeout: Duration) -> Option<String> {
    use std::net::{ToSocketAddrs, UdpSocket};
    use stunclient::StunClient;

    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.set_read_timeout(Some(timeout)).ok()?;
    let local_port = sock.local_addr().ok()?.port();

    let servers = ["stun.l.google.com:19302", "stun.cloudflare.com:3478"];
    let mut mapped = Vec::new();
    for s in servers {
        if let Some(addr) = s.to_socket_addrs().ok().and_then(|mut a| a.find(|a| a.is_ipv4())) {
            if let Ok(ext) = StunClient::new(addr).query_external_address(&sock) {
                mapped.push(ext);
            }
        }
    }

    Some(match mapped.as_slice() {
        [] => "blocked".to_string(),
        [_] => "unknown".to_string(),
        [a, b, ..] => {
            if a == b {
                if a.port() == local_port { "open".into() } else { "moderate".into() }
            } else {
                "strict".into()
            }
        }
    })
}

/// Is UPnP/IGD advertised by the gateway? A passive discovery — `Some(true)` if a
/// gateway responds, `Some(false)` if none (UPnP off/absent).
pub async fn detect_upnp(timeout: Duration) -> Option<bool> {
    match tokio::time::timeout(timeout, igd_next::aio::tokio::search_gateway(Default::default())).await
    {
        Ok(Ok(_gw)) => Some(true),
        Ok(Err(_)) => Some(false),
        Err(_) => Some(false),
    }
}

/// Best-effort firewall status via native CLIs. `None` = couldn't determine.
pub fn firewall_active() -> Option<bool> {
    #[cfg(target_os = "linux")]
    {
        // `ufw status` requires root, so it's useless for an unprivileged app.
        // The systemd service state is readable without root and covers the
        // common firewalls. `systemctl is-active <svc>` prints "active" (exit 0)
        // or "inactive"/"unknown" (nonzero) — we read stdout regardless of exit.
        let mut systemctl_ran = false;
        for svc in ["ufw", "firewalld", "nftables"] {
            if let Ok(out) = std::process::Command::new("systemctl")
                .args(["is-active", svc])
                .output()
            {
                systemctl_ran = true;
                if String::from_utf8_lossy(&out.stdout).trim() == "active" {
                    return Some(true);
                }
            }
        }
        // systemctl worked but nothing active -> off; systemctl absent -> unknown.
        if systemctl_ran {
            Some(false)
        } else {
            None
        }
    }
    #[cfg(target_os = "windows")]
    {
        // "netsh advfirewall show allprofiles state" lists State ON/OFF lines
        // and works without admin.
        let out = std::process::Command::new("netsh")
            .args(["advfirewall", "show", "allprofiles", "state"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())?;
        Some(out.to_uppercase().contains("STATE") && out.to_uppercase().contains("ON"))
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

/// Common service ports worth reporting when locally listening.
pub const COMMON_PORTS: &[u16] = &[
    22, 80, 135, 139, 443, 445, 631, 3000, 3306, 3389, 5432, 5900, 6379, 8000, 8080, 8443, 9000,
];

/// Scan `ports` on loopback and return those accepting a TCP connection.
/// This reports **locally listening** ports (a self-audit) — not what the
/// public internet can reach (which needs an external reflector).
pub async fn scan_local_ports(ports: &[u16], per_port: Duration) -> Vec<u16> {
    let mut open = Vec::new();
    for &port in ports {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        if let Ok(Ok(_)) = timeout(per_port, TcpStream::connect(addr)).await {
            open.push(port);
        }
    }
    open
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn vpn_detection_runs() {
        // Can't assert a value (depends on host), but it must not panic.
        let v = detect_vpn();
        // interfaces present iff active.
        assert_eq!(v.active, !v.interfaces.is_empty());
    }

    #[tokio::test]
    async fn scan_finds_a_listening_port() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { while listener.accept().await.is_ok() {} });

        let open = scan_local_ports(&[port], Duration::from_millis(300)).await;
        assert_eq!(open, vec![port]);
    }

    #[tokio::test]
    async fn scan_skips_closed_port() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let open = scan_local_ports(&[port], Duration::from_millis(200)).await;
        assert!(open.is_empty());
    }

    #[tokio::test]
    #[ignore = "hits the network; run with --ignored"]
    async fn real_nat_and_upnp() {
        let nat = detect_nat(Duration::from_secs(3));
        let upnp = detect_upnp(Duration::from_secs(3)).await;
        println!("nat: {nat:?}  upnp: {upnp:?}");
        assert!(nat.is_some());
    }

    #[test]
    #[ignore = "hits the real internet; run with --ignored"]
    fn encrypted_dns_reachable() {
        assert!(check_doh(Duration::from_secs(5)), "DoH should work");
        assert!(check_dot(Duration::from_secs(5)), "DoT should work");
    }
}

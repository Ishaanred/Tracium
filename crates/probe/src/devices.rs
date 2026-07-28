//! Connected-device discovery via the OS **ARP cache** — unprivileged and
//! dependency-free.
//!
//! Reading the ARP cache (`/proc/net/arp` on Linux, `arp -a` on Windows) lists
//! LAN neighbours the machine has recently talked to, with IP + MAC. This needs
//! no elevation (unlike *sending* ARP probes) and no extra crate. Friendly
//! names (mDNS via a crate like `mdns-sd`) are a future enrichment.

/// One ARP-cache entry: a LAN device.
#[derive(Debug, Clone, PartialEq)]
pub struct ArpEntry {
    pub ip: String,
    pub mac: String,
}

/// Discover LAN devices from the OS ARP cache.
///
/// On Linux, `/proc/net/arp` is system-wide — it includes every Docker/VM
/// bridge network's neighbour table too (`br-*`, `docker0`, `veth*`), which
/// churn through disposable MACs and would otherwise drown out real LAN
/// devices. We restrict results to the machine's default route interface
/// (e.g. `eth0`/`wlan0`), the one actually facing the LAN.
pub async fn discover_devices() -> Vec<ArpEntry> {
    #[cfg(target_os = "linux")]
    {
        let default_iface = netdev::get_default_interface().ok().map(|i| i.name);
        std::fs::read_to_string("/proc/net/arp")
            .map(|t| parse_proc_net_arp(&t, default_iface.as_deref()))
            .unwrap_or_default()
    }
    #[cfg(target_os = "windows")]
    {
        match tokio::process::Command::new("arp").arg("-a").output().await {
            Ok(o) if o.status.success() => parse_arp_a(&String::from_utf8_lossy(&o.stdout)),
            _ => Vec::new(),
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        Vec::new()
    }
}

/// True for MACs that are placeholders/broadcast, not real devices.
fn is_bogus_mac(mac: &str) -> bool {
    let m = mac.to_lowercase();
    m == "00:00:00:00:00:00" || m == "ff:ff:ff:ff:ff:ff" || m.is_empty()
}

/// Parse Linux `/proc/net/arp`. When `iface` is given, only entries on that
/// device are kept — see [`discover_devices`] for why.
pub fn parse_proc_net_arp(text: &str, iface: Option<&str>) -> Vec<ArpEntry> {
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        // IP  HWtype  Flags  HWaddress  Mask  Device
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 6 {
            continue;
        }
        // Flags 0x0 => incomplete entry; skip.
        if cols[2] == "0x0" {
            continue;
        }
        if let Some(want) = iface {
            if cols[5] != want {
                continue;
            }
        }
        let (ip, mac) = (cols[0], cols[3]);
        if is_bogus_mac(mac) {
            continue;
        }
        out.push(ArpEntry { ip: ip.to_string(), mac: mac.to_lowercase() });
    }
    out
}

/// Parse Windows `arp -a` output.
pub fn parse_arp_a(text: &str) -> Vec<ArpEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 2 {
            continue;
        }
        // A data line starts with an IPv4 address; MAC uses '-' separators.
        if cols[0].parse::<std::net::Ipv4Addr>().is_err() {
            continue;
        }
        let mac = cols[1].replace('-', ":").to_lowercase();
        if is_bogus_mac(&mac) || !mac.contains(':') {
            continue;
        }
        out.push(ArpEntry { ip: cols[0].to_string(), mac });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_net_arp() {
        let sample = "\
IP address       HW type     Flags       HW address            Mask     Device
192.168.1.1      0x1         0x2         aa:bb:cc:dd:ee:ff     *        wlan0
192.168.1.42     0x1         0x2         11:22:33:44:55:66     *        wlan0
192.168.1.99     0x1         0x0         00:00:00:00:00:00     *        wlan0";
        let d = parse_proc_net_arp(sample, None);
        assert_eq!(d.len(), 2, "incomplete entry should be skipped");
        assert_eq!(d[0], ArpEntry { ip: "192.168.1.1".into(), mac: "aa:bb:cc:dd:ee:ff".into() });
    }

    #[test]
    fn filters_to_the_given_interface() {
        // Docker bridge neighbours (br-*) should be excluded when we ask for
        // the real LAN interface only.
        let sample = "\
IP address       HW type     Flags       HW address            Mask     Device
192.168.1.1      0x1         0x2         aa:bb:cc:dd:ee:ff     *        enp8s0
172.18.0.3       0x1         0x2         f2:83:d0:5d:0a:c2     *        br-fbb094e3a367";
        let d = parse_proc_net_arp(sample, Some("enp8s0"));
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].ip, "192.168.1.1");
    }

    #[test]
    fn parses_windows_arp_a() {
        let sample = "\
Interface: 192.168.1.5 --- 0x5
  Internet Address      Physical Address      Type
  192.168.1.1           aa-bb-cc-dd-ee-ff     dynamic
  192.168.1.42          11-22-33-44-55-66     dynamic
  192.168.1.255         ff-ff-ff-ff-ff-ff     static";
        let d = parse_arp_a(sample);
        assert_eq!(d.len(), 2, "broadcast MAC should be skipped");
        assert_eq!(d[1], ArpEntry { ip: "192.168.1.42".into(), mac: "11:22:33:44:55:66".into() });
    }

    #[tokio::test]
    async fn discover_runs_on_this_host() {
        // Reads the real ARP cache; count is host-dependent but must not panic.
        let _ = discover_devices().await;
    }
}

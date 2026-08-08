//! Low-level NIC error/drop counters from `/proc/net/dev` (Linux).
//!
//! These are cumulative-since-boot counts of packets the interface errored on
//! or dropped — normally 0; non-zero points to cabling/driver/saturation
//! trouble. Windows (`GetIfTable`) is a future addition.

/// Aggregate error/drop counters across real interfaces (loopback excluded).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct IfaceErrors {
    pub rx_errors: i64,
    pub rx_drops: i64,
    pub tx_errors: i64,
    pub tx_drops: i64,
}

/// Read the current NIC error/drop totals, or `None` if unavailable.
pub fn interface_errors() -> Option<IfaceErrors> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/net/dev").ok().map(|t| parse_proc_net_dev(&t))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Parse `/proc/net/dev`, summing errors/drops over non-loopback interfaces.
/// Columns after `iface:` are: rx_bytes packets **errs drop** fifo frame
/// compressed multicast tx_bytes packets **errs drop** …
pub fn parse_proc_net_dev(text: &str) -> IfaceErrors {
    let mut e = IfaceErrors::default();
    for line in text.lines() {
        let Some((name, rest)) = line.split_once(':') else { continue };
        if name.trim() == "lo" {
            continue;
        }
        let f: Vec<i64> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        if f.len() >= 12 {
            e.rx_errors += f[2];
            e.rx_drops += f[3];
            e.tx_errors += f[10];
            e.tx_drops += f[11];
        }
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_net_dev() {
        let sample = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 1000       10    0    0    0     0          0         0     1000       10    0    0    0     0       0          0
  eth0: 5000       50    2    3    0     0          0         0     6000       60    4    5    0     0       0          0
 wlan0: 7000       70    1    0    0     0          0         0     8000       80    0    1    0     0       0          0";
        let e = parse_proc_net_dev(sample);
        // lo excluded; eth0 + wlan0 summed.
        assert_eq!(e.rx_errors, 3); // 2 + 1
        assert_eq!(e.rx_drops, 3); // 3 + 0
        assert_eq!(e.tx_errors, 4); // 4 + 0
        assert_eq!(e.tx_drops, 6); // 5 + 1
    }
}

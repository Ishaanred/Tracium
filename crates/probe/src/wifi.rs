//! Wi-Fi link metrics for the *currently connected* network, by parsing native
//! tools: `iw dev … link` on Linux, `netsh wlan show interfaces` on Windows.
//!
//! Reads the active link's RSSI, band, channel, and rate — no crate, no
//! privilege. Returns `None` when there's no Wi-Fi (e.g. wired hosts) or the
//! tool is absent. Noise/retransmit/MCS need deeper nl80211 access and are left
//! for a future native-netlink pass.

/// Current Wi-Fi link info. Fields are `None` when a platform doesn't report them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WifiInfo {
    pub ssid: Option<String>,
    pub bssid: Option<String>,
    pub rssi_dbm: Option<i64>,
    pub quality_pct: Option<f64>,
    pub link_mbps: Option<f64>,
    pub freq_mhz: Option<i64>,
    pub channel: Option<i64>,
    pub band: Option<String>,
}

/// Read the active Wi-Fi link, or `None` if unavailable.
pub async fn get_wifi() -> Option<WifiInfo> {
    #[cfg(target_os = "linux")]
    {
        let iface = linux_wifi_iface().await?;
        let out = run("iw", &["dev", &iface, "link"]).await?;
        if out.contains("Not connected") {
            return None;
        }
        let info = parse_iw_link(&out);
        (info != WifiInfo::default()).then_some(info)
    }
    #[cfg(target_os = "windows")]
    {
        let out = run("netsh", &["wlan", "show", "interfaces"]).await?;
        let info = parse_netsh(&out);
        (info != WifiInfo::default()).then_some(info)
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        None
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
async fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = tokio::process::Command::new(cmd).args(args).output().await.ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(target_os = "linux")]
async fn linux_wifi_iface() -> Option<String> {
    let out = run("iw", &["dev"]).await?;
    // Lines like: "\tInterface wlan0"
    out.lines()
        .find_map(|l| l.trim().strip_prefix("Interface ").map(|s| s.trim().to_string()))
}

/// 2.4 / 5 / 6 GHz band label from a frequency in MHz.
fn band_from_freq(mhz: i64) -> &'static str {
    if mhz < 2500 {
        "2.4"
    } else if mhz < 5925 {
        "5"
    } else {
        "6"
    }
}

/// Channel number from frequency (2.4 and 5/6 GHz plans).
fn channel_from_freq(mhz: i64) -> Option<i64> {
    match mhz {
        2412..=2472 => Some((mhz - 2407) / 5),
        2484 => Some(14),
        5000..=7115 => Some((mhz - 5000) / 5),
        _ => None,
    }
}

/// Parse `iw dev <iface> link` output.
pub fn parse_iw_link(text: &str) -> WifiInfo {
    let mut info = WifiInfo::default();
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("Connected to ") {
            info.bssid = rest.split_whitespace().next().map(|s| s.to_lowercase());
        } else if let Some(rest) = t.strip_prefix("SSID: ") {
            info.ssid = Some(rest.trim().to_string());
        } else if let Some(rest) = t.strip_prefix("freq: ") {
            info.freq_mhz = rest.trim().parse().ok();
        } else if let Some(rest) = t.strip_prefix("signal: ") {
            // "-45 dBm"
            info.rssi_dbm = rest.split_whitespace().next().and_then(|s| s.parse().ok());
        } else if let Some(rest) = t.strip_prefix("tx bitrate: ") {
            info.link_mbps = rest.split_whitespace().next().and_then(|s| s.parse().ok());
        }
    }
    if let Some(f) = info.freq_mhz {
        info.band = Some(band_from_freq(f).to_string());
        info.channel = channel_from_freq(f);
    }
    info
}

/// Parse `netsh wlan show interfaces` output.
pub fn parse_netsh(text: &str) -> WifiInfo {
    let mut info = WifiInfo::default();
    for line in text.lines() {
        let Some((key, val)) = line.split_once(':') else { continue };
        let (key, val) = (key.trim(), val.trim());
        match key {
            "SSID" => info.ssid = Some(val.to_string()),
            "BSSID" => info.bssid = Some(val.to_lowercase()),
            "Channel" => info.channel = val.parse().ok(),
            "Signal" => {
                let pct: Option<f64> = val.trim_end_matches('%').trim().parse().ok();
                info.quality_pct = pct;
                // Rough dBm from quality%: 0%→-100 dBm, 100%→-50 dBm.
                info.rssi_dbm = pct.map(|p| (p / 2.0 - 100.0) as i64);
            }
            "Receive rate (Mbps)" | "Transmit rate (Mbps)" => {
                if info.link_mbps.is_none() {
                    info.link_mbps = val.parse().ok();
                }
            }
            _ => {}
        }
    }
    if let Some(c) = info.channel {
        info.band = Some(if c <= 14 { "2.4" } else { "5" }.to_string());
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_iw_link() {
        let sample = "\
Connected to aa:bb:cc:dd:ee:ff (on wlan0)
	SSID: HomeNet
	freq: 5180
	signal: -45 dBm
	tx bitrate: 300.0 MBit/s";
        let w = parse_iw_link(sample);
        assert_eq!(w.ssid.as_deref(), Some("HomeNet"));
        assert_eq!(w.bssid.as_deref(), Some("aa:bb:cc:dd:ee:ff"));
        assert_eq!(w.rssi_dbm, Some(-45));
        assert_eq!(w.freq_mhz, Some(5180));
        assert_eq!(w.band.as_deref(), Some("5"));
        assert_eq!(w.channel, Some(36));
        assert_eq!(w.link_mbps, Some(300.0));
    }

    #[test]
    fn parses_netsh() {
        let sample = "\
    Name                   : Wi-Fi
    SSID                   : HomeNet
    BSSID                  : AA-BB-CC-DD-EE-FF
    Signal                 : 90%
    Channel                : 36
    Receive rate (Mbps)    : 300";
        let w = parse_netsh(sample);
        assert_eq!(w.ssid.as_deref(), Some("HomeNet"));
        assert_eq!(w.bssid.as_deref(), Some("aa-bb-cc-dd-ee-ff"));
        assert_eq!(w.quality_pct, Some(90.0));
        assert_eq!(w.rssi_dbm, Some(-55)); // 90/2 - 100
        assert_eq!(w.channel, Some(36));
        assert_eq!(w.band.as_deref(), Some("5"));
        assert_eq!(w.link_mbps, Some(300.0));
    }

    #[tokio::test]
    async fn get_wifi_runs() {
        // Wired/headless hosts return None; must not panic.
        let _ = get_wifi().await;
    }
}

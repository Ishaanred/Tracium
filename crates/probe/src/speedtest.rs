//! Speed test by wrapping `librespeed-cli` (invoked as an external process, so
//! its LGPL license doesn't affect NetPulse's licensing).
//!
//! Every open-source speed test needs a *server*; librespeed-cli talks to the
//! LibreSpeed community server list (or a self-hosted one) and prints JSON,
//! which we parse. Returns `None` if the tool isn't installed or the run fails.
//!
//! Bufferbloat (latency under load) is NOT provided by librespeed-cli — it
//! needs controlled load generation and is deferred to a future engine.

use std::time::Duration;

use serde::Deserialize;
use tokio::time::timeout;

/// A parsed speed-test result. Rates are Mbps; latencies are ms.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpeedResult {
    pub download_mbps: Option<f64>,
    pub upload_mbps: Option<f64>,
    pub ping_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
    pub server: Option<String>,
}

/// Run the default `librespeed-cli` (on PATH). See [`run_speedtest_with`].
pub async fn run_speedtest(to: Duration) -> Option<SpeedResult> {
    run_speedtest_with("librespeed-cli", to).await
}

/// Run a specific `librespeed-cli` binary (e.g. a bundled sidecar path) with
/// `--json` and parse the result. `to` bounds the whole run.
pub async fn run_speedtest_with(bin: &str, to: Duration) -> Option<SpeedResult> {
    let output = timeout(to, tokio::process::Command::new(bin).arg("--json").output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_librespeed_json(&String::from_utf8_lossy(&output.stdout))
}

#[derive(Deserialize)]
struct LsServer {
    name: Option<String>,
}
#[derive(Deserialize)]
struct LsResult {
    ping: Option<f64>,
    jitter: Option<f64>,
    download: Option<f64>,
    upload: Option<f64>,
    server: Option<LsServer>,
}

/// Parse librespeed-cli's JSON (an array with one result object).
pub fn parse_librespeed_json(s: &str) -> Option<SpeedResult> {
    let arr: Vec<LsResult> = serde_json::from_str(s).ok()?;
    let r = arr.into_iter().next()?;
    Some(SpeedResult {
        download_mbps: r.download,
        upload_mbps: r.upload,
        ping_ms: r.ping,
        jitter_ms: r.jitter,
        server: r.server.and_then(|s| s.name),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_librespeed_output() {
        let sample = r#"[
          {
            "timestamp": "2026-07-07T00:00:00Z",
            "server": {"name": "Community Server X", "url": "https://x"},
            "client": {"ip": "203.0.113.4"},
            "bytes_sent": 12345,
            "bytes_received": 67890,
            "ping": 12.3,
            "jitter": 1.4,
            "upload": 10.25,
            "download": 95.4
          }
        ]"#;
        let r = parse_librespeed_json(sample).unwrap();
        assert_eq!(r.download_mbps, Some(95.4));
        assert_eq!(r.upload_mbps, Some(10.25));
        assert_eq!(r.ping_ms, Some(12.3));
        assert_eq!(r.jitter_ms, Some(1.4));
        assert_eq!(r.server.as_deref(), Some("Community Server X"));
    }

    #[test]
    fn bad_json_is_none() {
        assert!(parse_librespeed_json("not json").is_none());
        assert!(parse_librespeed_json("[]").is_none());
    }
}

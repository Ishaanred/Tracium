//! Bufferbloat measurement: latency under load.
//!
//! librespeed-cli only measures ping at idle, so we run our own latency probe
//! *concurrently* while it saturates the link: baseline the idle latency, start
//! the speed test as the load generator, sample latency to a fast host
//! throughout, then grade A–F from how much latency rose under load. This is
//! the classic "responsiveness under load" that a plain speed test misses.

use std::time::Duration;

use crate::speedtest::{run_speedtest_with, SpeedResult};
use crate::{probe, ProbeConfig};

/// Bufferbloat outcome: idle vs under-load latency and a letter grade.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Bufferbloat {
    pub idle_ms: f64,
    pub loaded_ms: f64,
    /// Latency increase under load (ms) — the number the grade is based on.
    pub increase_ms: f64,
    pub grade: String,
}

/// A speed test plus its bufferbloat measurement (either may be `None`).
#[derive(Debug, Clone)]
pub struct SpeedAndBloat {
    pub speed: Option<SpeedResult>,
    pub bufferbloat: Option<Bufferbloat>,
}

/// A→F from the latency increase under load (Waveform-style thresholds).
pub fn grade(increase_ms: f64) -> &'static str {
    match increase_ms {
        x if x < 30.0 => "A",
        x if x < 60.0 => "B",
        x if x < 100.0 => "C",
        x if x < 200.0 => "D",
        _ => "F",
    }
}

/// Run `bin` (librespeed-cli) while probing latency to `latency_host:port`
/// throughout, and compute a bufferbloat grade. `to` bounds the whole run.
pub async fn run_speedtest_bufferbloat(
    bin: &str,
    latency_host: &str,
    port: u16,
    to: Duration,
) -> SpeedAndBloat {
    // 1. Idle baseline.
    let idle_cfg = ProbeConfig {
        count: 5,
        timeout: Duration::from_secs(2),
        gap: Duration::from_millis(100),
        ip_version: Some(4),
    };
    let idle = probe(latency_host, port, &idle_cfg).await.rtt_avg;

    // 2. Start the load (speed test) as a concurrent task.
    let bin_owned = bin.to_string();
    let handle = tokio::spawn(async move { run_speedtest_with(&bin_owned, to).await });

    // 3. Sample latency while the load runs.
    let one = ProbeConfig {
        count: 1,
        timeout: Duration::from_secs(2),
        gap: Duration::ZERO,
        ip_version: Some(4),
    };
    let mut loaded = Vec::new();
    while !handle.is_finished() {
        if let Some(ms) = probe(latency_host, port, &one).await.rtt_avg {
            loaded.push(ms);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let speed = handle.await.ok().flatten();

    // 4. Grade from the average under-load latency vs idle.
    let bufferbloat = match idle {
        Some(i) if !loaded.is_empty() => {
            let loaded_avg = loaded.iter().sum::<f64>() / loaded.len() as f64;
            let increase = (loaded_avg - i).max(0.0);
            Some(Bufferbloat {
                idle_ms: i,
                loaded_ms: loaded_avg,
                increase_ms: increase,
                grade: grade(increase).to_string(),
            })
        }
        _ => None,
    };

    SpeedAndBloat { speed, bufferbloat }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "runs a real speed test (uses data); needs librespeed-cli"]
    async fn real_bufferbloat() {
        let out = run_speedtest_bufferbloat("librespeed-cli", "1.1.1.1", 443, Duration::from_secs(90)).await;
        println!("speed: {:?}\nbufferbloat: {:?}", out.speed, out.bufferbloat);
        assert!(out.speed.is_some(), "speed test should complete");
        assert!(out.bufferbloat.is_some(), "should produce a bufferbloat grade");
    }

    #[test]
    fn grade_thresholds() {
        assert_eq!(grade(0.0), "A");
        assert_eq!(grade(29.9), "A");
        assert_eq!(grade(30.0), "B");
        assert_eq!(grade(59.9), "B");
        assert_eq!(grade(60.0), "C");
        assert_eq!(grade(150.0), "D");
        assert_eq!(grade(500.0), "F");
    }
}

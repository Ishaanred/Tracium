//! Quality-of-Experience scoring.
//!
//! Turns raw latency / jitter / loss into 0–100 scores per use-case. This is a
//! heuristic v1 derived purely from connectivity metrics; throughput-aware
//! factors (for streaming especially) arrive once speed-test data exists.
//!
//! Each score starts at 100 and subtracts penalties tuned to how sensitive that
//! activity is to each impairment. Gaming punishes latency + jitter hardest;
//! streaming tolerates latency but not loss; VoIP/video sit in between.

/// QoE scores, each clamped to 0–100 (higher is better).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Qoe {
    pub gaming: f64,
    pub video_call: f64,
    pub streaming: f64,
    pub web: f64,
    pub voip: f64,
}

impl Qoe {
    /// Score a connection from `latency_ms`, `jitter_ms`, and `loss_pct` (0–100).
    pub fn score(latency_ms: f64, jitter_ms: f64, loss_pct: f64) -> Self {
        Qoe {
            gaming: clamp(
                100.0 - ramp(latency_ms, 20.0, 150.0) - ramp(jitter_ms, 5.0, 50.0) - loss_pct * 8.0,
            ),
            voip: clamp(
                100.0 - ramp(latency_ms, 150.0, 400.0) - ramp(jitter_ms, 10.0, 60.0) - loss_pct * 10.0,
            ),
            video_call: clamp(
                100.0 - ramp(latency_ms, 150.0, 400.0) - ramp(jitter_ms, 15.0, 80.0) - loss_pct * 9.0,
            ),
            streaming: clamp(
                100.0 - ramp(latency_ms, 300.0, 1000.0) * 0.5 - loss_pct * 6.0,
            ),
            web: clamp(100.0 - ramp(latency_ms, 100.0, 800.0) - loss_pct * 4.0),
        }
    }
}

/// Linear penalty: 0 below `soft`, rising to 100 at `hard`, capped at 100.
fn ramp(v: f64, soft: f64, hard: f64) -> f64 {
    if v <= soft {
        0.0
    } else if v >= hard {
        100.0
    } else {
        (v - soft) / (hard - soft) * 100.0
    }
}

fn clamp(x: f64) -> f64 {
    x.clamp(0.0, 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pristine_connection_scores_near_perfect() {
        let q = Qoe::score(8.0, 1.0, 0.0);
        assert_eq!(q.gaming, 100.0);
        assert_eq!(q.voip, 100.0);
        assert_eq!(q.web, 100.0);
    }

    #[test]
    fn loss_tanks_realtime_scores() {
        let q = Qoe::score(30.0, 5.0, 5.0); // 5% loss
        assert!(q.gaming < 60.0, "gaming should suffer badly: {}", q.gaming);
        assert!(q.voip < q.streaming, "voip more loss-sensitive than streaming");
    }

    #[test]
    fn high_latency_hurts_gaming_more_than_streaming() {
        let q = Qoe::score(250.0, 5.0, 0.0);
        assert!(q.gaming < q.streaming);
        assert!(q.streaming > 90.0, "streaming tolerates latency: {}", q.streaming);
    }

    #[test]
    fn scores_never_leave_bounds() {
        let q = Qoe::score(5000.0, 500.0, 100.0);
        for s in [q.gaming, q.voip, q.video_call, q.streaming, q.web] {
            assert!((0.0..=100.0).contains(&s), "out of bounds: {s}");
        }
    }
}

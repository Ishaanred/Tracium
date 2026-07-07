//! Local bandwidth sampling via `sysinfo`.
//!
//! Reads OS interface byte counters cross-platform (Linux `/proc`, Windows
//! IpHelper). `sysinfo` reports bytes *since the last refresh*, so the sampler
//! keeps state between calls and divides by wall-clock elapsed to get a rate.
//! This is synchronous and fast; no async needed.

use std::time::Instant;

use sysinfo::Networks;

/// Per-interface byte deltas over the sampling window.
#[derive(Debug, Clone)]
pub struct IfaceDelta {
    pub iface: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// One bandwidth sample: aggregate rate (bits/sec) + per-interface deltas.
#[derive(Debug, Clone)]
pub struct BandwidthSample {
    pub rx_bps: u64,
    pub tx_bps: u64,
    pub per_iface: Vec<IfaceDelta>,
    pub secs: f64,
}

/// Stateful bandwidth sampler. Construct once, then call [`sample`] on a cadence.
pub struct BandwidthSampler {
    networks: Networks,
    last: Instant,
}

impl BandwidthSampler {
    pub fn new() -> Self {
        // Initial refresh establishes a baseline so the first sample() measures a
        // real interval rather than bytes-since-boot.
        let networks = Networks::new_with_refreshed_list();
        Self { networks, last: Instant::now() }
    }

    /// Refresh counters and return the deltas + rate since the previous call.
    pub fn sample(&mut self) -> BandwidthSample {
        self.networks.refresh();
        let secs = self.last.elapsed().as_secs_f64().max(1e-3);
        self.last = Instant::now();

        let mut per_iface = Vec::new();
        let (mut rx, mut tx) = (0u64, 0u64);
        for (name, data) in &self.networks {
            let (r, t) = (data.received(), data.transmitted());
            rx += r;
            tx += t;
            if r > 0 || t > 0 {
                per_iface.push(IfaceDelta { iface: name.clone(), rx_bytes: r, tx_bytes: t });
            }
        }

        BandwidthSample {
            rx_bps: (rx as f64 * 8.0 / secs) as u64,
            tx_bps: (tx as f64 * 8.0 / secs) as u64,
            per_iface,
            secs,
        }
    }
}

impl Default for BandwidthSampler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampler_produces_a_reading() {
        let mut s = BandwidthSampler::new();
        // Two samples; the second has a positive elapsed window and never panics.
        let _ = s.sample();
        let out = s.sample();
        assert!(out.secs > 0.0);
        // Rates are always non-negative by construction (u64).
        let _ = (out.rx_bps, out.tx_bps);
    }
}

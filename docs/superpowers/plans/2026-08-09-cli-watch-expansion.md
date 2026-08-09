# Expanded `traciumd watch` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expand `traciumd watch`'s live dashboard to surface wifi, bandwidth, security, devices, route, DNS, and recent-events data that's already in the store but currently only visible via separate subcommands — plus rolling latency/bandwidth sparklines, a one-time startup banner, and `--hide`/`--only` section filters — without adding any new background polling or unbounded state.

**Architecture:** All new data comes from `Store` methods that already exist and are already used by other `traciumd` subcommands (`wifi`, `security`, `devices`, `route`, `dns`, `bandwidth`, `events`). `watch()` in `crates/cli/src/main.rs` calls them every tick alongside its existing queries and renders each behind a small `HashSet<&str>` of enabled section keys resolved once from `--hide`/`--only` at startup. Two fixed-capacity `VecDeque<Option<f64>>` ring buffers (60 entries each) live inside `watch()` for the session's latency/bandwidth sparklines — no persistence, no growth. A one-time ASCII banner prints before the refresh loop starts.

**Tech Stack:** Rust, clap 4 (derive), tokio, existing `tracium-store`/`tracium-monitor`/`tracium-probe` crates. No new dependencies.

## Global Constraints

- No new dependencies — everything is built from `std`, `clap`, `tokio`, and existing `Store` methods.
- No new store/monitor/probe queries invented — only calls to methods that already exist and are already used by other subcommands (per the design spec's scope).
- In-memory state (sparkline ring buffers, previous route hash) is capped and session-local; nothing is persisted to disk or the DB.
- `--hide` and `--only` are mutually exclusive (clap-level `conflicts_with`); unknown section names in either warn to stderr and are ignored, never a hard failure.
- All sections are on by default (dense-by-default per the approved design).
- Follow the existing file's conventions: inline `#[cfg(test)] mod tests` at the bottom of `main.rs`, `#[test]` for pure functions (matches `crates/probe/src/ping.rs`'s convention).

Reference spec: `docs/superpowers/specs/2026-08-09-cli-watch-expansion-design.md`

---

## File Structure

Everything lives in the single existing file `crates/cli/src/main.rs` (the codebase keeps the whole CLI in one file already — this plan follows that, adding free functions rather than new modules, since the crate has no existing module split to extend):

- `Cli`'s `Cmd::Watch` variant gains `hide: Option<String>` and `only: Option<String>` fields.
- New free functions (placed above `watch()`): `resolve_sections`, `push_sample`, `sparkline`, `route_changed`, `print_banner`, `tri`.
- New constants: `SECTION_KEYS`, `SPARK_LEVELS`, `SPARK_CAPACITY`.
- `watch()` is rewritten to accept the new flags, print the banner, own the ring buffers/route-hash state, and gate each render block on `sections.contains(...)`.
- `main()`'s `Cmd::Watch` match arm passes the new fields through.
- A new `#[cfg(test)] mod tests` block at the end of the file covers `resolve_sections`, `sparkline`, and `route_changed`.

---

### Task 1: Section resolver (`--hide`/`--only` → enabled set)

**Files:**
- Modify: `crates/cli/src/main.rs`

**Interfaces:**
- Produces: `const SECTION_KEYS: [&str; 11]`, `fn resolve_sections(hide: Option<&str>, only: Option<&str>) -> std::collections::HashSet<&'static str>` — used by Task 5's `watch()`.

- [ ] **Step 1: Write the failing tests**

Add near the bottom of `crates/cli/src/main.rs` (this will be the start of the file's first test module — no existing tests to append to):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_sections_defaults_to_everything() {
        let set = resolve_sections(None, None);
        assert_eq!(set.len(), SECTION_KEYS.len());
        for k in SECTION_KEYS {
            assert!(set.contains(k), "expected {k} to be enabled by default");
        }
    }

    #[test]
    fn resolve_sections_hide_removes_listed_keys() {
        let set = resolve_sections(Some("wifi,devices"), None);
        assert!(!set.contains("wifi"));
        assert!(!set.contains("devices"));
        assert!(set.contains("status"));
        assert_eq!(set.len(), SECTION_KEYS.len() - 2);
    }

    #[test]
    fn resolve_sections_only_restricts_to_listed_keys() {
        let set = resolve_sections(None, Some("status, reliability"));
        assert_eq!(set.len(), 2);
        assert!(set.contains("status"));
        assert!(set.contains("reliability"));
        assert!(!set.contains("qoe"));
    }

    #[test]
    fn resolve_sections_unknown_key_is_ignored_not_fatal() {
        let set = resolve_sections(Some("bogus"), None);
        // everything except the (nonexistent) "bogus" key stays enabled
        assert_eq!(set.len(), SECTION_KEYS.len());
    }

    #[test]
    fn resolve_sections_only_with_unknown_key_yields_empty_for_that_key() {
        let set = resolve_sections(None, Some("bogus"));
        assert!(set.is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p tracium-cli resolve_sections`
Expected: FAIL to compile — `resolve_sections` and `SECTION_KEYS` not defined.

- [ ] **Step 3: Implement `SECTION_KEYS` and `resolve_sections`**

Add above `watch()` (after the existing `window_secs` helper is fine, or anywhere before first use):

```rust
/// All section keys `watch` understands, in render order.
const SECTION_KEYS: [&str; 11] = [
    "status", "reliability", "qoe", "sparkline", "wifi", "bandwidth",
    "security", "devices", "route", "dns", "events",
];

/// Resolve `--hide`/`--only` into the set of enabled section keys. Unknown
/// keys are warned about on stderr and ignored rather than failing — a typo
/// shouldn't kill a live dashboard. Clap's `conflicts_with` already
/// guarantees `hide` and `only` are never both `Some`.
fn resolve_sections(hide: Option<&str>, only: Option<&str>) -> std::collections::HashSet<&'static str> {
    let known = |raw: &str| SECTION_KEYS.iter().find(|k| **k == raw).copied();
    if let Some(only) = only {
        let mut set = std::collections::HashSet::new();
        for raw in only.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match known(raw) {
                Some(k) => { set.insert(k); }
                None => eprintln!("unknown watch section '{raw}', ignoring"),
            }
        }
        return set;
    }
    let mut set: std::collections::HashSet<&'static str> = SECTION_KEYS.iter().copied().collect();
    if let Some(hide) = hide {
        for raw in hide.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match known(raw) {
                Some(k) => { set.remove(k); }
                None => eprintln!("unknown watch section '{raw}', ignoring"),
            }
        }
    }
    set
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p tracium-cli resolve_sections`
Expected: PASS (5 tests)

- [ ] **Step 5: Commit**

```bash
git add crates/cli/src/main.rs
git commit -m "cli: add watch section resolver for --hide/--only"
```

---

### Task 2: Sparkline ring buffer + renderer

**Files:**
- Modify: `crates/cli/src/main.rs`

**Interfaces:**
- Produces: `const SPARK_CAPACITY: usize = 60`, `fn push_sample(buf: &mut std::collections::VecDeque<Option<f64>>, value: Option<f64>)`, `fn sparkline(values: &std::collections::VecDeque<Option<f64>>) -> String` — used by Task 5's `watch()` for the latency/bandwidth history buffers.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block from Task 1:

```rust
    #[test]
    fn sparkline_empty_buffer_is_empty_string() {
        let buf = std::collections::VecDeque::new();
        assert_eq!(sparkline(&buf), "");
    }

    #[test]
    fn sparkline_all_none_renders_dots() {
        let mut buf = std::collections::VecDeque::new();
        push_sample(&mut buf, None);
        push_sample(&mut buf, None);
        assert_eq!(sparkline(&buf), "··");
    }

    #[test]
    fn sparkline_flat_line_uses_lowest_level_uniformly() {
        let mut buf = std::collections::VecDeque::new();
        push_sample(&mut buf, Some(10.0));
        push_sample(&mut buf, Some(10.0));
        push_sample(&mut buf, Some(10.0));
        let s = sparkline(&buf);
        let chars: Vec<char> = s.chars().collect();
        assert_eq!(chars.len(), 3);
        assert!(chars.windows(2).all(|w| w[0] == w[1]), "flat input should render a flat sparkline");
    }

    #[test]
    fn sparkline_increasing_values_increase_level() {
        let mut buf = std::collections::VecDeque::new();
        push_sample(&mut buf, Some(0.0));
        push_sample(&mut buf, Some(50.0));
        push_sample(&mut buf, Some(100.0));
        let chars: Vec<char> = sparkline(&buf).chars().collect();
        assert_eq!(chars[0], SPARK_LEVELS[0]);
        assert_eq!(chars[2], SPARK_LEVELS[SPARK_LEVELS.len() - 1]);
    }

    #[test]
    fn push_sample_caps_at_capacity() {
        let mut buf = std::collections::VecDeque::new();
        for i in 0..(SPARK_CAPACITY + 10) {
            push_sample(&mut buf, Some(i as f64));
        }
        assert_eq!(buf.len(), SPARK_CAPACITY);
        // oldest entries should have been evicted, so the front is not 0.0
        assert_eq!(buf.front().copied().flatten(), Some(10.0));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p tracium-cli sparkline`
Expected: FAIL to compile — `sparkline`, `push_sample`, `SPARK_LEVELS`, `SPARK_CAPACITY` not defined.

- [ ] **Step 3: Implement the ring buffer helper and renderer**

Add above `watch()`, alongside Task 1's constants:

```rust
const SPARK_LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const SPARK_CAPACITY: usize = 60;

/// Push a sample into a fixed-capacity ring buffer, evicting the oldest
/// entry once past `SPARK_CAPACITY`. Used for the session-local latency and
/// bandwidth history — never persisted, dies with the process.
fn push_sample(buf: &mut std::collections::VecDeque<Option<f64>>, value: Option<f64>) {
    buf.push_back(value);
    if buf.len() > SPARK_CAPACITY {
        buf.pop_front();
    }
}

/// Render a ring buffer as a min-max-normalized unicode block sparkline.
/// `None` entries (a tick where the metric was unavailable) render as `·`
/// so the horizontal axis stays aligned with elapsed ticks.
fn sparkline(values: &std::collections::VecDeque<Option<f64>>) -> String {
    let present: Vec<f64> = values.iter().filter_map(|v| *v).collect();
    if present.is_empty() {
        return String::new();
    }
    let min = present.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = present.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let span = if (max - min).abs() < f64::EPSILON { 1.0 } else { max - min };
    values
        .iter()
        .map(|v| match v {
            Some(x) => {
                let idx = (((x - min) / span) * (SPARK_LEVELS.len() - 1) as f64).round() as usize;
                SPARK_LEVELS[idx.min(SPARK_LEVELS.len() - 1)]
            }
            None => '·',
        })
        .collect()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p tracium-cli sparkline`
Expected: PASS (5 tests), plus `push_sample_caps_at_capacity`.

- [ ] **Step 5: Commit**

```bash
git add crates/cli/src/main.rs
git commit -m "cli: add sparkline ring buffer and renderer for watch"
```

---

### Task 3: Route-change detector

**Files:**
- Modify: `crates/cli/src/main.rs`

**Interfaces:**
- Consumes: `TracerouteView.route_hash: String` (already exists in `tracium-store`, no change needed).
- Produces: `fn route_changed(prev_hash: &mut Option<String>, current_hash: &str) -> bool` — used by Task 5's `watch()`.

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block:

```rust
    #[test]
    fn route_changed_first_call_is_never_flagged() {
        let mut prev = None;
        assert!(!route_changed(&mut prev, "hash-a"));
        assert_eq!(prev.as_deref(), Some("hash-a"));
    }

    #[test]
    fn route_changed_same_hash_is_not_flagged() {
        let mut prev = Some("hash-a".to_string());
        assert!(!route_changed(&mut prev, "hash-a"));
    }

    #[test]
    fn route_changed_different_hash_is_flagged() {
        let mut prev = Some("hash-a".to_string());
        assert!(route_changed(&mut prev, "hash-b"));
        assert_eq!(prev.as_deref(), Some("hash-b"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p tracium-cli route_changed`
Expected: FAIL to compile — `route_changed` not defined.

- [ ] **Step 3: Implement the detector**

Add above `watch()`:

```rust
/// Compare this tick's traceroute hash to the previous tick's, updating
/// `prev_hash` in place. Returns `false` on the very first call (nothing to
/// compare against yet) — this is session-local only, it does not touch
/// the `events` table or duplicate the monitor's own route-change detection.
fn route_changed(prev_hash: &mut Option<String>, current_hash: &str) -> bool {
    let changed = prev_hash.as_deref().is_some_and(|p| p != current_hash);
    *prev_hash = Some(current_hash.to_string());
    changed
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p tracium-cli route_changed`
Expected: PASS (3 tests)

- [ ] **Step 5: Commit**

```bash
git add crates/cli/src/main.rs
git commit -m "cli: add session-local route-change detector for watch"
```

---

### Task 4: Startup banner

**Files:**
- Modify: `crates/cli/src/main.rs`

**Interfaces:**
- Consumes: `SECTION_KEYS` (Task 1).
- Produces: `fn print_banner(db: &std::path::Path, interval: f64, sections: &std::collections::HashSet<&str>)` — used by Task 5's `watch()`. Not unit tested (pure `println!` side effects) — verified visually in Task 5's manual check.

- [ ] **Step 1: Implement the banner**

Add above `watch()`:

```rust
/// One-time ASCII splash printed before the refresh loop starts. Purely
/// cosmetic — has no per-tick cost, unlike everything else in `watch()`.
fn print_banner(db: &std::path::Path, interval: f64, sections: &std::collections::HashSet<&str>) {
    println!(
        r#"
  _______ _____            _____ _____ _    _ __  __
 |__   __|  __ \     /\   / ____|_   _| |  | |  \/  |
    | |  | |__) |   /  \ | |      | | | |  | | \  / |
    | |  |  _  /   / /\ \| |      | | | |  | | |\/| |
    | |  | | \ \  / ____ \ |____ _| |_| |__| | |  | |
    |_|  |_|  \_\/_/    \_\_____|_____|\____/|_|  |_|
"#
    );
    let mut names: Vec<&str> = SECTION_KEYS.iter().copied().filter(|k| sections.contains(k)).collect();
    names.sort();
    println!("  traciumd {} — live watch", env!("CARGO_PKG_VERSION"));
    println!("  db:       {}", db.display());
    println!("  refresh:  {interval:.1}s");
    println!("  sections: {}\n", names.join(", "));
}
```

- [ ] **Step 2: Check the crate compiles**

Run: `cargo check -p tracium-cli`
Expected: compiles cleanly (an unused-function warning for `print_banner` is expected and fine — Task 5 wires it in).

- [ ] **Step 3: Commit**

```bash
git add crates/cli/src/main.rs
git commit -m "cli: add watch startup banner"
```

---

### Task 5: Wire flags and new sections into `watch()`

**Files:**
- Modify: `crates/cli/src/main.rs`

**Interfaces:**
- Consumes: `resolve_sections` (Task 1), `push_sample`/`sparkline`/`SPARK_CAPACITY` (Task 2), `route_changed` (Task 3), `print_banner` (Task 4); `Store::latest_wifi`, `Store::latest_security`, `Store::list_devices`, `Store::latest_traceroute`, `Store::dns_comparison`, `Store::latest_bandwidth`, `Store::recent_events` (all pre-existing on `tracium_store::Store`).
- Produces: the finished `watch()` — no other task depends on it.

- [ ] **Step 1: Add `--hide`/`--only` to the `Watch` subcommand**

In the `Cmd` enum, replace:

```rust
    /// Live terminal dashboard — refreshes in place (read-only, Ctrl-C to quit).
    Watch {
        #[arg(long, default_value_t = 2.0)]
        interval: f64,
    },
```

with:

```rust
    /// Live terminal dashboard — refreshes in place (read-only, Ctrl-C to quit).
    Watch {
        #[arg(long, default_value_t = 2.0)]
        interval: f64,
        /// Comma-separated section keys to hide: status,reliability,qoe,
        /// sparkline,wifi,bandwidth,security,devices,route,dns,events.
        #[arg(long, conflicts_with = "only")]
        hide: Option<String>,
        /// Comma-separated section keys to show exclusively (same keys as --hide).
        #[arg(long)]
        only: Option<String>,
    },
```

- [ ] **Step 2: Update the `main()` match arm**

Replace:

```rust
        Cmd::Watch { interval } => watch(&store, interval).await?,
```

with:

```rust
        Cmd::Watch { interval, hide, only } => watch(&store, &db, interval, hide, only).await?,
```

- [ ] **Step 3: Rewrite `watch()`**

Replace the entire existing `watch()` function body with:

```rust
/// Live in-place dashboard. Read-only, so it runs happily alongside the daemon.
async fn watch(
    store: &Store,
    db: &std::path::Path,
    interval: f64,
    hide: Option<String>,
    only: Option<String>,
) -> Result<(), Box<dyn Error>> {
    use std::collections::VecDeque;
    use std::io::Write;

    let sections = resolve_sections(hide.as_deref(), only.as_deref());
    print_banner(db, interval, &sections);
    tokio::time::sleep(Duration::from_millis(700)).await;

    let dur = Duration::from_secs_f64(interval.max(0.5));
    let f = |v: Option<f64>, u: &str| v.map(|x| format!("{x:.1}{u}")).unwrap_or_else(|| "—".into());
    let mut lat_hist: VecDeque<Option<f64>> = VecDeque::with_capacity(SPARK_CAPACITY);
    let mut bw_hist: VecDeque<Option<f64>> = VecDeque::with_capacity(SPARK_CAPACITY);
    let mut prev_route_hash: Option<String> = None;

    loop {
        let targets = store.latest_per_target().await?;
        let gateway = store.latest_gateway().await?;
        let h1 = store.reliability_since(now_ms() - 3_600_000).await?;
        let d1 = store.reliability_since(now_ms() - 86_400_000).await?;
        let qoe = store.qoe_average_since(now_ms() - 1_800_000).await?;
        let wifi = store.latest_wifi().await?;
        let bandwidth = store.latest_bandwidth().await?;
        let security = store.latest_security().await?;
        let devices = store.list_devices().await?;
        let route = store.latest_traceroute().await?;
        let dns = store.dns_comparison(now_ms() - 3_600_000).await?;
        let events = store.recent_events(3).await?;

        let up = targets.iter().filter(|t| t.up == Some(true)).count();
        let online = up > 0;
        let avg_latency = {
            let ups: Vec<f64> = targets.iter().filter_map(|t| t.rtt_avg).collect();
            if ups.is_empty() { None } else { Some(ups.iter().sum::<f64>() / ups.len() as f64) }
        };
        push_sample(&mut lat_hist, avg_latency);
        push_sample(&mut bw_hist, bandwidth.as_ref().map(|b| b.rx_bps as f64));
        let route_change = route
            .as_ref()
            .map(|r| route_changed(&mut prev_route_hash, &r.route_hash))
            .unwrap_or(false);

        let mut buf = String::new();
        buf.push_str("\x1b[2J\x1b[H"); // clear screen + cursor home
        buf.push_str(&format!("Tracium — live · refresh {interval:.0}s · Ctrl-C to quit\n\n"));
        buf.push_str(&format!(
            "  {}   {}/{} targets up\n",
            if online { "● ONLINE " } else { "○ OFFLINE" },
            up,
            targets.len()
        ));

        if sections.contains("status") {
            for t in &targets {
                let state = match t.up {
                    Some(true) => format!("{:.1} ms", t.rtt_avg.unwrap_or(0.0)),
                    Some(false) => "down".to_string(),
                    None => "—".to_string(),
                };
                buf.push_str(&format!("    {:14} {:24} {}\n", t.label, t.host, state));
            }
            if let Some(g) = &gateway {
                buf.push_str(&format!(
                    "  gateway: {} · loss {}\n",
                    g.gateway_rtt_ms.map(|v| format!("{v:.2} ms")).unwrap_or_else(|| "—".into()),
                    g.lan_loss_pct.map(|v| format!("{v:.0}%")).unwrap_or_else(|| "—".into()),
                ));
            }
        }

        if sections.contains("reliability") {
            buf.push_str(&format!(
                "\n  last 1h : uptime {:.1}%  lat {}  loss {}\n",
                h1.uptime_pct, f(h1.avg_latency_ms, " ms"), f(h1.avg_loss_pct, "%"),
            ));
            buf.push_str(&format!(
                "  last 24h: uptime {:.1}%  lat {}  loss {}  disconnects {}\n",
                d1.uptime_pct, f(d1.avg_latency_ms, " ms"), f(d1.avg_loss_pct, "%"), d1.disconnects,
            ));
        }

        if sections.contains("qoe") {
            if let Some(q) = &qoe {
                let g = |v: Option<f64>| v.map(|x| format!("{x:.0}")).unwrap_or_else(|| "—".into());
                buf.push_str(&format!(
                    "  QoE(30m): gaming {} · voip {} · video {} · streaming {} · web {}\n",
                    g(q.gaming), g(q.voip), g(q.video_call), g(q.streaming), g(q.web),
                ));
            }
        }

        if sections.contains("sparkline") {
            buf.push_str(&format!("\n  latency   {}\n", sparkline(&lat_hist)));
            buf.push_str(&format!("  bandwidth {}\n", sparkline(&bw_hist)));
        }

        if sections.contains("wifi") {
            buf.push('\n');
            match &wifi {
                Some(w) => buf.push_str(&format!(
                    "  wifi: {} · {} · {}\n",
                    w.ssid.as_deref().unwrap_or("?"),
                    w.rssi_dbm.map(|v| format!("{v} dBm")).unwrap_or_else(|| "—".into()),
                    w.link_speed_mbps.map(|v| format!("{v:.0} Mbps")).unwrap_or_else(|| "—".into()),
                )),
                None => buf.push_str("  wifi: not connected to Wi-Fi\n"),
            }
        }

        if sections.contains("bandwidth") {
            match &bandwidth {
                Some(b) => buf.push_str(&format!(
                    "  bandwidth: ↓ {:.1} Mbps · ↑ {:.1} Mbps\n",
                    b.rx_bps as f64 / 1e6,
                    b.tx_bps as f64 / 1e6,
                )),
                None => buf.push_str("  bandwidth: no samples yet\n"),
            }
        }

        if sections.contains("security") {
            match &security {
                Some(s) => buf.push_str(&format!(
                    "  security: vpn {} · firewall {} · doh {} · dot {}\n",
                    tri(s.vpn_detected), tri(s.firewall_active), tri(s.doh_active), tri(s.dot_active),
                )),
                None => buf.push_str("  security: no snapshot yet\n"),
            }
        }

        if sections.contains("devices") {
            let active: Vec<_> = devices.iter().filter(|d| d.is_active).collect();
            buf.push_str(&format!("  devices: {} online\n", active.len()));
            for d in active.iter().take(8) {
                buf.push_str(&format!(
                    "    {:16} {}\n",
                    d.ip.as_deref().unwrap_or("?"),
                    d.hostname.as_deref().unwrap_or_else(|| d.mac.as_deref().unwrap_or("")),
                ));
            }
            if active.len() > 8 {
                buf.push_str(&format!("    +{} more\n", active.len() - 8));
            }
        }

        if sections.contains("route") {
            match &route {
                Some(r) => {
                    let last_rtt = r.hops.last().and_then(|h| h.rtt_ms);
                    buf.push_str(&format!(
                        "  route: {} hops to {}{}{}\n",
                        r.hop_count,
                        r.target,
                        last_rtt.map(|v| format!(" · last hop {v:.1}ms")).unwrap_or_default(),
                        if route_change { " · route changed" } else { "" },
                    ));
                }
                None => buf.push_str("  route: no traceroute yet\n"),
            }
        }

        if sections.contains("dns") {
            if dns.is_empty() {
                buf.push_str("  dns: no samples yet\n");
            } else {
                for s in &dns {
                    buf.push_str(&format!(
                        "  dns: {:12} {:>8}  {} lookups, {} failures\n",
                        s.resolver,
                        s.avg_ms.map(|v| format!("{v:.1}ms")).unwrap_or_else(|| "—".into()),
                        s.count,
                        s.failures,
                    ));
                }
            }
        }

        if sections.contains("events") {
            buf.push_str("\n  recent events:\n");
            if events.is_empty() {
                buf.push_str("    none yet\n");
            } else {
                for e in &events {
                    buf.push_str(&format!("    {}  {:12} {}\n", fmt_ts(e.ts), e.kind, e.severity));
                }
            }
        }

        print!("{buf}");
        std::io::stdout().flush().ok();

        tokio::select! {
            _ = tokio::time::sleep(dur) => {}
            _ = tokio::signal::ctrl_c() => { println!(); break; }
        }
    }
    Ok(())
}

/// Format an `Option<bool>` as a compact yes/no/unknown tri-state string.
fn tri(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "yes",
        Some(false) => "no",
        None => "—",
    }
}
```

- [ ] **Step 4: Build the crate**

Run: `cargo build -p tracium-cli`
Expected: builds cleanly, no warnings about unused `print_banner`/`resolve_sections`/etc. (they're now all wired in).

- [ ] **Step 5: Run the full test suite for the crate**

Run: `cargo test -p tracium-cli`
Expected: PASS — all tests from Tasks 1–3 still green (13 tests total: 5 `resolve_sections`, 5 `sparkline`/`push_sample`, 3 `route_changed`).

- [ ] **Step 6: Manually verify `--help` shows the new flags**

Run: `cargo run -p tracium-cli --bin traciumd -- watch --help`
Expected: help text lists `--interval`, `--hide`, `--only` with the doc comments written in Step 1.

- [ ] **Step 7: Manually verify the dashboard renders**

Run: `cargo run -p tracium-cli --bin traciumd -- watch --interval 1` against whatever local/dev DB is configured (Ctrl-C to quit after a couple of ticks), and separately `cargo run -p tracium-cli --bin traciumd -- watch --hide devices,dns --interval 1` to confirm hidden sections disappear and the banner's section list reflects the filter.
Expected: banner prints once, then the dashboard renders with all new sections (or the filtered subset) without panicking, even on a DB with little/no wifi/security/route/device data yet (those should show their "no data yet" fallback text, not crash).

- [ ] **Step 8: Commit**

```bash
git add crates/cli/src/main.rs
git commit -m "cli: wire wifi/bandwidth/security/devices/route/dns/events and sparklines into watch"
```

---

## Self-Review Notes

- **Spec coverage:** all 11 sections from the design's table are implemented (Task 5); sparklines with 60-point buffers (Task 2); `--hide`/`--only` with mutual exclusion and unknown-key warnings (Task 1 + clap `conflicts_with` in Task 5 Step 1); route-change indicator using the existing `route_hash` field rather than re-hashing hops (Task 3, simplification the spec's "hash the hop list" language allows since `TracerouteView.route_hash` already is that hash); startup banner (Task 4). DNS/QoE/reliability windows match the spec's "compact summary" intent.
- **Placeholder scan:** none — every step has runnable code and concrete assertions.
- **Type consistency:** `resolve_sections`, `push_sample`, `sparkline`, `route_changed`, `print_banner`, `tri` are called in Task 5 with the exact signatures produced in Tasks 1–4; verified field names (`route_hash`, `rssi_dbm`, `is_active`, etc.) against the actual `tracium-store` struct definitions rather than assumed names.

//! `traciumd` — Tracium's headless daemon + CLI.
//!
//! Runs the full monitor with no GUI/webview, sharing the desktop app's SQLite
//! DB. Every read subcommand supports `--json` (for scripting) and, where it
//! makes sense, `--window` (e.g. 24h, 7d, 30d). Run `traciumd --help`.
//!
//! NOTE: run the daemon OR the GUI, not both writing at once.

use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use tracium_monitor::{now_ms, Monitor, MonitorConfig};
use tracium_store::{SpeedtestRow, Store};

#[derive(Parser)]
#[command(name = "traciumd", version, about = "Headless Tracium network monitor")]
struct Cli {
    /// Override the database location (defaults to the shared GUI database).
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    /// Emit machine-readable JSON instead of text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Collect forever (the daemon).
    Run,
    /// Current reachability per target, gateway, and public IP.
    Status,
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
    /// Reliability + QoE over a window (e.g. 24h, 7d, 30d).
    Report {
        #[arg(long, default_value = "24h")]
        window: String,
        /// Write a PDF report to this path instead of printing.
        #[arg(long)]
        pdf: Option<PathBuf>,
    },
    /// DNS resolver comparison over a window.
    Dns {
        #[arg(long, default_value = "24h")]
        window: String,
    },
    /// Current Wi-Fi link (if connected).
    Wifi,
    /// Security posture: firewall, DoH/DoT, VPN, open ports.
    Security,
    /// Devices seen on the local network.
    Devices,
    /// Latest traceroute (per-hop latency + loss).
    Route,
    /// Current bandwidth rate + totals over a window.
    Bandwidth {
        #[arg(long, default_value = "24h")]
        window: String,
    },
    /// Run a speed test now (uses data, ~30s).
    Speed,
    /// Recent events (timeline).
    Events {
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    /// Outage / incident log.
    Outages {
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
    /// Export CSV to stdout.
    Export {
        /// What to export: connectivity | events
        kind: String,
        /// Only include rows from the last N seconds (0 = everything).
        #[arg(long, default_value_t = 0)]
        since_secs: i64,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let db = cli.db.clone().unwrap_or_else(default_db);
    let store = Store::open(&db).await?;
    let j = cli.json;

    match cli.cmd {
        Cmd::Run => {
            let now = now_ms();
            let _ = store.seed_default_settings(now).await;
            let _ = store.seed_default_targets(now).await;
            eprintln!("traciumd: monitoring → {}", db.display());
            Monitor::new(store, MonitorConfig::default()).run(None).await;
        }
        Cmd::Status => status(&store, j).await?,
        Cmd::Watch { interval, hide, only } => watch(&store, &db, interval, hide, only).await?,
        Cmd::Report { window, pdf } => report(&store, window_secs(&window), j, pdf).await?,
        Cmd::Dns { window } => dns(&store, window_secs(&window), j).await?,
        Cmd::Wifi => opt(j, &store.latest_wifi().await?, "not connected to Wi-Fi"),
        Cmd::Security => opt(j, &store.latest_security().await?, "no security snapshot yet"),
        Cmd::Devices => print_json_or(j, &store.list_devices().await?, |d| {
            if d.is_empty() {
                println!("no devices discovered yet");
            }
            for dev in d {
                println!(
                    "  {:16} {:18} {}",
                    dev.ip.as_deref().unwrap_or("?"),
                    dev.mac.as_deref().unwrap_or("?"),
                    dev.hostname.as_deref().unwrap_or(""),
                );
            }
        }),
        Cmd::Route => route(&store, j).await?,
        Cmd::Bandwidth { window } => bandwidth(&store, window_secs(&window), j).await?,
        Cmd::Speed => speed(&store, j).await?,
        Cmd::Events { limit } => print_json_or(j, &store.recent_events(limit).await?, |ev| {
            for e in ev {
                println!("  {}  {:12} {}", fmt_ts(e.ts), e.kind, e.severity);
            }
        }),
        Cmd::Outages { limit } => print_json_or(j, &store.recent_outages(limit).await?, |os| {
            if os.is_empty() {
                println!("no outages recorded");
            }
            for o in os {
                println!(
                    "  {}  duration {}  reconnect {}",
                    fmt_ts(o.ts_start),
                    o.duration_ms.map(fmt_dur).unwrap_or_else(|| "ongoing".into()),
                    o.reconnect_ms.map(fmt_dur).unwrap_or_else(|| "—".into()),
                );
            }
        }),
        Cmd::Export { kind, since_secs } => {
            let since = if since_secs > 0 { now_ms() - since_secs * 1000 } else { 0 };
            let csv = match kind.as_str() {
                "events" => store.export_events_csv(since).await?,
                "connectivity" => store.export_connectivity_csv(since).await?,
                other => {
                    eprintln!("unknown export kind '{other}' (use connectivity|events)");
                    std::process::exit(2);
                }
            };
            print!("{csv}");
        }
    }
    Ok(())
}

fn default_db() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("com.tracium.app")
        .join("tracium.db")
}

/// Parse a window like "24h", "7d", or a bare number of seconds → seconds.
fn window_secs(s: &str) -> i64 {
    let s = s.trim();
    if let Some(n) = s.strip_suffix('h') {
        return n.parse::<i64>().unwrap_or(24) * 3600;
    }
    if let Some(n) = s.strip_suffix('d') {
        return n.parse::<i64>().unwrap_or(1) * 86400;
    }
    s.parse::<i64>().unwrap_or(86400)
}

/// All section keys `watch` understands, in render order.
const SECTION_KEYS: [&str; 11] = [
    "status", "reliability", "qoe", "sparkline", "wifi", "bandwidth",
    "security", "devices", "route", "dns", "events",
];

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
    if values.is_empty() {
        return String::new();
    }
    let present: Vec<f64> = values.iter().filter_map(|v| *v).collect();
    if present.is_empty() {
        // All values are None, render as dots
        return values.iter().map(|_| '·').collect();
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

fn print_json<T: serde::Serialize>(v: &T) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_else(|_| "null".into()));
}

/// JSON if `json`, else run the text closure.
fn print_json_or<T: serde::Serialize>(json: bool, v: &T, text: impl FnOnce(&T)) {
    if json {
        print_json(v);
    } else {
        text(v);
    }
}

/// Print an Option as JSON, or text (value's Debug) / a "none" message.
fn opt<T: serde::Serialize + std::fmt::Debug>(json: bool, v: &Option<T>, none_msg: &str) {
    if json {
        print_json(v);
    } else {
        match v {
            Some(x) => println!("{x:#?}"),
            None => println!("{none_msg}"),
        }
    }
}

async fn status(store: &Store, json: bool) -> Result<(), Box<dyn Error>> {
    let targets = store.latest_per_target().await?;
    let gateway = store.latest_gateway().await?;
    let public_ip = store.latest_public_ip().await?;
    if json {
        print_json(&serde_json::json!({
            "targets": targets, "gateway": gateway, "public_ip": public_ip,
        }));
        return Ok(());
    }
    let up = targets.iter().filter(|t| t.up == Some(true)).count();
    println!("Reachability: {}/{} targets up", up, targets.len());
    for t in &targets {
        let state = match t.up {
            Some(true) => format!("{:.1} ms", t.rtt_avg.unwrap_or(0.0)),
            Some(false) => "down".to_string(),
            None => "—".to_string(),
        };
        println!("  {:14} {:24} IPv{:<3} {}", t.label, t.host, t.ip_version.unwrap_or(0), state);
    }
    if let Some(g) = gateway {
        println!(
            "Gateway: {} · loss {}",
            g.gateway_rtt_ms.map(|v| format!("{v:.2} ms")).unwrap_or_else(|| "—".into()),
            g.lan_loss_pct.map(|v| format!("{v:.0}%")).unwrap_or_else(|| "—".into()),
        );
    }
    if let Some(ip) = public_ip {
        println!("Public IP: {ip}");
    }
    Ok(())
}

/// Compare this tick's traceroute hash to the previous tick's, updating
/// `prev_hash` in place. Returns `false` on the very first call (nothing to
/// compare against yet) — this is session-local only, it does not touch
/// the `events` table or duplicate the monitor's own route-change detection.
fn route_changed(prev_hash: &mut Option<String>, current_hash: &str) -> bool {
    let changed = prev_hash.as_deref().is_some_and(|p| p != current_hash);
    *prev_hash = Some(current_hash.to_string());
    changed
}

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

async fn report(
    store: &Store,
    since_secs: i64,
    json: bool,
    pdf: Option<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    let since = now_ms() - since_secs * 1000;
    let r = store.reliability_since(since).await?;
    let q = store.qoe_average_since(now_ms() - 1_800_000).await?;
    if let Some(path) = pdf {
        let dns = store.dns_comparison(since).await?;
        let outages = store.recent_outages(15).await?;
        write_report_pdf(&path, since_secs, &r, &q, &dns, &outages)?;
        println!("wrote report to {}", path.display());
        return Ok(());
    }
    if json {
        print_json(&serde_json::json!({ "window_secs": since_secs, "reliability": r, "qoe": q }));
        return Ok(());
    }
    let f = |v: Option<f64>, u: &str| v.map(|x| format!("{x:.1}{u}")).unwrap_or_else(|| "—".into());
    println!("Over the last {}:", human(since_secs));
    println!("  uptime      {:.1}%  ({} of {} cycles)", r.uptime_pct, r.up_samples, r.samples);
    println!("  avg latency {}", f(r.avg_latency_ms, " ms"));
    println!("  avg jitter  {}", f(r.avg_jitter_ms, " ms"));
    println!("  avg loss    {}", f(r.avg_loss_pct, "%"));
    println!("  disconnects {}", r.disconnects);
    if let Some(q) = q {
        let g = |v: Option<f64>| v.map(|x| format!("{x:.0}")).unwrap_or_else(|| "—".into());
        println!(
            "QoE (30m): gaming {} · voip {} · video {} · streaming {} · web {}",
            g(q.gaming), g(q.voip), g(q.video_call), g(q.streaming), g(q.web),
        );
    }
    Ok(())
}

async fn dns(store: &Store, since_secs: i64, json: bool) -> Result<(), Box<dyn Error>> {
    let stats = store.dns_comparison(now_ms() - since_secs * 1000).await?;
    if json {
        print_json(&stats);
        return Ok(());
    }
    if stats.is_empty() {
        println!("no DNS samples yet");
    }
    for s in &stats {
        println!(
            "  {:16} {:>8}  {} lookups, {} failures",
            s.resolver,
            s.avg_ms.map(|v| format!("{v:.1}ms")).unwrap_or_else(|| "—".into()),
            s.count,
            s.failures,
        );
    }
    Ok(())
}

async fn route(store: &Store, json: bool) -> Result<(), Box<dyn Error>> {
    let trace = store.latest_traceroute().await?;
    if json {
        print_json(&trace);
        return Ok(());
    }
    match trace {
        None => println!("no traceroute yet (needs the traceroute/tracert tool)"),
        Some(t) => {
            println!("Route to {} · {} hops", t.target, t.hop_count);
            for h in t.hops {
                println!(
                    "  {:2}  {:16} {:>9} {}",
                    h.hop_no,
                    h.ip.as_deref().unwrap_or("*"),
                    h.rtt_ms.map(|v| format!("{v:.1}ms")).unwrap_or_default(),
                    h.loss_pct.filter(|l| *l > 0.0).map(|l| format!("{l:.0}% loss")).unwrap_or_default(),
                );
            }
        }
    }
    Ok(())
}

async fn bandwidth(store: &Store, since_secs: i64, json: bool) -> Result<(), Box<dyn Error>> {
    let now = store.latest_bandwidth().await?;
    let totals = store.bandwidth_totals(now_ms() - since_secs * 1000).await?;
    if json {
        print_json(&serde_json::json!({ "now": now, "totals": totals, "window_secs": since_secs }));
        return Ok(());
    }
    match now {
        Some(b) => println!(
            "Live: ↓ {:.1} Mbps · ↑ {:.1} Mbps",
            b.rx_bps as f64 / 1e6,
            b.tx_bps as f64 / 1e6
        ),
        None => println!("no bandwidth samples yet"),
    }
    println!(
        "Total over {}: ↓ {:.2} GB · ↑ {:.2} GB",
        human(since_secs),
        totals.rx_bytes as f64 / 1e9,
        totals.tx_bytes as f64 / 1e9,
    );
    Ok(())
}

async fn speed(store: &Store, json: bool) -> Result<(), Box<dyn Error>> {
    let bin = std::env::var("TRACIUM_LIBRESPEED_CLI").unwrap_or_else(|_| "librespeed-cli".into());
    let out = tracium_probe::run_speedtest_bufferbloat(&bin, "1.1.1.1", 443, Duration::from_secs(90)).await;
    let Some(r) = out.speed else {
        eprintln!("speed test unavailable — install librespeed-cli (or set TRACIUM_LIBRESPEED_CLI)");
        std::process::exit(1);
    };
    let bb = out.bufferbloat;
    let row = SpeedtestRow {
        ts: now_ms(),
        engine: Some("librespeed-cli".into()),
        server: r.server.clone(),
        download_mbps: r.download_mbps,
        upload_mbps: r.upload_mbps,
        ping_ms: r.ping_ms,
        jitter_ms: r.jitter_ms,
        idle_latency_ms: bb.as_ref().map(|b| b.idle_ms),
        loaded_latency_ms: bb.as_ref().map(|b| b.loaded_ms),
        bufferbloat_grade: bb.as_ref().map(|b| b.grade.clone()),
    };
    store.insert_speedtest(&row).await?;
    if json {
        print_json(&row);
        return Ok(());
    }
    println!(
        "↓ {:.1} Mbps · ↑ {:.1} Mbps · ping {:.0} ms · bufferbloat {}",
        r.download_mbps.unwrap_or(0.0),
        r.upload_mbps.unwrap_or(0.0),
        r.ping_ms.unwrap_or(0.0),
        bb.map(|b| b.grade).unwrap_or_else(|| "—".into()),
    );
    if let Some(s) = r.server {
        println!("server: {s}");
    }
    Ok(())
}

/// Render a one-page summary PDF using printpdf's built-in Helvetica (no font
/// asset needed). Text/table only — charts are a future enhancement.
fn write_report_pdf(
    path: &std::path::Path,
    since_secs: i64,
    r: &tracium_store::Reliability,
    q: &Option<tracium_store::QoeAverage>,
    dns: &[tracium_store::DnsResolverStat],
    outages: &[tracium_store::Outage],
) -> Result<(), Box<dyn Error>> {
    use printpdf::{BuiltinFont, Mm, PdfDocument};
    use std::io::BufWriter;

    let (doc, page, layer) = PdfDocument::new("Tracium Network Report", Mm(210.0), Mm(297.0), "Layer 1");
    let reg = doc.add_builtin_font(BuiltinFont::Helvetica)?;
    let bold = doc.add_builtin_font(BuiltinFont::HelveticaBold)?;
    let l = doc.get_page(page).get_layer(layer);

    let mut y = 280.0_f64;
    let mut text = |s: &str, size: f64, b: bool, indent: f64, gap: f64| {
        l.use_text(s, size as f32, Mm((18.0 + indent) as f32), Mm(y as f32), if b { &bold } else { &reg });
        y -= gap;
    };
    let f = |v: Option<f64>, u: &str| v.map(|x| format!("{x:.1}{u}")).unwrap_or_else(|| "—".into());

    text("Tracium — Network Report", 20.0, true, 0.0, 10.0);
    text(&format!("Range: last {}", human(since_secs)), 11.0, false, 0.0, 12.0);

    text("Reliability", 14.0, true, 0.0, 7.0);
    text(&format!("Uptime: {:.1}%  ({} of {} cycles)", r.uptime_pct, r.up_samples, r.samples), 11.0, false, 4.0, 6.0);
    text(&format!("Avg latency: {}", f(r.avg_latency_ms, " ms")), 11.0, false, 4.0, 6.0);
    text(&format!("Avg jitter: {}", f(r.avg_jitter_ms, " ms")), 11.0, false, 4.0, 6.0);
    text(&format!("Avg packet loss: {}", f(r.avg_loss_pct, "%")), 11.0, false, 4.0, 6.0);
    text(&format!("Disconnects: {}", r.disconnects), 11.0, false, 4.0, 12.0);

    if let Some(q) = q {
        let g = |v: Option<f64>| v.map(|x| format!("{x:.0}")).unwrap_or_else(|| "—".into());
        text("Quality of experience (recent)", 14.0, true, 0.0, 7.0);
        text(
            &format!("Gaming {} · VoIP {} · Video {} · Streaming {} · Web {}",
                g(q.gaming), g(q.voip), g(q.video_call), g(q.streaming), g(q.web)),
            11.0, false, 4.0, 12.0,
        );
    }

    if !dns.is_empty() {
        text("DNS resolvers", 14.0, true, 0.0, 7.0);
        for d in dns {
            text(
                &format!("{:16} {:>8}  {} lookups, {} failures",
                    d.resolver, d.avg_ms.map(|v| format!("{v:.1}ms")).unwrap_or_else(|| "—".into()), d.count, d.failures),
                10.0, false, 4.0, 6.0,
            );
        }
        text("", 10.0, false, 0.0, 6.0);
    }

    text("Incidents", 14.0, true, 0.0, 7.0);
    if outages.is_empty() {
        text("No outages recorded.", 11.0, false, 4.0, 6.0);
    } else {
        for o in outages {
            text(
                &format!("start @{}  duration {}",
                    o.ts_start / 1000,
                    o.duration_ms.map(fmt_dur).unwrap_or_else(|| "ongoing".into())),
                10.0, false, 4.0, 6.0,
            );
        }
    }

    let mut buf = Vec::new();
    doc.save(&mut BufWriter::new(&mut buf))?;
    std::fs::write(path, buf)?;
    Ok(())
}

fn fmt_dur(ms: i64) -> String {
    let s = ms as f64 / 1000.0;
    if s < 60.0 {
        format!("{s:.0}s")
    } else if s < 3600.0 {
        format!("{}m", (s / 60.0).round())
    } else {
        format!("{:.1}h", s / 3600.0)
    }
}

fn fmt_ts(ms: i64) -> String {
    // Simple UTC-ish relative-free stamp; keeps the CLI dependency-light.
    let secs = ms / 1000;
    format!("@{secs}")
}

fn human(secs: i64) -> String {
    if secs % 86400 == 0 {
        format!("{}d", secs / 86400)
    } else if secs % 3600 == 0 {
        format!("{}h", secs / 3600)
    } else {
        format!("{secs}s")
    }
}

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
}

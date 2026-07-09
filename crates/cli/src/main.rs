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
    },
    /// Reliability + QoE over a window (e.g. 24h, 7d, 30d).
    Report {
        #[arg(long, default_value = "24h")]
        window: String,
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
        Cmd::Watch { interval } => watch(&store, interval).await?,
        Cmd::Report { window } => report(&store, window_secs(&window), j).await?,
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

/// Live in-place dashboard. Read-only, so it runs happily alongside the daemon.
async fn watch(store: &Store, interval: f64) -> Result<(), Box<dyn Error>> {
    use std::io::Write;
    let dur = Duration::from_secs_f64(interval.max(0.5));
    let f = |v: Option<f64>, u: &str| v.map(|x| format!("{x:.1}{u}")).unwrap_or_else(|| "—".into());
    loop {
        let targets = store.latest_per_target().await?;
        let gateway = store.latest_gateway().await?;
        let h1 = store.reliability_since(now_ms() - 3_600_000).await?;
        let d1 = store.reliability_since(now_ms() - 86_400_000).await?;
        let qoe = store.qoe_average_since(now_ms() - 1_800_000).await?;
        let up = targets.iter().filter(|t| t.up == Some(true)).count();
        let online = up > 0;

        let mut buf = String::new();
        buf.push_str("\x1b[2J\x1b[H"); // clear screen + cursor home
        buf.push_str(&format!("Tracium — live · refresh {interval:.0}s · Ctrl-C to quit\n\n"));
        buf.push_str(&format!(
            "  {}   {}/{} targets up\n",
            if online { "● ONLINE " } else { "○ OFFLINE" },
            up,
            targets.len()
        ));
        for t in &targets {
            let state = match t.up {
                Some(true) => format!("{:.1} ms", t.rtt_avg.unwrap_or(0.0)),
                Some(false) => "down".to_string(),
                None => "—".to_string(),
            };
            buf.push_str(&format!("    {:14} {:24} {}\n", t.label, t.host, state));
        }
        if let Some(g) = gateway {
            buf.push_str(&format!(
                "  gateway: {} · loss {}\n",
                g.gateway_rtt_ms.map(|v| format!("{v:.2} ms")).unwrap_or_else(|| "—".into()),
                g.lan_loss_pct.map(|v| format!("{v:.0}%")).unwrap_or_else(|| "—".into()),
            ));
        }
        buf.push_str(&format!(
            "\n  last 1h : uptime {:.1}%  lat {}  loss {}\n",
            h1.uptime_pct, f(h1.avg_latency_ms, " ms"), f(h1.avg_loss_pct, "%"),
        ));
        buf.push_str(&format!(
            "  last 24h: uptime {:.1}%  lat {}  loss {}  disconnects {}\n",
            d1.uptime_pct, f(d1.avg_latency_ms, " ms"), f(d1.avg_loss_pct, "%"), d1.disconnects,
        ));
        if let Some(q) = qoe {
            let g = |v: Option<f64>| v.map(|x| format!("{x:.0}")).unwrap_or_else(|| "—".into());
            buf.push_str(&format!(
                "  QoE(30m): gaming {} · voip {} · video {} · streaming {} · web {}\n",
                g(q.gaming), g(q.voip), g(q.video_call), g(q.streaming), g(q.web),
            ));
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

async fn report(store: &Store, since_secs: i64, json: bool) -> Result<(), Box<dyn Error>> {
    let r = store.reliability_since(now_ms() - since_secs * 1000).await?;
    let q = store.qoe_average_since(now_ms() - 1_800_000).await?;
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

//! `traciumd` — Tracium's headless daemon + CLI.
//!
//! Runs the full monitor with no GUI/webview (a few MB, ~0% idle CPU), sharing
//! the same SQLite DB as the desktop app. Intended to run as a system service.
//!
//!   traciumd run                 # collect forever (the daemon)
//!   traciumd status              # print current reachability + gateway
//!   traciumd report              # 24h reliability + recent QoE
//!   traciumd export [kind]       # CSV to stdout (kind: connectivity|events)
//!   [any command] --db <path>    # override the database location
//!
//! NOTE: run the daemon OR the GUI, not both writing at once — they'd double
//! up samples. (A future flag will let the GUI attach read-only to the daemon.)

use std::path::PathBuf;

use tracium_monitor::{now_ms, Monitor, MonitorConfig};
use tracium_store::Store;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("run");
    if matches!(cmd, "-h" | "--help" | "help") {
        print_help();
        return;
    }

    let db = db_path(&args);
    let store = match Store::open(&db).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("traciumd: failed to open database at {}: {e}", db.display());
            std::process::exit(1);
        }
    };

    match cmd {
        "run" => run(store, &db).await,
        "status" => status(&store).await,
        "report" => report(&store).await,
        "export" => export(&store, args.get(2).map(String::as_str).unwrap_or("connectivity")).await,
        other => {
            eprintln!("traciumd: unknown command '{other}'\n");
            print_help();
            std::process::exit(2);
        }
    }
}

/// Resolve the DB path: `--db <path>` override, else the same location the GUI
/// uses (platform data dir + the app identifier).
fn db_path(args: &[String]) -> PathBuf {
    if let Some(i) = args.iter().position(|a| a == "--db") {
        if let Some(p) = args.get(i + 1) {
            return PathBuf::from(p);
        }
    }
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("com.tracium.app").join("tracium.db")
}

async fn run(store: Store, db: &std::path::Path) {
    let now = now_ms();
    let _ = store.seed_default_settings(now).await;
    let _ = store.seed_default_targets(now).await;
    eprintln!("traciumd: monitoring → {}", db.display());
    // Runs until the process is stopped.
    Monitor::new(store, MonitorConfig::default()).run(None).await;
}

async fn status(store: &Store) {
    let targets = store.latest_per_target().await.unwrap_or_default();
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
    if let Ok(Some(g)) = store.latest_gateway().await {
        println!(
            "Gateway: {} · loss {}",
            g.gateway_rtt_ms.map(|v| format!("{v:.2} ms")).unwrap_or_else(|| "—".into()),
            g.lan_loss_pct.map(|v| format!("{v:.0}%")).unwrap_or_else(|| "—".into()),
        );
    }
}

async fn report(store: &Store) {
    match store.reliability_since(now_ms() - 86_400_000).await {
        Ok(r) => {
            let f = |v: Option<f64>, u: &str| v.map(|x| format!("{x:.1}{u}")).unwrap_or_else(|| "—".into());
            println!("Last 24h:");
            println!("  uptime      {:.1}%  ({} of {} cycles)", r.uptime_pct, r.up_samples, r.samples);
            println!("  avg latency {}", f(r.avg_latency_ms, " ms"));
            println!("  avg jitter  {}", f(r.avg_jitter_ms, " ms"));
            println!("  avg loss    {}", f(r.avg_loss_pct, "%"));
            println!("  disconnects {}", r.disconnects);
        }
        Err(e) => eprintln!("report failed: {e}"),
    }
    if let Ok(Some(q)) = store.qoe_average_since(now_ms() - 1_800_000).await {
        let g = |v: Option<f64>| v.map(|x| format!("{x:.0}")).unwrap_or_else(|| "—".into());
        println!(
            "QoE (30m): gaming {} · voip {} · video {} · streaming {} · web {}",
            g(q.gaming), g(q.voip), g(q.video_call), g(q.streaming), g(q.web),
        );
    }
}

async fn export(store: &Store, kind: &str) {
    let csv = match kind {
        "events" => store.export_events_csv(0).await,
        "connectivity" => store.export_connectivity_csv(0).await,
        other => {
            eprintln!("traciumd export: unknown kind '{other}' (use connectivity|events)");
            std::process::exit(2);
        }
    };
    match csv {
        Ok(text) => print!("{text}"),
        Err(e) => {
            eprintln!("export failed: {e}");
            std::process::exit(1);
        }
    }
}

fn print_help() {
    eprintln!(
        "traciumd — headless Tracium monitor\n\n\
         USAGE:\n\
         \x20 traciumd run                 collect forever (the daemon)\n\
         \x20 traciumd status              current reachability + gateway\n\
         \x20 traciumd report              24h reliability + recent QoE\n\
         \x20 traciumd export [kind]       CSV to stdout (connectivity|events)\n\
         \x20 <command> --db <path>        override the database location"
    );
}

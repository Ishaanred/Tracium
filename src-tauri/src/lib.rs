//! Tracium Tauri application shell.
//!
//! Owns the desktop window + tray, spawns the connectivity monitor, and exposes
//! the storage layer + live status to the frontend via Tauri commands and a
//! `status` event. All persistence lives in `tracium-store`; all probing in
//! `tracium-monitor`/`tracium-probe`.

use std::sync::Mutex;

use tracium_monitor::{now_ms, Monitor, MonitorConfig, StatusUpdate};
use tracium_store::{
    BandwidthNow, BandwidthTotals, ConnectivitySample, Device, DnsResolverStat, Event, NewTarget,
    GatewaySample, Outage, QoeAverage, Reliability, Rollup, SecuritySnapshot, SpeedtestRow, Store,
    Target, TargetStatus, TracerouteView, WifiSample,
};
use tauri::{Emitter, Manager, State};

/// Shared application state handed to every command.
struct AppState {
    store: Store,
    /// Most recent monitor status, for `current_status` on demand.
    latest: Mutex<Option<StatusUpdate>>,
}

/// Basic database health for the UI to confirm the store is live.
#[derive(serde::Serialize)]
struct DbHealth {
    ok: bool,
    table_count: i64,
}

#[tauri::command]
async fn db_health(state: State<'_, AppState>) -> Result<DbHealth, String> {
    let table_count = state.store.table_count().await.map_err(|e| e.to_string())?;
    Ok(DbHealth { ok: true, table_count })
}

#[tauri::command]
async fn list_targets(state: State<'_, AppState>) -> Result<Vec<Target>, String> {
    state.store.list_targets().await.map_err(|e| e.to_string())
}

/// Each enabled target with its latest sample (per-target latency + up/down).
#[tauri::command]
async fn target_status(state: State<'_, AppState>) -> Result<Vec<TargetStatus>, String> {
    state.store.latest_per_target().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn add_target(state: State<'_, AppState>, input: NewTarget) -> Result<Target, String> {
    state.store.add_target(input).await.map_err(|e| e.to_string())
}

/// The latest live status, if the monitor has completed a cycle.
#[tauri::command]
async fn current_status(state: State<'_, AppState>) -> Result<Option<StatusUpdate>, String> {
    Ok(state.latest.lock().unwrap().clone())
}

/// Smoothed QoE scores averaged over the last `window_secs` seconds.
#[tauri::command]
async fn qoe_average(
    state: State<'_, AppState>,
    window_secs: i64,
) -> Result<Option<QoeAverage>, String> {
    let since = now_ms() - window_secs * 1000;
    state.store.qoe_average_since(since).await.map_err(|e| e.to_string())
}

/// Reliability over the last `window_secs` seconds.
#[tauri::command]
async fn reliability(state: State<'_, AppState>, window_secs: i64) -> Result<Reliability, String> {
    let since = now_ms() - window_secs * 1000;
    state.store.reliability_since(since).await.map_err(|e| e.to_string())
}

/// The most recent connectivity samples, newest first.
#[tauri::command]
async fn recent_connectivity(
    state: State<'_, AppState>,
    limit: i64,
) -> Result<Vec<ConnectivitySample>, String> {
    state.store.recent_connectivity(limit).await.map_err(|e| e.to_string())
}

/// Aggregated history for a metric ("latency"/"loss"/"jitter") at a bucket
/// ("hour"/"day") over the last `window_secs` seconds.
#[tauri::command]
async fn metric_history(
    state: State<'_, AppState>,
    metric: String,
    bucket: String,
    window_secs: i64,
) -> Result<Vec<Rollup>, String> {
    let since = now_ms() - window_secs * 1000;
    state.store.rollups(&metric, &bucket, since).await.map_err(|e| e.to_string())
}

/// The latest aggregate bandwidth rate, if any.
#[tauri::command]
async fn bandwidth_now(state: State<'_, AppState>) -> Result<Option<BandwidthNow>, String> {
    state.store.latest_bandwidth().await.map_err(|e| e.to_string())
}

/// Total bytes transferred over the last `window_secs` seconds.
#[tauri::command]
async fn bandwidth_totals(
    state: State<'_, AppState>,
    window_secs: i64,
) -> Result<BandwidthTotals, String> {
    let since = now_ms() - window_secs * 1000;
    state.store.bandwidth_totals(since).await.map_err(|e| e.to_string())
}

/// Run a speed test now (invokes `librespeed-cli`), store it, and return it.
/// On-demand only — speed tests consume data, so they aren't auto-scheduled.
/// Resolve the librespeed-cli binary: env override, then a bundled sidecar next
/// to the app executable, else `librespeed-cli` on PATH.
fn resolve_speedtest_bin() -> String {
    if let Ok(p) = std::env::var("TRACIUM_LIBRESPEED_CLI") {
        if !p.is_empty() {
            return p;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let name = if cfg!(windows) { "librespeed-cli.exe" } else { "librespeed-cli" };
            let cand = dir.join(name);
            if cand.exists() {
                return cand.to_string_lossy().into_owned();
            }
        }
    }
    "librespeed-cli".to_string()
}

#[tauri::command]
async fn run_speedtest(state: State<'_, AppState>) -> Result<Option<SpeedtestRow>, String> {
    let bin = resolve_speedtest_bin();
    // Run the speed test while probing latency to Cloudflare for a bufferbloat grade.
    let out = tracium_probe::run_speedtest_bufferbloat(
        &bin,
        "1.1.1.1",
        443,
        std::time::Duration::from_secs(90),
    )
    .await;
    let Some(r) = out.speed else { return Ok(None) };
    let bb = out.bufferbloat;
    let row = SpeedtestRow {
        ts: now_ms(),
        engine: Some("librespeed-cli".into()),
        server: r.server,
        download_mbps: r.download_mbps,
        upload_mbps: r.upload_mbps,
        ping_ms: r.ping_ms,
        jitter_ms: r.jitter_ms,
        idle_latency_ms: bb.as_ref().map(|b| b.idle_ms),
        loaded_latency_ms: bb.as_ref().map(|b| b.loaded_ms),
        bufferbloat_grade: bb.as_ref().map(|b| b.grade.clone()),
    };
    state.store.insert_speedtest(&row).await.map_err(|e| e.to_string())?;
    Ok(Some(row))
}

/// Recent speed-test results.
#[tauri::command]
async fn speedtest_history(
    state: State<'_, AppState>,
    limit: i64,
) -> Result<Vec<SpeedtestRow>, String> {
    state.store.speedtest_history(limit).await.map_err(|e| e.to_string())
}

/// On-demand SNMP query of a router. `addr` is an IP (":161" appended if no
/// port); `community` is the SNMP v2c community string. Returns `None` if the
/// router is unreachable or SNMP is disabled.
#[tauri::command]
async fn router_status(
    addr: String,
    community: String,
) -> Result<Option<tracium_probe::RouterInfo>, String> {
    use std::net::ToSocketAddrs;
    let with_port = if addr.contains(':') { addr } else { format!("{addr}:161") };
    let sock = with_port
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or("could not resolve router address")?;
    Ok(tracium_probe::query_router(sock, &community, std::time::Duration::from_secs(3)).await)
}

/// The latest Wi-Fi link sample, if connected.
#[tauri::command]
async fn wifi(state: State<'_, AppState>) -> Result<Option<WifiSample>, String> {
    state.store.latest_wifi().await.map_err(|e| e.to_string())
}

/// Latest gateway (LAN) latency + loss, if measured.
#[tauri::command]
async fn gateway_status(state: State<'_, AppState>) -> Result<Option<GatewaySample>, String> {
    state.store.latest_gateway().await.map_err(|e| e.to_string())
}

/// Known LAN devices (most-recently-seen first).
#[tauri::command]
async fn devices(state: State<'_, AppState>) -> Result<Vec<Device>, String> {
    state.store.list_devices().await.map_err(|e| e.to_string())
}

/// The most recent traceroute with its hops, if any.
#[tauri::command]
async fn latest_traceroute(state: State<'_, AppState>) -> Result<Option<TracerouteView>, String> {
    state.store.latest_traceroute().await.map_err(|e| e.to_string())
}

/// The latest security-posture snapshot, if one has been taken.
#[tauri::command]
async fn security_status(state: State<'_, AppState>) -> Result<Option<SecuritySnapshot>, String> {
    state.store.latest_security().await.map_err(|e| e.to_string())
}

/// The most recently observed public IP, if known.
#[tauri::command]
async fn public_ip(state: State<'_, AppState>) -> Result<Option<String>, String> {
    state.store.latest_public_ip().await.map_err(|e| e.to_string())
}

/// Per-resolver DNS comparison over the last `window_secs` seconds.
#[tauri::command]
async fn dns_comparison(
    state: State<'_, AppState>,
    window_secs: i64,
) -> Result<Vec<DnsResolverStat>, String> {
    let since = now_ms() - window_secs * 1000;
    state.store.dns_comparison(since).await.map_err(|e| e.to_string())
}

/// Recent events (the timeline), newest first.
#[tauri::command]
async fn recent_events(state: State<'_, AppState>, limit: i64) -> Result<Vec<Event>, String> {
    state.store.recent_events(limit).await.map_err(|e| e.to_string())
}

/// Recent outages (the incident log), newest first.
#[tauri::command]
async fn recent_outages(state: State<'_, AppState>, limit: i64) -> Result<Vec<Outage>, String> {
    state.store.recent_outages(limit).await.map_err(|e| e.to_string())
}

/// Export `kind` ("connectivity" | "events") over `window_secs` to a CSV file
/// in the user's Downloads dir (falling back to the app data dir). Returns the
/// written path. No external plugin/dialog required.
#[tauri::command]
async fn export_csv(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    kind: String,
    window_secs: i64,
) -> Result<String, String> {
    let since = now_ms() - window_secs * 1000;
    let csv = match kind.as_str() {
        "events" => state.store.export_events_csv(since).await,
        _ => state.store.export_connectivity_csv(since).await,
    }
    .map_err(|e| e.to_string())?;

    let dir = app
        .path()
        .download_dir()
        .or_else(|_| app.path().app_data_dir())
        .map_err(|e| e.to_string())?;
    let path = dir.join(format!("tracium-{kind}.csv"));
    std::fs::write(&path, csv).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Open (create + migrate) the database in the per-user app data dir.
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let db_path = dir.join("tracium.db");

            let store = tauri::async_runtime::block_on(async {
                let store = Store::open(&db_path).await?;
                store.seed_default_settings(now_ms()).await?;
                store.seed_default_targets(now_ms()).await?;
                Ok::<_, tracium_store::StoreError>(store)
            })
            .map_err(|e| format!("failed to init database at {db_path:?}: {e}"))?;

            app.manage(AppState { store: store.clone(), latest: Mutex::new(None) });

            // Spawn the connectivity monitor; forward each status to the UI.
            let (tx, mut rx) = tokio::sync::mpsc::channel::<StatusUpdate>(16);
            let monitor = Monitor::new(store, MonitorConfig::default());
            tauri::async_runtime::spawn(async move { monitor.run(Some(tx)).await });

            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                while let Some(update) = rx.recv().await {
                    if let Some(state) = handle.try_state::<AppState>() {
                        *state.latest.lock().unwrap() = Some(update.clone());
                    }
                    let _ = handle.emit("status", update);
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            db_health,
            list_targets,
            target_status,
            add_target,
            current_status,
            reliability,
            recent_connectivity,
            metric_history,
            qoe_average,
            dns_comparison,
            public_ip,
            bandwidth_now,
            bandwidth_totals,
            security_status,
            latest_traceroute,
            devices,
            gateway_status,
            wifi,
            router_status,
            run_speedtest,
            speedtest_history,
            recent_events,
            recent_outages,
            export_csv
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tracium");
}

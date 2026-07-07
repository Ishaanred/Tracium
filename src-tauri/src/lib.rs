//! NetPulse Tauri application shell.
//!
//! Owns the desktop window + tray, spawns the connectivity monitor, and exposes
//! the storage layer + live status to the frontend via Tauri commands and a
//! `status` event. All persistence lives in `netpulse-store`; all probing in
//! `netpulse-monitor`/`netpulse-probe`.

use std::sync::Mutex;

use netpulse_monitor::{now_ms, Monitor, MonitorConfig, StatusUpdate};
use netpulse_store::{ConnectivitySample, NewTarget, Reliability, Store, Target};
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
    let table_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_master WHERE type='table' \
         AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations'",
    )
    .fetch_one(state.store.pool())
    .await
    .map_err(|e| e.to_string())?;
    Ok(DbHealth { ok: true, table_count })
}

#[tauri::command]
async fn list_targets(state: State<'_, AppState>) -> Result<Vec<Target>, String> {
    state.store.list_targets().await.map_err(|e| e.to_string())
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Open (create + migrate) the database in the per-user app data dir.
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let db_path = dir.join("netpulse.db");

            let store = tauri::async_runtime::block_on(async {
                let store = Store::open(&db_path).await?;
                store.seed_default_settings(now_ms()).await?;
                store.seed_default_targets(now_ms()).await?;
                Ok::<_, netpulse_store::StoreError>(store)
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
            add_target,
            current_status,
            reliability,
            recent_connectivity
        ])
        .run(tauri::generate_context!())
        .expect("error while running NetPulse");
}

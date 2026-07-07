//! NetPulse Tauri application shell.
//!
//! Owns the desktop window + tray and exposes the storage layer to the
//! frontend via Tauri commands. All persistence lives in `netpulse-store`.

use netpulse_store::{NewTarget, Store, Target};
use tauri::{Manager, State};

/// Shared application state handed to every command.
struct AppState {
    store: Store,
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Resolve the per-user app data dir and open (creating + migrating)
            // the database there.
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let db_path = dir.join("netpulse.db");

            let store = tauri::async_runtime::block_on(Store::open(&db_path))
                .map_err(|e| format!("failed to open database at {db_path:?}: {e}"))?;

            app.manage(AppState { store });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![db_health, list_targets, add_target])
        .run(tauri::generate_context!())
        .expect("error while running NetPulse");
}

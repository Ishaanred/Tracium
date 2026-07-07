//! NetPulse storage layer.
//!
//! Owns the SQLite database: connection setup (WAL + sane PRAGMAs), schema
//! migrations, and (eventually) typed read/write helpers for each metric
//! domain. Deliberately free of any Tauri dependency so it can be unit-tested
//! without a GUI toolchain.
//!
//! See `../../db/schema.sql` and `docs/data-model.md` for the schema rationale.

use std::path::Path;
use std::str::FromStr;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;

/// Embedded migrations, compiled into the binary from `crates/store/migrations`.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// A handle to the NetPulse database.
#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// Open (creating if absent) the database at `path`, apply PRAGMAs, and run
    /// all pending migrations.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let opts = base_options(SqliteConnectOptions::new().filename(path).create_if_missing(true));
        Self::from_options(opts).await
    }

    /// Open a private in-memory database (used by tests). Each call is isolated.
    pub async fn open_in_memory() -> Result<Self> {
        let opts = base_options(
            SqliteConnectOptions::from_str("sqlite::memory:").expect("valid in-memory url"),
        );
        // In-memory DBs are per-connection, so cap the pool at 1 to keep one DB.
        let pool = SqlitePoolOptions::new().max_connections(1).connect_with(opts).await?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    async fn from_options(opts: SqliteConnectOptions) -> Result<Self> {
        let pool = SqlitePoolOptions::new().connect_with(opts).await?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    /// Apply any pending migrations.
    pub async fn migrate(&self) -> Result<()> {
        MIGRATOR.run(&self.pool).await?;
        Ok(())
    }

    /// Borrow the underlying pool for queries.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// All probe targets, ordered by id.
    pub async fn list_targets(&self) -> Result<Vec<Target>> {
        let rows = sqlx::query_as::<_, Target>(
            "SELECT id, label, host, kind, ip_version, enabled, created_at \
             FROM targets ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Insert a probe target, returning the created row.
    pub async fn add_target(&self, input: NewTarget) -> Result<Target> {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO targets (label, host, kind, ip_version, enabled, created_at) \
             VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(&input.label)
        .bind(&input.host)
        .bind(&input.kind)
        .bind(input.ip_version)
        .bind(input.enabled as i64)
        .bind(input.created_at)
        .fetch_one(&self.pool)
        .await?;

        Ok(Target {
            id,
            label: input.label,
            host: input.host,
            kind: input.kind,
            ip_version: input.ip_version,
            enabled: input.enabled,
            created_at: input.created_at,
        })
    }
}

/// A configured probe target (a host NetPulse pings/queries).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct Target {
    pub id: i64,
    pub label: String,
    pub host: String,
    pub kind: String,
    pub ip_version: Option<i64>,
    pub enabled: bool,
    pub created_at: i64,
}

/// Fields required to create a new [`Target`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NewTarget {
    pub label: String,
    pub host: String,
    pub kind: String,
    pub ip_version: Option<i64>,
    pub enabled: bool,
    pub created_at: i64,
}

/// PRAGMAs applied to every connection (see docs/data-model.md).
fn base_options(opts: SqliteConnectOptions) -> SqliteConnectOptions {
    opts.journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(5))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every table declared in the schema. Guards against a migration silently
    /// dropping one.
    const EXPECTED_TABLES: &[&str] = &[
        "ai_insights",
        "app_bandwidth_samples",
        "bandwidth_samples",
        "connectivity_samples",
        "device_bandwidth_samples",
        "dns_samples",
        "events",
        "interface_samples",
        "local_devices",
        "meta",
        "metric_rollups",
        "outages",
        "qoe_scores",
        "security_snapshots",
        "settings",
        "speedtests",
        "targets",
        "traceroute_hops",
        "traceroutes",
        "wifi_samples",
    ];

    async fn table_names(store: &Store) -> Vec<String> {
        sqlx::query_scalar::<_, String>(
            "SELECT name FROM sqlite_master WHERE type='table' \
             AND name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations' \
             ORDER BY name",
        )
        .fetch_all(store.pool())
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn migrations_create_every_table() {
        let store = Store::open_in_memory().await.unwrap();
        let tables = table_names(&store).await;
        assert_eq!(tables, EXPECTED_TABLES, "schema tables drifted from expectation");
    }

    #[tokio::test]
    async fn foreign_keys_are_enforced() {
        let store = Store::open_in_memory().await.unwrap();
        // device_bandwidth_samples.device_id has a FK to local_devices(id).
        let now: i64 = 1_700_000_000_000;
        let res = sqlx::query(
            "INSERT INTO device_bandwidth_samples (ts, device_id, rx_bytes, tx_bytes) \
             VALUES (?, 999, 0, 0)",
        )
        .bind(now)
        .execute(store.pool())
        .await;
        assert!(res.is_err(), "FK violation should be rejected with foreign_keys=ON");
    }

    #[tokio::test]
    async fn migrations_are_idempotent() {
        let store = Store::open_in_memory().await.unwrap();
        // Running again should be a no-op, not an error.
        store.migrate().await.unwrap();
    }

    #[tokio::test]
    async fn add_and_list_targets() {
        let store = Store::open_in_memory().await.unwrap();
        assert!(store.list_targets().await.unwrap().is_empty());

        let created = store
            .add_target(NewTarget {
                label: "Cloudflare".into(),
                host: "1.1.1.1".into(),
                kind: "internet".into(),
                ip_version: Some(4),
                enabled: true,
                created_at: 1_700_000_000_000,
            })
            .await
            .unwrap();
        assert_eq!(created.id, 1);
        assert!(created.enabled);

        let all = store.list_targets().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].host, "1.1.1.1");
        assert_eq!(all[0].ip_version, Some(4));
    }

    #[tokio::test]
    async fn rollup_bucket_is_unique_per_series() {
        let store = Store::open_in_memory().await.unwrap();
        let insert = |target_id: i64| {
            sqlx::query(
                "INSERT INTO metric_rollups (metric, target_id, bucket, bucket_ts, count) \
                 VALUES ('latency', ?, 'hour', 0, 1)",
            )
            .bind(target_id)
            .execute(store.pool())
        };
        // First global (target_id=0) row for this bucket: fine.
        insert(0).await.unwrap();
        // Second global row for the SAME bucket must be rejected. This is why
        // target_id defaults to 0 rather than NULL (NULLs would each be unique).
        assert!(insert(0).await.is_err(), "duplicate global bucket must collide");
        // A per-target series for the same bucket is a distinct row: allowed.
        insert(1).await.unwrap();
    }

    #[tokio::test]
    async fn file_db_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("netpulse.db");
        let store = Store::open(&path).await.unwrap();
        sqlx::query(
            "INSERT INTO targets (label, host, kind, enabled, created_at) \
             VALUES ('Cloudflare', '1.1.1.1', 'internet', 1, 0)",
        )
        .execute(store.pool())
        .await
        .unwrap();
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM targets")
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert_eq!(n, 1);
        assert!(path.exists(), "db file should be created on disk");
    }
}

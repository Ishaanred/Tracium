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

    /// Seed the default internet probe targets if the table is empty. Idempotent.
    pub async fn seed_default_targets(&self, now: i64) -> Result<()> {
        let existing: i64 = sqlx::query_scalar("SELECT count(*) FROM targets")
            .fetch_one(&self.pool)
            .await?;
        if existing > 0 {
            return Ok(());
        }
        let defaults = [
            ("Cloudflare", "1.1.1.1", 4),
            ("Google", "8.8.8.8", 4),
            ("Cloudflare v6", "2606:4700:4700::1111", 6),
        ];
        for (label, host, ipv) in defaults {
            self.add_target(NewTarget {
                label: label.into(),
                host: host.into(),
                kind: "internet".into(),
                ip_version: Some(ipv),
                enabled: true,
                created_at: now,
            })
            .await?;
        }
        Ok(())
    }

    /// Seed default settings if absent (JSON scalar values). Idempotent.
    pub async fn seed_default_settings(&self, now: i64) -> Result<()> {
        let defaults = [
            ("retention.raw_days", "7"),
            ("rollups.global_enabled", "true"),
            ("rollups.per_target_enabled", "false"),
        ];
        for (k, v) in defaults {
            sqlx::query(
                "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, ?) \
                 ON CONFLICT(key) DO NOTHING",
            )
            .bind(k)
            .bind(v)
            .bind(now)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }

    /// Insert one connectivity probe-cycle result.
    pub async fn insert_connectivity_sample(&self, s: NewConnectivitySample) -> Result<()> {
        sqlx::query(
            "INSERT INTO connectivity_samples \
             (ts, target_id, ip_version, sent, received, loss_pct, \
              rtt_min, rtt_avg, rtt_max, rtt_jitter, up) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(s.ts)
        .bind(s.target_id)
        .bind(s.ip_version)
        .bind(s.sent)
        .bind(s.received)
        .bind(s.loss_pct)
        .bind(s.rtt_min)
        .bind(s.rtt_avg)
        .bind(s.rtt_max)
        .bind(s.rtt_jitter)
        .bind(s.up as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Most recent connectivity samples across all targets, newest first.
    pub async fn recent_connectivity(&self, limit: i64) -> Result<Vec<ConnectivitySample>> {
        let rows = sqlx::query_as::<_, ConnectivitySample>(
            "SELECT id, ts, target_id, ip_version, sent, received, loss_pct, \
                    rtt_min, rtt_avg, rtt_max, rtt_jitter, up \
             FROM connectivity_samples ORDER BY ts DESC, id DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// The currently-open outage (no `ts_end`), if the internet is down now.
    pub async fn current_open_outage(&self) -> Result<Option<Outage>> {
        let row = sqlx::query_as::<_, Outage>(
            "SELECT id, ts_start, ts_end, duration_ms, reconnect_ms, cause \
             FROM outages WHERE ts_end IS NULL ORDER BY ts_start DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Open a new outage starting at `ts`, returning its id.
    pub async fn open_outage(&self, ts: i64, cause: Option<&str>) -> Result<i64> {
        let id = sqlx::query_scalar(
            "INSERT INTO outages (ts_start, cause) VALUES (?, ?) RETURNING id",
        )
        .bind(ts)
        .bind(cause)
        .fetch_one(&self.pool)
        .await?;
        Ok(id)
    }

    /// Close an open outage, materializing duration and reconnect time.
    pub async fn close_outage(&self, id: i64, ts_end: i64, reconnect_ms: Option<i64>) -> Result<()> {
        sqlx::query(
            "UPDATE outages SET ts_end = ?, duration_ms = ? - ts_start, reconnect_ms = ? \
             WHERE id = ?",
        )
        .bind(ts_end)
        .bind(ts_end)
        .bind(reconnect_ms)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record a discrete event.
    pub async fn insert_event(
        &self,
        ts: i64,
        kind: &str,
        severity: &str,
        duration_ms: Option<i64>,
        payload: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO events (ts, kind, severity, duration_ms, payload) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(ts)
        .bind(kind)
        .bind(severity)
        .bind(duration_ms)
        .bind(payload)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Reliability summary over samples with `ts >= since`.
    pub async fn reliability_since(&self, since: i64) -> Result<Reliability> {
        let (samples, up_samples): (i64, i64) = sqlx::query_as(
            "SELECT count(*), coalesce(sum(up), 0) FROM connectivity_samples WHERE ts >= ?",
        )
        .bind(since)
        .fetch_one(&self.pool)
        .await?;

        let avg_latency: Option<f64> = sqlx::query_scalar(
            "SELECT avg(rtt_avg) FROM connectivity_samples WHERE ts >= ? AND rtt_avg IS NOT NULL",
        )
        .bind(since)
        .fetch_one(&self.pool)
        .await?;

        let avg_loss: Option<f64> = sqlx::query_scalar(
            "SELECT avg(loss_pct) FROM connectivity_samples WHERE ts >= ?",
        )
        .bind(since)
        .fetch_one(&self.pool)
        .await?;

        let disconnects: i64 =
            sqlx::query_scalar("SELECT count(*) FROM outages WHERE ts_start >= ?")
                .bind(since)
                .fetch_one(&self.pool)
                .await?;

        let uptime_pct = if samples > 0 {
            up_samples as f64 / samples as f64 * 100.0
        } else {
            100.0
        };

        Ok(Reliability {
            samples,
            up_samples,
            uptime_pct,
            avg_latency_ms: avg_latency,
            avg_loss_pct: avg_loss,
            disconnects,
        })
    }
}

/// A connectivity probe-cycle result ready to persist.
#[derive(Debug, Clone)]
pub struct NewConnectivitySample {
    pub ts: i64,
    pub target_id: i64,
    pub ip_version: i64,
    pub sent: i64,
    pub received: i64,
    pub loss_pct: f64,
    pub rtt_min: Option<f64>,
    pub rtt_avg: Option<f64>,
    pub rtt_max: Option<f64>,
    pub rtt_jitter: Option<f64>,
    pub up: bool,
}

/// A stored connectivity sample.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct ConnectivitySample {
    pub id: i64,
    pub ts: i64,
    pub target_id: i64,
    pub ip_version: i64,
    pub sent: i64,
    pub received: i64,
    pub loss_pct: f64,
    pub rtt_min: Option<f64>,
    pub rtt_avg: Option<f64>,
    pub rtt_max: Option<f64>,
    pub rtt_jitter: Option<f64>,
    pub up: bool,
}

/// A recorded (possibly ongoing) internet outage.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct Outage {
    pub id: i64,
    pub ts_start: i64,
    pub ts_end: Option<i64>,
    pub duration_ms: Option<i64>,
    pub reconnect_ms: Option<i64>,
    pub cause: Option<String>,
}

/// Reliability rollup over a time window.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Reliability {
    pub samples: i64,
    pub up_samples: i64,
    pub uptime_pct: f64,
    pub avg_latency_ms: Option<f64>,
    pub avg_loss_pct: Option<f64>,
    pub disconnects: i64,
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
    async fn seed_default_targets_is_idempotent() {
        let store = Store::open_in_memory().await.unwrap();
        store.seed_default_targets(0).await.unwrap();
        store.seed_default_targets(0).await.unwrap();
        let targets = store.list_targets().await.unwrap();
        assert_eq!(targets.len(), 3, "seeding twice should not duplicate");
    }

    #[tokio::test]
    async fn seed_default_settings_is_idempotent() {
        let store = Store::open_in_memory().await.unwrap();
        store.seed_default_settings(0).await.unwrap();
        store.seed_default_settings(0).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM settings")
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert_eq!(n, 3);
        let raw_days: String =
            sqlx::query_scalar("SELECT value FROM settings WHERE key = 'retention.raw_days'")
                .fetch_one(store.pool())
                .await
                .unwrap();
        assert_eq!(raw_days, "7");
    }

    #[tokio::test]
    async fn connectivity_and_reliability() {
        let store = Store::open_in_memory().await.unwrap();
        store.seed_default_targets(0).await.unwrap();

        let sample = |ts: i64, up: bool| NewConnectivitySample {
            ts,
            target_id: 1,
            ip_version: 4,
            sent: 5,
            received: if up { 5 } else { 0 },
            loss_pct: if up { 0.0 } else { 100.0 },
            rtt_min: up.then_some(10.0),
            rtt_avg: up.then_some(20.0),
            rtt_max: up.then_some(30.0),
            rtt_jitter: up.then_some(2.0),
            up,
        };
        // 3 up, 1 down.
        for (ts, up) in [(100, true), (200, true), (300, false), (400, true)] {
            store.insert_connectivity_sample(sample(ts, up)).await.unwrap();
        }

        let recent = store.recent_connectivity(10).await.unwrap();
        assert_eq!(recent.len(), 4);
        assert_eq!(recent[0].ts, 400, "newest first");

        let r = store.reliability_since(0).await.unwrap();
        assert_eq!(r.samples, 4);
        assert_eq!(r.up_samples, 3);
        assert_eq!(r.uptime_pct, 75.0);
        assert_eq!(r.avg_latency_ms, Some(20.0));
    }

    #[tokio::test]
    async fn outage_open_and_close() {
        let store = Store::open_in_memory().await.unwrap();
        assert!(store.current_open_outage().await.unwrap().is_none());

        let id = store.open_outage(1000, Some("all targets down")).await.unwrap();
        let open = store.current_open_outage().await.unwrap().unwrap();
        assert_eq!(open.id, id);
        assert!(open.ts_end.is_none());

        store.close_outage(id, 4000, Some(1200)).await.unwrap();
        assert!(store.current_open_outage().await.unwrap().is_none());

        let r = store.reliability_since(0).await.unwrap();
        assert_eq!(r.disconnects, 1);
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

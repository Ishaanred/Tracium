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

    /// Most recent events, newest first (the Event Timeline).
    pub async fn recent_events(&self, limit: i64) -> Result<Vec<Event>> {
        let rows = sqlx::query_as::<_, Event>(
            "SELECT id, ts, kind, severity, duration_ms, payload \
             FROM events ORDER BY ts DESC, id DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Most recent outages, newest first (the Incident Log).
    pub async fn recent_outages(&self, limit: i64) -> Result<Vec<Outage>> {
        let rows = sqlx::query_as::<_, Outage>(
            "SELECT id, ts_start, ts_end, duration_ms, reconnect_ms, cause \
             FROM outages ORDER BY ts_start DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// CSV export of connectivity samples with `ts >= since`.
    pub async fn export_connectivity_csv(&self, since: i64) -> Result<String> {
        let rows = sqlx::query_as::<_, ConnectivitySample>(
            "SELECT id, ts, target_id, ip_version, sent, received, loss_pct, \
                    rtt_min, rtt_avg, rtt_max, rtt_jitter, up \
             FROM connectivity_samples WHERE ts >= ? ORDER BY ts",
        )
        .bind(since)
        .fetch_all(&self.pool)
        .await?;

        let mut out = String::from(
            "ts,target_id,ip_version,sent,received,loss_pct,rtt_min,rtt_avg,rtt_max,rtt_jitter,up\n",
        );
        for r in rows {
            let opt = |v: Option<f64>| v.map(|x| x.to_string()).unwrap_or_default();
            out.push_str(&format!(
                "{},{},{},{},{},{},{},{},{},{},{}\n",
                r.ts,
                r.target_id,
                r.ip_version,
                r.sent,
                r.received,
                r.loss_pct,
                opt(r.rtt_min),
                opt(r.rtt_avg),
                opt(r.rtt_max),
                opt(r.rtt_jitter),
                r.up as i64,
            ));
        }
        Ok(out)
    }

    /// CSV export of events with `ts >= since`.
    pub async fn export_events_csv(&self, since: i64) -> Result<String> {
        let rows = self
            .recent_events(i64::MAX)
            .await?
            .into_iter()
            .filter(|e| e.ts >= since)
            .collect::<Vec<_>>();

        let mut out = String::from("ts,kind,severity,duration_ms,payload\n");
        for r in rows.iter().rev() {
            out.push_str(&format!(
                "{},{},{},{},{}\n",
                r.ts,
                csv_field(&r.kind),
                csv_field(&r.severity),
                r.duration_ms.map(|d| d.to_string()).unwrap_or_default(),
                csv_field(r.payload.as_deref().unwrap_or("")),
            ));
        }
        Ok(out)
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

    /// Record the aggregate bandwidth rate for a cycle.
    pub async fn insert_bandwidth_sample(&self, ts: i64, rx_bps: i64, tx_bps: i64) -> Result<()> {
        sqlx::query("INSERT INTO bandwidth_samples (ts, rx_bps, tx_bps) VALUES (?, ?, ?)")
            .bind(ts)
            .bind(rx_bps)
            .bind(tx_bps)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Record per-interface byte deltas (used for usage totals).
    pub async fn insert_interface_bytes(
        &self,
        ts: i64,
        iface: &str,
        rx_bytes: i64,
        tx_bytes: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO interface_samples (ts, iface, rx_bytes, tx_bytes) VALUES (?, ?, ?, ?)",
        )
        .bind(ts)
        .bind(iface)
        .bind(rx_bytes)
        .bind(tx_bytes)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The latest aggregate bandwidth rate, if any.
    pub async fn latest_bandwidth(&self) -> Result<Option<BandwidthNow>> {
        let row = sqlx::query_as::<_, BandwidthNow>(
            "SELECT ts, rx_bps, tx_bps FROM bandwidth_samples ORDER BY ts DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Total bytes transferred (summed over interfaces) since `since`.
    pub async fn bandwidth_totals(&self, since: i64) -> Result<BandwidthTotals> {
        let (rx, tx): (i64, i64) = sqlx::query_as(
            "SELECT coalesce(sum(rx_bytes), 0), coalesce(sum(tx_bytes), 0) \
             FROM interface_samples WHERE ts >= ?",
        )
        .bind(since)
        .fetch_one(&self.pool)
        .await?;
        Ok(BandwidthTotals { rx_bytes: rx, tx_bytes: tx })
    }

    /// Record a full security-posture snapshot.
    pub async fn insert_security_snapshot(&self, s: &SecuritySnapshot) -> Result<()> {
        sqlx::query(
            "INSERT INTO security_snapshots \
             (ts, public_ip, nat_type, upnp_enabled, firewall_active, vpn_detected, \
              doh_active, dot_active, open_ports) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(s.ts)
        .bind(&s.public_ip)
        .bind(&s.nat_type)
        .bind(s.upnp_enabled.map(|b| b as i64))
        .bind(s.firewall_active.map(|b| b as i64))
        .bind(s.vpn_detected.map(|b| b as i64))
        .bind(s.doh_active.map(|b| b as i64))
        .bind(s.dot_active.map(|b| b as i64))
        .bind(&s.open_ports)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The latest full security snapshot (one written with posture fields set,
    /// identified by a non-null `vpn_detected`).
    pub async fn latest_security(&self) -> Result<Option<SecuritySnapshot>> {
        let row = sqlx::query_as::<_, SecuritySnapshot>(
            "SELECT ts, public_ip, nat_type, upnp_enabled, firewall_active, vpn_detected, \
                    doh_active, dot_active, open_ports \
             FROM security_snapshots WHERE vpn_detected IS NOT NULL \
             ORDER BY ts DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    /// Record a security snapshot carrying the public IP.
    pub async fn insert_public_ip(&self, ts: i64, ip: Option<&str>) -> Result<()> {
        sqlx::query("INSERT INTO security_snapshots (ts, public_ip) VALUES (?, ?)")
            .bind(ts)
            .bind(ip)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The most recently observed public IP, if any.
    pub async fn latest_public_ip(&self) -> Result<Option<String>> {
        let ip = sqlx::query_scalar::<_, String>(
            "SELECT public_ip FROM security_snapshots \
             WHERE public_ip IS NOT NULL ORDER BY ts DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(ip)
    }

    /// Record one DNS lookup result.
    pub async fn insert_dns_sample(
        &self,
        ts: i64,
        resolver: &str,
        query_host: &str,
        lookup_ms: Option<f64>,
        success: bool,
        cached: Option<bool>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO dns_samples (ts, resolver, query_host, lookup_ms, success, cached) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(ts)
        .bind(resolver)
        .bind(query_host)
        .bind(lookup_ms)
        .bind(success as i64)
        .bind(cached.map(|c| c as i64))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Per-resolver DNS performance since `since`, fastest average first.
    pub async fn dns_comparison(&self, since: i64) -> Result<Vec<DnsResolverStat>> {
        let rows = sqlx::query_as::<_, DnsResolverStat>(
            "SELECT resolver, \
                    avg(lookup_ms) AS avg_ms, \
                    count(*) AS count, \
                    sum(CASE WHEN success = 0 THEN 1 ELSE 0 END) AS failures \
             FROM dns_samples WHERE ts >= ? \
             GROUP BY resolver ORDER BY avg_ms",
        )
        .bind(since)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Record a QoE score row.
    pub async fn insert_qoe(
        &self,
        ts: i64,
        gaming: f64,
        video_call: f64,
        streaming: f64,
        web: f64,
        voip: f64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO qoe_scores (ts, gaming, video_call, streaming, web, voip) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(ts)
        .bind(gaming)
        .bind(video_call)
        .bind(streaming)
        .bind(web)
        .bind(voip)
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

    /// Roll up connectivity metrics (latency, loss, jitter) into `metric_rollups`
    /// for the hour and day buckets, then prune raw samples older than the
    /// retention window. Safe to call repeatedly; only *closed* buckets that
    /// still have raw data are (re)computed, so already-pruned buckets are left
    /// untouched. Returns the number of rollup rows written.
    pub async fn maintain(&self, now: i64) -> Result<i64> {
        const HOUR_MS: i64 = 3_600_000;
        const DAY_MS: i64 = 86_400_000;
        let metrics = [("latency", "rtt_avg"), ("loss", "loss_pct"), ("jitter", "rtt_jitter")];

        let mut written = 0;
        for (metric, col) in metrics {
            written += self.rollup_metric(metric, col, HOUR_MS, "hour", now).await?;
            written += self.rollup_metric(metric, col, DAY_MS, "day", now).await?;
        }

        // Prune raw samples older than retention, aligned to the hour so we never
        // half-prune a bucket that a future rollup pass might recompute.
        let days = self.get_setting_i64("retention.raw_days").await?.unwrap_or(7);
        let cutoff = ((now - days * DAY_MS) / HOUR_MS) * HOUR_MS;
        self.prune_connectivity_before(cutoff).await?;

        Ok(written)
    }

    /// Aggregate one metric into `metric_rollups` (global series, `target_id=0`)
    /// for closed buckets. `col` is a fixed internal column name (never user
    /// input), so the formatted SQL is injection-safe.
    async fn rollup_metric(
        &self,
        metric: &str,
        col: &str,
        bucket_ms: i64,
        bucket_label: &str,
        now: i64,
    ) -> Result<i64> {
        let current_bucket = (now / bucket_ms) * bucket_ms;
        let sql = format!(
            "SELECT (ts / {bucket_ms}) * {bucket_ms} AS b, {col} AS v \
             FROM connectivity_samples \
             WHERE {col} IS NOT NULL AND ts < ? ORDER BY b",
        );
        let rows: Vec<(i64, f64)> =
            sqlx::query_as(&sql).bind(current_bucket).fetch_all(&self.pool).await?;

        let mut written = 0;
        let mut i = 0;
        while i < rows.len() {
            let bucket_ts = rows[i].0;
            let mut vals = Vec::new();
            while i < rows.len() && rows[i].0 == bucket_ts {
                vals.push(rows[i].1);
                i += 1;
            }
            self.upsert_rollup(metric, bucket_label, bucket_ts, &mut vals).await?;
            written += 1;
        }
        Ok(written)
    }

    async fn upsert_rollup(
        &self,
        metric: &str,
        bucket: &str,
        bucket_ts: i64,
        vals: &mut [f64],
    ) -> Result<()> {
        vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let count = vals.len() as i64;
        let sum: f64 = vals.iter().sum();
        let min = vals[0];
        let max = vals[vals.len() - 1];
        let avg = sum / count as f64;
        let p50 = percentile(vals, 50.0);
        let p95 = percentile(vals, 95.0);

        sqlx::query(
            "INSERT INTO metric_rollups \
             (metric, target_id, bucket, bucket_ts, count, min, avg, max, p50, p95, sum) \
             VALUES (?, 0, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(metric, target_id, bucket, bucket_ts) DO UPDATE SET \
               count = excluded.count, min = excluded.min, avg = excluded.avg, \
               max = excluded.max, p50 = excluded.p50, p95 = excluded.p95, sum = excluded.sum",
        )
        .bind(metric)
        .bind(bucket)
        .bind(bucket_ts)
        .bind(count)
        .bind(min)
        .bind(avg)
        .bind(max)
        .bind(p50)
        .bind(p95)
        .bind(sum)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn prune_connectivity_before(&self, cutoff: i64) -> Result<u64> {
        let r = sqlx::query("DELETE FROM connectivity_samples WHERE ts < ?")
            .bind(cutoff)
            .execute(&self.pool)
            .await?;
        Ok(r.rows_affected())
    }

    /// Read a settings value as i64 (values are JSON scalars; a bare number
    /// parses directly).
    pub async fn get_setting_i64(&self, key: &str) -> Result<Option<i64>> {
        let v: Option<String> =
            sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?;
        Ok(v.and_then(|s| s.trim().parse::<i64>().ok()))
    }

    /// Read rollup rows for a metric/bucket since `bucket_ts >= since` (global series).
    pub async fn rollups(&self, metric: &str, bucket: &str, since: i64) -> Result<Vec<Rollup>> {
        let rows = sqlx::query_as::<_, Rollup>(
            "SELECT metric, target_id, bucket, bucket_ts, count, min, avg, max, p50, p95, sum \
             FROM metric_rollups \
             WHERE metric = ? AND bucket = ? AND target_id = 0 AND bucket_ts >= ? \
             ORDER BY bucket_ts",
        )
        .bind(metric)
        .bind(bucket)
        .bind(since)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

/// Quote a CSV field if it contains a comma, quote, or newline (RFC 4180).
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Exact percentile (linear interpolation) over an already-sorted slice.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    match sorted.len() {
        0 => 0.0,
        1 => sorted[0],
        n => {
            let rank = p / 100.0 * (n as f64 - 1.0);
            let lo = rank.floor() as usize;
            let hi = rank.ceil() as usize;
            if lo == hi {
                sorted[lo]
            } else {
                let frac = rank - lo as f64;
                sorted[lo] + (sorted[hi] - sorted[lo]) * frac
            }
        }
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

/// A security-posture snapshot. Fields are `None` when a probe couldn't
/// determine a value (or isn't implemented yet, e.g. NAT/UPnP).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct SecuritySnapshot {
    pub ts: i64,
    pub public_ip: Option<String>,
    pub nat_type: Option<String>,
    pub upnp_enabled: Option<bool>,
    pub firewall_active: Option<bool>,
    pub vpn_detected: Option<bool>,
    pub doh_active: Option<bool>,
    pub dot_active: Option<bool>,
    /// JSON array of locally-listening ports.
    pub open_ports: Option<String>,
}

/// Latest aggregate bandwidth rate (bits/sec).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct BandwidthNow {
    pub ts: i64,
    pub rx_bps: i64,
    pub tx_bps: i64,
}

/// Total bytes transferred over a window.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BandwidthTotals {
    pub rx_bytes: i64,
    pub tx_bytes: i64,
}

/// Aggregated DNS performance for one resolver over a window.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct DnsResolverStat {
    pub resolver: String,
    pub avg_ms: Option<f64>,
    pub count: i64,
    pub failures: i64,
}

/// A discrete recorded event (disconnect, reconnect, roam, ...).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct Event {
    pub id: i64,
    pub ts: i64,
    pub kind: String,
    pub severity: String,
    pub duration_ms: Option<i64>,
    pub payload: Option<String>,
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

/// One aggregated metric bucket from `metric_rollups`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct Rollup {
    pub metric: String,
    pub target_id: i64,
    pub bucket: String,
    pub bucket_ts: i64,
    pub count: i64,
    pub min: Option<f64>,
    pub avg: Option<f64>,
    pub max: Option<f64>,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub sum: Option<f64>,
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
    async fn bandwidth_rate_and_totals() {
        let store = Store::open_in_memory().await.unwrap();
        assert!(store.latest_bandwidth().await.unwrap().is_none());

        store.insert_bandwidth_sample(100, 8_000_000, 1_000_000).await.unwrap();
        store.insert_bandwidth_sample(200, 16_000_000, 2_000_000).await.unwrap();
        store.insert_interface_bytes(100, "eth0", 1_000, 200).await.unwrap();
        store.insert_interface_bytes(200, "eth0", 3_000, 400).await.unwrap();

        let now = store.latest_bandwidth().await.unwrap().unwrap();
        assert_eq!(now.ts, 200);
        assert_eq!(now.rx_bps, 16_000_000);

        let totals = store.bandwidth_totals(0).await.unwrap();
        assert_eq!(totals.rx_bytes, 4_000);
        assert_eq!(totals.tx_bytes, 600);
    }

    #[tokio::test]
    async fn security_snapshot_roundtrip() {
        let store = Store::open_in_memory().await.unwrap();
        assert!(store.latest_security().await.unwrap().is_none());

        store
            .insert_security_snapshot(&SecuritySnapshot {
                ts: 100,
                firewall_active: Some(true),
                vpn_detected: Some(false),
                doh_active: Some(true),
                dot_active: Some(true),
                open_ports: Some("[22,443]".into()),
                ..Default::default()
            })
            .await
            .unwrap();

        let s = store.latest_security().await.unwrap().unwrap();
        assert_eq!(s.ts, 100);
        assert_eq!(s.firewall_active, Some(true));
        assert_eq!(s.vpn_detected, Some(false));
        assert_eq!(s.doh_active, Some(true));
        assert_eq!(s.open_ports.as_deref(), Some("[22,443]"));
        assert!(s.nat_type.is_none());
    }

    #[tokio::test]
    async fn public_ip_latest() {
        let store = Store::open_in_memory().await.unwrap();
        assert!(store.latest_public_ip().await.unwrap().is_none());
        store.insert_public_ip(100, Some("203.0.113.4")).await.unwrap();
        store.insert_public_ip(200, Some("203.0.113.9")).await.unwrap();
        assert_eq!(store.latest_public_ip().await.unwrap().as_deref(), Some("203.0.113.9"));
    }

    #[tokio::test]
    async fn dns_samples_and_comparison() {
        let store = Store::open_in_memory().await.unwrap();
        // Cloudflare: 10ms, 20ms (avg 15). Google: 30ms + 1 failure.
        store.insert_dns_sample(1, "1.1.1.1", "example.com", Some(10.0), true, None).await.unwrap();
        store.insert_dns_sample(2, "1.1.1.1", "example.com", Some(20.0), true, None).await.unwrap();
        store.insert_dns_sample(3, "8.8.8.8", "example.com", Some(30.0), true, None).await.unwrap();
        store.insert_dns_sample(4, "8.8.8.8", "example.com", None, false, None).await.unwrap();

        let cmp = store.dns_comparison(0).await.unwrap();
        assert_eq!(cmp.len(), 2);
        // Fastest average first -> Cloudflare.
        assert_eq!(cmp[0].resolver, "1.1.1.1");
        assert_eq!(cmp[0].avg_ms, Some(15.0));
        assert_eq!(cmp[0].failures, 0);
        assert_eq!(cmp[1].resolver, "8.8.8.8");
        assert_eq!(cmp[1].failures, 1);
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

    #[test]
    fn csv_field_escapes_specials() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[tokio::test]
    async fn events_timeline_and_csv() {
        let store = Store::open_in_memory().await.unwrap();
        store.insert_event(100, "disconnect", "critical", None, None).await.unwrap();
        store
            .insert_event(200, "reconnect", "info", Some(3000), Some(r#"{"note":"a,b"}"#))
            .await
            .unwrap();

        let events = store.recent_events(10).await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].ts, 200, "newest first");
        assert_eq!(events[0].kind, "reconnect");

        let csv = store.export_events_csv(0).await.unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "ts,kind,severity,duration_ms,payload");
        // Oldest first in export; payload with a comma must be quoted.
        assert!(lines[1].starts_with("100,disconnect,critical,,"));
        assert!(lines[2].contains("\"{\"\"note\"\":\"\"a,b\"\"}\""), "got: {}", lines[2]);
    }

    #[tokio::test]
    async fn connectivity_csv_has_header_and_rows() {
        let store = Store::open_in_memory().await.unwrap();
        store.seed_default_targets(0).await.unwrap();
        store
            .insert_connectivity_sample(NewConnectivitySample {
                ts: 500,
                target_id: 1,
                ip_version: 4,
                sent: 5,
                received: 5,
                loss_pct: 0.0,
                rtt_min: Some(10.0),
                rtt_avg: Some(12.0),
                rtt_max: Some(15.0),
                rtt_jitter: Some(1.0),
                up: true,
            })
            .await
            .unwrap();
        let csv = store.export_connectivity_csv(0).await.unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[1].starts_with("500,1,4,5,5,0"));
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

    #[test]
    fn percentile_interpolates() {
        let v = vec![10.0, 20.0, 30.0, 40.0, 50.0];
        assert_eq!(percentile(&v, 50.0), 30.0);
        assert_eq!(percentile(&v, 0.0), 10.0);
        assert_eq!(percentile(&v, 100.0), 50.0);
        // p95 of 5 points: rank = 0.95*4 = 3.8 -> 40 + 0.8*(50-40) = 48
        assert!((percentile(&v, 95.0) - 48.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn maintain_rolls_up_and_prunes() {
        let store = Store::open_in_memory().await.unwrap();
        store.seed_default_settings(0).await.unwrap();
        store.seed_default_targets(0).await.unwrap();

        const HOUR: i64 = 3_600_000;
        // Put 3 samples in hour bucket 0 with latencies 10/20/30.
        for (off, rtt) in [(0, 10.0), (60_000, 20.0), (120_000, 30.0)] {
            store
                .insert_connectivity_sample(NewConnectivitySample {
                    ts: off,
                    target_id: 1,
                    ip_version: 4,
                    sent: 5,
                    received: 5,
                    loss_pct: 0.0,
                    rtt_min: Some(rtt),
                    rtt_avg: Some(rtt),
                    rtt_max: Some(rtt),
                    rtt_jitter: Some(1.0),
                    up: true,
                })
                .await
                .unwrap();
        }

        // "now" well past that hour so the bucket is closed. Retention default 7d
        // keeps the raw rows (they're recent relative to `now` here = 5h).
        let now = 5 * HOUR;
        let written = store.maintain(now).await.unwrap();
        assert!(written >= 2, "expected latency+loss+jitter hour/day rollups");

        let latency = store.rollups("latency", "hour", 0).await.unwrap();
        assert_eq!(latency.len(), 1);
        assert_eq!(latency[0].bucket_ts, 0);
        assert_eq!(latency[0].count, 3);
        assert_eq!(latency[0].avg, Some(20.0));
        assert_eq!(latency[0].p50, Some(20.0));

        // Raw samples still present (within retention).
        assert_eq!(store.recent_connectivity(10).await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn maintain_prunes_old_raw_but_keeps_rollup() {
        let store = Store::open_in_memory().await.unwrap();
        // Retention 0 days => prune everything up to the current hour.
        store.seed_default_targets(0).await.unwrap();
        sqlx::query("INSERT INTO settings (key, value, updated_at) VALUES ('retention.raw_days','0',0)")
            .execute(store.pool())
            .await
            .unwrap();

        store
            .insert_connectivity_sample(NewConnectivitySample {
                ts: 0,
                target_id: 1,
                ip_version: 4,
                sent: 5,
                received: 5,
                loss_pct: 0.0,
                rtt_min: Some(15.0),
                rtt_avg: Some(15.0),
                rtt_max: Some(15.0),
                rtt_jitter: Some(0.0),
                up: true,
            })
            .await
            .unwrap();

        let now = 5 * 3_600_000;
        store.maintain(now).await.unwrap();

        // Rollup persisted...
        assert_eq!(store.rollups("latency", "hour", 0).await.unwrap().len(), 1);
        // ...but the old raw sample was pruned.
        assert_eq!(store.recent_connectivity(10).await.unwrap().len(), 0);
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

-- Tracium database schema
-- Engine: SQLite 3.37+ (STRICT tables), WAL mode.
-- Conventions:
--   * All timestamps are unix epoch MILLISECONDS, UTC, stored as INTEGER.
--   * Booleans are INTEGER 0/1.
--   * Flexible / kind-specific payloads are JSON stored as TEXT (json1 extension).
--   * Raw sample tables have SHORT retention; long-term history lives in metric_rollups.
--
-- This file is the source of truth for the schema. Migrations are derived from it
-- (see docs/data-model.md). Keep PRAGMAs in application setup, not here.

PRAGMA foreign_keys = ON;

-- ---------------------------------------------------------------------------
-- Meta / configuration
-- ---------------------------------------------------------------------------

-- Schema version + arbitrary app metadata.
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;

-- User-tunable settings (probe intervals, retention windows, thresholds,
-- declared ISP plan speeds, etc.). Values are JSON so a setting can be a
-- scalar or an object.
CREATE TABLE IF NOT EXISTS settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,           -- JSON
    updated_at INTEGER NOT NULL
) STRICT;

-- Probe targets (internet reachability hosts, the gateway, custom hosts).
CREATE TABLE IF NOT EXISTS targets (
    id         INTEGER PRIMARY KEY,
    label      TEXT NOT NULL,           -- "Cloudflare", "Gateway", ...
    host       TEXT NOT NULL,           -- 1.1.1.1, 8.8.8.8, gateway ip, hostname
    kind       TEXT NOT NULL,           -- 'internet' | 'gateway' | 'dns' | 'custom'
    ip_version INTEGER,                 -- 4 | 6 | NULL (either)
    enabled    INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL
) STRICT;

-- ---------------------------------------------------------------------------
-- Connectivity (hot path) -- one row per probe CYCLE, not per ping.
-- A cycle sends N pings and stores the aggregate. Keeps row count sane.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS connectivity_samples (
    id         INTEGER PRIMARY KEY,
    ts         INTEGER NOT NULL,        -- cycle end time
    target_id  INTEGER NOT NULL REFERENCES targets(id),
    ip_version INTEGER NOT NULL,        -- 4 | 6
    sent       INTEGER NOT NULL,
    received   INTEGER NOT NULL,
    loss_pct   REAL NOT NULL,           -- derived, stored for cheap querying
    rtt_min    REAL,                    -- ms; NULL if 100% loss
    rtt_avg    REAL,
    rtt_max    REAL,
    rtt_jitter REAL,                    -- mean deviation of rtt (ms)
    up         INTEGER NOT NULL         -- 1 if any ping succeeded this cycle
) STRICT;
CREATE INDEX IF NOT EXISTS ix_conn_ts        ON connectivity_samples(ts);
CREATE INDEX IF NOT EXISTS ix_conn_target_ts ON connectivity_samples(target_id, ts);

-- ---------------------------------------------------------------------------
-- Outages / reliability -- one row per detected internet outage.
-- ts_end / duration_ms are NULL while an outage is ongoing.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS outages (
    id           INTEGER PRIMARY KEY,
    ts_start     INTEGER NOT NULL,
    ts_end       INTEGER,
    duration_ms  INTEGER,               -- ts_end - ts_start, materialized on close
    reconnect_ms INTEGER,               -- time from first retry-success signal
    cause        TEXT                   -- best-effort classification (JSON or text)
) STRICT;
CREATE INDEX IF NOT EXISTS ix_outages_start ON outages(ts_start);

-- ---------------------------------------------------------------------------
-- Generic discrete events -- disconnect, reconnect, roam, route_change,
-- dns_failure, threshold_breach, incident_open/close, config_change, ...
-- kind-specific detail lives in `payload` (JSON).
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS events (
    id          INTEGER PRIMARY KEY,
    ts          INTEGER NOT NULL,
    kind        TEXT NOT NULL,
    severity    TEXT NOT NULL DEFAULT 'info',   -- 'info' | 'warn' | 'critical'
    duration_ms INTEGER,                        -- for events that span time
    payload     TEXT                            -- JSON
) STRICT;
CREATE INDEX IF NOT EXISTS ix_events_ts        ON events(ts);
CREATE INDEX IF NOT EXISTS ix_events_kind_ts   ON events(kind, ts);

-- ---------------------------------------------------------------------------
-- Speed tests + bufferbloat (measured together in one run).
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS speedtests (
    id               INTEGER PRIMARY KEY,
    ts               INTEGER NOT NULL,
    engine           TEXT,               -- which OSS engine ran it (credited)
    server           TEXT,
    server_location  TEXT,
    isp              TEXT,
    download_mbps    REAL,
    upload_mbps      REAL,
    ping_ms          REAL,               -- idle ping during test
    jitter_ms        REAL,
    loss_pct         REAL,               -- packet loss under load
    -- bufferbloat (latency under load)
    idle_latency_ms  REAL,
    down_latency_ms  REAL,               -- latency during download saturation
    up_latency_ms    REAL,               -- latency during upload saturation
    bufferbloat_grade TEXT               -- 'A'..'F'
) STRICT;
CREATE INDEX IF NOT EXISTS ix_speedtests_ts ON speedtests(ts);

-- ---------------------------------------------------------------------------
-- Wi-Fi samples (snapshots of the active wireless link).
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS wifi_samples (
    id             INTEGER PRIMARY KEY,
    ts             INTEGER NOT NULL,
    ssid           TEXT,
    bssid          TEXT,                 -- AP MAC; changes on roam
    rssi_dbm       INTEGER,
    quality_pct    REAL,
    link_speed_mbps REAL,                -- negotiated
    phy_rate_mbps  REAL,                 -- actual achieved
    band           TEXT,                 -- '2.4' | '5' | '6'
    channel        INTEGER,
    channel_width  INTEGER,              -- MHz (20/40/80/160)
    noise_dbm      INTEGER,
    retrans_pct    REAL
) STRICT;
CREATE INDEX IF NOT EXISTS ix_wifi_ts ON wifi_samples(ts);

-- ---------------------------------------------------------------------------
-- DNS samples (per lookup / per periodic probe).
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS dns_samples (
    id          INTEGER PRIMARY KEY,
    ts          INTEGER NOT NULL,
    resolver    TEXT NOT NULL,           -- which resolver answered
    query_host  TEXT NOT NULL,
    lookup_ms   REAL,
    success     INTEGER NOT NULL,
    cached      INTEGER                  -- 1/0/NULL if unknown
) STRICT;
CREATE INDEX IF NOT EXISTS ix_dns_ts          ON dns_samples(ts);
CREATE INDEX IF NOT EXISTS ix_dns_resolver_ts ON dns_samples(resolver, ts);

-- ---------------------------------------------------------------------------
-- Routing -- traceroute parent + hops. route_hash detects path changes.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS traceroutes (
    id         INTEGER PRIMARY KEY,
    ts         INTEGER NOT NULL,
    target     TEXT NOT NULL,
    hop_count  INTEGER NOT NULL,
    route_hash TEXT NOT NULL             -- hash of the ordered hop IPs
) STRICT;
CREATE INDEX IF NOT EXISTS ix_traceroutes_ts ON traceroutes(ts);

CREATE TABLE IF NOT EXISTS traceroute_hops (
    id            INTEGER PRIMARY KEY,
    traceroute_id INTEGER NOT NULL REFERENCES traceroutes(id) ON DELETE CASCADE,
    hop_no        INTEGER NOT NULL,
    ip            TEXT,
    hostname      TEXT,
    asn           TEXT,
    as_name       TEXT,
    rtt_ms        REAL,
    loss_pct      REAL
) STRICT;
CREATE INDEX IF NOT EXISTS ix_hops_traceroute ON traceroute_hops(traceroute_id, hop_no);

-- ---------------------------------------------------------------------------
-- Local network -- devices, per-device bandwidth, NIC/interface stats.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS local_devices (
    id         INTEGER PRIMARY KEY,
    mac        TEXT UNIQUE,             -- primary identity when available
    hostname   TEXT,
    ip         TEXT,
    vendor     TEXT,                    -- OUI lookup
    first_seen INTEGER NOT NULL,
    last_seen  INTEGER NOT NULL,
    is_active  INTEGER NOT NULL DEFAULT 1
) STRICT;

CREATE TABLE IF NOT EXISTS device_bandwidth_samples (
    id         INTEGER PRIMARY KEY,
    ts         INTEGER NOT NULL,
    device_id  INTEGER NOT NULL REFERENCES local_devices(id) ON DELETE CASCADE,
    rx_bytes   INTEGER NOT NULL,        -- delta since previous sample
    tx_bytes   INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS ix_devbw_ts        ON device_bandwidth_samples(ts);
CREATE INDEX IF NOT EXISTS ix_devbw_device_ts ON device_bandwidth_samples(device_id, ts);

-- Local NIC / interface counters + gateway health.
CREATE TABLE IF NOT EXISTS interface_samples (
    id           INTEGER PRIMARY KEY,
    ts           INTEGER NOT NULL,
    iface        TEXT NOT NULL,
    rx_bytes     INTEGER,               -- delta
    tx_bytes     INTEGER,
    rx_errors    INTEGER,
    tx_errors    INTEGER,
    rx_drops     INTEGER,
    tx_drops     INTEGER,
    gateway_rtt_ms REAL,
    lan_loss_pct REAL
) STRICT;
CREATE INDEX IF NOT EXISTS ix_iface_ts ON interface_samples(ts);

-- ---------------------------------------------------------------------------
-- Bandwidth (aggregate machine throughput) + per-application.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS bandwidth_samples (
    id       INTEGER PRIMARY KEY,
    ts       INTEGER NOT NULL,
    rx_bps   INTEGER NOT NULL,          -- current download rate (bits/sec)
    tx_bps   INTEGER NOT NULL           -- current upload rate
) STRICT;
CREATE INDEX IF NOT EXISTS ix_bw_ts ON bandwidth_samples(ts);

-- Per-application breakdown (advanced; requires privileged capture on both OSes).
CREATE TABLE IF NOT EXISTS app_bandwidth_samples (
    id       INTEGER PRIMARY KEY,
    ts       INTEGER NOT NULL,
    app_name TEXT NOT NULL,
    rx_bytes INTEGER NOT NULL,          -- delta
    tx_bytes INTEGER NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS ix_appbw_ts ON app_bandwidth_samples(ts);

-- ---------------------------------------------------------------------------
-- Security posture snapshots.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS security_snapshots (
    id             INTEGER PRIMARY KEY,
    ts             INTEGER NOT NULL,
    public_ip      TEXT,
    nat_type       TEXT,                -- 'open' | 'moderate' | 'strict' | ...
    upnp_enabled   INTEGER,
    firewall_active INTEGER,
    vpn_detected   INTEGER,
    doh_active     INTEGER,             -- DNS-over-HTTPS
    dot_active     INTEGER,             -- DNS-over-TLS
    open_ports     TEXT                 -- JSON array of ints
) STRICT;
CREATE INDEX IF NOT EXISTS ix_security_ts ON security_snapshots(ts);

-- ---------------------------------------------------------------------------
-- Quality of Experience scores (derived, stored so we can trend cheaply).
-- Each score 0..100.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS qoe_scores (
    id          INTEGER PRIMARY KEY,
    ts          INTEGER NOT NULL,
    gaming      REAL,
    video_call  REAL,
    streaming   REAL,
    web         REAL,
    voip        REAL
) STRICT;
CREATE INDEX IF NOT EXISTS ix_qoe_ts ON qoe_scores(ts);

-- ---------------------------------------------------------------------------
-- Rollups -- long-term history for ANY numeric metric. Written by the
-- aggregation job; raw samples above are pruned once rolled up.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS metric_rollups (
    id        INTEGER PRIMARY KEY,
    metric    TEXT NOT NULL,            -- 'latency', 'loss', 'download_mbps', ...
    target_id INTEGER NOT NULL DEFAULT 0, -- 0 = global aggregate; else targets(id).
                                        -- NOT nullable on purpose: SQLite treats
                                        -- each NULL as distinct, which would break
                                        -- the one-row-per-bucket UNIQUE guarantee.
    bucket    TEXT NOT NULL,            -- 'hour' | 'day' | 'week' | 'month'
    bucket_ts INTEGER NOT NULL,         -- start of the bucket
    count     INTEGER NOT NULL,
    min       REAL,
    avg       REAL,
    max       REAL,
    p50       REAL,                     -- median; computed exactly at rollup time
    p95       REAL,                     -- headline "when it's bad, how bad" value
    sum       REAL,
    UNIQUE(metric, target_id, bucket, bucket_ts)
) STRICT;
CREATE INDEX IF NOT EXISTS ix_rollup_lookup ON metric_rollups(metric, target_id, bucket, bucket_ts);

-- ---------------------------------------------------------------------------
-- AI insights -- locally-generated findings & recommendations.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS ai_insights (
    id         INTEGER PRIMARY KEY,
    ts         INTEGER NOT NULL,
    kind       TEXT NOT NULL,           -- 'isp_congestion' | 'wifi_congestion' |
                                        -- 'dns_issue' | 'bufferbloat' |
                                        -- 'device_saturation' | 'root_cause' |
                                        -- 'trend' | 'recommendation'
    severity   TEXT NOT NULL DEFAULT 'info',
    title      TEXT NOT NULL,
    body       TEXT NOT NULL,
    evidence   TEXT,                    -- JSON: refs to events/samples/rollups
    dismissed  INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE INDEX IF NOT EXISTS ix_insights_ts ON ai_insights(ts);

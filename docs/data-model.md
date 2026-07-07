# NetPulse — Data Model

This document explains the storage design. The authoritative schema is
[`db/schema.sql`](../db/schema.sql); this file explains *why* it looks the way it does.

## Goals

The README promises a monitor that "idles at near-zero CPU," "remembers everything,"
and keeps "all data on your machine." Those three promises drive every decision here:

1. **Local & embedded** — no server, no daemon to install, no cloud.
2. **Small on disk / cheap to write** — a monitor runs 24/7; naive per-ping rows
   would grow to gigabytes and thrash the disk.
3. **Fast to query** — the UI trends metrics over hours→months without scanning
   millions of raw rows.

## Engine: SQLite

- Embedded, single file (`netpulse.db`), zero configuration, ships inside the app.
- **WAL mode** — the background sampler holds one *writer* connection while the UI
  reads concurrently without blocking. This is the key to a responsive tray window
  during active probing.
- **STRICT tables** (SQLite ≥ 3.37) — real type enforcement on a greenfield schema.
- `json1` extension for flexible, kind-specific payloads (`events.payload`,
  `settings.value`, `security_snapshots.open_ports`, `ai_insights.evidence`).

Recommended PRAGMAs at connection setup (not in schema.sql):

```
PRAGMA journal_mode = WAL;
PRAGMA synchronous  = NORMAL;   -- safe with WAL, far fewer fsyncs
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
```

## Conventions

| Thing | Rule |
|---|---|
| Timestamps | unix epoch **milliseconds, UTC**, `INTEGER`. Never store local time. |
| Booleans | `INTEGER` 0/1 (STRICT has no BOOL type). |
| Flexible data | JSON in a `TEXT` column, queried with `json_extract`. |
| Rates | bits/sec (`*_bps`); byte counters store **deltas** since the previous sample. |
| Money-shot derived values | stored, not recomputed (e.g. `loss_pct`, QoE scores). |

## The two-tier strategy (the important part)

Metrics fall into three access patterns, stored differently:

### 1. Raw samples — short retention
`connectivity_samples`, `wifi_samples`, `dns_samples`, `bandwidth_samples`,
`interface_samples`, `device_bandwidth_samples`, `app_bandwidth_samples`.

- One row per **probe cycle**, never per individual ping. A cycle fires N pings and
  stores the aggregate (`sent/received/loss_pct/rtt_min/avg/max/jitter`). This alone
  cuts connectivity row volume by ~Nx.
- Kept at full resolution for a **short window** (default target: 7 days; tunable via
  `settings`), then pruned once rolled up.

### 2. Rollups — long retention
`metric_rollups` is a single generic table: `(metric, bucket, bucket_ts, count, min,
avg, max, p50, p95, sum)`. An aggregation job buckets raw samples into
hour/day/week/month rows. Any numeric metric trends over months by scanning a few
hundred rollup rows instead of millions of samples — and **new metrics need no new
schema**.

### 3. Discrete events & rich records — kept long, low volume
`events` (generic, typed by `kind` + JSON `payload`), `outages`, `speedtests`
(+ bufferbloat folded in), `traceroutes`/`traceroute_hops`, `security_snapshots`,
`ai_insights`. These are naturally sparse, so they're retained as-is and power the
Event Timeline / Incident Log directly.

### Lifecycle

```
probe cycle ──▶ raw *_samples ──(hourly job)──▶ metric_rollups ──▶ prune old raw rows
detector    ──▶ events / outages / ai_insights (retained, drive the timeline)
```

## Table map (README feature → tables)

| README section | Tables |
|---|---|
| Internet Connectivity | `connectivity_samples`, `outages`, `events` |
| Speed Tests + Bufferbloat | `speedtests` |
| Wi-Fi Metrics | `wifi_samples`, `events` (roaming) |
| Local Network | `local_devices`, `device_bandwidth_samples`, `interface_samples` |
| DNS | `dns_samples` |
| Routing | `traceroutes`, `traceroute_hops`, `events` (route change) |
| Bandwidth Usage | `bandwidth_samples`, `app_bandwidth_samples`, `device_bandwidth_samples` |
| QoE | `qoe_scores` |
| Security | `security_snapshots` |
| Historical Analytics | `metric_rollups`, `events`, `outages` |
| AI Insights | `ai_insights` (reads everything above) |
| Config | `settings`, `targets`, `meta` |

## Rust integration

- **`sqlx`** with the SQLite driver: compile-time-checked queries + built-in
  migrations. Migrations live in `db/migrations/` (derived from `schema.sql`);
  `0001_init.sql` = the current schema.
- **Connection model:** one dedicated **writer** connection owned by the sampler
  task, plus a small **read pool** for Tauri command handlers. WAL makes this safe.
- Schema version tracked in `meta` (`key='schema_version'`) *and* by sqlx's
  migration table — sqlx is authoritative; `meta` is for display/debugging.

## Deliberately deferred (documented, not built yet)

- **Per-application bandwidth** (`app_bandwidth_samples`) needs privileged packet
  capture and differs sharply between Windows and Linux — table exists, population
  is a later phase.
- **Router CPU/memory** — depends on the router exposing SNMP/an API; will attach to
  `events`/a small `router_samples` table when we tackle it.
- **Retention/rollup parameters** live in `settings` so they're tunable without a
  migration.

## Settings (defaults, all user-editable)

Stored in the `settings` table (JSON values), seeded on first run:

| Key | Default | Meaning |
|---|---|---|
| `retention.raw_days` | `7` | Days of full-resolution raw samples to keep before pruning to rollups. Lower it for a smaller footprint. |
| `rollups.global_enabled` | `true` | Compute the global (`target_id = 0`) series per metric. |
| `rollups.per_target_enabled` | `false` | Also compute one series per target/resolver. Opt-in — more rows, more detail. |

A manual **"Clear history"** action (a command, not schema) lets the user wipe
stored samples/rollups for a fresh scan — useful after moving house, changing
ISP, or debugging.

## Resolved decisions

1. **Raw-sample retention** — default **7 days**, editable via `retention.raw_days`,
   plus a manual clear-history action.
2. **Rollup scope** — support **both** global and per-target series in one table via
   `metric_rollups.target_id` (`0` = global), gated by the two `rollups.*` flags.
   Global on by default; per-target opt-in.
3. Percentiles — keep `p50` and `p95`,
   computed **exactly** at rollup time. Buckets are small (~720 samples/hour) and
   raw samples are still present when the hourly job runs, so we sort and pick the
   exact percentile — no streaming estimator (t-digest etc.) needed. `p95` is the
   headline "when it's bad, how bad" number for latency/jitter.

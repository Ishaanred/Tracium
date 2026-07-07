# NetPulse — Development

## Stack

| Layer | Tech |
|---|---|
| Desktop shell | [Tauri 2](https://tauri.app) (Rust) |
| Frontend | React 19 + Vite 6 + TypeScript |
| Storage | SQLite via `sqlx` (WAL), in the `netpulse-store` crate |

## Repository layout

```
Netpulse/
├─ Cargo.toml            # Rust workspace root
├─ crates/store/         # netpulse-store: schema, migrations, typed DB access
│  ├─ migrations/        # sqlx migrations (0001_init.sql = current schema)
│  └─ src/lib.rs
├─ src-tauri/            # Tauri app crate (window, tray, commands)
│  ├─ src/{main,lib}.rs
│  ├─ capabilities/      # Tauri v2 permissions
│  ├─ icons/             # generated app/tray icons
│  └─ tauri.conf.json
├─ src/                  # React frontend
├─ db/schema.sql         # authoritative schema (source of the migration)
└─ package.json          # frontend + `pnpm tauri` scripts
```

The storage layer is a **standalone crate with no Tauri dependency**, so it
compiles and tests without any GUI toolchain:

```bash
cargo test -p netpulse-store
```

## Prerequisites

- **Rust** (stable) and **Node 18+** with **pnpm**.
- **Linux only** — the WebKit/GTK system libraries Tauri renders with. On
  Debian/Ubuntu:

  ```bash
  sudo apt update && sudo apt install -y \
    libwebkit2gtk-4.1-dev \
    libgtk-3-dev \
    libjavascriptcoregtk-4.1-dev \
    libsoup-3.0-dev \
    librsvg2-dev \
    libayatana-appindicator3-dev \
    build-essential curl wget file
  ```

  Without these, `cargo build`/`pnpm tauri dev` fail at the `webkit2gtk` build
  step. The `netpulse-store` tests above do **not** need them.

## Common commands

```bash
pnpm install            # install frontend deps
pnpm build              # typecheck + build the frontend (no webkit needed)
pnpm tauri dev          # run the full desktop app (needs webkit libs)
pnpm tauri build        # produce a release bundle
cargo test -p netpulse-store   # storage layer tests
```

## How connectivity probing works

NetPulse measures reachability with **unprivileged TCP-connect timing**, not
ICMP echo. ICMP needs raw sockets (root / `CAP_NET_RAW`) on Linux, which breaks
the zero-setup promise. Timing the TCP handshake to a known-open port (443 on
1.1.1.1 / 8.8.8.8) needs no privileges, is identical on Windows and Linux, and
is a stable signal for latency/jitter/loss.

- `netpulse-probe` — measurement, no DB/GUI deps. Modules:
  - connectivity (TCP-connect RTT/jitter/loss),
  - `dns` (time a lookup against a specific resolver, `hickory-resolver`),
  - `netinfo` (public IP via HTTP, `ureq`),
  - `bandwidth` (interface byte-rate + per-iface deltas, `sysinfo`).
  Unit-tested with local sockets; real-internet checks behind `--ignored`.
- `netpulse-monitor` — samples every enabled internet target on an interval,
  writes `connectivity_samples`, and opens/closes `outages` (internet is "down"
  only when *every* target fails a cycle). Emits a `status` event to the UI.
  Also runs **maintenance** hourly: rolls up latency/loss/jitter into
  `metric_rollups` (exact p50/p95) and prunes raw samples past
  `retention.raw_days`. Computes **QoE scores** (`qoe.rs`) each cycle from
  latency/jitter/loss. On their own cadences it samples **DNS** resolvers
  (60s), the **public IP** (10min), and **bandwidth** (every cycle).

## Database

- Created on first launch at the platform app-data dir
  (`~/.local/share/com.netpulse.app/netpulse.db` on Linux), migrated
  automatically on open.
- To regenerate the migration after editing `db/schema.sql`:

  ```bash
  grep -v '^PRAGMA foreign_keys = ON;' db/schema.sql \
    > crates/store/migrations/0001_init.sql
  ```

  (The `foreign_keys` PRAGMA is applied per-connection in `store`, not in the
  migration, because it is a no-op inside SQLite's transaction.) See
  [`data-model.md`](./data-model.md).

## Regenerating icons

```bash
pnpm tauri icon assets/icon.png
```

# Expanded `traciumd watch` — design

## Why

`traciumd watch` (`crates/cli/src/main.rs`) is a read-only, 2-second-refresh
terminal dashboard. Today it only shows what `status`, `reliability_since`,
and `qoe_average_since` already return: per-target reachability, gateway
RTT/loss, 1h/24h reliability, and QoE scores. Every other subcommand —
`wifi`, `security`, `devices`, `route`, `dns`, `bandwidth`, `events` — reads
data that already exists in the store (written by the monitor on its own
cadences) but is invisible unless you run that subcommand separately while
`watch` is running. The live view is meant to be the one place you glance at
during a session; right now it's a small slice of what's already being
collected for free.

The goal is to fold those existing signals into `watch` itself, plus two new
purely-visual additions (rolling sparklines, a startup banner), while
keeping the loop's cost profile the same: a handful of cheap indexed SELECTs
against SQLite per tick, no new background polling, no unbounded in-memory
state.

## Scope

1. `watch` gains new sections: `wifi`, `bandwidth` (live number), `security`,
   `devices`, `route`, `dns`, `events` — each backed by the same `Store`
   method its existing subcommand already calls.
2. Two new 60-point in-memory sparklines (latency, bandwidth rx) rendered as
   unicode block graphs, scoped to the lifetime of one `watch` invocation.
3. `--hide <comma,list>` and `--only <comma,list>` flags to control which
   sections render, mutually exclusive via a clap `ArgGroup`.
4. A one-time ASCII startup banner printed before the refresh loop begins.
5. Route-change detection: a "route changed" indicator on the `route`
   section, derived by hashing the hop list tick-to-tick (in-memory only).

Explicitly out of scope: any change to what the monitor collects or how
often (cadences are unchanged); a full device list or full hop-by-hop route
table in `watch` (those stay in their dedicated subcommands — `watch` shows
compact summaries); persisting sparkline history across `watch` restarts;
changing the non-`watch` subcommands.

## Sections

All sections are on by default (dense-by-default, per user preference).
Render order top to bottom:

| key | content | store call | notes |
|---|---|---|---|
| `status` | targets + gateway | `latest_per_target`, `latest_gateway` | existing, unchanged |
| `reliability` | 1h/24h uptime/lat/loss | `reliability_since` | existing, unchanged |
| `qoe` | gaming/voip/video/streaming/web | `qoe_average_since` | existing, unchanged |
| `sparkline` | latency + bandwidth mini-graphs | in-memory ring buffers | new, see below |
| `wifi` | SSID, signal, link speed | `latest_wifi` | new; existing fallback text if `None` |
| `bandwidth` | live ↓/↑ Mbps | `latest_bandwidth` | new; existing fallback text if `None` |
| `security` | VPN / firewall / DoH-DoT one-liner | `latest_security` | new; existing fallback text if `None` |
| `devices` | count online + up to 8 most-recent, `+N more` | `list_devices` | new; capped list, not the full table |
| `route` | hop count, last-hop RTT, "route changed" flag | `latest_traceroute` | new; summary only, no per-hop dump |
| `dns` | one line per resolver: avg ms, lookups, failures | `dns_comparison` | new; same compact format as `dns` subcommand |
| `events` | last 3 events | `recent_events` | new; ticker, not the full `events` list |

A section whose store call returns `None`/empty still renders its slot with
the same "no data yet" message the corresponding subcommand already prints
(e.g. `wifi`'s "not connected to Wi-Fi") — sections don't disappear based on
data availability, only based on `--hide`/`--only`. This keeps the screen
layout stable tick to tick instead of jumping around as data comes and goes.

## Flags

```
traciumd watch --hide wifi,devices
traciumd watch --only status,reliability,sparkline
```

- Both flags accept a comma-separated list of section keys (the `key` column
  above).
- `--hide` and `--only` are mutually exclusive; passing both is a clap usage
  error at parse time.
- An unknown key in either list prints a warning to stderr (`unknown watch
  section 'foo', ignoring`) and is otherwise ignored — a typo shouldn't kill
  a live dashboard.
- Internally this resolves to one `HashSet<&str>` of enabled section keys,
  computed once before the loop starts; each tick does a cheap `contains`
  check per section.

## Sparklines

Two fixed-capacity ring buffers owned by the `watch()` function itself (not
the store, not the monitor):

- `VecDeque<Option<f64>>`, capacity 60, for the tick's average target
  latency (`rtt_avg`, mirroring what `status`'s existing line already
  computes).
- `VecDeque<Option<f64>>`, capacity 60, for `rx_bps` from `latest_bandwidth`.

Each tick pushes the current value and pops from the front once length
exceeds 60 (~2 minutes of history at the default 2s interval). Total memory
is a small, fixed number of `Option<f64>` — on the order of 1 KB — and is
dropped when the process exits; nothing is written to disk.

Rendering: an 8-level unicode block ramp (`▁▂▃▄▅▆▇█`), min-max normalized
over whatever's currently in the buffer (so the graph fills in progressively
from an empty screen rather than waiting for 60 ticks to populate). `None`
entries (a tick where the metric was unavailable) render as a blank/dim
character rather than being skipped, so the graph's horizontal axis stays
aligned with elapsed ticks.

This is a pure, independently testable function:
`fn sparkline(values: &VecDeque<Option<f64>>) -> String`.

## Route-change detection

`watch` keeps a local `Option<u64>` (hash of the previous tick's hop IP
list) alongside the ring buffers. Each tick, if `route` is enabled and a
traceroute is available, it hashes the current hop list and compares to the
stored hash: a mismatch (and a prior hash existing, so the very first tick
doesn't fire) renders a "route changed" indicator on that line for that
tick. This is purely a `watch`-session-local observation — it does not
write to the `events` table or duplicate the monitor's own route-change
event detection (which already exists in `crates/monitor`).

## Startup banner

Printed once, before the refresh loop starts: ASCII wordmark ("TRACIUM"),
`CARGO_PKG_VERSION`, the resolved db path, the refresh interval, and the
active section list (post `--hide`/`--only` resolution). A short fixed pause
(~700ms) follows so it's readable, then the screen clears and the existing
refresh loop takes over exactly as it does today. This is a one-time,
non-repeating print — it has no per-tick cost.

## Error handling

- Store query failures inside the loop currently propagate via `?` and end
  `watch` entirely (existing behavior, unchanged) — a dashboard that
  silently stops updating on a DB error would be worse than a clean exit
  with a message.
- `--hide`/`--only` parsing errors (mutual exclusion) are a clap-level exit
  before the loop starts.
- Unknown section names degrade to a warning, not a failure (see Flags).

## Testing

- Unit test the section-name resolver: `--hide` list → enabled set,
  `--only` list → enabled set, unknown name → warning + ignored, both flags
  given → error. Pure function, no DB.
- Unit test `sparkline()`: empty buffer, single value, all-`None` buffer,
  min == max (flat line), a normal mixed range.
- Unit test the route-hash comparison helper: same hops → no change, empty
  → non-empty → no false "changed" on first tick, changed IP → flagged.
- No new integration/DB tests: every data path already has coverage via its
  existing subcommand's tests (or lack thereof, matching current parity).

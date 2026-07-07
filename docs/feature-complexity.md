# NetPulse — Feature Complexity Split

A build-planning view of every metric/feature in the [README](../README.md),
split by implementation complexity. Use this to sequence work: knock out the
**one-shot** features fast (they populate the dashboard and feed the derived
scores), and schedule the **complex** ones into dedicated phases.

> "Targets" here = the feature/metric roadmap, not the `targets` DB table.

## Classification rule

**One-shot (Simple)** — buildable with the Rust standard networking stack, our
own math, and the SQLite store we already have. No OS-specific native APIs, no
elevated privileges, no external engine/dataset, no traffic-load generation.
These are "measure a simple thing" or "derive from data we already store."

**Complex** — needs at least one of:
- **Platform-native APIs** that differ between Windows and Linux (Wi-Fi radio
  info, firewall state, interface tables).
- **Elevated privileges / packet capture** (per-app or per-device traffic).
- **An external engine, dataset, or service** to integrate (speed-test servers,
  IP→ASN data, STUN) — this is where the README's "integrate, don't reinvent"
  principle applies.
- **Sustained load generation** (bufferbloat / under-load measurements).
- **Network scanning / discovery** (device enumeration, port scans).

---

## Internet Connectivity

| Metric | Tier | Notes / approach |
|---|---|---|
| Latency (min/avg/max) | **One-shot** | ICMP/echo ping cycle → `connectivity_samples`. |
| Jitter | **One-shot** | Mean deviation of RTT within a cycle. |
| Packet Loss | **One-shot** | sent vs received per cycle. |
| Internet Uptime | **One-shot** | Derived from `up` flag over time. |
| Disconnect Count | **One-shot** | Count outage rows. |
| Outage Duration | **One-shot** | `outages.duration_ms`. |
| Time to Reconnect | **One-shot** | `outages.reconnect_ms`. |
| IPv4 & IPv6 Connectivity | **One-shot** | Two probe stacks; IPv6 simply may be unavailable. |

## Speed Tests

| Metric | Tier | Notes / approach |
|---|---|---|
| Download & Upload Speed | **Complex** | Integrate an OSS engine (LibreSpeed / Ookla CLI); needs remote servers + heavy transfer. |
| Ping & Jitter During Test | **Complex** | Captured by the speed-test engine under load. |
| Packet Loss Under Load | **Complex** | Requires the load run above. |
| Speed vs ISP Plan | **One-shot** | Compare stored result to a `settings` plan value. |
| Peak Hour vs Off-Peak | **One-shot** | Group speed-test history by hour (rollups). |
| Full Speed Test History | **One-shot** | Storage + list UI (`speedtests`). |

## Wi-Fi Metrics

*Whole domain is platform-native (Linux `nl80211`/`iw`, Windows WLAN API), so
all land in Complex. The basic read subset (RSSI, quality, band, channel, link
speed) is the **easiest** of the complex tier — do it first within this domain.*

| Metric | Tier | Notes / approach |
|---|---|---|
| Signal Strength (RSSI) | **Complex** | Native radio query per OS. |
| Signal Quality | **Complex** | Same source as RSSI. |
| Link Speed & PHY Rate | **Complex** | Native. |
| Frequency Band | **Complex** | Native. |
| Channel & Channel Width | **Complex** | Native. |
| Noise Level | **Complex** | Often not exposed by driver/OS (esp. Windows). |
| Retransmission Rate | **Complex** | Low-level, driver-dependent stats. |
| Roaming Events | **Complex** | Detect BSSID change across samples → `events`. |

## Local Network

| Metric | Tier | Notes / approach |
|---|---|---|
| Gateway Latency | **One-shot** | Ping the default gateway. |
| LAN Packet Loss | **One-shot** | Loss to the gateway. |
| Interface Errors & Drops | **Complex** | Platform-native counters (`/sys`+`/proc` vs `GetIfTable`). |
| Connected Device Count | **Complex** | ARP/mDNS discovery; scanning + privileges. |
| Per-Device Bandwidth | **Complex** | Needs router SNMP/API or capture — one host can't see others' traffic. |
| Top Bandwidth Consumers | **Complex** | Ranking on top of per-device data. |
| Router CPU & Memory | **Complex** | SNMP / router API; router-dependent, optional. |

## DNS

| Metric | Tier | Notes / approach |
|---|---|---|
| Lookup Time | **One-shot** | Time a resolve → `dns_samples`. |
| Failure Count | **One-shot** | Count failed resolves. |
| Server Used | **One-shot** | Read system resolver config. |
| Server Comparison | **One-shot** | Query several resolvers, compare timings. |
| Cache Performance | **Complex** | Hit rates generally not exposed; hard to measure reliably. |

## Routing

| Metric | Tier | Notes / approach |
|---|---|---|
| Traceroute (visual) | **Complex** | Raw sockets/privileges or wrap system `traceroute`/`tracert`. |
| Hop Count | **Complex** | Derived from the traceroute run. |
| Per-Hop Latency | **Complex** | From the traceroute run. |
| Route Changes | **Complex** | Compare `route_hash` over time (cheap *once* traceroute exists). |
| Packet Loss by Hop | **Complex** | mtr-style sustained per-hop probing. |
| AS/ISP Path | **Complex** | Needs an IP→ASN dataset/API. |

## Bufferbloat

| Metric | Tier | Notes / approach |
|---|---|---|
| Idle Latency | **One-shot** | Baseline ping (we already have it). |
| Download Latency Under Load | **Complex** | Requires load generation (tied to speed test). |
| Upload Latency Under Load | **Complex** | Same, upstream. |
| Bufferbloat Grade (A–F) | **One-shot** | Pure formula once idle + under-load numbers exist. |

## Reliability

| Metric | Tier | Notes / approach |
|---|---|---|
| Daily/Weekly/Monthly Uptime | **One-shot** | Derived from rollups. |
| Disconnect Frequency | **One-shot** | Count over windows. |
| Average / Longest Outage | **One-shot** | Aggregate `outages`. |

## Bandwidth Usage

| Metric | Tier | Notes / approach |
|---|---|---|
| Current Download & Upload Rate | **One-shot** | Delta of local interface byte counters. |
| Daily/Weekly/Monthly Totals | **One-shot** | Accumulate the deltas. |
| Per-Application Breakdown | **Complex — DEFERRED** | Per-process accounting needs elevation: eBPF/nethogs/bandwhich (Linux, privileged), ETW (Windows). Breaks the unprivileged design; parked by decision — see `oss-integration.md` step 7. |
| Per-Device Breakdown | **Complex — DEFERRED** | Can't see other devices' traffic on a switched LAN without the router (SNMP per-client, rare) or promiscuous capture. Parked; see `oss-integration.md` step 7. |

## Quality of Experience (QoE)

*All one-shot: pure scoring formulas over metrics we already collect. Build after
their inputs land.*

| Metric | Tier |
|---|---|
| Gaming Score | **One-shot** |
| Video Call Score | **One-shot** |
| Streaming Score | **One-shot** |
| Web Browsing Score | **One-shot** |
| VoIP Score | **One-shot** |

## Security

| Metric | Tier | Notes / approach |
|---|---|---|
| Public IP | **One-shot** | Query an external "what's my IP" endpoint. |
| Open Ports | **Complex** | Requires an external reflector / port-scan of the public IP. |
| NAT Type | **Complex** | STUN-based detection. |
| UPnP Status | **Complex** | SSDP discovery / gateway query. |
| Firewall Status | **Complex** | Platform-native firewall query. |
| VPN Detection | **Complex** | Inspect interfaces/routes heuristically. |
| DNS-over-HTTPS/TLS | **Complex** | Probe resolver capability. |

## Historical Analytics

| Metric | Tier | Notes / approach |
|---|---|---|
| Hourly/Daily/Weekly/Monthly Trends | **One-shot** | Read `metric_rollups` + chart. |
| Event Timeline | **One-shot** | Read `events`. |
| Incident Log | **One-shot** | Read `outages`/`events`. |
| CSV Export | **One-shot** | Serialize rows. |
| PDF Export | **Complex** | Needs a PDF/layout engine. |

## AI Insights

*Its own later layer — every item is a detection heuristic / analysis over
accumulated data. All Complex, and dependent on the metrics above existing first.*

| Feature | Tier |
|---|---|
| ISP Congestion Detection | **Complex** |
| Wi-Fi Congestion Detection | **Complex** |
| DNS Issue Detection | **Complex** |
| Bufferbloat Detection | **Complex** |
| Device Saturation Detection | **Complex** |
| Automatic Root Cause Analysis | **Complex** |
| Performance Trend Analysis | **Complex** |
| Personalized Recommendations | **Complex** |

---

## Summary

| Bucket | Count |
|---|---|
| **One-shot (Simple)** | 34 |
| **Complex** | 42 |
| **Total** | 76 |

That's **~45% one-shot / 55% complex** overall. The complex side is inflated by
the 8 AI Insights features, which are really a separate final layer. **Excluding
AI Insights, it's an even 34 / 34 — exactly the ~50/50 split expected.**

### What makes the complex ones complex (grouped)

1. **Platform-native APIs** — all Wi-Fi metrics, interface errors, firewall status.
2. **External engine/data/service** — speed tests, AS/ISP path, NAT type, open ports, public-IP-adjacent security probes.
3. **Privileges / packet capture** — per-app & per-device bandwidth.
4. **Load generation** — under-load latency/loss, bufferbloat under load.
5. **Scanning / discovery** — connected devices, open ports.
6. **Analysis layer** — all AI Insights.

### Recommended build order

1. **Connectivity core** (all one-shot) — ping engine feeding `connectivity_samples`; unlocks uptime, outages, jitter, loss.
2. **Derived one-shots** — reliability, bufferbloat *grade*, QoE scores, historical trends/timeline, CSV export. Cheap wins that fill the UI once #1 exists.
3. **DNS one-shots + local one-shots** (gateway latency/loss, bandwidth rate/totals, public IP).
4. **Complex, self-contained** — speed test + under-load + bufferbloat (one engine integration covers several metrics), traceroute + route changes.
5. **Complex, platform-native** — Wi-Fi (basic subset first), interface errors, firewall/VPN.
6. **Complex, privileged/scanning** — device discovery, per-app/per-device bandwidth, open ports, NAT type.
7. **AI Insights** — last, once enough data and metrics exist to reason over.

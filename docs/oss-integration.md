# NetPulse — OSS Integration Plan (Complex Tier)

Per the README's "integrate, don't reinvent" principle, the complex features
lean on existing open-source projects. This maps each to candidate OSS.

**Licensing rule of thumb:** prefer **MIT / Apache-2.0** Rust library crates (we
can depend on them directly). **GPL** tools are acceptable **only when invoked as
a separate process** (a bundled/optional CLI), never linked/vendored into our
binary. Anything we must ship should keep NetPulse permissively licensable.

Status legend: **✓ verified** (checked on crates.io/GitHub, Jul 2026) ·
**⚠ unverified** (from prior knowledge — confirm before adding).

---

## Security probes — ✓ verified

The schema already has `security_snapshots`. All picks below are permissive Rust
crates except port-scanning (we roll our own).

| Probe | Pick | License | Notes |
|---|---|---|---|
| **NAT type / external IP** | `stunclient` (external IP) + `stun-rs` w/ RFC5780 feature (NAT classification) | MIT/Apache-2.0 | Full NAT-type needs a STUN server with two IPs (RFC5780). Google STUN gives external IP only. |
| **UPnP / IGD status** | `igd-next` (0.17.1, active) | MIT | "No gateway found" is a valid result (UPnP often disabled). Gives port mappings + external IP. |
| **Open ports** | **DIY** async `tokio` connect-scan | MIT (our code) | Unprivileged on both OSes. Avoids GPL. Optional RustScan/nmap only as user-invoked external process. |
| **VPN / interface detection** | `netdev` (0.45, active) — or lighter `if-addrs` | MIT | VPN detection is heuristic: match `wg*/tun*/tap*/ppp*` names or default route via virtual adapter. `netdev` gives interface type + gateway. |
| **DoH / DoT capability** | `hickory-resolver` w/ `https-ring` + `tls-ring` features | MIT/Apache-2.0 | **Already a dependency** (we use it for DNS). One crate proves both DoH and DoT reachability. |
| **Firewall status** | Shell out to native CLIs | n/a | No good cross-platform crate. `netsh advfirewall show allprofiles` (Win); `ufw status` / `systemctl is-active nftables\|firewalld` (Linux). Degrade to "requires admin" when reads need elevation. |

**Why this is a strong next step:** mostly permissive Rust crates, `hickory` is
already in the tree, `security_snapshots` already exists, and open-ports needs
no new dependency (reuses our TCP-connect approach). Low friction, high coverage.

---

## Speed test + bufferbloat — ⚠ unverified

- **librespeed-cli** (Go binary, LGPL-3.0) — bundle & invoke; download/upload/ping/jitter, self-hostable servers. LGPL is fine as a separate binary.
- **Cloudflare speed endpoints** (`speed.cloudflare.com`) — no dependency; download/upload chunks over HTTP with our existing client, measure latency-under-load ourselves for the bufferbloat grade. Most "integrate-friendly" but more of our own code.
- **`crusader`** (Rust) — network tester incl. bufferbloat; verify license + whether a library crate.
- **IETF `goresponsiveness`** (Go, Apache-2.0) — the standardized "responsiveness under load" metric.

**Tentative pick:** Cloudflare endpoints for a no-dep v1 (throughput + under-load
latency → bufferbloat grade), keep librespeed-cli as an optional engine. *Confirm
crusader's license/crate status before choosing it.*

## Traceroute / routing — ⚠ unverified

- **`trippy` / `trippy-core`** (Rust, believed dual MIT/Apache) — traceroute + mtr-style per-hop latency/loss **and** ASN lookup (Team Cymru). If `trippy-core` is a usable library crate, this single dependency covers hop count, per-hop latency, packet-loss-by-hop, route changes, and AS/ISP path.
- **Team Cymru IP-to-ASN** (DNS-based, free) — for AS/ISP path if not using trippy's built-in.
- Fallback: wrap system `traceroute`/`tracert`/`mtr`.

**Tentative pick:** `trippy-core` if it's a consumable library crate — biggest
single win in the complex tier. *Verify crate availability + license first.*

## Wi-Fi metrics — ⚠ unverified

- **`wifiscanner`** (MIT) — basic cross-platform (parses `iw`/`netsh`/`airport`): SSID, RSSI, channel. Limited fields.
- **Full Linux:** `nl80211` via `neli`/`wl-nl80211` (RSSI, PHY rate, band, width, noise, retransmits).
- **Full Windows:** WLAN API via the `windows` crate.

**Tentative pick:** `wifiscanner` for the basic subset first; platform-native
(`nl80211` + `windows` crate) for the full metric set later.

## Per-application bandwidth — ⚠ unverified

- **`bandwhich`** (Rust, MIT) — per-process bandwidth via packet capture. Needs privileges (pcap/eBPF on Linux, npcap on Windows). Verify whether a library crate or binary-only.
- **nethogs** (GPL binary) — Linux only; separate-process invocation only.

**Tentative pick:** `bandwhich` approach; gate behind an explicit
"enable per-app monitoring (needs elevation)" toggle.

## Per-device bandwidth / device count / router stats — ⚠ unverified

- **`mdns-sd`** (Rust) — LAN device discovery.
- **ARP-scan** crates — connected device enumeration (needs privileges).
- **SNMP** (`csnmp` / `snmp2`) — router CPU/memory + per-device stats, router-dependent.

**Tentative pick:** `mdns-sd` for device count first; SNMP for router stats
where the router supports it. Per-device bandwidth realistically needs router
SNMP or capture — hardest item; schedule last.

---

## Recommended complex-tier build order

1. **Security probes** — verified, mostly permissive crates, schema ready, low friction. *(Build next.)*
2. **Speed test + bufferbloat** — one engine/approach unlocks several metrics.
3. **Traceroute / routing** — `trippy-core` likely covers the whole domain.
4. **Wi-Fi** — basic subset, then platform-native full set.
5. **Per-app bandwidth** — privileged, opt-in.
6. **Per-device / router** — SNMP/discovery, hardest; last.
7. **AI Insights** — our own analysis layer over all of the above.

> The ⚠ items still need a quick license/crate-availability check (blocked now
> by the research session limit) before we add the dependency.

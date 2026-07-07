# Tracium — OSS Integration Plan (Complex Tier)

Per the README's "integrate, don't reinvent" principle, the complex features
lean on existing open-source projects. This maps each to candidate OSS.

**Licensing rule of thumb:** prefer **MIT / Apache-2.0** Rust library crates (we
can depend on them directly). **GPL** tools are acceptable **only when invoked as
a separate process** (a bundled/optional CLI), never linked/vendored into our
binary. Anything we must ship should keep Tracium permissively licensable.

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

## Traceroute / routing — ✓ verified (cleanest complex win)

- **`trippy-core`** (Rust library crate, **Apache-2.0**, active) — ICMP/UDP/TCP
  traceroute, RTT, **per-hop packet loss**, reverse DNS, MPLS, NAT detection, and
  **ASN lookup built in** (default Team Cymru DNS; optional MaxMind/IPinfo).
  Supports an **unprivileged mode** (some probe types may still need admin/root).
- One dependency covers hop count, per-hop latency, packet-loss-by-hop, route
  changes (hash the hop list), and AS/ISP path.

**Pick:** **`trippy-core`** + Team Cymru DNS for ASN. Single permissive library
crate for the entire routing domain — do this first among the unverified areas.

## Wi-Fi metrics — ✓ verified

- **`wifiscanner`** (MIT, limited maintenance) — only SSID/BSSID/RSSI/channel;
  **missing noise, PHY rate, retries, MCS**. OK for a quick first pass only.
- **Full Linux:** `neli` + direct **nl80211** — RSSI, frequency, channel, band,
  PHY/TX-RX bitrate, MCS, noise, retries (driver-dependent).
- **Full Windows:** the **`windows` crate** + native WLAN API — RSSI, channel,
  PHY type, link quality, TX/RX rate, SSID, BSSID.
- No better-maintained high-level cross-platform crate exists.

**Pick:** platform-native (`neli`/nl80211 on Linux, `windows` crate on Windows)
for the real metric set; `wifiscanner` only if we want a trivial SSID/RSSI stub
first. This is two native implementations behind one Tracium trait.

## Speed test + bufferbloat — ✓ verified (needs a backend decision)

**Every good option requires a server** — this is the catch:
- **Crusader** (MIT/Apache) — measures download/upload/latency/**latency-under-
  load (bufferbloat)**/loss, but is **binary-first (no stable library API)** and
  **requires a Crusader server**. Not embeddable as a crate.
- **librespeed-cli** (Go, **LGPL-3.0**) — ping/jitter/download/upload; fine to
  **bundle as an external binary**, but needs a **LibreSpeed-compatible server**.
- **Cloudflare `speed.cloudflare.com`** — technically possible but **not
  recommended for commercial embedding** (no public API/license for it).
- **No mature Rust crate** does full speed test — projects roll their own or
  shell out to librespeed-cli.

**Decision required before building:** either (a) **host/point at a LibreSpeed
backend** and bundle `librespeed-cli`, or (b) **write our own client** against
public LibreSpeed community servers (Crusader-inspired for the bufferbloat part).
Because it needs infra, this is *not* a quick add — schedule accordingly.

## Step 7 — Per-app / per-device bandwidth — DEFERRED BY DECISION

> **Status:** intentionally postponed (decision, 2026-07-07). It is the one
> feature that cannot be done within Tracium's unprivileged design. Everything
> else we shipped is privilege-free (TCP-connect not ICMP, ARP *cache* not
> *scan*, wrapping the OS traceroute). Step 7 breaks that, so it waits until
> there's a clear need and a decision to accept elevation.

**Per-application bandwidth — needs privileges/capture.**
- The OS exposes **no per-process byte counters** without elevation (no `/proc`
  file for it).
- Real options all require elevation: **eBPF** or **libpcap** on Linux
  (root / `CAP_NET_ADMIN` / `CAP_BPF`), **Npcap** + admin on Windows, or
  shelling out to **`bandwhich`** / **`nethogs`** (both **GPL-3.0** → external
  process only, never linked) with `sudo`/`setcap`.
- **No mature cross-platform Rust crate** — realistically native per-OS collectors.

**Per-device bandwidth — mostly infeasible on a normal LAN.**
- One host **cannot see another device's traffic** on a switched network.
  It requires either the **router** to report per-client stats (SNMP/vendor
  API — rare on consumer gear) or **promiscuous packet capture** (privileged,
  and still limited on a switch).
- Device *discovery* (count/list) is already done in step 3 via the ARP cache;
  only the per-device *byte* half is blocked.

**When revisited — the plan:**
1. **Per-app (Linux first):** an explicit opt-in — *"Enable per-app monitoring
   (requires elevated permissions)"* — that shells out to `bandwhich`/`nethogs`
   with `setcap`/`sudo` and parses their output (same wrap-the-tool pattern as
   traceroute/librespeed). Windows later via ETW.
2. **Per-device:** only where the **router** exposes per-client counters
   (extends the step-5 `csnmp` work); otherwise surfaced as
   "requires router support," not faked.

Keep this panel clearly labelled as needing elevation so the default install
stays zero-privilege.

## Device discovery / router stats — ✓ verified

- **`mdns-sd`** (Rust, **MIT/Apache**, maintained, Win+Linux) — LAN device
  discovery (printers, TVs, Chromecast, NAS, IoT). Good for device **count/list**.
- **`csnmp`** (Rust, **MIT/Apache**) — router CPU/memory/interface counters/
  bandwidth/uptime via SNMP. Requires **SNMP enabled on the router** + community
  string / SNMPv3 creds.
- **ARP discovery** — only low-level crates; reading the ARP **cache** needs no
  admin, but **sending ARP probes/raw frames usually needs root/admin**.

**Pick:** `mdns-sd` for device discovery first (unprivileged, permissive);
`csnmp` for router stats where supported. Per-device *bandwidth* still needs
router SNMP or capture — hardest, schedule last.

---

## Recommended complex-tier build order

1. **Security probes** — verified, permissive crates, schema ready, low friction. *(Build next.)*
2. **Traceroute / routing** — `trippy-core`, one permissive library crate for the whole domain.
3. **Device discovery** — `mdns-sd`, unprivileged, permissive; easy device count/list.
4. **Wi-Fi** — `wifiscanner` stub, then platform-native (`neli`/nl80211 + `windows` crate) for the full set.
5. **Router stats** — `csnmp`, where the router supports SNMP.
6. **Speed test + bufferbloat** — needs a LibreSpeed backend decision (host vs bundle vs own client); more infra than code.
7. **Per-app / per-device bandwidth** — **DEFERRED BY DECISION** (privileged/
   capture; breaks the unprivileged design — see the dedicated section above).
8. **AI Insights** — our own analysis layer over all of the above (future).

## Verified stack summary

| Feature | Pick | License | Form |
|---|---|---|---|
| Traceroute + ASN | `trippy-core` + Team Cymru DNS | Apache-2.0 | Rust lib |
| Device discovery | `mdns-sd` | MIT/Apache | Rust lib |
| Router stats | `csnmp` | MIT/Apache | Rust lib |
| NAT / external IP | `stunclient` (+ `stun-rs` RFC5780) | MIT/Apache | Rust lib |
| UPnP/IGD | `igd-next` | MIT | Rust lib |
| VPN/interfaces | `netdev` (or `if-addrs`) | MIT | Rust lib |
| DoH/DoT | `hickory-resolver` (already in tree) | MIT/Apache | Rust lib |
| Open ports | DIY `tokio` connect-scan | MIT (ours) | our code |
| Firewall | native CLIs (`netsh`/`ufw`/`systemctl`) | n/a | shell-out |
| Wi-Fi (Linux) | `neli` + nl80211 | — | Rust lib |
| Wi-Fi (Windows) | `windows` crate WLAN API | — | Rust lib |
| Speed/bufferbloat | own client OR bundled `librespeed-cli` + backend | LGPL (binary) | binary/own |
| Per-app bandwidth | native collectors; optional `bandwhich` | GPL (external) | native/binary |

<p align="center">
  <img src="assets/icon.svg" alt="Tracium" width="96" height="96" />
</p>

<h1 align="center">Tracium</h1>

<p align="center">
  <strong>Know your network. Inside and out.</strong>
</p>

<p align="center">
  A seamless, lightweight desktop monitor that watches your connection in real time<br />
  and remembers everything — so you don't have to wonder what happened while you were away.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/platform-Windows%20%7C%20Linux-0891b2?style=flat-square" alt="Platforms" />
  <img src="https://img.shields.io/badge/status-private-inactive?style=flat-square" alt="Status" />
  <img src="https://img.shields.io/badge/license-MIT-0891b2?style=flat-square" alt="License: MIT" />
</p>

---

## Why Tracium?

Most network tools feel like they were built for sysadmins in 2003. Tracium doesn't.

It watches your connection like a fitness tracker watches your heart — quietly, continuously, and with enough intelligence to tell you when something's off *before* your Zoom call drops. No command-line flags to memorize. No dashboards that look like a spreadsheet had a bad day. Just a clean window that shows you exactly what's happening, right now, and what happened while you were sleeping.

Built for enthusiasts who care about their network, not enterprises with a budget line for "observability."

---

## What It Watches

### Internet Connectivity

The fundamentals. Is the internet actually working?

- **Latency** — Min, average, max. Not just a number — the full picture.
- **Jitter** — Because consistency matters more than a single good ping.
- **Packet Loss** — As a percentage. Zero is the only acceptable answer.
- **Internet Uptime** — Real uptime, measured from your end, not your ISP's claims.
- **Disconnect Count** — How many times it dropped, and when.
- **Outage Duration** — Total time you were in the dark.
- **Time to Reconnect** — How long it actually takes to come back.
- **IPv4 & IPv6 Connectivity** — Both stacks, independently monitored.

### Speed Tests

Not just "how fast am I right now" — how fast are you *supposed* to be?

- **Download & Upload Speed** — Measured on your terms, on your schedule.
- **Ping & Jitter During Test** — What happens to latency under load.
- **Packet Loss Under Load** — The real stress test.
- **Speed vs. ISP Plan** — Are you getting what you pay for?
- **Peak Hour vs. Off-Peak** — See when your neighborhood saturates the node.
- **Full Speed Test History** — Every result, forever, searchable.

### Wi-Fi Metrics

Your wireless connection, dissected.

- **Signal Strength (RSSI)** — In dBm, because bars are meaningless.
- **Signal Quality** — As a clean percentage.
- **Link Speed & PHY Rate** — What your adapter negotiated vs. what it's actually getting.
- **Frequency Band** — 2.4, 5, or 6 GHz. Know which one you're on.
- **Channel & Channel Width** — And whether your neighbor's router is stomping on yours.
- **Noise Level** — What's competing with your signal.
- **Retransmission Rate** — Packets that had to be sent twice. Should be near zero.
- **Roaming Events** — Every time your device jumped between APs, logged.

### Local Network

Your LAN isn't a black box.

- **Gateway Latency** — How fast your router responds.
- **LAN Packet Loss** — Internal drops. Shouldn't happen.
- **Interface Errors & Drops** — Low-level NIC stats that tell the real story.
- **Connected Device Count** — Who's on your network right now.
- **Per-Device Bandwidth** — Which device is hogging the pipe.
- **Top Bandwidth Consumers** — Ranked. Ruthlessly.
- **Router CPU & Memory** — If your router supports it, Tracium reads it.

### DNS

Your internet doesn't start until DNS resolves. Watch it.

- **Lookup Time** — Per query, averaged, trended.
- **Failure Count** — Every DNS failure, logged.
- **Server Used** — Which resolver answered.
- **Server Comparison** — Side-by-side speed comparison across resolvers.
- **Cache Performance** — Hit rates, when available.

### Routing

See the path your packets actually take.

- **Traceroute** — Visual, not a wall of text.
- **Hop Count** — More hops = more places for things to go wrong.
- **Per-Hop Latency** — Which hop is the bottleneck.
- **Route Changes** — Did your path suddenly shift? Tracium notices.
- **Packet Loss by Hop** — Pinpoint where the drops happen.
- **AS/ISP Path** — Who's carrying your packets, hop by hop.

### Bufferbloat

The silent killer of real-time performance.

- **Idle Latency** — Your baseline.
- **Download Latency Under Load** — How much bufferbloat costs you.
- **Upload Latency Under Load** — Same, upstream.
- **Bufferbloat Grade** — A through F. Simple, brutal, honest.

### Reliability

The long view.

- **Daily, Weekly, Monthly Uptime** — Real numbers, not ISP marketing.
- **Disconnect Frequency** — Patterns emerge over weeks.
- **Average Outage Length** — And your longest outage. Ever.

### Bandwidth Usage

What's actually moving through your connection.

- **Current Download & Upload Rate** — Live, right now.
- **Daily, Weekly, Monthly Totals** — Data caps hate this.
- **Per-Application Breakdown** — Which app is doing what.
- **Per-Device Breakdown** — Which device is streaming 4K without asking.

### Quality of Experience (QoE)

Numbers are numbers. Scores tell you how it *feels*.

- **Gaming Score** — Latency, jitter, packet loss → one number.
- **Video Call Score** — Will your next meeting be a disaster?
- **Streaming Score** — Buffering risk, right now.
- **Web Browsing Score** — DNS + latency + throughput.
- **VoIP Score** — Because dropped words ruin conversations.

### Security

Know what's exposed.

- **Open Ports** — What the outside world can see.
- **Public IP** — Your face to the internet.
- **NAT Type** — Strict, moderate, or open.
- **UPnP Status** — On? Off? Should it be?
- **Firewall Status** — Active and healthy?
- **VPN Detection** — Are you actually protected right now?
- **DNS-over-HTTPS/TLS** — Encrypted DNS status check.

### Historical Analytics

Everything above, tracked over time. Not just snapshots — stories.

- **Hourly, Daily, Weekly, Monthly Trends** — Zoom out to see patterns.
- **Event Timeline** — Every significant event, chronologically.
- **Incident Log** — What happened, when, for how long.
- **Exportable Reports** — PDF and CSV. Share with your ISP, your IT person, or your own records.

---

## Bring your own AI

Tracium deliberately doesn't bundle an AI model or opinionated "insights" engine.
Instead it makes your data easy to hand to whatever assistant you already use:

- **Export** a summary as **PDF** (`traciumd report --pdf`) or the raw history as
  **CSV** (`traciumd export`), then paste it into ChatGPT/Claude/etc. and ask
  "what's wrong with my connection and how do I fix it?"
- Your data stays on your machine until *you* choose to share it — no telemetry,
  no cloud, no bundled model deciding what matters.

This keeps Tracium lean and private, and lets the analysis improve as the AI you
prefer improves — without us shipping (and maintaining) a model.

---

## Design Philosophy

Tracium takes cues from tools that feel invisible until you need them — clean, fast, and respectful of your screen real estate.

- **Seamless** — Lives in your system tray. One click to expand.
- **Lightweight** — Idles at near-zero CPU. Does its work, gets out of the way.
- **Honest** — Raw numbers when you want them. Simple scores when you don't.
- **Cross-Platform** — Windows and Linux. Same experience. Same codebase.

### Footprint (measured)

Not just a claim — measured with [`scripts/bench.sh`](scripts/bench.sh) on a
**release build**, idle, with the window open (RAM shown as PSS, which accounts
for shared libraries):

| Platform | CPU (idle) | RAM | Notes |
|---|---|---|---|
| **Linux** (WebKitGTK) | **~0.15%** of one core | **~158 MB** | Ubuntu 26.04, release build |
| **Windows** (WebView2) | *TBD* | *TBD* | `scripts/bench.ps1`, pending Windows testing |

The **monitoring engine itself is negligible** — it wakes every 15 s to run a
handful of TCP probes and a SQLite write, then sleeps (hence ~0.15% CPU). Almost
all the RAM is the GUI's webview. A planned **headless CLI/daemon** (no webview)
will run in single-digit MB with effectively no measurable CPU — for truly
invisible background monitoring.

---

## How It's Built

We're not here to reinvent networking. If there's a great open-source tool that already does something well — a speed test engine, a traceroute visualizer, a packet analyzer — we'll integrate it, credit it, and build on top of it. Tracium is about the *experience*: stitching the right pieces together into something that feels like one seamless tool, not a loose collection of utilities.

Every dependency will be transparently credited. Good open source deserves recognition.

---

## Install & Run

> Prebuilt downloads (`.deb` / `.AppImage` / `.msi` + a `traciumd` binary) are
> planned via GitHub Releases. For now, build from source.

**Prerequisites:** Rust (stable), Node 18+ with pnpm. On Linux the desktop app
also needs the WebKitGTK libraries — see [`docs/development.md`](docs/development.md).

**Desktop app** (tray GUI):

```bash
pnpm install
pnpm tauri dev          # run from source
pnpm tauri build        # produce an installable bundle
```

**Headless daemon** (`traciumd`) — background monitoring with no GUI, a few MB
of RAM, effectively no idle CPU. Ideal for servers or always-on collection:

```bash
cargo build --release -p tracium-cli
./target/release/traciumd run        # collect continuously
./target/release/traciumd status     # current reachability + gateway
./target/release/traciumd report     # 24h reliability + QoE
./target/release/traciumd export connectivity > data.csv
```

Run it on boot as an unprivileged **systemd user service**:

```bash
install -Dm755 target/release/traciumd ~/.local/bin/traciumd
install -Dm644 packaging/systemd/tracium.service ~/.config/systemd/user/tracium.service
systemctl --user daemon-reload
systemctl --user enable --now tracium.service
loginctl enable-linger "$USER"       # keep running across reboots/logout
```

The daemon and the desktop app share one local database
(`com.tracium.app/tracium.db`). Run **one writer at a time** (daemon *or* GUI).

---

## Coming Soon

Tracium is currently in active development. Some advanced features may be offered under a sustainable licensing model, but the core monitoring experience will always be free and open source.

---

<p align="center">
  <sub>Built for people who want to understand their network, not just complain about it.</sub>
</p>

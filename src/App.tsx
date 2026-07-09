import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface Qoe {
  gaming: number;
  video_call: number;
  streaming: number;
  web: number;
  voip: number;
}

interface QoeAverage {
  samples: number;
  gaming: number | null;
  video_call: number | null;
  streaming: number | null;
  web: number | null;
  voip: number | null;
}

interface StatusUpdate {
  ts: number;
  online: boolean;
  targets_up: number;
  targets_total: number;
  best_latency_ms: number | null;
  avg_loss_pct: number | null;
  avg_jitter_ms: number | null;
  outage_ongoing: boolean;
  qoe: Qoe | null;
}

interface Rollup {
  bucket_ts: number;
  min: number | null;
  avg: number | null;
  max: number | null;
  p95: number | null;
}

interface NetEvent {
  id: number;
  ts: number;
  kind: string;
  severity: string;
  duration_ms: number | null;
}

interface Outage {
  id: number;
  ts_start: number;
  ts_end: number | null;
  duration_ms: number | null;
  reconnect_ms: number | null;
  cause: string | null;
}

interface DnsStat {
  resolver: string;
  avg_ms: number | null;
  count: number;
  failures: number;
}

interface BandwidthNow {
  rx_bps: number;
  tx_bps: number;
}
interface BandwidthTotals {
  rx_bytes: number;
  tx_bytes: number;
}

interface Security {
  firewall_active: boolean | null;
  vpn_detected: boolean | null;
  doh_active: boolean | null;
  dot_active: boolean | null;
  open_ports: string | null;
}

interface Speedtest {
  ts: number;
  server: string | null;
  download_mbps: number | null;
  upload_mbps: number | null;
  ping_ms: number | null;
  jitter_ms: number | null;
  idle_latency_ms: number | null;
  loaded_latency_ms: number | null;
  bufferbloat_grade: string | null;
}

interface IspPlan {
  down_mbps: number;
  up_mbps: number;
}

interface RouterInfo {
  descr: string | null;
  name: string | null;
  uptime_secs: number | null;
}

interface Wifi {
  ssid: string | null;
  bssid: string | null;
  rssi_dbm: number | null;
  link_speed_mbps: number | null;
  band: string | null;
  channel: number | null;
}

interface Gateway {
  ts: number;
  gateway_rtt_ms: number | null;
  lan_loss_pct: number | null;
}

interface LanDevice {
  id: number;
  mac: string | null;
  ip: string | null;
  hostname: string | null;
}

interface TraceHop {
  hop_no: number;
  ip: string | null;
  rtt_ms: number | null;
}
interface Traceroute {
  target: string;
  hop_count: number;
  hops: TraceHop[];
}

interface Reliability {
  samples: number;
  up_samples: number;
  uptime_pct: number;
  avg_latency_ms: number | null;
  avg_loss_pct: number | null;
  avg_jitter_ms: number | null;
  disconnects: number;
}

interface TargetStatus {
  id: number;
  label: string;
  host: string;
  ip_version: number | null;
  rtt_avg: number | null;
  loss_pct: number | null;
  up: boolean | null;
}

interface Target {
  id: number;
  label: string;
  host: string;
  kind: string;
  ip_version: number | null;
  enabled: boolean;
}

const DAY_SECS = 24 * 60 * 60;
const QOE_WINDOW_SECS = 30 * 60; // smooth QoE over the last 30 minutes

function fmtMs(v: number | null | undefined): string {
  return v == null ? "—" : `${v.toFixed(0)} ms`;
}
function fmtPct(v: number | null | undefined): string {
  return v == null ? "—" : `${v.toFixed(1)}%`;
}
function fmtRate(bps: number | null | undefined): string {
  if (bps == null) return "—";
  const mbps = bps / 1e6;
  return mbps >= 1 ? `${mbps.toFixed(1)} Mbps` : `${(bps / 1e3).toFixed(0)} kbps`;
}
function fmtBytes(b: number | null | undefined): string {
  if (b == null) return "—";
  const gb = b / 1e9;
  if (gb >= 1) return `${gb.toFixed(2)} GB`;
  return `${(b / 1e6).toFixed(1)} MB`;
}
function fmtDur(ms: number | null | undefined): string {
  if (ms == null) return "ongoing";
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(0)}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${Math.round(s % 60)}s`;
  return `${(s / 3600).toFixed(1)}h`;
}

export default function App() {
  const [status, setStatus] = useState<StatusUpdate | null>(null);
  const [qoe, setQoe] = useState<QoeAverage | null>(null);
  const [rel, setRel] = useState<Reliability | null>(null);
  const [history, setHistory] = useState<Rollup[]>([]);
  const [targetStatus, setTargetStatus] = useState<TargetStatus[]>([]);
  const [events, setEvents] = useState<NetEvent[]>([]);
  const [outages, setOutages] = useState<Outage[]>([]);
  const [speedHistory, setSpeedHistory] = useState<Speedtest[]>([]);
  const [dns, setDns] = useState<DnsStat[]>([]);
  const [publicIp, setPublicIp] = useState<string | null>(null);
  const [bw, setBw] = useState<BandwidthNow | null>(null);
  const [bwTotal, setBwTotal] = useState<BandwidthTotals | null>(null);
  const [security, setSecurity] = useState<Security | null>(null);
  const [trace, setTrace] = useState<Traceroute | null>(null);
  const [devices, setDevices] = useState<LanDevice[]>([]);
  const [gateway, setGateway] = useState<Gateway | null>(null);
  const [wifi, setWifi] = useState<Wifi | null>(null);
  const [targets, setTargets] = useState<Target[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [exportMsg, setExportMsg] = useState<string | null>(null);
  const [routerAddr, setRouterAddr] = useState("");
  const [routerCommunity, setRouterCommunity] = useState("public");
  const [router, setRouter] = useState<RouterInfo | null>(null);
  const [routerMsg, setRouterMsg] = useState<string | null>(null);
  const [speed, setSpeed] = useState<Speedtest | null>(null);
  const [speedRunning, setSpeedRunning] = useState(false);
  const [speedMsg, setSpeedMsg] = useState<string | null>(null);
  const [ispPlan, setIspPlan] = useState<IspPlan | null>(null);
  const [planDown, setPlanDown] = useState("");
  const [planUp, setPlanUp] = useState("");

  const saveIspPlan = () => {
    const d = parseFloat(planDown);
    const u = parseFloat(planUp);
    if (!(d > 0) || !(u > 0)) return;
    invoke("set_isp_plan", { downMbps: d, upMbps: u })
      .then(() => setIspPlan({ down_mbps: d, up_mbps: u }))
      .catch(() => {});
  };

  const runSpeedtest = () => {
    setSpeedRunning(true);
    setSpeedMsg("Running… this uses data and takes ~30s.");
    invoke<Speedtest | null>("run_speedtest")
      .then((r) => {
        if (r) {
          setSpeed(r);
          setSpeedMsg(null);
        } else {
          setSpeedMsg("Speed test unavailable — install librespeed-cli.");
        }
      })
      .catch((e) => setSpeedMsg(String(e)))
      .finally(() => setSpeedRunning(false));
  };

  const queryRouter = () => {
    setRouterMsg("Querying…");
    invoke<RouterInfo | null>("router_status", { addr: routerAddr, community: routerCommunity })
      .then((r) => {
        setRouter(r);
        setRouterMsg(r ? null : "No response (SNMP disabled or wrong community?)");
      })
      .catch((e) => setRouterMsg(String(e)));
  };

  // Refresh derived summaries (called on mount + after each status tick).
  const refreshDerived = () => {
    invoke<Reliability>("reliability", { windowSecs: DAY_SECS }).then(setRel).catch(() => {});
    invoke<QoeAverage | null>("qoe_average", { windowSecs: QOE_WINDOW_SECS })
      .then(setQoe)
      .catch(() => {});
    invoke<Rollup[]>("metric_history", {
      metric: "latency",
      bucket: "hour",
      windowSecs: DAY_SECS,
    })
      .then(setHistory)
      .catch(() => {});
    invoke<TargetStatus[]>("target_status").then(setTargetStatus).catch(() => {});
    invoke<NetEvent[]>("recent_events", { limit: 20 }).then(setEvents).catch(() => {});
    invoke<DnsStat[]>("dns_comparison", { windowSecs: DAY_SECS }).then(setDns).catch(() => {});
    invoke<string | null>("public_ip").then(setPublicIp).catch(() => {});
    invoke<BandwidthNow | null>("bandwidth_now").then(setBw).catch(() => {});
    invoke<BandwidthTotals>("bandwidth_totals", { windowSecs: DAY_SECS })
      .then(setBwTotal)
      .catch(() => {});
    invoke<Security | null>("security_status").then(setSecurity).catch(() => {});
    invoke<Traceroute | null>("latest_traceroute").then(setTrace).catch(() => {});
    invoke<LanDevice[]>("devices").then(setDevices).catch(() => {});
    invoke<Gateway | null>("gateway_status").then(setGateway).catch(() => {});
    invoke<Wifi | null>("wifi").then(setWifi).catch(() => {});
    invoke<Speedtest[]>("speedtest_history", { limit: 10 })
      .then((h) => {
        setSpeedHistory(h);
        if (h[0]) setSpeed(h[0]);
      })
      .catch(() => {});
    invoke<Outage[]>("recent_outages", { limit: 20 }).then(setOutages).catch(() => {});
    invoke<IspPlan | null>("get_isp_plan")
      .then((p) => {
        setIspPlan(p);
        if (p) {
          setPlanDown(String(p.down_mbps));
          setPlanUp(String(p.up_mbps));
        }
      })
      .catch(() => {});
  };

  const doExport = (kind: "connectivity" | "events") => {
    invoke<string>("export_csv", { kind, windowSecs: 7 * DAY_SECS })
      .then((path) => setExportMsg(`Saved ${kind} CSV → ${path}`))
      .catch((e) => setExportMsg(String(e)));
  };

  useEffect(() => {
    (async () => {
      try {
        setTargets(await invoke<Target[]>("list_targets"));
        setStatus(await invoke<StatusUpdate | null>("current_status"));
        refreshDerived();
      } catch (e) {
        setError(String(e));
      }
    })();

    const unlisten = listen<StatusUpdate>("status", (e) => {
      setStatus(e.payload);
      refreshDerived();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const online = status?.online ?? null;

  return (
    <main className="app">
      <header className="app__header">
        <span
          className={`app__pulse ${online === false ? "app__pulse--down" : ""}`}
          aria-hidden
        />
        <div>
          <h1>Tracium</h1>
          <p className="app__tagline">
            {publicIp ? `Public IP · ${publicIp}` : "Know your network. Inside and out."}
          </p>
        </div>
        <span className={`badge ${online ? "badge--ok" : online === false ? "badge--bad" : ""}`}>
          {online == null ? "Starting…" : online ? "Online" : "Offline"}
        </span>
      </header>

      {error && <p className="status status--bad">{error}</p>}

      <section className="grid">
        <Stat
          label="Latency"
          value={fmtMs(status?.best_latency_ms)}
          hint="best of targets"
          info="Round-trip time for a packet to reach a server and come back. Lower is snappier — under ~30 ms feels instant; over ~150 ms is noticeable in calls and games."
        />
        <Stat
          label="Jitter"
          value={fmtMs(status?.avg_jitter_ms)}
          hint="this cycle"
          info="How much latency varies between packets. Low jitter means a steady connection; high jitter causes choppy calls and rubber-banding in games, even when average latency looks fine."
        />
        <Stat
          label="Packet loss"
          value={fmtPct(status?.avg_loss_pct)}
          hint="this cycle"
          info="Share of probe packets that never came back. 0% is ideal; even 1–2% causes stutter in calls, games and video."
        />
        <Stat
          label="Targets up"
          value={status ? `${status.targets_up}/${status.targets_total}` : "—"}
          hint="reachable now"
          info="How many of the monitored servers responded this cycle. The internet is only marked offline when every target fails."
        />
      </section>

      {targetStatus.length > 0 && (
        <section className="card">
          <CardTitle
            title="Targets"
            info="Each server Tracium probes, with its own live latency and reachability. The Latency tile above shows the best of these. A permanently-down IPv6 target just means your network has no IPv6 — it isn't counted as loss."
          />
          <ul className="targets">
            {targetStatus.map((t) => (
              <li key={t.id}>
                <span className={`dot dot--${t.up ? "info" : t.up === false ? "critical" : ""}`} aria-hidden />
                <strong>{t.label}</strong>
                <span className="targets__host">{t.host}</span>
                <span className="targets__kind">
                  {t.up === false
                    ? "down"
                    : t.rtt_avg != null
                      ? `${t.rtt_avg.toFixed(1)} ms`
                      : "—"}
                  {" · IPv"}
                  {t.ip_version ?? "?"}
                </span>
              </li>
            ))}
          </ul>
        </section>
      )}

      <section className="card">
        <CardTitle
          title="Speed test"
          info="On-demand download/upload throughput + ping via librespeed-cli. Uses data and takes ~30s, so it isn't run automatically."
        />
        <div className="row">
          <button className="btn" onClick={runSpeedtest} disabled={speedRunning}>
            {speedRunning ? "Running…" : "Run speed test"}
          </button>
          {speed && (
            <span className="status" style={{ margin: 0 }}>
              {speed.server ?? ""}
            </span>
          )}
        </div>
        {speed && (
          <div className="grid" style={{ marginTop: 12 }}>
            <Stat
              label="Download"
              value={fmtRate((speed.download_mbps ?? 0) * 1e6)}
              hint={
                ispPlan && speed.download_mbps != null
                  ? `${((speed.download_mbps / ispPlan.down_mbps) * 100).toFixed(0)}% of ${ispPlan.down_mbps} plan`
                  : undefined
              }
            />
            <Stat
              label="Upload"
              value={fmtRate((speed.upload_mbps ?? 0) * 1e6)}
              hint={
                ispPlan && speed.upload_mbps != null
                  ? `${((speed.upload_mbps / ispPlan.up_mbps) * 100).toFixed(0)}% of ${ispPlan.up_mbps} plan`
                  : undefined
              }
            />
            <Stat label="Ping" value={fmtMs(speed.ping_ms)} />
            <Stat label="Jitter" value={fmtMs(speed.jitter_ms)} />
            {speed.bufferbloat_grade && (
              <div className="stat">
                <span className="stat__label">
                  Bufferbloat
                  <Info text="How much latency rises when the link is saturated (idle vs under-load). A is great; D/F means video calls and games suffer during uploads/downloads even on a 'fast' connection." />
                </span>
                <span
                  className="stat__value"
                  style={{ color: scoreColor(speed.bufferbloat_grade <= "B" ? 100 : speed.bufferbloat_grade === "C" ? 60 : 30) }}
                >
                  {speed.bufferbloat_grade}
                </span>
                <span className="stat__hint">
                  {fmtMs(speed.idle_latency_ms)} → {fmtMs(speed.loaded_latency_ms)}
                </span>
              </div>
            )}
          </div>
        )}
        {speedMsg && <p className="status">{speedMsg}</p>}
        <div className="row" style={{ marginTop: 12 }}>
          <span className="stat__label" style={{ alignSelf: "center" }}>
            ISP plan
            <Info text="Enter your subscribed plan speeds to see what % of them you're actually getting." />
          </span>
          <input
            className="input input--sm"
            type="number"
            placeholder="down Mbps"
            value={planDown}
            onChange={(e) => setPlanDown(e.target.value)}
          />
          <input
            className="input input--sm"
            type="number"
            placeholder="up Mbps"
            value={planUp}
            onChange={(e) => setPlanUp(e.target.value)}
          />
          <button className="btn" onClick={saveIspPlan} disabled={!planDown || !planUp}>
            Save
          </button>
        </div>
        {speedHistory.length > 1 && (
          <ul className="events" style={{ marginTop: 12 }}>
            {speedHistory.map((s) => (
              <li key={s.ts}>
                <span className="events__kind">
                  {(s.download_mbps ?? 0).toFixed(0)}↓ / {(s.upload_mbps ?? 0).toFixed(0)}↑ Mbps
                </span>
                <span className="events__dur">{fmtMs(s.ping_ms)}</span>
                <span className="events__time">{new Date(s.ts).toLocaleString()}</span>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="card">
        <CardTitle
          title="Quality of experience"
          info="0–100 scores estimating how good each activity feels, averaged over the last 30 minutes so they settle instead of jumping each cycle. Computed from latency, jitter and packet loss — 80+ is great, under 50 is rough."
        />
        {qoe && qoe.samples > 0 ? (
          <>
            <div className="grid">
              <Score label="Gaming" value={qoe.gaming ?? 0} />
              <Score label="Video call" value={qoe.video_call ?? 0} />
              <Score label="Streaming" value={qoe.streaming ?? 0} />
              <Score label="VoIP" value={qoe.voip ?? 0} />
              <Score label="Web" value={qoe.web ?? 0} />
            </div>
            <p className="status" style={{ marginTop: 8, fontSize: 12 }}>
              averaged over last 30 min · {qoe.samples} samples
            </p>
          </>
        ) : (
          <p className="status">{online === false ? "Offline." : "Scoring…"}</p>
        )}
      </section>

      <section className="card">
        <CardTitle
          title="Latency — last 24h (hourly avg)"
          info="Hourly average round-trip time. p95 = the worst-hour 95th percentile, i.e. how bad latency gets during the rough patches — a better 'is it laggy?' signal than the average."
        />
        {history.length > 1 ? (
          <>
            <Sparkline points={history.map((h) => h.avg ?? 0)} />
            <p className="status" style={{ marginTop: 8, fontSize: 12 }}>
              min {fmtMs(Math.min(...history.map((h) => h.min ?? Infinity)))} · p95 (worst hour){" "}
              {fmtMs(Math.max(...history.map((h) => h.p95 ?? 0)))} · max{" "}
              {fmtMs(Math.max(...history.map((h) => h.max ?? 0)))}
            </p>
          </>
        ) : (
          <p className="status">Not enough history yet — building hourly rollups.</p>
        )}
      </section>

      <section className="card">
        <h2>Last 24 hours</h2>
        {rel ? (
          <div className="grid">
            <Stat
              label="Uptime"
              value={fmtPct(rel.uptime_pct)}
              hint={`${rel.samples} samples`}
              info="Share of samples in the last 24h where the internet was reachable. Measured from your machine, not your ISP's claims."
            />
            <Stat label="Avg latency" value={fmtMs(rel.avg_latency_ms)} info="Average round-trip time across all samples in the window." />
            <Stat label="Avg jitter" value={fmtMs(rel.avg_jitter_ms)} info="Average latency variation across the window." />
            <Stat label="Avg loss" value={fmtPct(rel.avg_loss_pct)} info="Average packet loss across the window." />
            <Stat
              label="Disconnects"
              value={String(rel.disconnects)}
              info="Number of full outages (every target unreachable) that started in the window."
            />
          </div>
        ) : (
          <p className="status">Gathering data…</p>
        )}
      </section>

      <section className="card">
        <CardTitle
          title="Security"
          info="A snapshot of your network's security posture — firewall, encrypted DNS, VPN and locally-open ports."
        />
        {security ? (
          <div className="grid">
            <Flag
              label="Firewall"
              value={security.firewall_active}
              goodWhenTrue
              info="Whether the OS firewall is active (ufw/firewalld on Linux, Windows Firewall)."
            />
            <Flag
              label="DNS-over-HTTPS"
              value={security.doh_active}
              goodWhenTrue
              info="Encrypted DNS over HTTPS is reachable — your lookups can be hidden from the local network/ISP."
            />
            <Flag
              label="DNS-over-TLS"
              value={security.dot_active}
              goodWhenTrue
              info="The encrypted-DNS port (853) is reachable — another way to keep DNS private."
            />
            <Flag
              label="VPN"
              value={security.vpn_detected}
              goodWhenTrue={false}
              neutral
              info="Whether a VPN/tunnel interface (wg, tun, tap…) is currently active."
            />
            <Stat
              label="Open ports"
              value={
                security.open_ports
                  ? (JSON.parse(security.open_ports) as number[]).join(", ") || "none"
                  : "—"
              }
              hint="locally listening"
              info="Ports on THIS machine currently accepting connections (a local self-audit — not what the public internet can reach)."
            />
          </div>
        ) : (
          <p className="status">Scanning…</p>
        )}
      </section>

      <section className="card">
        <CardTitle
          title="Bandwidth"
          info="Live throughput through this machine's network interfaces, plus totals moved today. Not per-app or per-device."
        />
        <div className="grid">
          <Stat label="Download" value={fmtRate(bw?.rx_bps)} hint="live" />
          <Stat label="Upload" value={fmtRate(bw?.tx_bps)} hint="live" />
          <Stat label="Down today" value={fmtBytes(bwTotal?.rx_bytes)} />
          <Stat label="Up today" value={fmtBytes(bwTotal?.tx_bytes)} />
        </div>
      </section>

      <section className="card">
        <CardTitle
          title="DNS resolvers — last 24h"
          info="DNS turns names like example.com into IP addresses before anything loads. This compares how fast each resolver answers — lower is better."
        />
        {dns.length === 0 ? (
          <p className="status">No DNS samples yet.</p>
        ) : (
          <ul className="targets">
            {dns.map((d) => (
              <li key={d.resolver}>
                <strong>{d.resolver}</strong>
                <span className="targets__host">{fmtMs(d.avg_ms)} avg</span>
                <span className="targets__kind">
                  {d.failures > 0 ? `${d.failures} fail` : `${d.count} ok`}
                </span>
              </li>
            ))}
          </ul>
        )}
      </section>

      {wifi && (
        <section className="card">
          <h2>Wi-Fi{wifi.ssid ? ` · ${wifi.ssid}` : ""}</h2>
          <div className="grid">
            <Stat
              label="Signal"
              value={wifi.rssi_dbm != null ? `${wifi.rssi_dbm} dBm` : "—"}
              info="Wi-Fi signal strength in dBm (closer to 0 is stronger): −50 excellent, −67 good, below −70 weak."
            />
            <Stat
              label="Band"
              value={wifi.band ? `${wifi.band} GHz` : "—"}
              info="2.4 GHz reaches further but is slower/more congested; 5/6 GHz is faster over shorter range."
            />
            <Stat
              label="Channel"
              value={wifi.channel != null ? String(wifi.channel) : "—"}
              info="The radio channel in use. Neighbours on the same channel cause interference."
            />
            <Stat
              label="Link rate"
              value={wifi.link_speed_mbps != null ? `${wifi.link_speed_mbps} Mbps` : "—"}
              info="Negotiated link speed with the access point (a ceiling, not your internet speed)."
            />
          </div>
        </section>
      )}

      {gateway && (
        <section className="card">
          <CardTitle
            title="Local network"
            info="Latency and packet loss to your router (the gateway), measured with ICMP ping. High gateway latency or LAN loss points to a local problem (Wi-Fi, cabling, router) rather than your ISP."
          />
          <div className="grid">
            <Stat label="Gateway latency" value={fmtMs(gateway.gateway_rtt_ms)} />
            <Stat label="LAN packet loss" value={fmtPct(gateway.lan_loss_pct)} />
          </div>
        </section>
      )}

      <section className="card">
        <h2>
          Devices on network{devices.length ? ` · ${devices.length}` : ""}
          <Info text="Devices seen on your local network, from the ARP cache (neighbours this machine has recently talked to). Not a full active scan." />
        </h2>
        {devices.length === 0 ? (
          <p className="status">No devices discovered yet.</p>
        ) : (
          <ul className="targets">
            {devices.map((d) => (
              <li key={d.id}>
                <strong>{d.ip ?? "?"}</strong>
                <span className="targets__host">{d.mac ?? ""}</span>
                {d.hostname && <span className="targets__kind">{d.hostname}</span>}
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="card">
        <h2>
          Route{trace ? ` to ${trace.target} · ${trace.hop_count} hops` : ""}
          <Info text="The path your packets take to reach the target, hop by hop, with the round-trip time to each hop. More hops = more places for delays." />
        </h2>
        {trace && trace.hops.length > 0 ? (
          <ul className="hops">
            {trace.hops.map((h) => (
              <li key={h.hop_no}>
                <span className="hops__no">{h.hop_no}</span>
                <span className="hops__ip">{h.ip ?? "* (no reply)"}</span>
                <span className="hops__rtt">{h.rtt_ms != null ? `${h.rtt_ms.toFixed(1)} ms` : ""}</span>
              </li>
            ))}
          </ul>
        ) : (
          <p className="status">
            No traceroute yet — needs the <code>traceroute</code>/<code>tracert</code> tool installed.
          </p>
        )}
      </section>

      <section className="card">
        <h2>Event timeline</h2>
        {events.length === 0 ? (
          <p className="status">No events yet.</p>
        ) : (
          <ul className="events">
            {events.map((e) => (
              <li key={e.id}>
                <span className={`dot dot--${e.severity}`} aria-hidden />
                <span className="events__kind">{e.kind}</span>
                {e.duration_ms != null && (
                  <span className="events__dur">{(e.duration_ms / 1000).toFixed(0)}s</span>
                )}
                <span className="events__time">{new Date(e.ts).toLocaleString()}</span>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="card">
        <CardTitle
          title="Incident log"
          info="Every internet outage (all targets unreachable), with how long it lasted and how long it took to reconnect."
        />
        {outages.length === 0 ? (
          <p className="status status--ok">No outages recorded. 🎉</p>
        ) : (
          <>
            <p className="status" style={{ marginBottom: 10, fontSize: 12 }}>
              longest outage:{" "}
              {fmtDur(Math.max(...outages.map((o) => o.duration_ms ?? 0)))} · {outages.length} total
            </p>
            <ul className="events">
              {outages.map((o) => (
                <li key={o.id}>
                  <span className={`dot dot--${o.ts_end == null ? "critical" : "warn"}`} aria-hidden />
                  <span className="events__kind">{fmtDur(o.duration_ms)}</span>
                  {o.reconnect_ms != null && (
                    <span className="events__dur">recovered in {fmtDur(o.reconnect_ms)}</span>
                  )}
                  <span className="events__time">{new Date(o.ts_start).toLocaleString()}</span>
                </li>
              ))}
            </ul>
          </>
        )}
      </section>

      <section className="card">
        <h2>Router (SNMP)</h2>
        <div className="row">
          <input
            className="input"
            placeholder="Router IP (e.g. 192.168.1.1)"
            value={routerAddr}
            onChange={(e) => setRouterAddr(e.target.value)}
          />
          <input
            className="input input--sm"
            placeholder="community"
            value={routerCommunity}
            onChange={(e) => setRouterCommunity(e.target.value)}
          />
          <button className="btn" onClick={queryRouter} disabled={!routerAddr}>
            Query
          </button>
        </div>
        {router && (
          <ul className="targets" style={{ marginTop: 12 }}>
            {router.name && (
              <li>
                <strong>Name</strong>
                <span className="targets__host">{router.name}</span>
              </li>
            )}
            {router.descr && (
              <li>
                <strong>Description</strong>
                <span className="targets__host">{router.descr}</span>
              </li>
            )}
            {router.uptime_secs != null && (
              <li>
                <strong>Uptime</strong>
                <span className="targets__host">
                  {Math.floor(router.uptime_secs / 3600)} h
                </span>
              </li>
            )}
          </ul>
        )}
        {routerMsg && <p className="status">{routerMsg}</p>}
      </section>

      <section className="card">
        <h2>Export</h2>
        <div className="row">
          <button className="btn" onClick={() => doExport("connectivity")}>
            Connectivity CSV
          </button>
          <button className="btn" onClick={() => doExport("events")}>
            Events CSV
          </button>
        </div>
        {exportMsg && <p className="status status--ok">{exportMsg}</p>}
      </section>

      <section className="card">
        <h2>Probe targets</h2>
        {targets.length === 0 ? (
          <p className="status">No targets configured yet.</p>
        ) : (
          <ul className="targets">
            {targets.map((t) => (
              <li key={t.id}>
                <strong>{t.label}</strong>
                <span className="targets__host">{t.host}</span>
                <span className="targets__kind">IPv{t.ip_version ?? "?"}</span>
              </li>
            ))}
          </ul>
        )}
      </section>
    </main>
  );
}

function scoreColor(v: number): string {
  if (v >= 80) return "var(--ok)";
  if (v >= 50) return "#fbbf24";
  return "var(--bad)";
}

function Score({ label, value }: { label: string; value: number }) {
  return (
    <div className="stat">
      <span className="stat__label">{label}</span>
      <span className="stat__value" style={{ color: scoreColor(value) }}>
        {value.toFixed(0)}
      </span>
    </div>
  );
}

function Flag({
  label,
  value,
  goodWhenTrue,
  neutral,
  info,
}: {
  label: string;
  value: boolean | null;
  goodWhenTrue: boolean;
  neutral?: boolean;
  info?: string;
}) {
  let color = "var(--muted)";
  let text = "Unknown";
  if (value != null) {
    text = value ? "Yes" : "No";
    if (neutral) color = "var(--text)";
    else color = value === goodWhenTrue ? "var(--ok)" : "var(--bad)";
  }
  return (
    <div className="stat">
      <span className="stat__label">
        {label}
        {info && <Info text={info} />}
      </span>
      <span className="stat__value" style={{ color, fontSize: 20 }}>
        {text}
      </span>
    </div>
  );
}

function Sparkline({ points }: { points: number[] }) {
  const W = 600;
  const H = 60;
  const min = Math.min(...points);
  const max = Math.max(...points);
  const span = max - min || 1;
  const step = points.length > 1 ? W / (points.length - 1) : W;
  const path = points
    .map((v, i) => {
      const x = i * step;
      const y = H - ((v - min) / span) * (H - 8) - 4;
      return `${i === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
  return (
    <svg className="spark" viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" role="img">
      <path d={path} fill="none" stroke="var(--accent)" strokeWidth="2" vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

function Info({ text }: { text: string }) {
  return (
    <span className="tip" tabIndex={0}>
      <span className="tip__icon" aria-hidden>
        i
      </span>
      <span className="tip__bubble" role="tooltip">
        {text}
      </span>
    </span>
  );
}

function Stat({
  label,
  value,
  hint,
  info,
}: {
  label: string;
  value: string;
  hint?: string;
  info?: string;
}) {
  return (
    <div className="stat">
      <span className="stat__label">
        {label}
        {info && <Info text={info} />}
      </span>
      <span className="stat__value">{value}</span>
      {hint && <span className="stat__hint">{hint}</span>}
    </div>
  );
}

/** A section heading with an optional hover explanation. */
function CardTitle({ title, info }: { title: string; info?: string }) {
  return (
    <h2>
      {title}
      {info && <Info text={info} />}
    </h2>
  );
}

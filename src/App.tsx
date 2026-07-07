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

interface StatusUpdate {
  ts: number;
  online: boolean;
  targets_up: number;
  targets_total: number;
  best_latency_ms: number | null;
  avg_loss_pct: number | null;
  outage_ongoing: boolean;
  qoe: Qoe | null;
}

interface Rollup {
  bucket_ts: number;
  avg: number | null;
  p95: number | null;
}

interface NetEvent {
  id: number;
  ts: number;
  kind: string;
  severity: string;
  duration_ms: number | null;
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

interface Reliability {
  samples: number;
  up_samples: number;
  uptime_pct: number;
  avg_latency_ms: number | null;
  avg_loss_pct: number | null;
  disconnects: number;
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

export default function App() {
  const [status, setStatus] = useState<StatusUpdate | null>(null);
  const [rel, setRel] = useState<Reliability | null>(null);
  const [history, setHistory] = useState<Rollup[]>([]);
  const [events, setEvents] = useState<NetEvent[]>([]);
  const [dns, setDns] = useState<DnsStat[]>([]);
  const [publicIp, setPublicIp] = useState<string | null>(null);
  const [bw, setBw] = useState<BandwidthNow | null>(null);
  const [bwTotal, setBwTotal] = useState<BandwidthTotals | null>(null);
  const [targets, setTargets] = useState<Target[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [exportMsg, setExportMsg] = useState<string | null>(null);

  // Refresh derived summaries (called on mount + after each status tick).
  const refreshDerived = () => {
    invoke<Reliability>("reliability", { windowSecs: DAY_SECS }).then(setRel).catch(() => {});
    invoke<Rollup[]>("metric_history", {
      metric: "latency",
      bucket: "hour",
      windowSecs: DAY_SECS,
    })
      .then(setHistory)
      .catch(() => {});
    invoke<NetEvent[]>("recent_events", { limit: 20 }).then(setEvents).catch(() => {});
    invoke<DnsStat[]>("dns_comparison", { windowSecs: DAY_SECS }).then(setDns).catch(() => {});
    invoke<string | null>("public_ip").then(setPublicIp).catch(() => {});
    invoke<BandwidthNow | null>("bandwidth_now").then(setBw).catch(() => {});
    invoke<BandwidthTotals>("bandwidth_totals", { windowSecs: DAY_SECS })
      .then(setBwTotal)
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
          <h1>NetPulse</h1>
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
        <Stat label="Latency" value={fmtMs(status?.best_latency_ms)} hint="best of targets" />
        <Stat label="Packet loss" value={fmtPct(status?.avg_loss_pct)} hint="this cycle" />
        <Stat
          label="Targets up"
          value={status ? `${status.targets_up}/${status.targets_total}` : "—"}
          hint="reachable now"
        />
      </section>

      <section className="card">
        <h2>Quality of experience</h2>
        {status?.qoe ? (
          <div className="grid">
            <Score label="Gaming" value={status.qoe.gaming} />
            <Score label="Video call" value={status.qoe.video_call} />
            <Score label="Streaming" value={status.qoe.streaming} />
            <Score label="VoIP" value={status.qoe.voip} />
            <Score label="Web" value={status.qoe.web} />
          </div>
        ) : (
          <p className="status">{online === false ? "Offline." : "Scoring…"}</p>
        )}
      </section>

      <section className="card">
        <h2>Latency — last 24h (hourly avg)</h2>
        {history.length > 1 ? (
          <Sparkline points={history.map((h) => h.avg ?? 0)} />
        ) : (
          <p className="status">Not enough history yet — building hourly rollups.</p>
        )}
      </section>

      <section className="card">
        <h2>Last 24 hours</h2>
        {rel ? (
          <div className="grid">
            <Stat label="Uptime" value={fmtPct(rel.uptime_pct)} hint={`${rel.samples} samples`} />
            <Stat label="Avg latency" value={fmtMs(rel.avg_latency_ms)} />
            <Stat label="Avg loss" value={fmtPct(rel.avg_loss_pct)} />
            <Stat label="Disconnects" value={String(rel.disconnects)} />
          </div>
        ) : (
          <p className="status">Gathering data…</p>
        )}
      </section>

      <section className="card">
        <h2>Bandwidth</h2>
        <div className="grid">
          <Stat label="Download" value={fmtRate(bw?.rx_bps)} hint="live" />
          <Stat label="Upload" value={fmtRate(bw?.tx_bps)} hint="live" />
          <Stat label="Down today" value={fmtBytes(bwTotal?.rx_bytes)} />
          <Stat label="Up today" value={fmtBytes(bwTotal?.tx_bytes)} />
        </div>
      </section>

      <section className="card">
        <h2>DNS resolvers — last 24h</h2>
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

function Stat({ label, value, hint }: { label: string; value: string; hint?: string }) {
  return (
    <div className="stat">
      <span className="stat__label">{label}</span>
      <span className="stat__value">{value}</span>
      {hint && <span className="stat__hint">{hint}</span>}
    </div>
  );
}

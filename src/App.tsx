import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface StatusUpdate {
  ts: number;
  online: boolean;
  targets_up: number;
  targets_total: number;
  best_latency_ms: number | null;
  avg_loss_pct: number | null;
  outage_ongoing: boolean;
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

export default function App() {
  const [status, setStatus] = useState<StatusUpdate | null>(null);
  const [rel, setRel] = useState<Reliability | null>(null);
  const [targets, setTargets] = useState<Target[]>([]);
  const [error, setError] = useState<string | null>(null);

  // Refresh the reliability summary (called on mount + after each status tick).
  const refreshReliability = () =>
    invoke<Reliability>("reliability", { windowSecs: DAY_SECS }).then(setRel).catch(() => {});

  useEffect(() => {
    (async () => {
      try {
        setTargets(await invoke<Target[]>("list_targets"));
        setStatus(await invoke<StatusUpdate | null>("current_status"));
        await refreshReliability();
      } catch (e) {
        setError(String(e));
      }
    })();

    const unlisten = listen<StatusUpdate>("status", (e) => {
      setStatus(e.payload);
      refreshReliability();
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
          <p className="app__tagline">Know your network. Inside and out.</p>
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

function Stat({ label, value, hint }: { label: string; value: string; hint?: string }) {
  return (
    <div className="stat">
      <span className="stat__label">{label}</span>
      <span className="stat__value">{value}</span>
      {hint && <span className="stat__hint">{hint}</span>}
    </div>
  );
}

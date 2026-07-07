import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface DbHealth {
  ok: boolean;
  table_count: number;
}

interface Target {
  id: number;
  label: string;
  host: string;
  kind: string;
  ip_version: number | null;
  enabled: boolean;
  created_at: number;
}

export default function App() {
  const [health, setHealth] = useState<DbHealth | null>(null);
  const [targets, setTargets] = useState<Target[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    (async () => {
      try {
        setHealth(await invoke<DbHealth>("db_health"));
        setTargets(await invoke<Target[]>("list_targets"));
      } catch (e) {
        setError(String(e));
      }
    })();
  }, []);

  return (
    <main className="app">
      <header className="app__header">
        <span className="app__pulse" aria-hidden />
        <div>
          <h1>NetPulse</h1>
          <p className="app__tagline">Know your network. Inside and out.</p>
        </div>
      </header>

      <section className="card">
        <h2>Storage</h2>
        {error && <p className="status status--bad">{error}</p>}
        {!error && !health && <p className="status">Connecting to database…</p>}
        {health && (
          <p className="status status--ok">
            Database live · {health.table_count} tables migrated
          </p>
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
                <span className="targets__kind">{t.kind}</span>
              </li>
            ))}
          </ul>
        )}
      </section>
    </main>
  );
}

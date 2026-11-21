import { useEffect, useState } from "react";
import { type HealthPayload, fetchHealth } from "../../lib/api";

export function HealthCard() {
  const [health, setHealth] = useState<HealthPayload | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    fetchHealth()
      .then((data) => {
        if (active) {
          setHealth(data);
        }
      })
      .catch(() => {
        if (active) {
          setError("Unable to reach server health endpoint.");
        }
      });
    return () => {
      active = false;
    };
  }, []);

  return (
    <section className="w-full max-w-md rounded-xl border border-slate-800 bg-slate-900 p-6 shadow-lg">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold tracking-tight text-white">
          Maxi Loop
        </h1>
        <span className="rounded-full bg-slate-800 px-2.5 py-0.5 text-xs font-medium text-slate-300">
          {health?.version ?? "0.1.0"}
        </span>
      </div>
      <p className="mt-2 text-sm text-slate-400">
        Lightweight Sentry-compatible error tracking &amp; performance monitoring.
      </p>

      <div className="mt-6 rounded-lg border border-slate-800 bg-slate-950/60 p-4 text-sm">
        {error ? (
          <p className="text-rose-400">{error}</p>
        ) : health ? (
          <dl className="space-y-2">
            <div className="flex justify-between">
              <dt className="text-slate-400">Service</dt>
              <dd className="font-mono text-slate-100">{health.service}</dd>
            </div>
            <div className="flex justify-between">
              <dt className="text-slate-400">Status</dt>
              <dd
                className={
                  health.status === "ok"
                    ? "font-medium text-emerald-400"
                    : "font-medium text-amber-400"
                }
              >
                {health.status}
              </dd>
            </div>
            <div className="flex justify-between">
              <dt className="text-slate-400">PostgreSQL</dt>
              <dd
                className={
                  health.database === "ok"
                    ? "font-medium text-emerald-400"
                    : "font-medium text-rose-400"
                }
              >
                {health.database}
              </dd>
            </div>
          </dl>
        ) : (
          <p className="text-slate-400">Checking service health...</p>
        )}
      </div>
    </section>
  );
}

import { useEffect, useState } from "react";
import { CheckCircle2, Database, HardDrive, ShieldCheck } from "lucide-react";
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

  const isHealthy = health?.status === "ok" && health?.database === "ok";

  return (
    <div className="flex-1 min-w-0 bg-[var(--color-bg)] px-6 py-4 overflow-y-auto">
      <div className="text-xs text-[var(--color-text-dim)]">
        Default Workspace › System &amp; retention
      </div>
      <h1 className="mt-1.5 text-2xl font-semibold text-[var(--color-text)] tracking-tight">
        System &amp; Retention Health
      </h1>
      <p className="mt-1 text-xs text-[var(--color-text-muted)]">
        Single-binary Rust runtime, daily PostgreSQL 16 partitions, and baseline PII scrubber status.
      </p>

      <div className="mt-4 grid grid-cols-1 md:grid-cols-3 gap-4 max-w-4xl">
        <section className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-card)] p-4 shadow-[var(--pm-shadow-card)]">
          <div className="flex items-center justify-between">
            <span className="text-xs font-bold text-[var(--color-text-muted)]">
              Runtime &amp; Database
            </span>
            <Database size={15} className="text-[var(--color-primary)]" />
          </div>
          {error ? (
            <p className="mt-3 text-xs text-[var(--color-danger-text)]">{error}</p>
          ) : (
            <dl className="mt-3 space-y-2 text-xs">
              <div className="flex justify-between">
                <dt className="text-[var(--color-text-dim)]">Service</dt>
                <dd className="pm-mono font-semibold text-[var(--color-text)]">
                  {health?.service ?? "maxi-loop"}
                </dd>
              </div>
              <div className="flex justify-between">
                <dt className="text-[var(--color-text-dim)]">Version</dt>
                <dd className="pm-mono text-[var(--color-text)]">
                  v{health?.version ?? "0.1.0"}
                </dd>
              </div>
              <div className="flex justify-between">
                <dt className="text-[var(--color-text-dim)]">PostgreSQL 16</dt>
                <dd>
                  <span
                    className={`pm-lozenge ${
                      isHealthy ? "pm-lozenge-success" : "pm-lozenge-warning"
                    }`}
                  >
                    {health?.database ?? "checking"}
                  </span>
                </dd>
              </div>
            </dl>
          )}
        </section>

        <section className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-card)] p-4 shadow-[var(--pm-shadow-card)]">
          <div className="flex items-center justify-between">
            <span className="text-xs font-bold text-[var(--color-text-muted)]">
              Partition &amp; Retention
            </span>
            <HardDrive size={15} className="text-[var(--color-primary)]" />
          </div>
          <dl className="mt-3 space-y-2 text-xs">
            <div className="flex justify-between">
              <dt className="text-[var(--color-text-dim)]">Global retention</dt>
              <dd className="font-semibold text-[var(--color-text)]">
                30 days (DROP PARTITION)
              </dd>
            </div>
            <div className="flex justify-between">
              <dt className="text-[var(--color-text-dim)]">Full events cap</dt>
              <dd className="font-semibold text-[var(--color-text)]">
                50 / issue (+ sample)
              </dd>
            </div>
            <div className="flex justify-between">
              <dt className="text-[var(--color-text-dim)]">Short overrides</dt>
              <dd className="text-[var(--color-text-muted)]">
                Bounded batch delete
              </dd>
            </div>
          </dl>
        </section>

        <section className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-card)] p-4 shadow-[var(--pm-shadow-card)]">
          <div className="flex items-center justify-between">
            <span className="text-xs font-bold text-[var(--color-text-muted)]">
              Security &amp; PII Scrubbing
            </span>
            <ShieldCheck size={15} className="text-[var(--color-success)]" />
          </div>
          <ul className="mt-3 space-y-1.5 text-xs text-[var(--color-text-muted)]">
            <li className="flex items-center gap-1.5">
              <CheckCircle2 size={12} className="text-[var(--color-success)] shrink-0" />
              <span>Authorization &amp; Cookie headers scrubbed</span>
            </li>
            <li className="flex items-center gap-1.5">
              <CheckCircle2 size={12} className="text-[var(--color-success)] shrink-0" />
              <span>password / token / secret / api_key filtered</span>
            </li>
            <li className="flex items-center gap-1.5">
              <CheckCircle2 size={12} className="text-[var(--color-success)] shrink-0" />
              <span>Row-Level Security (app.tenant_id) active</span>
            </li>
          </ul>
        </section>
      </div>
    </div>
  );
}

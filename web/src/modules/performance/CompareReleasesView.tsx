import { useCallback, useEffect, useState } from "react";
import {
  AlertTriangle,
  ArrowRight,
  CheckCircle2,
  GitCompare,
  MinusCircle,
  TrendingDown,
  TrendingUp,
} from "lucide-react";
import {
  type ProjectView,
  type ReleaseComparisonView,
  type ReleaseView,
  fetchCompareReleases,
  fetchReleases,
} from "../../lib/api";

interface CompareReleasesViewProps {
  projects: ProjectView[];
  selectedProjectSlug: string;
  onSelectProjectSlug: (slug: string) => void;
  environment: string;
}

function formatMs(ms: number): string {
  if (Math.abs(ms) >= 1000) return `${(ms / 1000).toFixed(2)}s`;
  if (Math.abs(ms) >= 10) return `${ms.toFixed(0)}ms`;
  return `${ms.toFixed(1)}ms`;
}

function formatPct(rate: number): string {
  return `${(rate * 100).toFixed(2)}%`;
}

function formatSignedDeltaMs(deltaMs: number, deltaPct: number | null): string {
  const sign = deltaMs > 0 ? "+" : "";
  const pctStr =
    deltaPct !== null ? ` (${deltaPct > 0 ? "+" : ""}${deltaPct.toFixed(1)}%)` : "";
  return `${sign}${formatMs(deltaMs)}${pctStr}`;
}

export function CompareReleasesView({
  projects,
  selectedProjectSlug,
  onSelectProjectSlug,
  environment,
}: CompareReleasesViewProps) {
  const effectiveProject = selectedProjectSlug || projects[0]?.slug || "";
  const [releases, setReleases] = useState<ReleaseView[]>([]);
  const [baseRelease, setBaseRelease] = useState("");
  const [targetRelease, setTargetRelease] = useState("");
  const [hours, setHours] = useState<number>(720);

  const [comparison, setComparison] = useState<ReleaseComparisonView | null>(
    null,
  );
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!effectiveProject) {
      setReleases([]);
      return;
    }
    let active = true;
    fetchReleases(effectiveProject)
      .then((list) => {
        if (!active) return;
        setReleases(list);
        if (list.length >= 2) {
          setTargetRelease((prev) => prev || list[0].version);
          setBaseRelease((prev) => prev || list[1].version);
        } else if (list.length === 1) {
          setTargetRelease((prev) => prev || list[0].version);
        }
      })
      .catch(() => {
        if (active) setReleases([]);
      });
    return () => {
      active = false;
    };
  }, [effectiveProject]);

  const runCompare = useCallback(async () => {
    if (!effectiveProject || !baseRelease.trim() || !targetRelease.trim()) {
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const res = await fetchCompareReleases({
        project_slug: effectiveProject,
        base_release: baseRelease.trim(),
        target_release: targetRelease.trim(),
        environment: environment.trim() || undefined,
        hours,
        limit: 50,
      });
      setComparison(res);
    } catch (err) {
      setComparison(null);
      setError(
        err instanceof Error ? err.message : "Failed to compare releases",
      );
    } finally {
      setLoading(false);
    }
  }, [effectiveProject, baseRelease, targetRelease, environment, hours]);

  useEffect(() => {
    if (
      effectiveProject &&
      baseRelease.trim() &&
      targetRelease.trim() &&
      baseRelease.trim() !== targetRelease.trim()
    ) {
      void runCompare();
    }
  }, [effectiveProject, baseRelease, targetRelease, environment, hours, runCompare]);

  const verdictBadge = (verdict: string) => {
    switch (verdict) {
      case "regressed":
        return (
          <span className="pm-lozenge pm-lozenge-danger inline-flex items-center gap-1">
            <TrendingUp size={11} />
            <span>REGRESSED</span>
          </span>
        );
      case "improved":
        return (
          <span className="pm-lozenge pm-lozenge-success inline-flex items-center gap-1">
            <TrendingDown size={11} />
            <span>IMPROVED</span>
          </span>
        );
      case "stable":
        return (
          <span className="pm-lozenge inline-flex items-center gap-1">
            <CheckCircle2 size={11} />
            <span>STABLE</span>
          </span>
        );
      default:
        return (
          <span className="pm-lozenge inline-flex items-center gap-1">
            <MinusCircle size={11} />
            <span>INSUFFICIENT DATA</span>
          </span>
        );
    }
  };

  return (
    <div className="flex-1 min-h-0 flex flex-col overflow-hidden">
      {/* Selector Bar */}
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void runCompare();
        }}
        className="px-4 md:px-6 py-2.5 border-b border-[var(--color-border)] bg-[var(--color-bg)] flex flex-wrap items-center gap-2 shrink-0"
      >
        <select
          value={effectiveProject}
          onChange={(e) => {
            onSelectProjectSlug(e.target.value);
            setBaseRelease("");
            setTargetRelease("");
            setComparison(null);
          }}
          aria-label="Project for release comparison"
          className="pm-input h-8 text-sm w-44"
        >
          {projects.length === 0 && <option value="">No projects</option>}
          {projects.map((p) => (
            <option key={p.id} value={p.slug}>
              {p.slug}
            </option>
          ))}
        </select>

        <div className="flex items-center gap-1.5">
          <input
            type="text"
            list="compare-release-options"
            value={baseRelease}
            onChange={(e) => setBaseRelease(e.target.value)}
            placeholder="Base release (older)"
            className="pm-input h-8 text-sm w-44 pm-mono"
          />
          <ArrowRight size={14} className="text-[var(--color-text-dim)]" />
          <input
            type="text"
            list="compare-release-options"
            value={targetRelease}
            onChange={(e) => setTargetRelease(e.target.value)}
            placeholder="Target release (newer)"
            className="pm-input h-8 text-sm w-44 pm-mono"
          />
          <datalist id="compare-release-options">
            {releases.map((r) => (
              <option key={r.id} value={r.version} />
            ))}
          </datalist>
        </div>

        <select
          value={hours}
          onChange={(e) => setHours(Number(e.target.value))}
          aria-label="Comparison window"
          className="pm-input h-8 text-sm w-32"
        >
          <option value={24}>Last 24 hours</option>
          <option value={168}>Last 7 days</option>
          <option value={720}>Last 30 days</option>
          <option value={2160}>Last 90 days</option>
        </select>

        <button
          type="submit"
          disabled={
            loading ||
            !effectiveProject ||
            !baseRelease.trim() ||
            !targetRelease.trim()
          }
          className="h-8 px-3 rounded bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-40 text-white text-sm font-semibold inline-flex items-center gap-1.5 cursor-pointer"
        >
          <GitCompare size={13} />
          <span>{loading ? "Comparing..." : "Compare"}</span>
        </button>
      </form>

      {error && (
        <div className="mx-6 mt-3 px-3 py-2 rounded bg-[#ffeceb] border border-[#f87168] text-xs text-[#ae2e24]">
          {error}
        </div>
      )}

      <div className="flex-1 min-h-0 overflow-y-auto p-6 space-y-5">
        {!comparison ? (
          <div className="p-10 text-center space-y-2 border border-dashed border-[var(--color-border-strong)] rounded-md bg-[var(--color-bg-muted)]">
            <GitCompare
              size={28}
              className="mx-auto text-[var(--color-text-dim)]"
            />
            <div className="text-sm font-semibold text-[var(--color-text)]">
              Compare two releases side by side
            </div>
            <p className="text-xs text-[var(--color-text-dim)] max-w-md mx-auto">
              Select a project and enter a base release and target release to
              compare p50/p95/p99 latency, throughput, error rate, and newly
              introduced issues.
            </p>
          </div>
        ) : (
          <>
            {/* Verdict & Release Headers */}
            <div className="p-4 rounded border border-[var(--color-border)] bg-[var(--color-bg-muted)] flex flex-wrap items-center justify-between gap-3">
              <div className="flex flex-wrap items-center gap-2 text-xs">
                <span className="font-bold text-[var(--color-text)]">
                  {comparison.project_slug}
                </span>
                <span className="pm-lozenge pm-mono">
                  {comparison.base.version}
                  {comparison.base.commit_sha
                    ? ` (${comparison.base.commit_sha.slice(0, 7)})`
                    : ""}
                </span>
                <ArrowRight size={13} className="text-[var(--color-text-dim)]" />
                <span className="pm-lozenge pm-lozenge-info pm-mono">
                  {comparison.target.version}
                  {comparison.target.commit_sha
                    ? ` (${comparison.target.commit_sha.slice(0, 7)})`
                    : ""}
                </span>
              </div>
              <div className="flex items-center gap-2">
                {verdictBadge(comparison.verdict)}
              </div>
            </div>

            {/* Summary KPI Delta Grid */}
            <div className="grid grid-cols-2 lg:grid-cols-5 gap-3 text-xs">
              <div className="p-3 rounded border border-[var(--color-border)] bg-[var(--color-bg)]">
                <div className="text-[11px] text-[var(--color-text-dim)]">
                  p50 Latency
                </div>
                <div className="mt-1 pm-mono font-bold text-sm text-[var(--color-text)]">
                  {formatMs(comparison.base.performance.p50_ms)} →{" "}
                  {formatMs(comparison.target.performance.p50_ms)}
                </div>
                <div
                  className={`mt-0.5 text-[11px] pm-mono font-semibold ${
                    comparison.delta.p50_delta_ms > 15
                      ? "text-[#ae2e24]"
                      : comparison.delta.p50_delta_ms < -15
                        ? "text-[#216e4e]"
                        : "text-[var(--color-text-dim)]"
                  }`}
                >
                  {formatSignedDeltaMs(
                    comparison.delta.p50_delta_ms,
                    comparison.delta.p50_delta_pct,
                  )}
                </div>
              </div>

              <div className="p-3 rounded border border-[var(--color-border)] bg-[var(--color-bg)]">
                <div className="text-[11px] text-[var(--color-text-dim)]">
                  p95 Latency
                </div>
                <div className="mt-1 pm-mono font-bold text-sm text-[var(--color-text)]">
                  {formatMs(comparison.base.performance.p95_ms)} →{" "}
                  {formatMs(comparison.target.performance.p95_ms)}
                </div>
                <div
                  className={`mt-0.5 text-[11px] pm-mono font-semibold ${
                    comparison.delta.p95_delta_ms >= 15
                      ? "text-[#ae2e24]"
                      : comparison.delta.p95_delta_ms <= -15
                        ? "text-[#216e4e]"
                        : "text-[var(--color-text-dim)]"
                  }`}
                >
                  {formatSignedDeltaMs(
                    comparison.delta.p95_delta_ms,
                    comparison.delta.p95_delta_pct,
                  )}
                </div>
              </div>

              <div className="p-3 rounded border border-[var(--color-border)] bg-[var(--color-bg)]">
                <div className="text-[11px] text-[var(--color-text-dim)]">
                  Error Rate
                </div>
                <div className="mt-1 pm-mono font-bold text-sm text-[var(--color-text)]">
                  {formatPct(comparison.base.performance.error_rate)} →{" "}
                  {formatPct(comparison.target.performance.error_rate)}
                </div>
                <div
                  className={`mt-0.5 text-[11px] pm-mono font-semibold ${
                    comparison.delta.error_rate_delta >= 0.02
                      ? "text-[#ae2e24]"
                      : comparison.delta.error_rate_delta <= -0.02
                        ? "text-[#216e4e]"
                        : "text-[var(--color-text-dim)]"
                  }`}
                >
                  {comparison.delta.error_rate_delta > 0 ? "+" : ""}
                  {(comparison.delta.error_rate_delta * 100).toFixed(2)} pp
                </div>
              </div>

              <div className="p-3 rounded border border-[var(--color-border)] bg-[var(--color-bg)]">
                <div className="text-[11px] text-[var(--color-text-dim)]">
                  Throughput (RPM)
                </div>
                <div className="mt-1 pm-mono font-bold text-sm text-[var(--color-text)]">
                  {comparison.base.performance.throughput_rpm.toFixed(1)} →{" "}
                  {comparison.target.performance.throughput_rpm.toFixed(1)}
                </div>
                <div className="mt-0.5 text-[11px] pm-mono text-[var(--color-text-dim)]">
                  {comparison.base.performance.total_count} →{" "}
                  {comparison.target.performance.total_count} reqs
                </div>
              </div>

              <div className="p-3 rounded border border-[var(--color-border)] bg-[var(--color-bg)]">
                <div className="text-[11px] text-[var(--color-text-dim)]">
                  Issues (New / Linked)
                </div>
                <div className="mt-1 pm-mono font-bold text-sm text-[var(--color-text)]">
                  {comparison.target.issues.new_issues_count} new /{" "}
                  {comparison.target.issues.linked_issues_count} linked
                </div>
                <div
                  className={`mt-0.5 text-[11px] pm-mono font-semibold ${
                    comparison.delta.new_issues_delta > 0
                      ? "text-[#ae2e24]"
                      : "text-[var(--color-text-dim)]"
                  }`}
                >
                  Base: {comparison.base.issues.new_issues_count} new /{" "}
                  {comparison.base.issues.linked_issues_count} linked
                </div>
              </div>
            </div>

            {/* Newly Introduced Issues in Target Release */}
            {comparison.new_issues_in_target.length > 0 && (
              <div className="rounded border border-[#f87168] bg-[#fff7f6] overflow-hidden">
                <div className="px-4 py-2 border-b border-[#ffd5d2] flex items-center gap-2 text-xs font-bold text-[#ae2e24]">
                  <AlertTriangle size={13} />
                  <span>
                    New Issues Seen Only in {comparison.target.version} (
                    {comparison.new_issues_in_target.length})
                  </span>
                </div>
                <ul className="divide-y divide-[#ffd5d2] text-xs">
                  {comparison.new_issues_in_target.map((iss) => (
                    <li
                      key={iss.id}
                      className="px-4 py-2 flex items-center justify-between gap-3"
                    >
                      <div className="min-w-0">
                        <span className="pm-mono font-bold text-[#ae2e24] mr-2">
                          #{iss.id}
                        </span>
                        <span className="font-semibold text-[var(--color-text)]">
                          {iss.title}
                        </span>
                      </div>
                      <div className="flex items-center gap-2 shrink-0">
                        <span className="pm-lozenge">{iss.status}</span>
                        <span className="pm-mono text-[11px] text-[var(--color-text-dim)]">
                          {iss.event_count} events
                        </span>
                      </div>
                    </li>
                  ))}
                </ul>
              </div>
            )}

            {/* Per-Transaction Delta Table */}
            <div className="rounded border border-[var(--color-border)] overflow-hidden">
              <div className="px-4 py-2.5 bg-[var(--color-bg-sidebar)] border-b border-[var(--color-border)] flex items-center justify-between text-xs">
                <span className="font-bold text-[var(--color-text)]">
                  Transaction Breakdown ({comparison.transactions.length} of{" "}
                  {comparison.total_transactions_compared})
                </span>
              </div>
              {comparison.transactions.length === 0 ? (
                <div className="p-6 text-center text-xs text-[var(--color-text-dim)]">
                  No transactions recorded for either release in this time
                  window.
                </div>
              ) : (
                <table className="w-full max-md:min-w-[640px] text-left border-collapse text-[13px]">
                  <thead className="bg-[var(--color-bg-muted)] border-b border-[var(--color-border)] text-[11px] text-[var(--color-text-dim)]">
                    <tr>
                      <th className="py-2 pl-4 pr-3 font-semibold">
                        Transaction
                      </th>
                      <th className="py-2 px-3 font-semibold">Status</th>
                      <th className="py-2 px-3 font-semibold text-right">
                        Base p95
                      </th>
                      <th className="py-2 px-3 font-semibold text-right">
                        Target p95
                      </th>
                      <th className="py-2 px-3 font-semibold text-right">
                        p95 Delta
                      </th>
                      <th className="py-2 px-3 font-semibold text-right">
                        Error Rate
                      </th>
                      <th className="py-2 pr-4 pl-3 font-semibold text-right">
                        RPM
                      </th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-[var(--color-border)]">
                    {comparison.transactions.map((tx) => (
                      <tr
                        key={`${tx.op}:${tx.transaction_name}`}
                        className="hover:bg-[var(--color-bg-hover)]"
                      >
                        <td className="py-2 pl-4 pr-3 max-w-[320px]">
                          <div className="font-semibold text-[var(--color-text)] truncate">
                            {tx.transaction_name}
                          </div>
                          <div className="text-[11px] pm-mono text-[var(--color-text-dim)]">
                            {tx.op}
                          </div>
                        </td>
                        <td className="py-2 px-3">
                          <span
                            className={`pm-lozenge ${
                              tx.status === "regressed"
                                ? "pm-lozenge-danger"
                                : tx.status === "improved"
                                  ? "pm-lozenge-success"
                                  : tx.status === "new"
                                    ? "pm-lozenge-info"
                                    : ""
                            }`}
                          >
                            {tx.status.toUpperCase()}
                          </span>
                        </td>
                        <td className="py-2 px-3 text-right pm-mono">
                          {tx.base ? formatMs(tx.base.p95_ms) : "—"}
                        </td>
                        <td className="py-2 px-3 text-right pm-mono font-semibold">
                          {tx.target ? formatMs(tx.target.p95_ms) : "—"}
                        </td>
                        <td className="py-2 px-3 text-right pm-mono font-semibold">
                          {tx.p95_delta_ms !== null ? (
                            <span
                              className={
                                tx.p95_delta_ms >= 15
                                  ? "text-[#ae2e24]"
                                  : tx.p95_delta_ms <= -15
                                    ? "text-[#216e4e]"
                                    : "text-[var(--color-text-dim)]"
                              }
                            >
                              {formatSignedDeltaMs(
                                tx.p95_delta_ms,
                                tx.p95_delta_pct,
                              )}
                            </span>
                          ) : (
                            "—"
                          )}
                        </td>
                        <td className="py-2 px-3 text-right pm-mono">
                          {tx.base ? formatPct(tx.base.error_rate) : "—"} →{" "}
                          {tx.target ? formatPct(tx.target.error_rate) : "—"}
                        </td>
                        <td className="py-2 pr-4 pl-3 text-right pm-mono text-[var(--color-text-muted)]">
                          {tx.base ? tx.base.throughput_rpm.toFixed(1) : "—"} →{" "}
                          {tx.target
                            ? tx.target.throughput_rpm.toFixed(1)
                            : "—"}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}

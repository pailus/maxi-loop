import { ExternalLink, Zap } from "lucide-react";
import type { IssueEventView, PerformanceProblemPayload } from "../../lib/api";

interface PerformanceProblemViewProps {
  sampleEvent: IssueEventView | null | undefined;
  compact?: boolean;
  onOpenTrace?: (traceId: string) => void;
}

const EXPLANATIONS: Record<string, string> = {
  performance_n_plus_one_db:
    "The same query ran many times in one request. Fetch the rows in one query (JOIN, IN (...), select_related / prefetch_related, eager loading).",
  performance_n_plus_one_api:
    "The same HTTP endpoint was called many times in one request. Use a batch endpoint or cache the responses.",
  performance_slow_db_query:
    "A single database query took over 1 second. Check the query plan (EXPLAIN ANALYZE) for missing indexes or full scans.",
  performance_consecutive_db:
    "Independent queries ran one after another. Running them concurrently, or combining them, would save the time shown.",
  performance_consecutive_http:
    "Slow HTTP calls ran one after another. Running them concurrently would save the time shown.",
};

export function extractPerformanceProblem(
  sampleEvent: IssueEventView | null | undefined,
): PerformanceProblemPayload | null {
  const payload = sampleEvent?.payload as { performance_problem?: unknown } | undefined;
  const pp = payload?.performance_problem as PerformanceProblemPayload | undefined;
  if (!pp || typeof pp !== "object" || !pp.problem) return null;
  return pp;
}

function fmtMs(ms: number | undefined): string {
  if (ms === undefined || !Number.isFinite(ms)) return "—";
  return ms >= 1000 ? `${(ms / 1000).toFixed(2)} s` : `${ms.toFixed(1)} ms`;
}

export function PerformanceProblemView({
  sampleEvent,
  compact = false,
  onOpenTrace,
}: PerformanceProblemViewProps) {
  const pp = extractPerformanceProblem(sampleEvent);
  if (!pp) {
    return (
      <div className="text-xs text-[var(--color-text-dim)]">
        No sample event stored for this performance issue.
      </div>
    );
  }

  const { problem } = pp;
  const spans = compact ? problem.offending_spans.slice(0, 5) : problem.offending_spans;
  const share =
    pp.transaction_duration_ms > 0
      ? Math.min(100, (problem.offending_spans_total_ms / pp.transaction_duration_ms) * 100)
      : null;

  return (
    <div className="space-y-3">
      <div className="flex items-start gap-2 text-xs text-[var(--color-text-muted)]">
        <Zap size={14} className="mt-0.5 shrink-0 text-[var(--color-purple-text)]" />
        <span>{EXPLANATIONS[pp.issue_type] ?? "Inefficient span pattern detected."}</span>
      </div>

      <dl className="grid grid-cols-2 sm:grid-cols-4 gap-2 text-xs">
        <div className="px-2.5 py-2 rounded border border-[var(--color-border)] bg-[var(--color-bg-muted)]">
          <dt className="text-[11px] text-[var(--color-text-dim)]">Offending spans</dt>
          <dd className="font-semibold text-[var(--color-text)]">{problem.offending_span_count}</dd>
        </div>
        <div className="px-2.5 py-2 rounded border border-[var(--color-border)] bg-[var(--color-bg-muted)]">
          <dt className="text-[11px] text-[var(--color-text-dim)]">Time in spans</dt>
          <dd className="font-semibold text-[var(--color-text)]">
            {fmtMs(problem.offending_spans_total_ms)}
            {share !== null && (
              <span className="font-normal text-[var(--color-text-dim)]"> ({share.toFixed(0)}%)</span>
            )}
          </dd>
        </div>
        <div className="px-2.5 py-2 rounded border border-[var(--color-border)] bg-[var(--color-bg-muted)]">
          <dt className="text-[11px] text-[var(--color-text-dim)]">Transaction</dt>
          <dd className="font-semibold text-[var(--color-text)]">{fmtMs(pp.transaction_duration_ms)}</dd>
        </div>
        <div className="px-2.5 py-2 rounded border border-[var(--color-border)] bg-[var(--color-bg-muted)]">
          <dt className="text-[11px] text-[var(--color-text-dim)]">Potential savings</dt>
          <dd className="font-semibold text-[var(--color-text)]">{fmtMs(problem.potential_savings_ms)}</dd>
        </div>
      </dl>

      <div>
        <div className="flex items-center justify-between mb-1.5">
          <span className="text-xs font-bold text-[var(--color-text-muted)]">
            Offending spans{" "}
            {problem.offending_span_count > spans.length &&
              `(showing ${spans.length} of ${problem.offending_span_count})`}
          </span>
          {onOpenTrace && pp.trace_id && (
            <button
              type="button"
              onClick={() => onOpenTrace(pp.trace_id)}
              className="text-xs font-medium text-[var(--color-link)] hover:underline cursor-pointer inline-flex items-center gap-1"
            >
              View trace <ExternalLink size={12} />
            </button>
          )}
        </div>
        <div className="overflow-x-auto rounded border border-[var(--color-border)]">
          <table className="w-full text-xs">
            <thead className="bg-[var(--color-bg-muted)] text-[var(--color-text-dim)]">
              <tr>
                <th className="text-left font-semibold px-2.5 py-1.5">op</th>
                <th className="text-left font-semibold px-2.5 py-1.5">Description (normalized)</th>
                <th className="text-right font-semibold px-2.5 py-1.5 whitespace-nowrap">Start</th>
                <th className="text-right font-semibold px-2.5 py-1.5 whitespace-nowrap">Duration</th>
              </tr>
            </thead>
            <tbody>
              {spans.map((s) => (
                <tr key={s.span_id} className="border-t border-[var(--color-border-light)]">
                  <td className="px-2.5 py-1.5 pm-mono text-[var(--color-text-muted)] whitespace-nowrap">{s.op}</td>
                  <td className="px-2.5 py-1.5 pm-mono text-[var(--color-text)] break-all">{s.description}</td>
                  <td className="px-2.5 py-1.5 text-right text-[var(--color-text-muted)] whitespace-nowrap">
                    +{fmtMs(s.start_offset_ms)}
                  </td>
                  <td className="px-2.5 py-1.5 text-right font-medium text-[var(--color-text)] whitespace-nowrap">
                    {fmtMs(s.duration_ms)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  );
}

import { useCallback, useEffect, useState } from "react";
import { Database, Flame, Search } from "lucide-react";
import {
  type PaginatedSlowOperationsResponse,
  type SlowOperationSort,
  fetchSlowOperations,
} from "../../lib/api";

interface SlowOperationsViewProps {
  workspaceSlug?: string;
  selectedProjectSlug: string;
  environment: string;
  release: string;
  hours: number;
  refreshSignal: number;
  onOpenTrace: (traceId: string) => void;
}

function formatMs(ms: number): string {
  if (ms >= 1000) return `${(ms / 1000).toFixed(2)}s`;
  if (ms >= 10) return `${ms.toFixed(0)}ms`;
  return `${ms.toFixed(1)}ms`;
}

function formatPct(rate: number): string {
  return `${(rate * 100).toFixed(1)}%`;
}

export function SlowOperationsView({
  workspaceSlug,
  selectedProjectSlug,
  environment,
  release,
  hours,
  refreshSignal,
  onOpenTrace,
}: SlowOperationsViewProps) {
  const [dbOnly, setDbOnly] = useState(true);
  const [opFilter, setOpFilter] = useState("");
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SlowOperationSort>("p95");
  const [page, setPage] = useState(1);

  const [data, setData] = useState<PaginatedSlowOperationsResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const res = await fetchSlowOperations({
        workspace_slug: workspaceSlug,
        project_slug: selectedProjectSlug || undefined,
        environment: environment.trim() || undefined,
        release: release.trim() || undefined,
        op: opFilter.trim() || undefined,
        db_only: opFilter.trim() ? undefined : dbOnly,
        query: query.trim() || undefined,
        hours,
        sort,
        page,
        per_page: 25,
      });
      setData(res);
    } catch (err) {
      setError(
        err instanceof Error ? err.message : "Failed to load slow operations",
      );
    } finally {
      setLoading(false);
    }
  }, [
    workspaceSlug,
    selectedProjectSlug,
    environment,
    release,
    opFilter,
    dbOnly,
    query,
    hours,
    sort,
    page,
  ]);

  useEffect(() => {
    void load();
  }, [load, refreshSignal]);

  const items = data?.items ?? [];

  return (
    <div className="flex-1 min-h-0 flex flex-col overflow-hidden">
      {/* Sub-toolbar specific to Slow Queries & Operations */}
      <div className="px-6 py-2 border-b border-[var(--color-border)] bg-[var(--color-bg)] flex flex-wrap items-center gap-2 shrink-0">
        <label className="inline-flex items-center gap-1.5 text-xs font-medium text-[var(--color-text)] cursor-pointer select-none px-2 py-1 rounded border border-[var(--color-border)] bg-[var(--color-bg-muted)]">
          <input
            type="checkbox"
            checked={dbOnly && !opFilter.trim()}
            disabled={Boolean(opFilter.trim())}
            onChange={(e) => {
              setDbOnly(e.target.checked);
              setPage(1);
            }}
            className="accent-[var(--color-primary)]"
          />
          <span>DB queries only (db.*)</span>
        </label>

        <input
          type="text"
          value={opFilter}
          onChange={(e) => {
            setOpFilter(e.target.value);
            setPage(1);
          }}
          placeholder="Op filter (e.g. db.sql.query, http.client)"
          className="pm-input h-8 text-xs w-56"
        />

        <div className="relative flex-1 min-w-[200px] max-w-[320px]">
          <Search
            size={13}
            className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--color-text-dim)] pointer-events-none"
          />
          <input
            type="text"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setPage(1);
            }}
            placeholder="Search normalized SQL / operation..."
            className="pm-input h-8 text-xs pl-8 w-full"
          />
        </div>

        <select
          value={sort}
          onChange={(e) => {
            setSort(e.target.value as SlowOperationSort);
            setPage(1);
          }}
          aria-label="Sort slow operations"
          className="pm-input h-8 text-xs w-40 ml-auto"
        >
          <option value="p95">Sort: p95 latency</option>
          <option value="total_ms">Sort: Total time spent</option>
          <option value="avg">Sort: Avg latency</option>
          <option value="p50">Sort: p50 latency</option>
          <option value="p99">Sort: p99 latency</option>
          <option value="count">Sort: Call count</option>
          <option value="error_rate">Sort: Error rate</option>
        </select>
      </div>

      {error && (
        <div className="mx-6 mt-3 px-3 py-2 rounded bg-[#ffeceb] border border-[#f87168] text-xs text-[#ae2e24]">
          {error}
        </div>
      )}

      <div className="flex-1 min-h-0 overflow-y-auto">
        {loading && items.length === 0 ? (
          <div className="p-8 text-center text-xs text-[var(--color-text-dim)]">
            Loading aggregated span operations...
          </div>
        ) : items.length === 0 ? (
          <div className="p-10 text-center space-y-2">
            <Database
              size={28}
              className="mx-auto text-[var(--color-text-dim)]"
            />
            <div className="text-sm font-semibold text-[var(--color-text)]">
              No {dbOnly && !opFilter.trim() ? "DB queries" : "span operations"}{" "}
              recorded in this window
            </div>
            <p className="text-xs text-[var(--color-text-dim)] max-w-md mx-auto">
              Normalized DB queries (<code className="pm-mono">db.*</code>),
              outbound HTTP calls (<code className="pm-mono">http.client</code>
              ), and cache operations are aggregated automatically from ingested
              transaction spans.
            </p>
          </div>
        ) : (
          <div>
            <table className="w-full text-left border-collapse text-xs">
              <thead className="sticky top-0 bg-[var(--color-bg-sidebar)] border-b border-[var(--color-border)] text-[11px] text-[var(--color-text-dim)]">
                <tr>
                  <th className="py-2.5 pl-6 pr-3 font-semibold">
                    Normalized Query / Operation
                  </th>
                  <th className="py-2.5 px-3 font-semibold">Op</th>
                  <th className="py-2.5 px-3 font-semibold">Project</th>
                  <th className="py-2.5 px-3 font-semibold text-right">
                    Calls
                  </th>
                  <th className="py-2.5 px-3 font-semibold text-right">p50</th>
                  <th className="py-2.5 px-3 font-semibold text-right">p95</th>
                  <th className="py-2.5 px-3 font-semibold text-right">Avg</th>
                  <th className="py-2.5 px-3 font-semibold text-right">
                    Total
                  </th>
                  <th className="py-2.5 px-3 font-semibold text-right">
                    Error %
                  </th>
                  <th className="py-2.5 pr-6 pl-3 font-semibold text-right">
                    Trace
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-[var(--color-border)]">
                {items.map((row) => (
                  <tr
                    key={`${row.project_id}:${row.op}:${row.description_hash}`}
                    className="hover:bg-[var(--color-bg-hover)] transition"
                  >
                    <td className="py-2.5 pl-6 pr-3 max-w-[440px]">
                      <div
                        className="pm-mono text-[11px] text-[var(--color-text)] break-words line-clamp-2"
                        title={row.normalized_description}
                      >
                        {row.normalized_description}
                      </div>
                    </td>
                    <td className="py-2.5 px-3 whitespace-nowrap">
                      <span className="pm-lozenge pm-mono">{row.op}</span>
                    </td>
                    <td className="py-2.5 px-3 whitespace-nowrap">
                      <span className="pm-lozenge">{row.project_slug}</span>
                    </td>
                    <td className="py-2.5 px-3 text-right pm-mono">
                      {row.total_count.toLocaleString()}
                    </td>
                    <td className="py-2.5 px-3 text-right pm-mono">
                      {formatMs(row.p50_ms)}
                    </td>
                    <td className="py-2.5 px-3 text-right pm-mono font-semibold">
                      <span
                        className={
                          row.p95_ms >= 200
                            ? "text-[#974f0c]"
                            : "text-[var(--color-text)]"
                        }
                      >
                        {formatMs(row.p95_ms)}
                      </span>
                    </td>
                    <td className="py-2.5 px-3 text-right pm-mono text-[var(--color-text-muted)]">
                      {formatMs(row.avg_ms)}
                    </td>
                    <td className="py-2.5 px-3 text-right pm-mono font-semibold text-[var(--color-text)]">
                      {formatMs(row.total_ms)}
                    </td>
                    <td className="py-2.5 px-3 text-right">
                      <span
                        className={`pm-lozenge ${
                          row.error_rate > 0.05
                            ? "pm-lozenge-danger"
                            : row.error_rate > 0
                              ? "pm-lozenge-warning"
                              : "pm-lozenge-success"
                        }`}
                      >
                        {formatPct(row.error_rate)}
                      </span>
                    </td>
                    <td className="py-2.5 pr-6 pl-3 text-right whitespace-nowrap">
                      {row.sample_trace_id ? (
                        <button
                          type="button"
                          onClick={() =>
                            onOpenTrace(row.sample_trace_id as string)
                          }
                          className="h-6 px-2 rounded border border-[var(--color-border-strong)] bg-[var(--color-bg)] hover:border-[var(--color-primary)] text-[11px] font-semibold text-[var(--color-link)] inline-flex items-center gap-1 cursor-pointer"
                        >
                          <Flame size={11} />
                          <span>Waterfall</span>
                        </button>
                      ) : (
                        <span className="text-[11px] text-[var(--color-text-dim)]">
                          —
                        </span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>

            {data && data.total_pages > 1 && (
              <div className="px-6 py-3 border-t border-[var(--color-border)] flex items-center justify-between text-xs text-[var(--color-text-dim)]">
                <span>
                  Page {data.page} of {data.total_pages} ({data.total}{" "}
                  operations)
                </span>
                <div className="flex gap-2">
                  <button
                    type="button"
                    disabled={page <= 1}
                    onClick={() => setPage((p) => Math.max(1, p - 1))}
                    className="h-7 px-2.5 rounded border border-[var(--color-border-strong)] disabled:opacity-40 cursor-pointer"
                  >
                    Previous
                  </button>
                  <button
                    type="button"
                    disabled={page >= data.total_pages}
                    onClick={() => setPage((p) => p + 1)}
                    className="h-7 px-2.5 rounded border border-[var(--color-border-strong)] disabled:opacity-40 cursor-pointer"
                  >
                    Next
                  </button>
                </div>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

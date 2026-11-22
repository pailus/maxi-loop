import { useCallback, useEffect, useState } from "react";
import {
  Activity,
  ChevronRight,
  Clock,
  Flame,
  Gauge,
  RefreshCw,
  Search,
  X,
} from "lucide-react";
import {
  type PaginatedTransactionsResponse,
  type ProjectView,
  type TraceSummaryView,
  type TransactionDetailView,
  type TransactionSort,
  type TransactionSummaryView,
  fetchTraces,
  fetchTransactionDetail,
  fetchTransactions,
} from "../../lib/api";
import { CompareReleasesView } from "./CompareReleasesView";
import { SlowOperationsView } from "./SlowOperationsView";
import { TraceWaterfallView } from "./TraceWaterfallView";

export type PerfTab = "transactions" | "traces" | "slow-queries" | "compare";

interface PerformanceViewProps {
  workspaceName: string;
  workspaceSlug?: string;
  projects: ProjectView[];
  selectedProjectSlug: string;
  onSelectProjectSlug: (slug: string) => void;
  initialTraceId?: string;
  initialTab?: PerfTab;
  onClearInitialTraceId?: () => void;
  onShowToast: (message: string) => void;
}

function formatMs(ms: number): string {
  if (ms >= 1000) return `${(ms / 1000).toFixed(2)}s`;
  if (ms >= 10) return `${ms.toFixed(0)}ms`;
  return `${ms.toFixed(1)}ms`;
}

function formatPct(rate: number): string {
  return `${(rate * 100).toFixed(1)}%`;
}

function formatRelativeTime(iso: string): string {
  const diffSec = Math.max(0, Math.floor((Date.now() - Date.parse(iso)) / 1000));
  if (diffSec < 60) return `${diffSec}s ago`;
  if (diffSec < 3600) return `${Math.floor(diffSec / 60)}m ago`;
  if (diffSec < 86400) return `${Math.floor(diffSec / 3600)}h ago`;
  return `${Math.floor(diffSec / 86400)}d ago`;
}

export function PerformanceView({
  workspaceName,
  workspaceSlug,
  projects,
  selectedProjectSlug,
  onSelectProjectSlug,
  initialTraceId,
  initialTab,
  onClearInitialTraceId,
  onShowToast,
}: PerformanceViewProps) {
  const [activeTab, setActiveTab] = useState<PerfTab>(
    initialTab ?? "transactions",
  );
  const [refreshSignal, setRefreshSignal] = useState(0);
  const [environment, setEnvironment] = useState("");
  const [release, setRelease] = useState("");
  const [query, setQuery] = useState("");
  const [hours, setHours] = useState<number>(24);
  const [sort, setSort] = useState<TransactionSort>("p95");
  const [sampleReasonFilter, setSampleReasonFilter] = useState("");
  const [page, setPage] = useState(1);

  const [txPage, setTxPage] = useState<PaginatedTransactionsResponse | null>(
    null,
  );
  const [traces, setTraces] = useState<TraceSummaryView[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [selectedTx, setSelectedTx] = useState<TransactionSummaryView | null>(
    null,
  );
  const [txDetail, setTxDetail] = useState<TransactionDetailView | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);

  const [activeTraceId, setActiveTraceId] = useState<string | null>(
    initialTraceId || null,
  );

  useEffect(() => {
    if (initialTraceId) {
      setActiveTraceId(initialTraceId);
    }
  }, [initialTraceId]);

  const loadData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [txResp, traceList] = await Promise.all([
        fetchTransactions({
          workspace_slug: workspaceSlug,
          project_slug: selectedProjectSlug || undefined,
          environment: environment.trim() || undefined,
          release: release.trim() || undefined,
          query: query.trim() || undefined,
          hours,
          sort,
          page,
          per_page: 25,
        }),
        fetchTraces({
          project_slug: selectedProjectSlug || undefined,
          environment: environment.trim() || undefined,
          release: release.trim() || undefined,
          sample_reason: sampleReasonFilter || undefined,
          hours,
          limit: 50,
        }),
      ]);
      setTxPage(txResp);
      setTraces(traceList);
    } catch (err) {
      setError(
        err instanceof Error ? err.message : "Failed to load performance data",
      );
    } finally {
      setLoading(false);
    }
  }, [
    workspaceSlug,
    selectedProjectSlug,
    environment,
    release,
    query,
    hours,
    sort,
    sampleReasonFilter,
    page,
  ]);

  useEffect(() => {
    void loadData();
  }, [loadData]);

  useEffect(() => {
    if (!selectedTx) {
      setTxDetail(null);
      return;
    }
    let active = true;
    setDetailLoading(true);
    fetchTransactionDetail({
      project_slug: selectedTx.project_slug,
      transaction_name: selectedTx.transaction_name,
      op: selectedTx.op,
      environment: environment.trim() || undefined,
      release: release.trim() || undefined,
      hours,
    })
      .then((detail) => {
        if (active) setTxDetail(detail);
      })
      .catch(() => {
        if (active) setTxDetail(null);
      })
      .finally(() => {
        if (active) setDetailLoading(false);
      });
    return () => {
      active = false;
    };
  }, [selectedTx, environment, release, hours]);

  if (activeTraceId) {
    return (
      <TraceWaterfallView
        traceId={activeTraceId}
        onBack={() => {
          setActiveTraceId(null);
          onClearInitialTraceId?.();
        }}
        onShowToast={onShowToast}
      />
    );
  }

  const items = txPage?.items ?? [];
  const totalRequests = items.reduce((acc, it) => acc + it.total_count, 0);
  const totalErrors = items.reduce((acc, it) => acc + it.error_count, 0);
  const overallErrRate = totalRequests > 0 ? totalErrors / totalRequests : 0;
  const maxP95 = items.reduce((acc, it) => Math.max(acc, it.p95_ms), 0);

  return (
    <div className="flex-1 min-w-0 flex flex-col overflow-hidden bg-[var(--color-bg)]">
      {/* Page Header */}
      <div className="px-6 pt-4 pb-3 border-b border-[var(--color-border)] bg-[var(--color-bg)] shrink-0">
        <div className="flex items-center justify-between gap-4">
          <div>
            <div className="text-[11px] font-medium text-[var(--color-text-dim)]">
              {workspaceName} / Performance Monitoring
            </div>
            <h1 className="text-lg font-bold text-[var(--color-text)] flex items-center gap-2 mt-0.5">
              <Gauge size={18} className="text-[var(--color-primary)]" />
              <span>Transactions &amp; Distributed Traces</span>
            </h1>
          </div>

          <button
            type="button"
            onClick={() => {
              void loadData();
              setRefreshSignal((s) => s + 1);
            }}
            className="h-8 px-3 rounded border border-[var(--color-border-strong)] bg-[var(--color-bg)] hover:bg-[var(--color-bg-hover)] text-xs font-medium text-[var(--color-text)] inline-flex items-center gap-1.5 cursor-pointer"
          >
            <RefreshCw size={13} className={loading ? "animate-spin" : ""} />
            <span>Refresh</span>
          </button>
        </div>

        {/* Sub-tabs */}
        <div className="mt-3 flex items-center gap-4 border-b border-transparent">
          <button
            type="button"
            onClick={() => setActiveTab("transactions")}
            className={`pm-tab pb-2 text-xs font-semibold cursor-pointer ${
              activeTab === "transactions"
                ? "pm-tab-active text-[var(--color-primary)]"
                : "text-[var(--color-text-muted)]"
            }`}
          >
            Transactions ({txPage?.total ?? 0})
          </button>
          <button
            type="button"
            onClick={() => setActiveTab("traces")}
            className={`pm-tab pb-2 text-xs font-semibold cursor-pointer ${
              activeTab === "traces"
                ? "pm-tab-active text-[var(--color-primary)]"
                : "text-[var(--color-text-muted)]"
            }`}
          >
            Sampled Traces ({traces.length})
          </button>
          <button
            type="button"
            onClick={() => setActiveTab("slow-queries")}
            className={`pm-tab pb-2 text-xs font-semibold cursor-pointer ${
              activeTab === "slow-queries"
                ? "pm-tab-active text-[var(--color-primary)]"
                : "text-[var(--color-text-muted)]"
            }`}
          >
            Slow Queries &amp; Ops
          </button>
          <button
            type="button"
            onClick={() => setActiveTab("compare")}
            className={`pm-tab pb-2 text-xs font-semibold cursor-pointer ${
              activeTab === "compare"
                ? "pm-tab-active text-[var(--color-primary)]"
                : "text-[var(--color-text-muted)]"
            }`}
          >
            Compare Releases
          </button>
        </div>
      </div>

      {activeTab === "compare" ? (
        <CompareReleasesView
          projects={projects}
          selectedProjectSlug={selectedProjectSlug}
          onSelectProjectSlug={onSelectProjectSlug}
          environment={environment}
        />
      ) : (
        <>
      {/* Filter Toolbar */}
      <div className="px-6 py-2.5 border-b border-[var(--color-border)] bg-[var(--color-bg-muted)] flex flex-wrap items-center gap-2 shrink-0">
        <select
          value={selectedProjectSlug}
          onChange={(e) => {
            onSelectProjectSlug(e.target.value);
            setPage(1);
          }}
          aria-label="Filter by project"
          className="pm-input h-8 text-xs w-44"
        >
          <option value="">All projects</option>
          {projects.map((p) => (
            <option key={p.id} value={p.slug}>
              {p.slug}
            </option>
          ))}
        </select>

        <select
          value={hours}
          onChange={(e) => {
            setHours(Number(e.target.value));
            setPage(1);
          }}
          aria-label="Time window"
          className="pm-input h-8 text-xs w-32"
        >
          <option value={1}>Last 1 hour</option>
          <option value={6}>Last 6 hours</option>
          <option value={24}>Last 24 hours</option>
          <option value={168}>Last 7 days</option>
          <option value={720}>Last 30 days</option>
        </select>

        <input
          type="text"
          value={environment}
          onChange={(e) => {
            setEnvironment(e.target.value);
            setPage(1);
          }}
          placeholder="Environment (e.g. production)"
          className="pm-input h-8 text-xs w-44"
        />

        <input
          type="text"
          value={release}
          onChange={(e) => {
            setRelease(e.target.value);
            setPage(1);
          }}
          placeholder="Release version"
          className="pm-input h-8 text-xs w-36"
        />

        {activeTab === "transactions" ? (
          <>
            <div className="relative flex-1 min-w-[180px] max-w-[260px]">
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
                placeholder="Filter transaction name..."
                className="pm-input h-8 text-xs pl-8 w-full"
              />
            </div>

            <select
              value={sort}
              onChange={(e) => setSort(e.target.value as TransactionSort)}
              aria-label="Sort transactions"
              className="pm-input h-8 text-xs w-36 ml-auto"
            >
              <option value="p95">Sort: p95 latency</option>
              <option value="p99">Sort: p99 latency</option>
              <option value="p50">Sort: p50 latency</option>
              <option value="throughput">Sort: Throughput</option>
              <option value="error_rate">Sort: Error rate</option>
              <option value="avg">Sort: Avg latency</option>
              <option value="name">Sort: Name</option>
            </select>
          </>
        ) : activeTab === "traces" ? (
          <select
            value={sampleReasonFilter}
            onChange={(e) => setSampleReasonFilter(e.target.value)}
            aria-label="Filter by sample reason"
            className="pm-input h-8 text-xs w-40 ml-auto"
          >
            <option value="">All sample reasons</option>
            <option value="slow">Slow traces</option>
            <option value="error">Error traces</option>
            <option value="random">Random sample (1-5%)</option>
          </select>
        ) : null}
      </div>

      {activeTab === "slow-queries" ? (
        <SlowOperationsView
          workspaceSlug={workspaceSlug}
          selectedProjectSlug={selectedProjectSlug}
          environment={environment}
          release={release}
          hours={hours}
          refreshSignal={refreshSignal}
          onOpenTrace={(tid) => setActiveTraceId(tid)}
        />
      ) : (
        <>
      {/* Compact Metrics Strip */}
      <div className="px-6 py-2.5 border-b border-[var(--color-border)] bg-[var(--color-bg)] grid grid-cols-2 md:grid-cols-4 gap-3 text-xs shrink-0">
        <div className="px-3 py-2 rounded border border-[var(--color-border)] bg-[var(--color-bg-muted)]">
          <div className="text-[11px] text-[var(--color-text-dim)]">
            Observed Transactions
          </div>
          <div className="text-sm font-bold pm-mono text-[var(--color-text)] mt-0.5">
            {totalRequests.toLocaleString()}
          </div>
        </div>
        <div className="px-3 py-2 rounded border border-[var(--color-border)] bg-[var(--color-bg-muted)]">
          <div className="text-[11px] text-[var(--color-text-dim)]">
            Peak p95 Latency
          </div>
          <div className="text-sm font-bold pm-mono text-[var(--color-text)] mt-0.5">
            {formatMs(maxP95)}
          </div>
        </div>
        <div className="px-3 py-2 rounded border border-[var(--color-border)] bg-[var(--color-bg-muted)]">
          <div className="text-[11px] text-[var(--color-text-dim)]">
            Overall Error Rate
          </div>
          <div
            className={`text-sm font-bold pm-mono mt-0.5 ${
              overallErrRate > 0.05
                ? "text-[var(--color-danger)]"
                : "text-[var(--color-text)]"
            }`}
          >
            {formatPct(overallErrRate)}
          </div>
        </div>
        <div className="px-3 py-2 rounded border border-[var(--color-border)] bg-[var(--color-bg-muted)]">
          <div className="text-[11px] text-[var(--color-text-dim)]">
            Sampled zstd Traces
          </div>
          <div className="text-sm font-bold pm-mono text-[var(--color-text)] mt-0.5">
            {traces.length}
          </div>
        </div>
      </div>

      {error && (
        <div className="mx-6 mt-3 px-3 py-2 rounded bg-[#ffeceb] border border-[#f87168] text-xs text-[#ae2e24]">
          {error}
        </div>
      )}

      {/* Main Master-Detail Split */}
      <div className="flex-1 min-h-0 flex overflow-hidden">
        <div className="flex-1 min-w-0 overflow-y-auto">
          {loading && items.length === 0 && traces.length === 0 ? (
            <div className="p-8 text-center text-xs text-[var(--color-text-dim)]">
              Loading performance metrics...
            </div>
          ) : activeTab === "transactions" ? (
            items.length === 0 ? (
              <div className="p-10 text-center space-y-2">
                <Activity
                  size={28}
                  className="mx-auto text-[var(--color-text-dim)]"
                />
                <div className="text-sm font-semibold text-[var(--color-text)]">
                  No transactions recorded in this window
                </div>
                <p className="text-xs text-[var(--color-text-dim)] max-w-md mx-auto">
                  Enable{" "}
                  <code className="pm-mono bg-[var(--color-bg-muted)] px-1 py-0.5 rounded">
                    traces_sample_rate=1.0
                  </code>{" "}
                  in your Sentry SDK (FastAPI, Django, Go, Rust, or React) using
                  your project DSN to ingest transactions and distributed
                  traces.
                </p>
              </div>
            ) : (
              <div>
                <table className="w-full text-left border-collapse text-xs">
                  <thead className="sticky top-0 bg-[var(--color-bg-sidebar)] border-b border-[var(--color-border)] text-[11px] text-[var(--color-text-dim)]">
                    <tr>
                      <th className="py-2.5 pl-6 pr-3 font-semibold">
                        Transaction
                      </th>
                      <th className="py-2.5 px-3 font-semibold">Project</th>
                      <th className="py-2.5 px-3 font-semibold text-right">
                        RPM
                      </th>
                      <th className="py-2.5 px-3 font-semibold text-right">
                        p50
                      </th>
                      <th className="py-2.5 px-3 font-semibold text-right">
                        p95
                      </th>
                      <th className="py-2.5 px-3 font-semibold text-right">
                        p99
                      </th>
                      <th className="py-2.5 pr-6 pl-3 font-semibold text-right">
                        Error Rate
                      </th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-[var(--color-border)]">
                    {items.map((tx) => {
                      const isSelected =
                        selectedTx?.project_id === tx.project_id &&
                        selectedTx?.transaction_name === tx.transaction_name &&
                        selectedTx?.op === tx.op;
                      return (
                        <tr
                          key={`${tx.project_id}:${tx.op}:${tx.transaction_name}`}
                          onClick={() => setSelectedTx(tx)}
                          className={`cursor-pointer transition ${
                            isSelected
                              ? "bg-[var(--color-primary-light)]"
                              : "hover:bg-[var(--color-bg-hover)]"
                          }`}
                        >
                          <td className="py-2.5 pl-6 pr-3 max-w-[340px]">
                            <div className="font-semibold text-[var(--color-text)] truncate">
                              {tx.transaction_name}
                            </div>
                            <div className="text-[11px] text-[var(--color-text-dim)] flex items-center gap-2 mt-0.5">
                              <span className="pm-mono">{tx.op}</span>
                              <span>·</span>
                              <span>{tx.total_count} req</span>
                            </div>
                          </td>
                          <td className="py-2.5 px-3">
                            <span className="pm-lozenge">
                              {tx.project_slug}
                            </span>
                          </td>
                          <td className="py-2.5 px-3 text-right pm-mono text-[var(--color-text-muted)]">
                            {tx.throughput_rpm.toFixed(2)}
                          </td>
                          <td className="py-2.5 px-3 text-right pm-mono">
                            {formatMs(tx.p50_ms)}
                          </td>
                          <td className="py-2.5 px-3 text-right pm-mono font-semibold">
                            <span
                              className={
                                tx.p95_ms >= 500
                                  ? "text-[#974f0c]"
                                  : "text-[var(--color-text)]"
                              }
                            >
                              {formatMs(tx.p95_ms)}
                            </span>
                          </td>
                          <td className="py-2.5 px-3 text-right pm-mono text-[var(--color-text-muted)]">
                            {formatMs(tx.p99_ms)}
                          </td>
                          <td className="py-2.5 pr-6 pl-3 text-right">
                            <span
                              className={`pm-lozenge ${
                                tx.error_rate > 0.05
                                  ? "pm-lozenge-danger"
                                  : tx.error_rate > 0
                                    ? "pm-lozenge-warning"
                                    : "pm-lozenge-success"
                              }`}
                            >
                              {formatPct(tx.error_rate)}
                            </span>
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>

                {txPage && txPage.total_pages > 1 && (
                  <div className="px-6 py-3 border-t border-[var(--color-border)] flex items-center justify-between text-xs text-[var(--color-text-dim)]">
                    <span>
                      Page {txPage.page} of {txPage.total_pages} ({txPage.total}{" "}
                      transactions)
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
                        disabled={page >= txPage.total_pages}
                        onClick={() => setPage((p) => p + 1)}
                        className="h-7 px-2.5 rounded border border-[var(--color-border-strong)] disabled:opacity-40 cursor-pointer"
                      >
                        Next
                      </button>
                    </div>
                  </div>
                )}
              </div>
            )
          ) : traces.length === 0 ? (
            <div className="p-10 text-center text-xs text-[var(--color-text-dim)]">
              No sampled traces found for this filter.
            </div>
          ) : (
            <table className="w-full text-left border-collapse text-xs">
              <thead className="sticky top-0 bg-[var(--color-bg-sidebar)] border-b border-[var(--color-border)] text-[11px] text-[var(--color-text-dim)]">
                <tr>
                  <th className="py-2.5 pl-6 pr-3 font-semibold">
                    Transaction / Trace ID
                  </th>
                  <th className="py-2.5 px-3 font-semibold">Project</th>
                  <th className="py-2.5 px-3 font-semibold">Sample</th>
                  <th className="py-2.5 px-3 font-semibold text-right">
                    Spans
                  </th>
                  <th className="py-2.5 px-3 font-semibold text-right">
                    Duration
                  </th>
                  <th className="py-2.5 pr-6 pl-3 font-semibold text-right">
                    Seen
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-[var(--color-border)]">
                {traces.map((tr) => (
                  <tr
                    key={`${tr.event_id}:${tr.trace_id}`}
                    onClick={() => setActiveTraceId(tr.trace_id)}
                    className="hover:bg-[var(--color-bg-hover)] cursor-pointer"
                  >
                    <td className="py-2.5 pl-6 pr-3 max-w-[340px]">
                      <div className="font-semibold text-[var(--color-text)] truncate">
                        {tr.transaction_name}
                      </div>
                      <div className="text-[11px] pm-mono text-[var(--color-link)] truncate">
                        {tr.trace_id}
                      </div>
                    </td>
                    <td className="py-2.5 px-3">
                      <span className="pm-lozenge">{tr.project_slug}</span>
                    </td>
                    <td className="py-2.5 px-3">
                      <span
                        className={`pm-lozenge ${
                          tr.sample_reason === "error"
                            ? "pm-lozenge-danger"
                            : tr.sample_reason === "slow"
                              ? "pm-lozenge-warning"
                              : ""
                        }`}
                      >
                        {tr.sample_reason.toUpperCase()}
                      </span>
                    </td>
                    <td className="py-2.5 px-3 text-right pm-mono">
                      {tr.span_count}
                    </td>
                    <td className="py-2.5 px-3 text-right pm-mono font-semibold">
                      {formatMs(tr.duration_ms)}
                    </td>
                    <td className="py-2.5 pr-6 pl-3 text-right text-[var(--color-text-dim)]">
                      {formatRelativeTime(tr.received_at)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>

        {/* Right Transaction Detail & Recent Sampled Traces Inspector */}
        {activeTab === "transactions" && selectedTx && (
          <aside className="w-[400px] shrink-0 border-l border-[var(--color-border)] bg-[var(--color-bg-sidebar)] flex flex-col overflow-hidden">
            <div className="px-4 py-3 border-b border-[var(--color-border)] bg-[var(--color-bg)] flex items-start justify-between gap-2">
              <div className="min-w-0">
                <div className="text-[11px] pm-mono text-[var(--color-text-dim)]">
                  {selectedTx.project_slug} · {selectedTx.op}
                </div>
                <h2 className="text-sm font-bold text-[var(--color-text)] break-all">
                  {selectedTx.transaction_name}
                </h2>
              </div>
              <button
                type="button"
                onClick={() => setSelectedTx(null)}
                className="p-1 rounded hover:bg-[var(--color-bg-hover)] text-[var(--color-text-dim)] cursor-pointer"
                aria-label="Close transaction detail"
              >
                <X size={15} />
              </button>
            </div>

            <div className="flex-1 overflow-y-auto p-4 space-y-4 text-xs">
              <div className="grid grid-cols-3 gap-2">
                <div className="p-2.5 rounded border border-[var(--color-border)] bg-[var(--color-bg)]">
                  <div className="text-[10px] uppercase text-[var(--color-text-dim)]">
                    p50
                  </div>
                  <div className="text-sm font-bold pm-mono">
                    {formatMs(selectedTx.p50_ms)}
                  </div>
                </div>
                <div className="p-2.5 rounded border border-[var(--color-border)] bg-[var(--color-bg)]">
                  <div className="text-[10px] uppercase text-[var(--color-text-dim)]">
                    p95
                  </div>
                  <div className="text-sm font-bold pm-mono text-[#974f0c]">
                    {formatMs(selectedTx.p95_ms)}
                  </div>
                </div>
                <div className="p-2.5 rounded border border-[var(--color-border)] bg-[var(--color-bg)]">
                  <div className="text-[10px] uppercase text-[var(--color-text-dim)]">
                    p99
                  </div>
                  <div className="text-sm font-bold pm-mono">
                    {formatMs(selectedTx.p99_ms)}
                  </div>
                </div>
              </div>

              {/* Mini p95 Latency Trend Bars */}
              <div className="p-3 rounded border border-[var(--color-border)] bg-[var(--color-bg)]">
                <div className="flex items-center justify-between mb-2">
                  <span className="text-[11px] font-bold uppercase tracking-wide text-[var(--color-text-dim)]">
                    p95 Latency Trend ({txDetail?.interval ?? "1m"})
                  </span>
                  <span className="text-[11px] pm-mono text-[var(--color-text-dim)]">
                    avg {formatMs(selectedTx.avg_ms)}
                  </span>
                </div>
                {detailLoading ? (
                  <div className="h-20 flex items-center justify-center text-[11px] text-[var(--color-text-dim)]">
                    Loading series...
                  </div>
                ) : txDetail && txDetail.series.length > 0 ? (
                  <div className="h-20 flex items-end gap-1">
                    {(() => {
                      const maxVal = Math.max(
                        1,
                        ...txDetail.series.map((b) => b.p95_ms),
                      );
                      return txDetail.series.slice(-36).map((b) => {
                        const hPct = Math.max(8, (b.p95_ms / maxVal) * 100);
                        return (
                          <div
                            key={b.bucket}
                            className="flex-1 rounded-t-[2px] transition-all"
                            style={{
                              height: `${hPct}%`,
                              backgroundColor:
                                b.error_count > 0 ? "#C9372C" : "#0C66E4",
                            }}
                            title={`${new Date(b.bucket).toLocaleTimeString()} · p50 ${formatMs(b.p50_ms)} · p95 ${formatMs(b.p95_ms)} · ${b.total_count} req (${b.error_count} err)`}
                          />
                        );
                      });
                    })()}
                  </div>
                ) : (
                  <div className="h-16 flex items-center justify-center text-[11px] text-[var(--color-text-dim)]">
                    No time-series buckets available
                  </div>
                )}
              </div>

              {/* Recent Sampled Traces */}
              <div>
                <div className="text-[11px] font-bold uppercase tracking-wide text-[var(--color-text-dim)] mb-2 flex items-center gap-1.5">
                  <Flame size={12} />
                  <span>Sampled Traces (Click for Waterfall)</span>
                </div>
                {detailLoading ? (
                  <div className="text-[11px] text-[var(--color-text-dim)]">
                    Loading sampled traces...
                  </div>
                ) : !txDetail || txDetail.recent_traces.length === 0 ? (
                  <div className="p-3 rounded border border-[var(--color-border)] bg-[var(--color-bg)] text-[11px] text-[var(--color-text-dim)]">
                    No sampled full traces stored for this transaction in the
                    selected window.
                  </div>
                ) : (
                  <div className="space-y-1.5">
                    {txDetail.recent_traces.map((tr) => (
                      <button
                        key={tr.event_id}
                        type="button"
                        onClick={() => setActiveTraceId(tr.trace_id)}
                        className="w-full text-left p-2.5 rounded border border-[var(--color-border)] bg-[var(--color-bg)] hover:border-[var(--color-primary)] transition flex items-center justify-between gap-2 cursor-pointer"
                      >
                        <div className="min-w-0">
                          <div className="flex items-center gap-1.5">
                            <span
                              className={`pm-lozenge ${
                                tr.sample_reason === "error"
                                  ? "pm-lozenge-danger"
                                  : tr.sample_reason === "slow"
                                    ? "pm-lozenge-warning"
                                    : ""
                              }`}
                            >
                              {tr.sample_reason.toUpperCase()}
                            </span>
                            <span className="pm-mono font-bold text-[var(--color-text)]">
                              {formatMs(tr.duration_ms)}
                            </span>
                            <span className="text-[11px] text-[var(--color-text-dim)]">
                              ({tr.span_count} spans)
                            </span>
                          </div>
                          <div className="mt-1 text-[11px] pm-mono text-[var(--color-link)] truncate">
                            {tr.trace_id}
                          </div>
                        </div>
                        <div className="flex items-center gap-1 text-[11px] text-[var(--color-text-dim)] shrink-0">
                          <Clock size={11} />
                          <span>{formatRelativeTime(tr.received_at)}</span>
                          <ChevronRight size={13} />
                        </div>
                      </button>
                    ))}
                  </div>
                )}
              </div>
            </div>
          </aside>
        )}
      </div>
        </>
      )}
        </>
      )}
    </div>
  );
}

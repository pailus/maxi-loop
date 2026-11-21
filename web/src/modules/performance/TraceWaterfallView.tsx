import { useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  ArrowLeft,
  Clock,
  Copy,
  Flame,
  GitBranch,
  Layers,
  X,
} from "lucide-react";
import {
  type TraceWaterfallView,
  type WaterfallSpanNode,
  fetchTraceWaterfall,
} from "../../lib/api";

interface TraceWaterfallViewProps {
  traceId: string;
  onBack: () => void;
  onShowToast: (message: string) => void;
}

const SERVICE_COLORS = [
  "#0C66E4",
  "#22A06B",
  "#8270DB",
  "#D97706",
  "#0E7490",
  "#C9372C",
];

function formatDuration(ms: number): string {
  if (ms >= 1000) return `${(ms / 1000).toFixed(2)}s`;
  if (ms >= 10) return `${ms.toFixed(0)}ms`;
  return `${ms.toFixed(1)}ms`;
}

export function TraceWaterfallView({
  traceId,
  onBack,
  onShowToast,
}: TraceWaterfallViewProps) {
  const [trace, setTrace] = useState<TraceWaterfallView | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedSpan, setSelectedSpan] = useState<WaterfallSpanNode | null>(
    null,
  );
  const [onlySlowOrErr, setOnlySlowOrErr] = useState(false);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setError(null);
    setSelectedSpan(null);

    fetchTraceWaterfall(traceId)
      .then((data) => {
        if (!active) return;
        setTrace(data);
        if (data.spans.length > 0) {
          const slow = data.spans.find((s) => s.is_slow) ?? data.spans[0];
          setSelectedSpan(slow);
        }
      })
      .catch((err) => {
        if (!active) return;
        setError(err instanceof Error ? err.message : "Failed to load trace");
      })
      .finally(() => {
        if (active) setLoading(false);
      });

    return () => {
      active = false;
    };
  }, [traceId]);

  const serviceColorMap = useMemo(() => {
    const map = new Map<string, string>();
    (trace?.projects ?? []).forEach((proj, idx) => {
      map.set(proj, SERVICE_COLORS[idx % SERVICE_COLORS.length]);
    });
    return map;
  }, [trace]);

  const visibleSpans = useMemo(() => {
    if (!trace) return [];
    if (!onlySlowOrErr) return trace.spans;
    return trace.spans.filter(
      (s) => s.is_slow || s.is_error || s.is_transaction_root,
    );
  }, [trace, onlySlowOrErr]);

  async function copyTraceId() {
    try {
      await navigator.clipboard.writeText(traceId);
      onShowToast("Trace ID copied to clipboard");
    } catch {
      onShowToast("Failed to copy Trace ID");
    }
  }

  return (
    <div className="flex-1 min-w-0 flex flex-col overflow-hidden bg-[var(--color-bg)]">
      {/* Header bar */}
      <div className="px-5 py-3.5 border-b border-[var(--color-border)] bg-[var(--color-bg)] flex items-center gap-3 shrink-0">
        <button
          type="button"
          onClick={onBack}
          className="h-8 px-2.5 rounded border border-[var(--color-border-strong)] bg-[var(--color-bg)] hover:bg-[var(--color-bg-hover)] text-xs font-medium text-[var(--color-text)] inline-flex items-center gap-1.5 cursor-pointer"
        >
          <ArrowLeft size={14} />
          <span>Back to Performance</span>
        </button>

        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2 flex-wrap">
            <h1 className="text-sm font-bold text-[var(--color-text)] truncate">
              {trace ? trace.root_transaction : "Distributed Trace Waterfall"}
            </h1>
            {trace?.is_error ? (
              <span className="pm-lozenge pm-lozenge-danger">ERROR</span>
            ) : (
              <span className="pm-lozenge pm-lozenge-success">OK</span>
            )}
            {trace?.sample_reasons.map((reason) => (
              <span
                key={reason}
                className={`pm-lozenge ${
                  reason === "error"
                    ? "pm-lozenge-danger"
                    : reason === "slow"
                      ? "pm-lozenge-warning"
                      : ""
                }`}
              >
                SAMPLE: {reason.toUpperCase()}
              </span>
            ))}
          </div>
          <div className="mt-0.5 flex items-center gap-3 text-[11px] text-[var(--color-text-dim)] flex-wrap">
            <button
              type="button"
              onClick={() => void copyTraceId()}
              className="pm-mono inline-flex items-center gap-1 hover:text-[var(--color-primary)] cursor-pointer"
              title="Copy full trace ID"
            >
              <span>trace_id: {traceId}</span>
              <Copy size={11} />
            </button>
            {trace && (
              <>
                <span className="inline-flex items-center gap-1">
                  <Clock size={11} />
                  <span>Total: {formatDuration(trace.total_duration_ms)}</span>
                </span>
                <span className="inline-flex items-center gap-1">
                  <Layers size={11} />
                  <span>{trace.span_count} spans</span>
                </span>
                <span className="inline-flex items-center gap-1">
                  <GitBranch size={11} />
                  <span>
                    {trace.services_count}{" "}
                    {trace.services_count === 1 ? "service" : "services"}
                  </span>
                </span>
              </>
            )}
          </div>
        </div>

        <label className="inline-flex items-center gap-1.5 text-xs text-[var(--color-text-muted)] select-none cursor-pointer">
          <input
            type="checkbox"
            checked={onlySlowOrErr}
            onChange={(e) => setOnlySlowOrErr(e.target.checked)}
            className="rounded border-[var(--color-border-strong)]"
          />
          <span>Slow &amp; errors only</span>
        </label>
      </div>

      {loading ? (
        <div className="flex-1 flex items-center justify-center text-xs text-[var(--color-text-dim)]">
          Decompressing zstd trace &amp; assembling waterfall...
        </div>
      ) : error || !trace ? (
        <div className="p-6 text-xs text-[var(--color-danger)]">
          {error ?? "Trace not found"}
        </div>
      ) : (
        <div className="flex-1 min-h-0 flex overflow-hidden">
          {/* Waterfall Main Area */}
          <div className="flex-1 min-w-0 flex flex-col overflow-hidden">
            {/* Services Legend + Slowest Spans Strip */}
            <div className="px-5 py-2.5 bg-[var(--color-bg-muted)] border-b border-[var(--color-border)] flex flex-wrap items-center gap-3 text-xs shrink-0">
              <div className="flex items-center gap-1.5 flex-wrap">
                <span className="text-[11px] font-semibold uppercase tracking-wide text-[var(--color-text-dim)]">
                  Services:
                </span>
                {trace.projects.map((proj) => (
                  <span
                    key={proj}
                    className="inline-flex items-center gap-1.5 px-2 py-0.5 rounded bg-[var(--color-bg)] border border-[var(--color-border)] text-[11px] font-medium text-[var(--color-text)]"
                  >
                    <span
                      className="w-2 h-2 rounded-full shrink-0"
                      style={{
                        backgroundColor:
                          serviceColorMap.get(proj) ?? SERVICE_COLORS[0],
                      }}
                    />
                    <span>{proj}</span>
                  </span>
                ))}
              </div>

              {trace.slowest_spans.length > 0 && (
                <div className="ml-auto flex items-center gap-2 flex-wrap">
                  <span className="inline-flex items-center gap-1 text-[11px] font-semibold text-[#974f0c]">
                    <Flame size={12} />
                    <span>Slowest:</span>
                  </span>
                  {trace.slowest_spans.slice(0, 3).map((s) => (
                    <button
                      key={s.span_id}
                      type="button"
                      onClick={() => {
                        const match = trace.spans.find(
                          (sp) => sp.span_id === s.span_id,
                        );
                        if (match) setSelectedSpan(match);
                      }}
                      className="px-2 py-0.5 rounded bg-[#fff7d6] border border-[#f5cd47] text-[11px] text-[#533f04] hover:brightness-95 cursor-pointer truncate max-w-[220px]"
                      title={`${s.op}: ${s.description} (${formatDuration(s.duration_ms)})`}
                    >
                      <span className="font-semibold">{s.op}</span>{" "}
                      <span className="pm-mono">
                        {formatDuration(s.duration_ms)}
                      </span>
                    </button>
                  ))}
                </div>
              )}
            </div>

            {/* Timeline Column Headers */}
            <div className="grid grid-cols-[minmax(300px,42%)_1fr] px-5 py-2 border-b border-[var(--color-border)] bg-[var(--color-bg-sidebar)] text-[11px] font-semibold text-[var(--color-text-dim)] shrink-0">
              <div>Operation &amp; Span Hierarchy</div>
              <div className="flex justify-between pm-mono">
                <span>0ms</span>
                <span>{formatDuration(trace.total_duration_ms * 0.25)}</span>
                <span>{formatDuration(trace.total_duration_ms * 0.5)}</span>
                <span>{formatDuration(trace.total_duration_ms * 0.75)}</span>
                <span>{formatDuration(trace.total_duration_ms)}</span>
              </div>
            </div>

            {/* Waterfall Rows */}
            <div className="flex-1 overflow-y-auto divide-y divide-[var(--color-border)]">
              {visibleSpans.map((span) => {
                const total = Math.max(trace.total_duration_ms, 0.01);
                const leftPct = Math.min(
                  99,
                  Math.max(0, (span.offset_ms / total) * 100),
                );
                const widthPct = Math.max(
                  0.8,
                  Math.min(100 - leftPct, (span.duration_ms / total) * 100),
                );
                const isSelected = selectedSpan?.span_id === span.span_id;
                const barColor = span.is_error
                  ? "#C9372C"
                  : span.is_slow
                    ? "#D97706"
                    : (serviceColorMap.get(span.project_slug) ?? "#0C66E4");

                return (
                  <button
                    key={span.span_id}
                    type="button"
                    onClick={() => setSelectedSpan(span)}
                    className={`w-full text-left grid grid-cols-[minmax(300px,42%)_1fr] items-center px-5 py-2 text-xs transition cursor-pointer ${
                      isSelected
                        ? "bg-[var(--color-primary-light)]"
                        : "hover:bg-[var(--color-bg-hover)]"
                    }`}
                  >
                    {/* Left Hierarchy Cell */}
                    <div
                      className="min-w-0 pr-3 flex items-center gap-1.5"
                      style={{
                        paddingLeft: `${Math.min(span.depth, 8) * 16}px`,
                      }}
                    >
                      <span
                        className="w-2 h-2 rounded-full shrink-0"
                        style={{
                          backgroundColor:
                            serviceColorMap.get(span.project_slug) ??
                            SERVICE_COLORS[0],
                        }}
                        title={span.project_slug}
                      />
                      <span className="px-1.5 py-0.5 rounded bg-[var(--color-bg-muted)] border border-[var(--color-border)] text-[10px] font-semibold pm-mono text-[var(--color-text-muted)] shrink-0">
                        {span.op}
                      </span>
                      <span className="truncate text-[var(--color-text)] font-medium">
                        {span.description || span.span_id}
                      </span>
                      {span.is_slow && (
                        <span
                          className="shrink-0 text-[#974f0c]"
                          title="Slowest span bottleneck"
                        >
                          <Flame size={12} />
                        </span>
                      )}
                      {span.is_error && (
                        <span
                          className="shrink-0 text-[var(--color-danger)]"
                          title={`Error: ${span.status}`}
                        >
                          <AlertTriangle size={12} />
                        </span>
                      )}
                    </div>

                    {/* Right Gantt Bar Cell */}
                    <div className="relative h-6 flex items-center">
                      {/* Quarter gridlines */}
                      <div className="absolute inset-0 pointer-events-none flex justify-between opacity-40">
                        <span className="border-l border-dashed border-[var(--color-border-strong)] h-full" />
                        <span className="border-l border-dashed border-[var(--color-border-strong)] h-full" />
                        <span className="border-l border-dashed border-[var(--color-border-strong)] h-full" />
                        <span className="border-l border-dashed border-[var(--color-border-strong)] h-full" />
                        <span className="border-l border-dashed border-[var(--color-border-strong)] h-full" />
                      </div>

                      <div
                        className="absolute h-4 rounded-[3px] transition-all"
                        style={{
                          left: `${leftPct}%`,
                          width: `${widthPct}%`,
                          backgroundColor: barColor,
                          opacity: span.is_transaction_root ? 0.95 : 0.82,
                        }}
                        title={`${span.op} · ${formatDuration(span.duration_ms)} (offset +${formatDuration(span.offset_ms)})`}
                      />
                      <span
                        className="absolute text-[10px] pm-mono font-semibold text-[var(--color-text-muted)] whitespace-nowrap"
                        style={{
                          left:
                            leftPct + widthPct < 82
                              ? `calc(${leftPct + widthPct}% + 6px)`
                              : undefined,
                          right:
                            leftPct + widthPct >= 82
                              ? `calc(${100 - leftPct}% + 6px)`
                              : undefined,
                        }}
                      >
                        {formatDuration(span.duration_ms)}
                      </span>
                    </div>
                  </button>
                );
              })}
            </div>
          </div>

          {/* Right Span Inspector Drawer */}
          {selectedSpan && (
            <aside className="w-[340px] shrink-0 border-l border-[var(--color-border)] bg-[var(--color-bg-sidebar)] flex flex-col overflow-hidden">
              <div className="px-4 py-3 border-b border-[var(--color-border)] flex items-center justify-between bg-[var(--color-bg)]">
                <div className="min-w-0">
                  <div className="text-xs font-bold text-[var(--color-text)] truncate">
                    Span Inspector
                  </div>
                  <div className="text-[11px] pm-mono text-[var(--color-text-dim)] truncate">
                    {selectedSpan.span_id}
                  </div>
                </div>
                <button
                  type="button"
                  onClick={() => setSelectedSpan(null)}
                  className="p-1 rounded hover:bg-[var(--color-bg-hover)] text-[var(--color-text-dim)] cursor-pointer"
                  aria-label="Close span inspector"
                >
                  <X size={14} />
                </button>
              </div>

              <div className="flex-1 overflow-y-auto p-4 space-y-4 text-xs">
                <div className="space-y-1">
                  <div className="flex items-center gap-1.5 flex-wrap">
                    <span className="pm-lozenge">{selectedSpan.op}</span>
                    <span className="pm-lozenge">
                      {selectedSpan.project_slug}
                    </span>
                    {selectedSpan.is_slow && (
                      <span className="pm-lozenge pm-lozenge-warning">
                        SLOW
                      </span>
                    )}
                    {selectedSpan.is_error && (
                      <span className="pm-lozenge pm-lozenge-danger">
                        {selectedSpan.status.toUpperCase()}
                      </span>
                    )}
                  </div>
                  <div className="pt-1 font-medium text-[var(--color-text)] break-all">
                    {selectedSpan.description || "(no description)"}
                  </div>
                </div>

                <div className="grid grid-cols-2 gap-2">
                  <div className="p-2.5 rounded border border-[var(--color-border)] bg-[var(--color-bg)]">
                    <div className="text-[10px] uppercase text-[var(--color-text-dim)]">
                      Duration
                    </div>
                    <div className="text-sm font-bold pm-mono text-[var(--color-text)]">
                      {formatDuration(selectedSpan.duration_ms)}
                    </div>
                  </div>
                  <div className="p-2.5 rounded border border-[var(--color-border)] bg-[var(--color-bg)]">
                    <div className="text-[10px] uppercase text-[var(--color-text-dim)]">
                      Self Time
                    </div>
                    <div className="text-sm font-bold pm-mono text-[var(--color-text)]">
                      {formatDuration(selectedSpan.self_duration_ms)}
                    </div>
                  </div>
                  <div className="p-2.5 rounded border border-[var(--color-border)] bg-[var(--color-bg)]">
                    <div className="text-[10px] uppercase text-[var(--color-text-dim)]">
                      Start Offset
                    </div>
                    <div className="text-sm font-bold pm-mono text-[var(--color-text)]">
                      +{formatDuration(selectedSpan.offset_ms)}
                    </div>
                  </div>
                  <div className="p-2.5 rounded border border-[var(--color-border)] bg-[var(--color-bg)]">
                    <div className="text-[10px] uppercase text-[var(--color-text-dim)]">
                      % of Trace
                    </div>
                    <div className="text-sm font-bold pm-mono text-[var(--color-text)]">
                      {selectedSpan.pct_of_trace.toFixed(1)}%
                    </div>
                  </div>
                </div>

                <div className="space-y-1.5 text-[11px] text-[var(--color-text-muted)]">
                  <div className="flex justify-between gap-2">
                    <span className="text-[var(--color-text-dim)]">Status</span>
                    <span className="pm-mono font-medium">
                      {selectedSpan.status}
                    </span>
                  </div>
                  <div className="flex justify-between gap-2">
                    <span className="text-[var(--color-text-dim)]">
                      Environment
                    </span>
                    <span className="pm-mono">{selectedSpan.environment}</span>
                  </div>
                  {selectedSpan.release && (
                    <div className="flex justify-between gap-2">
                      <span className="text-[var(--color-text-dim)]">
                        Release
                      </span>
                      <span className="pm-mono truncate">
                        {selectedSpan.release}
                      </span>
                    </div>
                  )}
                  {selectedSpan.parent_span_id && (
                    <div className="flex justify-between gap-2">
                      <span className="text-[var(--color-text-dim)]">
                        Parent Span
                      </span>
                      <span className="pm-mono">
                        {selectedSpan.parent_span_id}
                      </span>
                    </div>
                  )}
                </div>

                {selectedSpan.tags &&
                  Object.keys(selectedSpan.tags).length > 0 && (
                    <div>
                      <div className="text-[11px] font-bold uppercase tracking-wide text-[var(--color-text-dim)] mb-1.5">
                        Tags
                      </div>
                      <pre className="p-2.5 rounded border border-[var(--color-border)] bg-[var(--color-bg)] text-[11px] pm-mono overflow-x-auto max-h-40">
                        {JSON.stringify(selectedSpan.tags, null, 2)}
                      </pre>
                    </div>
                  )}

                {selectedSpan.data &&
                  Object.keys(selectedSpan.data).length > 0 && (
                    <div>
                      <div className="text-[11px] font-bold uppercase tracking-wide text-[var(--color-text-dim)] mb-1.5">
                        Span Data (PII Scrubbed)
                      </div>
                      <pre className="p-2.5 rounded border border-[var(--color-border)] bg-[var(--color-bg)] text-[11px] pm-mono overflow-x-auto max-h-56">
                        {JSON.stringify(selectedSpan.data, null, 2)}
                      </pre>
                    </div>
                  )}
              </div>
            </aside>
          )}
        </div>
      )}
    </div>
  );
}

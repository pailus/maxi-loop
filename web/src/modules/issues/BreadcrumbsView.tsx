import { useMemo, useState } from "react";
import {
  Activity,
  ChevronDown,
  ChevronRight,
  Compass,
  Database,
  Globe,
  MousePointerClick,
  Search,
  Terminal,
} from "lucide-react";
import type { BreadcrumbView } from "../../lib/api";

interface BreadcrumbsViewProps {
  breadcrumbs?: BreadcrumbView[];
  compact?: boolean;
}

function breadcrumbIcon(type: string, category: string) {
  const key = `${type} ${category}`.toLowerCase();
  if (key.includes("http") || key.includes("fetch") || key.includes("xhr")) {
    return <Globe size={13} className="text-[var(--color-primary)] shrink-0" />;
  }
  if (key.includes("nav") || key.includes("route")) {
    return <Compass size={13} className="text-[#216e4e] shrink-0" />;
  }
  if (key.includes("db") || key.includes("query") || key.includes("sql")) {
    return <Database size={13} className="text-[#5E4DB2] shrink-0" />;
  }
  if (key.includes("ui") || key.includes("click") || key.includes("dom")) {
    return <MousePointerClick size={13} className="text-[#a54800] shrink-0" />;
  }
  if (key.includes("console") || key.includes("log")) {
    return <Terminal size={13} className="text-[var(--color-text-muted)] shrink-0" />;
  }
  return <Activity size={13} className="text-[var(--color-text-dim)] shrink-0" />;
}

function levelLozengeClass(level: string): string {
  switch (level.toLowerCase()) {
    case "fatal":
    case "error":
      return "pm-lozenge-danger";
    case "warning":
    case "warn":
      return "pm-lozenge-warning";
    case "info":
      return "pm-lozenge-primary";
    default:
      return "";
  }
}

function formatCrumbTime(ts?: string): string {
  if (!ts) return "—";
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  return d.toLocaleTimeString("en-GB", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    fractionalSecondDigits: 3,
  });
}

function summarizeData(data: unknown): string {
  if (!data || typeof data !== "object") {
    return data !== undefined ? String(data) : "";
  }
  const entries = Object.entries(data as Record<string, unknown>);
  if (entries.length === 0) return "";
  return entries
    .slice(0, 4)
    .map(([k, v]) => `${k}=${typeof v === "string" ? v : JSON.stringify(v)}`)
    .join(" · ");
}

export function BreadcrumbsView({
  breadcrumbs = [],
  compact = false,
}: BreadcrumbsViewProps) {
  const [search, setSearch] = useState("");
  const [levelFilter, setLevelFilter] = useState<string>("all");
  const [expandedRows, setExpandedRows] = useState<Record<number, boolean>>({});

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    return breadcrumbs.filter((b) => {
      if (levelFilter !== "all" && b.level.toLowerCase() !== levelFilter) {
        return false;
      }
      if (!q) return true;
      const hay = [
        b.category,
        b.type,
        b.level,
        b.message ?? "",
        b.data ? JSON.stringify(b.data) : "",
      ]
        .join(" ")
        .toLowerCase();
      return hay.includes(q);
    });
  }, [breadcrumbs, search, levelFilter]);

  const visible = compact ? filtered.slice(-4) : filtered;

  if (breadcrumbs.length === 0) {
    return (
      <div className="rounded-md bg-[var(--color-bg-muted)] border border-[var(--color-border)] p-3.5 text-xs text-[var(--color-text-dim)] flex items-center gap-2">
        <Activity size={14} />
        <span>No breadcrumbs recorded on this event.</span>
      </div>
    );
  }

  return (
    <div className="space-y-2.5">
      {!compact && (
        <div className="flex items-center justify-between gap-2 flex-wrap">
          <div className="flex items-center gap-2">
            <h3 className="text-sm font-semibold text-[var(--color-text)]">
              Breadcrumbs
            </h3>
            <span className="px-1.5 py-0.5 rounded-full bg-[var(--color-bg-hover-strong)] text-[11px] font-bold text-[var(--color-text-muted)]">
              {filtered.length} / {breadcrumbs.length}
            </span>
          </div>

          <div className="flex items-center gap-1.5 flex-wrap">
            <div className="relative w-44">
              <Search
                size={12}
                className="absolute left-2 top-1/2 -translate-y-1/2 text-[var(--pm-text-faint)] pointer-events-none"
              />
              <input
                type="text"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder="Filter breadcrumbs..."
                aria-label="Filter breadcrumbs"
                className="w-full h-7 pl-4 md:pl-6 pr-2 rounded border border-[var(--color-border)] bg-[var(--color-bg)] text-xs text-[var(--color-text)] outline-none focus:border-[var(--color-primary)]"
              />
            </div>

            <select
              value={levelFilter}
              onChange={(e) => setLevelFilter(e.target.value)}
              aria-label="Filter breadcrumbs by level"
              className="h-7 px-2 rounded bg-[var(--pm-col)] text-xs font-medium text-[var(--color-text-muted)] border-0 cursor-pointer"
            >
              <option value="all">Level: All</option>
              <option value="error">Error</option>
              <option value="warning">Warning</option>
              <option value="info">Info</option>
              <option value="debug">Debug</option>
            </select>
          </div>
        </div>
      )}

      {visible.length === 0 ? (
        <div className="rounded-md bg-[var(--color-bg-muted)] border border-[var(--color-border)] p-3 text-xs text-[var(--color-text-dim)]">
          No breadcrumbs match the current filter.
        </div>
      ) : (
        <div className="rounded-md border border-[var(--color-border)] bg-[var(--color-bg-card)] divide-y divide-[var(--color-border-light)] overflow-hidden">
          {visible.map((crumb, idx) => {
            const hasData = crumb.data !== undefined && crumb.data !== null;
            const expanded = Boolean(expandedRows[idx]);
            const preview = crumb.message || summarizeData(crumb.data);

            return (
              <div key={`${crumb.timestamp ?? idx}-${crumb.category}-${idx}`} className="text-xs">
                <div
                  role={hasData && !compact ? "button" : undefined}
                  tabIndex={hasData && !compact ? 0 : undefined}
                  onClick={() => {
                    if (hasData && !compact) {
                      setExpandedRows((prev) => ({ ...prev, [idx]: !prev[idx] }));
                    }
                  }}
                  onKeyDown={(e) => {
                    if (hasData && !compact && (e.key === "Enter" || e.key === " ")) {
                      e.preventDefault();
                      setExpandedRows((prev) => ({ ...prev, [idx]: !prev[idx] }));
                    }
                  }}
                  className={`px-3 py-2 flex items-start gap-2.5 ${
                    hasData && !compact ? "hover:bg-[var(--pm-card-hover)] cursor-pointer" : ""
                  }`}
                >
                  <div className="mt-0.5 flex items-center gap-1 shrink-0">
                    {!compact && hasData ? (
                      expanded ? (
                        <ChevronDown size={12} className="text-[var(--color-text-dim)]" />
                      ) : (
                        <ChevronRight size={12} className="text-[var(--color-text-dim)]" />
                      )
                    ) : null}
                    {breadcrumbIcon(crumb.type, crumb.category)}
                  </div>

                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-1.5 flex-wrap">
                      <span className="pm-mono font-semibold text-[var(--color-text)]">
                        {crumb.category}
                      </span>
                      {crumb.type !== "default" && crumb.type !== crumb.category && (
                        <span className="pm-lozenge">{crumb.type}</span>
                      )}
                      <span className={`pm-lozenge ${levelLozengeClass(crumb.level)}`}>
                        {crumb.level}
                      </span>
                    </div>
                    {preview && (
                      <p className="mt-0.5 pm-mono text-[11px] text-[var(--color-text-muted)] break-words">
                        {preview}
                      </p>
                    )}
                  </div>

                  <span className="pm-mono text-[11px] text-[var(--color-text-dim)] shrink-0">
                    {formatCrumbTime(crumb.timestamp)}
                  </span>
                </div>

                {!compact && hasData && expanded && (
                  <div className="px-3 pb-2.5 pl-9">
                    <pre className="pm-mono text-[11px] leading-relaxed rounded bg-[var(--pm-col)] border border-[var(--color-border)] p-2 text-[var(--color-text)] overflow-x-auto whitespace-pre-wrap break-all">
                      {JSON.stringify(crumb.data, null, 2)}
                    </pre>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

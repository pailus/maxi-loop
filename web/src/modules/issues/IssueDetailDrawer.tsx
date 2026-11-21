import { useState } from "react";
import {
  AlertTriangle,
  Check,
  Copy,
  EyeOff,
  Maximize2,
  RotateCcw,
  ShieldCheck,
  X,
} from "lucide-react";
import type { IssueDetailView, IssueStatus } from "../../lib/api";
import { ProjectIcon } from "../projects/ProjectIcon";
import { OccurrenceChart } from "./OccurrenceChart";
import { StackTraceView } from "./StackTraceView";

interface IssueDetailDrawerProps {
  detail: IssueDetailView | null;
  loading: boolean;
  canTriage: boolean;
  triageBusy: boolean;
  onUpdateStatus: (issueId: number, status: IssueStatus) => void;
  onOpenFullView: () => void;
  onClose: () => void;
}

export function normalizeTagsList(tags: unknown): Array<[string, string]> {
  if (!tags) return [];
  if (Array.isArray(tags)) {
    return tags
      .filter(
        (pair): pair is [unknown, unknown] =>
          Array.isArray(pair) && pair.length >= 2,
      )
      .map(([k, v]) => [String(k), String(v)]);
  }
  if (typeof tags === "object") {
    return Object.entries(tags as Record<string, unknown>).map(([k, v]) => [
      k,
      typeof v === "string" ? v : JSON.stringify(v),
    ]);
  }
  return [];
}

export function formatShortDate(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString("en-GB", {
    day: "numeric",
    month: "short",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function formatRelativeAge(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const diffSec = Math.max(0, Math.floor((Date.now() - d.getTime()) / 1000));
  if (diffSec < 60) return `${diffSec}s ago`;
  const diffMin = Math.floor(diffSec / 60);
  if (diffMin < 60) return `${diffMin}m ago`;
  const diffHr = Math.floor(diffMin / 60);
  if (diffHr < 24) return `${diffHr}h ago`;
  const diffDays = Math.floor(diffHr / 24);
  return `${diffDays}d ago`;
}

export function IssueDetailDrawer({
  detail,
  loading,
  canTriage,
  triageBusy,
  onUpdateStatus,
  onOpenFullView,
  onClose,
}: IssueDetailDrawerProps) {
  const [interval, setInterval] = useState<"1h" | "1d">("1h");
  const [copiedId, setCopiedId] = useState(false);

  if (loading && !detail) {
    return (
      <aside className="w-full lg:w-[460px] xl:w-[480px] shrink-0 bg-[var(--color-bg-card)] pm-card-panel flex flex-col">
        <div className="h-12 px-4 border-b border-[var(--color-border)] flex items-center justify-between text-xs text-[var(--color-text-dim)]">
          <span>Loading issue inspector...</span>
          <button
            type="button"
            onClick={onClose}
            className="p-1 rounded hover:bg-[var(--color-bg-hover)] cursor-pointer"
            aria-label="Close inspector"
          >
            <X size={16} />
          </button>
        </div>
        <div className="p-5 space-y-4">
          <div className="h-6 w-3/4 rounded bg-[var(--pm-col)] animate-pulse" />
          <div className="h-28 rounded bg-[var(--pm-col)] animate-pulse" />
          <div className="h-36 rounded bg-[var(--pm-col)] animate-pulse" />
        </div>
      </aside>
    );
  }

  if (!detail) {
    return null;
  }

  const { issue, sample_event } = detail;
  const tags = normalizeTagsList(detail.tags);
  const issueKey = `${issue.project_slug.slice(0, 3).toUpperCase()}-${issue.id}`;

  function handleCopyEventId() {
    const text = sample_event?.event_id ?? issue.sample_event_id ?? issue.fingerprint;
    navigator.clipboard?.writeText(text).catch(() => {});
    setCopiedId(true);
    setTimeout(() => setCopiedId(false), 1600);
  }

  return (
    <aside className="w-full lg:w-[460px] xl:w-[480px] shrink-0 bg-[var(--color-bg-card)] pm-card-panel flex flex-col max-h-[calc(100vh-56px)] overflow-y-auto">
      {/* Drawer Header (.ph) */}
      <div className="h-12 px-4 border-b border-[var(--color-border)] flex items-center justify-between gap-2 shrink-0 sticky top-0 bg-[var(--color-bg-card)] z-10">
        <div className="flex items-center gap-2 min-w-0">
          <ProjectIcon slug={issue.project_slug} size={18} />
          <span className="text-xs font-semibold text-[var(--color-text-dim)] truncate">
            {issue.project_slug} / {issueKey}
          </span>
        </div>

        <div className="flex items-center gap-1.5 shrink-0">
          <button
            type="button"
            onClick={onOpenFullView}
            className="h-7 px-2.5 rounded border border-[var(--color-border)] bg-[var(--color-bg)] hover:bg-[var(--color-bg-hover)] text-xs font-medium text-[var(--color-text-muted)] inline-flex items-center gap-1.5 cursor-pointer"
          >
            <Maximize2 size={12} />
            <span>Full view</span>
          </button>
          <button
            type="button"
            onClick={onClose}
            className="w-7 h-7 rounded hover:bg-[var(--color-bg-hover)] text-[var(--color-text-dim)] inline-flex items-center justify-center cursor-pointer"
            aria-label="Close drawer"
          >
            <X size={16} />
          </button>
        </div>
      </div>

      {/* Drawer Body (.pb) */}
      <div className="p-5 space-y-4">
        <div>
          <h2 className="text-lg font-semibold text-[var(--color-text)] leading-snug break-words">
            {issue.title}
          </h2>
          <p className="mt-1 pm-mono text-xs text-[var(--color-text-muted)] truncate">
            {issue.culprit || "No culprit function"}
          </p>
        </div>

        {/* Triage Actions Row */}
        <div className="flex items-center gap-2 flex-wrap">
          <span
            className={`pm-lozenge py-1 px-2 text-xs ${
              issue.status === "resolved"
                ? "pm-lozenge-success"
                : issue.status === "ignored"
                  ? ""
                  : "pm-lozenge-primary"
            }`}
          >
            {issue.status}
          </span>

          {canTriage && (
            <>
              {issue.status !== "resolved" && (
                <button
                  type="button"
                  disabled={triageBusy}
                  onClick={() => onUpdateStatus(issue.id, "resolved")}
                  className="h-7 px-2.5 rounded border border-[#4bce97] bg-[var(--color-success-light)] hover:brightness-95 text-[var(--color-success-text)] text-xs font-semibold inline-flex items-center gap-1 cursor-pointer disabled:opacity-50"
                >
                  <Check size={13} />
                  <span>Resolve</span>
                </button>
              )}

              {issue.status !== "ignored" && (
                <button
                  type="button"
                  disabled={triageBusy}
                  onClick={() => onUpdateStatus(issue.id, "ignored")}
                  className="h-7 px-2.5 rounded bg-[var(--pm-col)] hover:bg-[var(--color-bg-hover)] text-[var(--color-text-muted)] text-xs font-medium inline-flex items-center gap-1 cursor-pointer disabled:opacity-50"
                >
                  <EyeOff size={13} />
                  <span>Ignore</span>
                </button>
              )}

              {issue.status !== "unresolved" && (
                <button
                  type="button"
                  disabled={triageBusy}
                  onClick={() => onUpdateStatus(issue.id, "unresolved")}
                  className="h-7 px-2.5 rounded border border-[var(--color-border)] bg-[var(--color-bg)] hover:bg-[var(--color-bg-hover)] text-[var(--color-text-muted)] text-xs font-medium inline-flex items-center gap-1 cursor-pointer disabled:opacity-50"
                >
                  <RotateCcw size={12} />
                  <span>Unresolve</span>
                </button>
              )}
            </>
          )}

          <button
            type="button"
            onClick={handleCopyEventId}
            className="h-7 px-2.5 rounded bg-[var(--pm-col)] hover:bg-[var(--color-bg-hover)] text-[var(--color-text-muted)] text-xs font-medium inline-flex items-center gap-1 cursor-pointer ml-auto"
          >
            {copiedId ? <Check size={12} /> : <Copy size={12} />}
            <span>{copiedId ? "Copied" : "Copy ID"}</span>
          </button>
        </div>

        {/* Regression Callout Banner */}
        {issue.is_regressed && (
          <div className="px-3 py-2 rounded bg-[var(--color-warning-light)] text-[var(--color-warning-text)] text-xs font-semibold flex items-center gap-2">
            <AlertTriangle size={14} className="shrink-0" />
            <span>
              Auto-reopened: regression detected
              {issue.last_release ? ` in release ${issue.last_release}` : ""} (
              {formatRelativeAge(issue.last_seen)})
            </span>
          </div>
        )}

        {/* Structured Fields Grid (.fields) */}
        <dl className="grid grid-cols-[110px_1fr] gap-y-2.5 gap-x-3 text-xs border border-[var(--color-border)] rounded-md p-3.5">
          <dt className="font-semibold text-[var(--color-text-dim)]">Level</dt>
          <dd>
            <span
              className={`pm-lozenge ${
                issue.level === "error" || issue.level === "fatal"
                  ? "pm-lozenge-danger"
                  : "pm-lozenge-warning"
              }`}
            >
              {issue.level}
            </span>
          </dd>

          <dt className="font-semibold text-[var(--color-text-dim)]">Environment</dt>
          <dd className="font-medium text-[var(--color-text)]">
            {issue.last_environment || "production"}
          </dd>

          <dt className="font-semibold text-[var(--color-text-dim)]">Release</dt>
          <dd className="pm-mono text-[var(--color-text)]">
            {issue.last_release ?? "—"}
          </dd>

          <dt className="font-semibold text-[var(--color-text-dim)]">Occurrences</dt>
          <dd className="font-medium text-[var(--color-text)]">
            {issue.event_count.toLocaleString()} events ({issue.stored_full_events} full stored)
          </dd>

          <dt className="font-semibold text-[var(--color-text-dim)]">First / Last</dt>
          <dd className="text-[var(--color-text-muted)]">
            {formatShortDate(issue.first_seen)} · {formatRelativeAge(issue.last_seen)}
          </dd>
        </dl>

        {/* 24h / 30d Mini Occurrence Chart */}
        <OccurrenceChart
          hourlySeries={detail.hourly_series}
          dailySeries={detail.daily_series}
          interval={interval}
          onIntervalChange={setInterval}
          compact
          isRegressed={issue.is_regressed}
        />

        {/* Top In-App Stack Frame Preview */}
        <div>
          <div className="flex items-center justify-between mb-1.5">
            <span className="text-xs font-bold text-[var(--color-text-muted)]">
              Top stack frame
            </span>
            <button
              type="button"
              onClick={onOpenFullView}
              className="text-xs font-medium text-[var(--color-link)] hover:underline cursor-pointer"
            >
              View full stack trace
            </button>
          </div>
          <StackTraceView rawFrames={detail.stack_frames} compact />
        </div>

        {/* Event Tags & PII Scrubbed Request Summary */}
        <div>
          <div className="flex items-center justify-between mb-1.5">
            <span className="text-xs font-bold text-[var(--color-text-muted)]">
              Event tags &amp; scrubbed sample
            </span>
            <span className="pm-lozenge pm-lozenge-success">
              <ShieldCheck size={11} />
              <span>PII: [Filtered]</span>
            </span>
          </div>

          {tags.length > 0 && (
            <div className="flex flex-wrap gap-1.5 mb-2.5">
              {tags.slice(0, 6).map(([k, v]) => (
                <span key={`${k}:${v}`} className="pm-lozenge pm-mono lowercase">
                  {k}: {v}
                </span>
              ))}
            </div>
          )}

          {sample_event && (
            <div className="rounded-md bg-[var(--color-bg-muted)] border border-[var(--color-border)] p-2.5">
              <div className="text-[11px] pm-mono text-[var(--color-text-dim)] mb-1.5">
                event_id: {sample_event.event_id} · platform: {sample_event.platform}
              </div>
              <pre className="pm-mono text-[11px] leading-relaxed text-[var(--color-text)] max-h-40 overflow-auto whitespace-pre-wrap break-all">
                {JSON.stringify(sample_event.payload, null, 2)}
              </pre>
            </div>
          )}
        </div>
      </div>
    </aside>
  );
}

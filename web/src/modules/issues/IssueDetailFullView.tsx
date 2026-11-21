import { useEffect, useState } from "react";
import {
  AlertTriangle,
  ArrowLeft,
  Check,
  EyeOff,
  RotateCcw,
  ShieldCheck,
} from "lucide-react";
import {
  type IssueDetailView,
  type IssueEventView,
  type IssueStatus,
  fetchIssueEvents,
} from "../../lib/api";
import { ProjectIcon } from "../projects/ProjectIcon";
import {
  formatRelativeAge,
  formatShortDate,
  normalizeTagsList,
} from "./IssueDetailDrawer";
import { OccurrenceChart } from "./OccurrenceChart";
import { StackTraceView } from "./StackTraceView";

interface IssueDetailFullViewProps {
  detail: IssueDetailView;
  canTriage: boolean;
  triageBusy: boolean;
  onUpdateStatus: (issueId: number, status: IssueStatus) => void;
  onBack: () => void;
}

export function IssueDetailFullView({
  detail,
  canTriage,
  triageBusy,
  onUpdateStatus,
  onBack,
}: IssueDetailFullViewProps) {
  const [activeTab, setActiveTab] = useState<"overview" | "events" | "counters">("overview");
  const [interval, setInterval] = useState<"1h" | "1d">("1h");
  const [events, setEvents] = useState<IssueEventView[]>([]);
  const [loadingEvents, setLoadingEvents] = useState(false);

  const { issue, sample_event } = detail;
  const tags = normalizeTagsList(detail.tags);
  const issueKey = `${issue.project_slug.slice(0, 3).toUpperCase()}-${issue.id}`;

  useEffect(() => {
    if (activeTab !== "events") return;
    let active = true;
    setLoadingEvents(true);
    fetchIssueEvents(issue.id, 50)
      .then((list) => {
        if (active) setEvents(list);
      })
      .catch(() => {})
      .finally(() => {
        if (active) setLoadingEvents(false);
      });
    return () => {
      active = false;
    };
  }, [activeTab, issue.id]);

  return (
    <div className="flex-1 min-w-0 bg-[var(--color-bg)] px-6 py-4 overflow-y-auto">
      {/* Breadcrumb */}
      <div className="flex items-center gap-2 text-xs text-[var(--color-text-dim)]">
        <button
          type="button"
          onClick={onBack}
          className="inline-flex items-center gap-1 text-[var(--color-link)] hover:underline font-medium cursor-pointer"
        >
          <ArrowLeft size={13} />
          <span>Issues</span>
        </button>
        <span>›</span>
        <ProjectIcon slug={issue.project_slug} size={15} />
        <span className="font-medium text-[var(--color-text-muted)]">
          {issue.project_slug}
        </span>
        <span>›</span>
        <span className="pm-mono font-semibold text-[var(--color-text)]">{issueKey}</span>
      </div>

      {/* Title & Triage Toolbar */}
      <div className="mt-2 flex flex-col lg:flex-row lg:items-start lg:justify-between gap-3">
        <div className="min-w-0">
          <h1 className="text-xl font-semibold text-[var(--color-text)] leading-snug break-words">
            {issue.title}
          </h1>
          <div className="mt-2 flex items-center gap-2 flex-wrap">
            <span
              className={`pm-lozenge ${
                issue.level === "error" || issue.level === "fatal"
                  ? "pm-lozenge-danger"
                  : "pm-lozenge-warning"
              }`}
            >
              {issue.level}
            </span>
            <span
              className={`pm-lozenge ${
                issue.status === "resolved"
                  ? "pm-lozenge-success"
                  : issue.status === "ignored"
                    ? ""
                    : "pm-lozenge-primary"
              }`}
            >
              {issue.status}
            </span>
            {issue.is_regressed && (
              <span className="pm-lozenge pm-lozenge-warning">
                <AlertTriangle size={11} />
                <span>
                  REGRESSED{issue.last_release ? ` IN ${issue.last_release}` : ""}
                </span>
              </span>
            )}
            <span className="pm-mono text-xs text-[var(--color-text-dim)]">
              {issue.culprit || "message fingerprint"} · fingerprint{" "}
              {issue.fingerprint.slice(0, 16)}
            </span>
          </div>
        </div>

        {canTriage && (
          <div className="flex items-center gap-2 shrink-0">
            <button
              type="button"
              disabled={triageBusy || issue.status === "resolved"}
              onClick={() => onUpdateStatus(issue.id, "resolved")}
              className="h-8 px-3 rounded bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-50 text-white text-xs font-semibold inline-flex items-center gap-1.5 cursor-pointer"
            >
              <Check size={14} />
              <span>Resolve</span>
            </button>
            <button
              type="button"
              disabled={triageBusy || issue.status === "ignored"}
              onClick={() => onUpdateStatus(issue.id, "ignored")}
              className="h-8 px-3 rounded bg-[var(--pm-col)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50 text-[var(--color-text-muted)] text-xs font-medium inline-flex items-center gap-1.5 cursor-pointer"
            >
              <EyeOff size={14} />
              <span>Ignore</span>
            </button>
            <button
              type="button"
              disabled={triageBusy || issue.status === "unresolved"}
              onClick={() => onUpdateStatus(issue.id, "unresolved")}
              className="h-8 px-3 rounded border border-[var(--color-border)] bg-[var(--color-bg)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50 text-[var(--color-text-muted)] text-xs font-medium inline-flex items-center gap-1.5 cursor-pointer"
            >
              <RotateCcw size={13} />
              <span>Unresolve</span>
            </button>
          </div>
        )}
      </div>

      {/* Horizontal MaxiBoard Tabs */}
      <nav className="mt-4 flex items-center gap-1 border-b-2 border-[var(--color-border)]">
        <button
          type="button"
          role="tab"
          aria-selected={activeTab === "overview"}
          onClick={() => setActiveTab("overview")}
          className="pm-tab"
        >
          Overview &amp; stack trace
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={activeTab === "events"}
          onClick={() => setActiveTab("events")}
          className="pm-tab"
        >
          <span>Stored events</span>
          <span className="px-1.5 py-0.5 rounded-full bg-[var(--color-bg-hover)] text-[11px] font-bold">
            {issue.stored_full_events} / 50 cap
          </span>
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={activeTab === "counters"}
          onClick={() => setActiveTab("counters")}
          className="pm-tab"
        >
          Hourly &amp; daily counters
        </button>
      </nav>

      {/* Top Row: Occurrence Chart + Issue Metadata Card */}
      <div className="mt-4 grid grid-cols-1 xl:grid-cols-[1fr_380px] gap-4">
        <section className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-card)] p-4">
          <OccurrenceChart
            hourlySeries={detail.hourly_series}
            dailySeries={detail.daily_series}
            interval={interval}
            onIntervalChange={setInterval}
            isRegressed={issue.is_regressed}
          />
        </section>

        <section className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-card)] p-4">
          <h2 className="text-xs font-bold text-[var(--color-text)] pb-2.5 border-b border-[var(--color-border-light)]">
            Issue details
          </h2>
          <dl className="mt-3 grid grid-cols-[112px_1fr] gap-y-2.5 gap-x-3 text-xs">
            <dt className="font-semibold text-[var(--color-text-dim)]">Project</dt>
            <dd className="flex items-center gap-1.5 font-medium text-[var(--color-text)]">
              <ProjectIcon slug={issue.project_slug} size={16} />
              <span>{issue.project_slug}</span>
            </dd>

            <dt className="font-semibold text-[var(--color-text-dim)]">Total events</dt>
            <dd className="font-bold text-[var(--color-text)]">
              {issue.event_count.toLocaleString()}
            </dd>

            <dt className="font-semibold text-[var(--color-text-dim)]">Full payloads</dt>
            <dd className="text-[var(--color-text)]">
              {issue.stored_full_events} / 50 stored (+ sample)
            </dd>

            <dt className="font-semibold text-[var(--color-text-dim)]">First seen</dt>
            <dd className="text-[var(--color-text-muted)]">
              {formatShortDate(issue.first_seen)}
            </dd>

            <dt className="font-semibold text-[var(--color-text-dim)]">Last seen</dt>
            <dd className="text-[var(--color-text-muted)]">
              {formatShortDate(issue.last_seen)} ({formatRelativeAge(issue.last_seen)})
            </dd>

            <dt className="font-semibold text-[var(--color-text-dim)]">PII scrubbing</dt>
            <dd className="text-[var(--color-success-text)] font-medium">
              Active (Authorization, Cookie, secrets)
            </dd>
          </dl>
        </section>
      </div>

      {activeTab === "overview" && (
        <div className="mt-4 grid grid-cols-1 xl:grid-cols-[1fr_380px] gap-4 pb-8">
          {/* Left: Stack Trace & Tags */}
          <section className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-card)] p-4 space-y-4">
            <StackTraceView rawFrames={detail.stack_frames} />

            <div className="pt-2 border-t border-[var(--color-border-light)] flex items-center gap-2 flex-wrap">
              <span className="text-xs font-semibold text-[var(--color-text-dim)]">
                Tags:
              </span>
              {tags.length === 0 ? (
                <span className="text-xs text-[var(--color-text-dim)]">
                  No tags attached
                </span>
              ) : (
                tags.map(([k, v]) => (
                  <span key={`${k}:${v}`} className="pm-lozenge pm-mono lowercase">
                    {k}: {v}
                  </span>
                ))
              )}
            </div>
          </section>

          {/* Right: Sample Event JSON Payload */}
          <section className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-card)] p-4 flex flex-col">
            <div className="flex items-center justify-between gap-2">
              <h3 className="text-sm font-semibold text-[var(--color-text)]">
                Sample event payload
              </h3>
              <span className="pm-lozenge pm-lozenge-success">
                <ShieldCheck size={11} />
                <span>PII SCRUBBED</span>
              </span>
            </div>
            <p className="mt-1 pm-mono text-[11px] text-[var(--color-text-dim)]">
              event_id: {sample_event?.event_id ?? "none"} · is_sample:{" "}
              {String(sample_event?.is_sample ?? true)}
            </p>

            <div className="mt-3 flex-1 rounded-md bg-[var(--pm-col)] border border-[var(--color-border)] p-3 overflow-auto max-h-[440px]">
              <pre className="pm-mono text-xs leading-relaxed text-[var(--color-text)] whitespace-pre-wrap break-all">
                {sample_event
                  ? JSON.stringify(sample_event.payload, null, 2)
                  : "No full event payload stored."}
              </pre>
            </div>
          </section>
        </div>
      )}

      {activeTab === "events" && (
        <section className="mt-4 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-card)] overflow-hidden mb-8">
          <div className="px-4 py-3 bg-[var(--color-bg-muted)] border-b border-[var(--color-border)] flex items-center justify-between">
            <span className="text-xs font-semibold text-[var(--color-text)]">
              Stored event payloads (capped at 50 per issue + regression sample)
            </span>
            <span className="text-xs text-[var(--color-text-dim)]">
              {events.length} events loaded
            </span>
          </div>

          {loadingEvents ? (
            <div className="p-6 text-xs text-[var(--color-text-dim)]">
              Loading stored events...
            </div>
          ) : events.length === 0 ? (
            <div className="p-6 text-xs text-[var(--color-text-dim)]">
              No stored events found for this issue.
            </div>
          ) : (
            <div className="divide-y divide-[var(--color-border-light)]">
              {events.map((ev) => (
                <div
                  key={ev.event_id}
                  className="px-4 py-3 flex items-center justify-between gap-4 text-xs hover:bg-[var(--pm-card-hover)]"
                >
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <span className="pm-mono font-semibold text-[var(--color-text)]">
                        {ev.event_id}
                      </span>
                      {ev.is_sample && (
                        <span className="pm-lozenge pm-lozenge-primary">SAMPLE</span>
                      )}
                      <span className="pm-lozenge">{ev.environment}</span>
                      {ev.release && (
                        <span className="pm-lozenge pm-mono">{ev.release}</span>
                      )}
                    </div>
                    <div className="mt-1 text-[var(--color-text-muted)] truncate">
                      {ev.exception_type}: {ev.exception_value}
                    </div>
                  </div>
                  <span className="text-[var(--color-text-dim)] shrink-0">
                    {formatShortDate(ev.received_at)}
                  </span>
                </div>
              ))}
            </div>
          )}
        </section>
      )}

      {activeTab === "counters" && (
        <section className="mt-4 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-card)] overflow-hidden mb-8">
          <div className="px-4 py-3 bg-[var(--color-bg-muted)] border-b border-[var(--color-border)] text-xs font-semibold text-[var(--color-text)]">
            Aggregated hourly &amp; daily counter buckets
          </div>
          <div className="p-4 grid grid-cols-1 md:grid-cols-2 gap-4">
            <div>
              <h4 className="text-xs font-bold text-[var(--color-text-muted)] mb-2">
                Hourly buckets (1h)
              </h4>
              {detail.hourly_series.length === 0 ? (
                <p className="text-xs text-[var(--color-text-dim)]">No hourly buckets.</p>
              ) : (
                <ul className="divide-y divide-[var(--color-border-light)] border border-[var(--color-border)] rounded-md">
                  {detail.hourly_series.slice(-12).map((pt) => (
                    <li
                      key={pt.bucket}
                      className="px-3 py-2 flex items-center justify-between text-xs"
                    >
                      <span className="pm-mono text-[var(--color-text-muted)]">
                        {formatShortDate(pt.bucket)}
                      </span>
                      <span className="font-bold text-[var(--color-text)]">
                        {pt.event_count.toLocaleString()} events
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </div>

            <div>
              <h4 className="text-xs font-bold text-[var(--color-text-muted)] mb-2">
                Daily buckets (1d)
              </h4>
              {detail.daily_series.length === 0 ? (
                <p className="text-xs text-[var(--color-text-dim)]">No daily buckets.</p>
              ) : (
                <ul className="divide-y divide-[var(--color-border-light)] border border-[var(--color-border)] rounded-md">
                  {detail.daily_series.slice(-12).map((pt) => (
                    <li
                      key={pt.bucket}
                      className="px-3 py-2 flex items-center justify-between text-xs"
                    >
                      <span className="pm-mono text-[var(--color-text-muted)]">
                        {formatShortDate(pt.bucket)}
                      </span>
                      <span className="font-bold text-[var(--color-text)]">
                        {pt.event_count.toLocaleString()} events
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </div>
        </section>
      )}
    </div>
  );
}

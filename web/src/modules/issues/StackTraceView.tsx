import { useState } from "react";
import { ChevronDown, ChevronRight, Code2 } from "lucide-react";
import type { StackFrameItem } from "../../lib/api";

interface StackTraceViewProps {
  rawFrames: StackFrameItem[] | unknown;
  compact?: boolean;
}

export function normalizeStackFrames(raw: StackFrameItem[] | unknown): StackFrameItem[] {
  if (!Array.isArray(raw)) {
    return [];
  }
  const typed = raw.filter(
    (item): item is StackFrameItem => typeof item === "object" && item !== null,
  );
  // Display most recent frame first
  return [...typed].reverse();
}

export function StackTraceView({ rawFrames, compact = false }: StackTraceViewProps) {
  const allFrames = normalizeStackFrames(rawFrames);
  const inAppFrames = allFrames.filter((f) => f.in_app !== false);
  const [inAppOnly, setInAppOnly] = useState(true);
  const [expandedMap, setExpandedMap] = useState<Record<number, boolean>>({});

  const visibleFrames =
    inAppOnly && inAppFrames.length > 0 ? inAppFrames : allFrames;
  const displayedFrames = compact ? visibleFrames.slice(0, 1) : visibleFrames;

  function isFrameExpanded(idx: number, frame: StackFrameItem): boolean {
    if (expandedMap[idx] !== undefined) {
      return expandedMap[idx];
    }
    return idx < 2 || Boolean(frame.context_line);
  }

  function toggleFrame(idx: number, current: boolean) {
    setExpandedMap((prev) => ({ ...prev, [idx]: !current }));
  }

  if (allFrames.length === 0) {
    return (
      <div className="rounded-md bg-[var(--color-bg-muted)] border border-[var(--color-border)] p-4 text-xs text-[var(--color-text-dim)] flex items-center gap-2">
        <Code2 size={15} />
        <span>No stack frames captured on this event (message-only fingerprint).</span>
      </div>
    );
  }

  return (
    <div className="space-y-2.5">
      {!compact && (
        <div className="flex items-center justify-between gap-2">
          <h3 className="text-sm font-semibold text-[var(--color-text)]">
            Exception stack trace (most recent call first)
          </h3>
          <div className="inline-flex items-center gap-1">
            <button
              type="button"
              onClick={() => setInAppOnly(true)}
              className={`h-6 px-2.5 rounded text-[11px] font-semibold cursor-pointer transition ${
                inAppOnly
                  ? "bg-[var(--color-primary)] text-white"
                  : "bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)]"
              }`}
            >
              In-app only ({inAppFrames.length})
            </button>
            <button
              type="button"
              onClick={() => setInAppOnly(false)}
              className={`h-6 px-2.5 rounded text-[11px] font-semibold cursor-pointer transition ${
                !inAppOnly
                  ? "bg-[var(--color-primary)] text-white"
                  : "bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)]"
              }`}
            >
              All frames ({allFrames.length})
            </button>
          </div>
        </div>
      )}

      {displayedFrames.map((frame, idx) => {
        const fileOrModule = frame.filename || frame.module || "<anonymous>";
        const fnName = frame.function || "<unknown>";
        const lineNo = frame.lineno ?? null;
        const isInApp = frame.in_app !== false;
        const pre = Array.isArray(frame.pre_context) ? frame.pre_context : [];
        const post = Array.isArray(frame.post_context) ? frame.post_context : [];
        const hasCodeContext =
          typeof frame.context_line === "string" || pre.length > 0 || post.length > 0;
        const expanded = isFrameExpanded(idx, frame);

        const startLine = lineNo !== null ? Math.max(1, lineNo - pre.length) : 1;

        return (
          <div
            key={`${fileOrModule}-${fnName}-${lineNo ?? idx}-${idx}`}
            className="rounded-md border border-[var(--color-border)] bg-[var(--color-bg-card)] overflow-hidden"
          >
            <button
              type="button"
              onClick={() => toggleFrame(idx, expanded)}
              className="w-full px-3 py-2 bg-[var(--color-bg-muted)] hover:bg-[var(--color-bg-hover)] flex items-center gap-2 text-left cursor-pointer transition"
            >
              {hasCodeContext ? (
                expanded ? (
                  <ChevronDown size={14} className="text-[var(--color-text-dim)] shrink-0" />
                ) : (
                  <ChevronRight size={14} className="text-[var(--color-text-dim)] shrink-0" />
                )
              ) : null}
              <span
                className={`pm-lozenge ${
                  isInApp ? "pm-lozenge-primary" : ""
                }`}
              >
                {isInApp ? "IN-APP" : "SYSTEM"}
              </span>
              <span className="pm-mono text-xs text-[var(--color-text)] font-semibold truncate">
                {fileOrModule}
              </span>
              <span className="text-xs text-[var(--color-text-dim)]">in</span>
              <span className="pm-mono text-xs font-semibold text-[var(--color-primary-hover)] truncate">
                {fnName}
              </span>
              {lineNo !== null && (
                <span className="ml-auto pm-mono text-xs text-[var(--color-text-muted)] shrink-0">
                  :{lineNo}
                  {frame.colno ? `:${frame.colno}` : ""}
                </span>
              )}
            </button>

            {hasCodeContext && expanded && (
              <div className="border-t border-[var(--color-border)] bg-[var(--pm-col)] py-1.5 pm-mono text-xs overflow-x-auto">
                {pre.map((codeLine, pIdx) => {
                  const ln = startLine + pIdx;
                  return (
                    <div
                      key={`pre-${ln}`}
                      className="px-3 py-0.5 flex items-baseline gap-3 text-[var(--color-text-muted)] whitespace-pre"
                    >
                      <span className="w-8 text-right text-[var(--pm-text-faint)] select-none shrink-0">
                        {ln}
                      </span>
                      <span>{codeLine}</span>
                    </div>
                  );
                })}

                {typeof frame.context_line === "string" && (
                  <div
                    className={`px-3 py-1 flex items-baseline gap-3 font-semibold whitespace-pre border-l-[3px] ${
                      idx === 0
                        ? "bg-[var(--color-danger-light)] text-[var(--color-danger-text)] border-[var(--color-danger)]"
                        : "bg-[var(--color-primary-light)] text-[var(--color-primary-hover)] border-[var(--color-primary)]"
                    }`}
                  >
                    <span className="w-8 text-right select-none shrink-0">
                      {lineNo ?? "→"}
                    </span>
                    <span>{frame.context_line}</span>
                  </div>
                )}

                {post.map((codeLine, postIdx) => {
                  const ln = (lineNo ?? startLine + pre.length) + 1 + postIdx;
                  return (
                    <div
                      key={`post-${ln}`}
                      className="px-3 py-0.5 flex items-baseline gap-3 text-[var(--color-text-muted)] whitespace-pre"
                    >
                      <span className="w-8 text-right text-[var(--pm-text-faint)] select-none shrink-0">
                        {ln}
                      </span>
                      <span>{codeLine}</span>
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

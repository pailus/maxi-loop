import { useMemo } from "react";
import type { CounterBucketPoint } from "../../lib/api";

interface OccurrenceChartProps {
  hourlySeries: CounterBucketPoint[];
  dailySeries: CounterBucketPoint[];
  interval: "1h" | "1d";
  onIntervalChange?: (next: "1h" | "1d") => void;
  compact?: boolean;
  isRegressed?: boolean;
}

interface NormalizedBar {
  label: string;
  count: number;
  isSpike: boolean;
}

function buildNormalizedBars(
  series: CounterBucketPoint[],
  interval: "1h" | "1d",
  slotCount: number,
): NormalizedBar[] {
  const now = new Date();
  const stepMs = interval === "1h" ? 3_600_000 : 86_400_000;
  const alignedNowMs =
    interval === "1h"
      ? Date.UTC(
          now.getUTCFullYear(),
          now.getUTCMonth(),
          now.getUTCDate(),
          now.getUTCHours(),
        )
      : Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate());

  const lookup = new Map<number, number>();
  for (const pt of series) {
    const d = new Date(pt.bucket);
    if (Number.isNaN(d.getTime())) continue;
    const key =
      interval === "1h"
        ? Date.UTC(
            d.getUTCFullYear(),
            d.getUTCMonth(),
            d.getUTCDate(),
            d.getUTCHours(),
          )
        : Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate());
    lookup.set(key, (lookup.get(key) ?? 0) + pt.event_count);
  }

  const raw: { ts: number; count: number }[] = [];
  for (let i = slotCount - 1; i >= 0; i--) {
    const ts = alignedNowMs - i * stepMs;
    raw.push({ ts, count: lookup.get(ts) ?? 0 });
  }

  const maxCount = Math.max(1, ...raw.map((r) => r.count));
  return raw.map((r, idx) => {
    const d = new Date(r.ts);
    const label =
      interval === "1h"
        ? `${String(d.getUTCHours()).padStart(2, "0")}:00 UTC`
        : `${d.getUTCMonth() + 1}/${d.getUTCDate()} UTC`;
    return {
      label,
      count: r.count,
      isSpike: r.count > 0 && (r.count >= maxCount * 0.7 || idx >= slotCount - 4),
    };
  });
}

export function OccurrenceChart({
  hourlySeries,
  dailySeries,
  interval,
  onIntervalChange,
  compact = false,
  isRegressed = false,
}: OccurrenceChartProps) {
  const activeSeries = interval === "1h" ? hourlySeries : dailySeries;
  const slotCount = interval === "1h" ? 24 : 24;

  const bars = useMemo(
    () => buildNormalizedBars(activeSeries, interval, slotCount),
    [activeSeries, interval, slotCount],
  );

  const maxCount = Math.max(1, ...bars.map((b) => b.count));
  const totalInWindow = bars.reduce((acc, b) => acc + b.count, 0);
  const peakBar = bars.reduce(
    (best, cur) => (cur.count > best.count ? cur : best),
    bars[0] ?? { label: "now", count: 0, isSpike: false },
  );

  const maxBarPx = compact ? 64 : 92;

  return (
    <div className="w-full">
      <div className="flex items-center justify-between gap-2 mb-2">
        <div>
          <div className="text-xs font-bold text-[var(--color-text-muted)]">
            {compact
              ? `Occurrence frequency (last ${interval === "1h" ? "24h · 1h" : "24d · 1d"} buckets)`
              : "Event occurrences (issue_hourly_counters)"}
          </div>
          {!compact && (
            <div className="text-xs text-[var(--color-text-dim)] mt-0.5">
              {totalInWindow.toLocaleString()} events in window · Peak{" "}
              <span className="font-semibold text-[var(--color-text)]">
                {peakBar.count.toLocaleString()}
              </span>{" "}
              at {peakBar.label}
            </div>
          )}
        </div>

        {onIntervalChange && (
          <div className="inline-flex items-center gap-1">
            <button
              type="button"
              onClick={() => onIntervalChange("1h")}
              className={`h-6 px-2.5 rounded text-[11px] font-semibold cursor-pointer transition ${
                interval === "1h"
                  ? "bg-[var(--color-primary)] text-white"
                  : "bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)]"
              }`}
            >
              24h (1h)
            </button>
            <button
              type="button"
              onClick={() => onIntervalChange("1d")}
              className={`h-6 px-2.5 rounded text-[11px] font-semibold cursor-pointer transition ${
                interval === "1d"
                  ? "bg-[var(--color-primary)] text-white"
                  : "bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)]"
              }`}
            >
              30d (1d)
            </button>
          </div>
        )}
      </div>

      <div
        className={`rounded-md bg-[var(--color-bg-muted)] border border-[var(--color-border)] px-3 ${
          compact ? "pt-3 pb-2" : "pt-4 pb-2.5"
        }`}
      >
        <div
          className="flex items-end gap-1.5"
          style={{ height: maxBarPx }}
          role="img"
          aria-label={`Occurrence bar chart (${interval})`}
        >
          {bars.map((bar, idx) => {
            const heightPx =
              bar.count === 0
                ? 4
                : Math.max(8, Math.round((bar.count / maxCount) * maxBarPx));
            const colorClass =
              bar.count === 0
                ? "bg-[var(--color-border)]"
                : isRegressed && bar.isSpike
                  ? "bg-[var(--color-danger)]"
                  : "bg-[#1d7afc]";
            return (
              <div
                key={`${bar.label}-${idx}`}
                className="flex-1 flex flex-col justify-end h-full group relative"
                title={`${bar.label}: ${bar.count} events`}
              >
                <div
                  className={`w-full rounded-t-[2px] transition-all ${colorClass}`}
                  style={{ height: heightPx }}
                />
              </div>
            );
          })}
        </div>

        <div className="mt-2 flex items-center justify-between text-[10px] text-[var(--pm-text-faint)]">
          <span>{interval === "1h" ? "24h ago" : "24d ago"}</span>
          {!compact && <span>{interval === "1h" ? "12h ago" : "12d ago"}</span>}
          <span
            className={
              peakBar.count > 0 && isRegressed
                ? "font-semibold text-[var(--color-danger-text)]"
                : "font-medium text-[var(--color-text-muted)]"
            }
          >
            Peak: {peakBar.count.toLocaleString()}/{interval === "1h" ? "hr" : "day"} ({peakBar.label})
          </span>
        </div>
      </div>
    </div>
  );
}

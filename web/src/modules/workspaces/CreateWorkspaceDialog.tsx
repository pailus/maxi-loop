import { type FormEvent, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { AlertCircle, Check, Loader2, X } from "lucide-react";
import {
  type TenantSummary,
  type WorkspaceView,
  createWorkspace,
} from "../../lib/api";
import {
  WORKSPACE_COLORS,
  saveWorkspaceColor,
  slugifyWorkspaceName,
} from "./workspacePrefs";

interface CreateWorkspaceDialogProps {
  tenant: TenantSummary;
  initialName?: string;
  onClose: () => void;
  onCreated: (workspace: WorkspaceView) => void;
}

export function CreateWorkspaceDialog({
  tenant,
  initialName = "",
  onClose,
  onCreated,
}: CreateWorkspaceDialogProps) {
  const [name, setName] = useState(initialName);
  const [slug, setSlug] = useState(() => slugifyWorkspaceName(initialName));
  const [slugTouched, setSlugTouched] = useState(false);
  const [color, setColor] = useState<string>(WORKSPACE_COLORS[0]);
  const [retentionOverride, setRetentionOverride] = useState("");
  const [rateLimitOverride, setRateLimitOverride] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const dialogRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  function handleNameChange(value: string) {
    setName(value);
    if (!slugTouched) {
      setSlug(slugifyWorkspaceName(value));
    }
  }

  async function handleSubmit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const cleanName = name.trim();
    const cleanSlug = slug.trim();

    if (!cleanName) {
      setError("Workspace name is required.");
      return;
    }
    if (!cleanSlug || !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(cleanSlug)) {
      setError(
        "Workspace slug must be lowercase alphanumeric and hyphen-separated (e.g. core-platform).",
      );
      return;
    }

    const parsedRetention = retentionOverride.trim()
      ? Number.parseInt(retentionOverride.trim(), 10)
      : null;
    if (
      parsedRetention !== null &&
      (Number.isNaN(parsedRetention) ||
        parsedRetention < 1 ||
        parsedRetention > tenant.retention_days)
    ) {
      setError(
        `Retention override must be between 1 and ${tenant.retention_days} days.`,
      );
      return;
    }

    const parsedRpm = rateLimitOverride.trim()
      ? Number.parseInt(rateLimitOverride.trim(), 10)
      : null;
    if (
      parsedRpm !== null &&
      (Number.isNaN(parsedRpm) || parsedRpm < 1 || parsedRpm > 1_000_000)
    ) {
      setError(
        "Default project rate limit override must be between 1 and 1,000,000 envelopes/min.",
      );
      return;
    }

    setBusy(true);
    setError(null);
    try {
      const created = await createWorkspace({
        name: cleanName,
        slug: cleanSlug,
        retention_days_override: parsedRetention,
        default_rate_limit_per_minute_override: parsedRpm,
      });
      saveWorkspaceColor(tenant.slug, created.slug, color);
      onCreated(created);
    } catch (err) {
      setError(
        err instanceof Error ? err.message : "Failed to create workspace.",
      );
      setBusy(false);
    }
  }

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-start justify-center pt-[8vh] px-4">
      <div
        className="absolute inset-0 bg-[rgba(9,30,66,0.54)] pm-fade-in"
        onClick={onClose}
      />
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="create-workspace-title"
        className="relative w-full max-w-[460px] max-h-[84vh] flex flex-col rounded-xl bg-[var(--color-bg-card)] shadow-[var(--pm-shadow-raised)] outline-none pm-fade-in"
      >
        {/* Header */}
        <div className="flex items-center h-14 px-5 shrink-0 border-b border-[var(--color-border)]">
          <h2
            id="create-workspace-title"
            className="flex-1 text-base font-semibold text-[var(--color-text)]"
          >
            Create workspace
          </h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="w-8 h-8 -mr-1.5 flex items-center justify-center rounded-md text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)] cursor-pointer"
          >
            <X size={18} />
          </button>
        </div>

        {/* Body */}
        <form
          id="ml-create-workspace-form"
          onSubmit={handleSubmit}
          className="flex-1 overflow-y-auto p-5 space-y-4"
        >
          <div>
            <label
              htmlFor="ml-ws-name"
              className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1"
            >
              Workspace name
            </label>
            <input
              id="ml-ws-name"
              type="text"
              autoFocus
              required
              maxLength={128}
              value={name}
              onChange={(e) => handleNameChange(e.target.value)}
              placeholder="e.g. Core Platform, Payments, Client A"
              className="pm-input"
            />
            <p className="mt-1 text-[11px] text-[var(--color-text-dim)]">
              Group projects, team roles, PII rules, and rate limits by team,
              product, or client.
            </p>
          </div>

          <div>
            <label
              htmlFor="ml-ws-slug"
              className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1"
            >
              Workspace slug (URL-safe)
            </label>
            <input
              id="ml-ws-slug"
              type="text"
              required
              maxLength={64}
              value={slug}
              onChange={(e) => {
                setSlugTouched(true);
                setSlug(slugifyWorkspaceName(e.target.value));
              }}
              placeholder="core-platform"
              className="pm-input pm-mono"
            />
            <p className="mt-1 text-[11px] text-[var(--color-text-dim)] pm-mono">
              /api/v1/t/{tenant.slug}/w/{slug || "workspace-slug"}/projects
            </p>
          </div>

          {/* MaxiBoard Color Swatches */}
          <div>
            <span className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1.5">
              Workspace color
            </span>
            <div
              role="radiogroup"
              aria-label="Workspace color"
              className="flex flex-wrap gap-1.5"
            >
              {WORKSPACE_COLORS.map((c) => {
                const selected = color === c;
                return (
                  <button
                    key={c}
                    type="button"
                    role="radio"
                    aria-checked={selected}
                    aria-label={c}
                    onClick={() => setColor(c)}
                    className="w-7 h-7 rounded-md flex items-center justify-center cursor-pointer transition"
                    style={{
                      backgroundColor: c,
                      boxShadow: selected
                        ? `0 0 0 2px var(--color-bg-card), 0 0 0 4px ${c}`
                        : undefined,
                    }}
                  >
                    {selected && (
                      <Check size={14} className="text-white" strokeWidth={3} />
                    )}
                  </button>
                );
              })}
            </div>
          </div>

          {/* Inherited Parameter Overrides */}
          <div className="pt-2 border-t border-[var(--color-border-light)]">
            <div className="text-[11px] font-bold uppercase tracking-wider text-[var(--color-text-dim)] mb-2">
              Optional parameter overrides
            </div>
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
              <div>
                <label
                  htmlFor="ml-ws-retention"
                  className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1"
                >
                  Retention override (days)
                </label>
                <input
                  id="ml-ws-retention"
                  type="number"
                  min={1}
                  max={tenant.retention_days}
                  value={retentionOverride}
                  onChange={(e) => setRetentionOverride(e.target.value)}
                  placeholder={`Inherit (${tenant.retention_days}d)`}
                  className="pm-input pm-mono"
                />
                <p className="mt-1 text-[11px] text-[var(--color-text-dim)]">
                  Max {tenant.retention_days}d (tenant limit)
                </p>
              </div>

              <div>
                <label
                  htmlFor="ml-ws-rpm"
                  className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1"
                >
                  Default limit (/min)
                </label>
                <input
                  id="ml-ws-rpm"
                  type="number"
                  min={1}
                  max={1_000_000}
                  value={rateLimitOverride}
                  onChange={(e) => setRateLimitOverride(e.target.value)}
                  placeholder={`Inherit (${tenant.default_rate_limit_per_minute})`}
                  className="pm-input pm-mono"
                />
                <p className="mt-1 text-[11px] text-[var(--color-text-dim)]">
                  Default per-project cap
                </p>
              </div>
            </div>
          </div>

          {error && (
            <div
              role="alert"
              className="p-2.5 rounded bg-[var(--color-danger-light)] border border-[#f87168] text-[var(--color-danger-text)] text-xs font-medium flex items-center gap-2"
            >
              <AlertCircle size={14} className="shrink-0" />
              <span>{error}</span>
            </div>
          )}
        </form>

        {/* Footer */}
        <div className="px-5 py-3 border-t border-[var(--color-border)] flex items-center justify-end gap-2 shrink-0">
          <button
            type="button"
            onClick={onClose}
            className="h-8 px-3 rounded-md text-sm font-medium text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text)] cursor-pointer"
          >
            Cancel
          </button>
          <button
            type="submit"
            form="ml-create-workspace-form"
            disabled={busy || !name.trim() || !slug.trim()}
            className="h-8 px-3.5 rounded-md bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-50 text-white text-sm font-semibold inline-flex items-center gap-1.5 cursor-pointer"
          >
            {busy && <Loader2 size={13} className="animate-spin" />}
            <span>{busy ? "Creating..." : "Create workspace"}</span>
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

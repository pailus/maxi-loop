import { type FormEvent, useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { AlertCircle, Loader2, Plus, X } from "lucide-react";
import {
  type ProjectView,
  createProject,
} from "../../lib/api";
import {
  PROJECT_ICON_OPTIONS,
  PROJECT_SWATCH_COLORS,
  ProjectIcon,
  type ProjectIconId,
  saveProjectVisual,
} from "./ProjectIcon";

interface CreateProjectDialogProps {
  workspaceName: string;
  workspaceSlug?: string;
  maxRetentionDays: number;
  defaultRateLimitPerMinute: number;
  existingProjects: ProjectView[];
  onClose: () => void;
  onCreated: (project: ProjectView) => Promise<void> | void;
}

function slugifyProjectName(raw: string): string {
  return raw
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 48);
}

function parseTagsCsv(raw: string): string[] {
  const seen = new Set<string>();
  for (const part of raw.split(",")) {
    const clean = part.trim().toLowerCase();
    if (clean) seen.add(clean);
  }
  return [...seen].sort();
}

function toggleTagCsv(raw: string, tag: string): string {
  const cur = parseTagsCsv(raw);
  if (cur.includes(tag)) return cur.filter((t) => t !== tag).join(", ");
  return [...cur, tag].join(", ");
}

export function CreateProjectDialog({
  workspaceName,
  workspaceSlug,
  maxRetentionDays,
  defaultRateLimitPerMinute,
  existingProjects,
  onClose,
  onCreated,
}: CreateProjectDialogProps) {
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [slugTouched, setSlugTouched] = useState(false);
  const [platform, setPlatform] = useState("python");
  const [tagsInput, setTagsInput] = useState("");
  const [retentionOverride, setRetentionOverride] = useState("");
  const [rateLimit, setRateLimit] = useState("");
  const [selectedIcon, setSelectedIcon] = useState<ProjectIconId>(
    "software-development",
  );
  const [selectedColor, setSelectedColor] = useState(PROJECT_SWATCH_COLORS[0]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  const existingTags = useMemo(() => {
    const set = new Set<string>();
    for (const p of existingProjects) {
      if (Array.isArray(p.tags)) {
        for (const t of p.tags) set.add(t);
      }
    }
    return [...set].sort((a, b) => a.localeCompare(b));
  }, [existingProjects]);

  function handleNameChange(val: string) {
    setName(val);
    if (!slugTouched) {
      setSlug(slugifyProjectName(val));
    }
  }

  async function handleSubmit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const cleanName = name.trim();
    const cleanSlug = slug.trim();
    if (!cleanName || !cleanSlug) {
      setError("Project name and URL-safe slug are required.");
      return;
    }
    if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(cleanSlug)) {
      setError(
        "Project slug must be lowercase alphanumeric and hyphen-separated (e.g. checkout-api).",
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
        parsedRetention > maxRetentionDays)
    ) {
      setError(
        `Retention override must be between 1 and ${maxRetentionDays} days.`,
      );
      return;
    }

    const parsedRate = rateLimit.trim()
      ? Number.parseInt(rateLimit.trim(), 10)
      : null;
    if (
      parsedRate !== null &&
      (Number.isNaN(parsedRate) || parsedRate < 1 || parsedRate > 1_000_000)
    ) {
      setError("Rate limit must be between 1 and 1,000,000 envelopes/min.");
      return;
    }

    setBusy(true);
    setError(null);
    try {
      const created = await createProject({
        name: cleanName,
        slug: cleanSlug,
        platform,
        tags: parseTagsCsv(tagsInput),
        retention_days_override: parsedRetention,
        rate_limit_per_minute: parsedRate,
        workspace_slug: workspaceSlug,
      });
      saveProjectVisual(created.slug, selectedIcon, selectedColor);
      await onCreated(created);
    } catch (err) {
      setError(
        err instanceof Error ? err.message : "Failed to create project.",
      );
      setBusy(false);
    }
  }

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-start justify-center pt-[6vh] px-4">
      <div
        className="absolute inset-0 bg-[rgba(9,30,66,0.54)] pm-fade-in"
        onClick={onClose}
      />
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="create-project-dialog-title"
        className="relative w-full max-w-[480px] max-h-[86vh] flex flex-col rounded-xl bg-[var(--color-bg-card)] shadow-[var(--pm-shadow-raised)] outline-none pm-fade-in"
      >
        {/* Header */}
        <div className="flex items-center h-14 px-5 shrink-0 border-b border-[var(--color-border)]">
          <div className="flex-1 min-w-0">
            <h2
              id="create-project-dialog-title"
              className="text-base font-semibold text-[var(--color-text)] truncate"
            >
              Create project
            </h2>
            <p className="text-[11px] text-[var(--color-text-dim)] truncate">
              Workspace: <span className="font-semibold">{workspaceName}</span>
            </p>
          </div>
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
          id="ml-create-project-dialog-form"
          onSubmit={handleSubmit}
          className="flex-1 overflow-y-auto p-5 space-y-4"
        >
          {/* MaxiBoard Project Icon Picker */}
          <div>
            <span className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1.5">
              Project icon &amp; accent
            </span>
            <div className="flex items-center gap-1.5 flex-wrap">
              {PROJECT_ICON_OPTIONS.slice(0, 8).map((opt) => {
                const active = selectedIcon === opt.id;
                return (
                  <button
                    key={opt.id}
                    type="button"
                    onClick={() => {
                      setSelectedIcon(opt.id);
                      setSelectedColor(opt.defaultColor);
                    }}
                    title={opt.label}
                    className={`p-1.5 rounded-md border cursor-pointer transition ${
                      active
                        ? "border-[var(--color-primary)] bg-[var(--color-primary-light)]"
                        : "border-[var(--color-border)] bg-[var(--color-bg)] hover:bg-[var(--color-bg-hover)]"
                    }`}
                  >
                    <ProjectIcon
                      slug="preview"
                      iconOverride={opt.id}
                      colorOverride={
                        active ? selectedColor : opt.defaultColor
                      }
                      size={22}
                    />
                  </button>
                );
              })}
            </div>
          </div>

          <div>
            <label
              htmlFor="ml-dlg-proj-name"
              className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1"
            >
              Project name
            </label>
            <input
              id="ml-dlg-proj-name"
              type="text"
              autoFocus
              required
              maxLength={128}
              value={name}
              onChange={(e) => handleNameChange(e.target.value)}
              placeholder="Checkout API"
              className="pm-input"
            />
          </div>

          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
            <div>
              <label
                htmlFor="ml-dlg-proj-slug"
                className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1"
              >
                Project slug (URL-safe)
              </label>
              <input
                id="ml-dlg-proj-slug"
                type="text"
                required
                maxLength={64}
                value={slug}
                onChange={(e) => {
                  setSlugTouched(true);
                  setSlug(slugifyProjectName(e.target.value));
                }}
                placeholder="checkout-api"
                className="pm-input pm-mono"
              />
            </div>

            <div>
              <label
                htmlFor="ml-dlg-proj-platform"
                className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1"
              >
                Platform / SDK
              </label>
              <select
                id="ml-dlg-proj-platform"
                value={platform}
                onChange={(e) => setPlatform(e.target.value)}
                className="pm-input"
              >
                <option value="python">python (FastAPI / Django)</option>
                <option value="rust">rust (sentry + tracing)</option>
                <option value="go">go (sentry-go)</option>
                <option value="javascript-react">
                  javascript-react (Browser)
                </option>
              </select>
            </div>
          </div>

          <div>
            <label
              htmlFor="ml-dlg-proj-tags"
              className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1"
            >
              Group tags (optional, comma-separated)
            </label>
            <input
              id="ml-dlg-proj-tags"
              type="text"
              value={tagsInput}
              onChange={(e) => setTagsInput(e.target.value)}
              placeholder="e.g. backend, payments"
              className="pm-input pm-mono"
            />
            {existingTags.length > 0 && (
              <div className="mt-1.5 flex items-center gap-1 flex-wrap">
                {existingTags.map((t) => {
                  const selected = parseTagsCsv(tagsInput).includes(t);
                  return (
                    <button
                      key={t}
                      type="button"
                      onClick={() =>
                        setTagsInput((cur) => toggleTagCsv(cur, t))
                      }
                      className={`px-1.5 py-0.5 rounded text-[10px] font-semibold cursor-pointer transition ${
                        selected
                          ? "bg-[var(--color-primary)] text-white"
                          : "bg-[var(--color-bg-muted)] border border-[var(--color-border)] text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)]"
                      }`}
                    >
                      {selected ? `✓ ${t}` : `+ ${t}`}
                    </button>
                  );
                })}
              </div>
            )}
          </div>

          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3 pt-2 border-t border-[var(--color-border-light)]">
            <div>
              <label
                htmlFor="ml-dlg-proj-retention"
                className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1"
              >
                Retention (max {maxRetentionDays}d)
              </label>
              <input
                id="ml-dlg-proj-retention"
                type="number"
                min={1}
                max={maxRetentionDays}
                value={retentionOverride}
                onChange={(e) => setRetentionOverride(e.target.value)}
                placeholder={`Inherit (${maxRetentionDays}d)`}
                className="pm-input pm-mono"
              />
            </div>

            <div>
              <label
                htmlFor="ml-dlg-proj-rate"
                className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1"
              >
                Rate limit (/min)
              </label>
              <input
                id="ml-dlg-proj-rate"
                type="number"
                min={1}
                max={1_000_000}
                value={rateLimit}
                onChange={(e) => setRateLimit(e.target.value)}
                placeholder={`Inherit (${defaultRateLimitPerMinute})`}
                className="pm-input pm-mono"
              />
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
            className="h-8 px-3 rounded-md text-xs font-medium text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text)] cursor-pointer"
          >
            Cancel
          </button>
          <button
            type="submit"
            form="ml-create-project-dialog-form"
            disabled={busy || !name.trim() || !slug.trim()}
            className="h-8 px-3.5 rounded-md bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-50 text-white text-xs font-semibold inline-flex items-center gap-1.5 cursor-pointer"
          >
            {busy ? (
              <Loader2 size={13} className="animate-spin" />
            ) : (
              <Plus size={13} />
            )}
            <span>{busy ? "Creating..." : "Create project"}</span>
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

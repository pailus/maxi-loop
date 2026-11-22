import { type FormEvent, type ReactNode, useEffect, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import {
  AlertCircle,
  ArrowLeft,
  ArrowRight,
  Check,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Copy,
  Loader2,
  Plus,
  Search,
  X,
} from "lucide-react";
import {
  type ProjectView,
  createProject,
  fetchIssues,
  fetchTransactions,
} from "../../lib/api";
import {
  PROJECT_ICON_OPTIONS,
  PROJECT_SWATCH_COLORS,
  ProjectIcon,
  type ProjectIconId,
  saveProjectVisual,
} from "./ProjectIcon";
import {
  ONBOARDING_PLATFORMS,
  PLATFORM_CATEGORY_LABELS,
  type PlatformCategory,
  buildSdkSteps,
  findPlatform,
} from "./onboardingPlatforms";
import { PlatformLogo } from "./PlatformLogo";

interface CreateProjectDialogProps {
  workspaceName: string;
  workspaceSlug?: string;
  maxRetentionDays: number;
  defaultRateLimitPerMinute: number;
  existingProjects: ProjectView[];
  onClose: () => void;
  /** Called right after the project is created (dialog stays open on the SDK setup step). */
  onCreated: (project: ProjectView) => Promise<void> | void;
  /** Called when the user finishes onboarding and wants to see the project's issues. */
  onViewIssues: (project: ProjectView) => void;
}

type WizardStep = 1 | 2 | 3;

const STEP_LABELS: Record<WizardStep, string> = {
  1: "Choose platform",
  2: "Name project",
  3: "Set up SDK",
};

const FIRST_EVENT_POLL_MS = 3000;

type FirstEventState =
  | { kind: "waiting" }
  | { kind: "received"; what: "error" | "transaction"; title: string };

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

function defaultIconForPlatform(platformId: string): ProjectIconId {
  if (platformId.startsWith("javascript")) return "web-development";
  if (platformId === "go" || platformId === "rust") return "infrastructure-iot";
  return "software-development";
}

function CodeBlock({ code }: { code: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="relative group">
      <pre className="p-3 pr-16 rounded-md bg-[var(--pm-col)] border border-[var(--color-border)] pm-mono text-xs leading-relaxed text-[var(--color-text)] overflow-x-auto whitespace-pre">
        {code}
      </pre>
      <button
        type="button"
        onClick={() => {
          navigator.clipboard?.writeText(code).catch(() => {});
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1500);
        }}
        className="absolute top-1.5 right-1.5 h-6 px-1.5 rounded text-[10px] font-semibold inline-flex items-center gap-1 bg-[var(--color-bg-card)] border border-[var(--color-border)] text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)] cursor-pointer"
      >
        {copied ? <Check size={11} /> : <Copy size={11} />}
        <span>{copied ? "Copied" : "Copy"}</span>
      </button>
    </div>
  );
}

function Stepper({ step }: { step: WizardStep }) {
  return (
    <ol className="flex items-center gap-2 text-[11px] font-semibold">
      {([1, 2, 3] as const).map((n, idx) => {
        const done = n < step;
        const active = n === step;
        return (
          <li key={n} className="flex items-center gap-2 min-w-0">
            {idx > 0 && <span className="w-5 h-px bg-[var(--color-border)]" />}
            <span
              className={`w-5 h-5 rounded-full inline-flex items-center justify-center shrink-0 ${
                done
                  ? "bg-[var(--color-success)] text-white"
                  : active
                    ? "bg-[var(--color-primary)] text-white"
                    : "bg-[var(--color-bg-muted)] text-[var(--color-text-dim)] border border-[var(--color-border)]"
              }`}
            >
              {done ? <Check size={11} /> : n}
            </span>
            <span
              className={`truncate hidden sm:inline ${
                active ? "text-[var(--color-text)]" : "text-[var(--color-text-dim)]"
              }`}
            >
              {STEP_LABELS[n]}
            </span>
          </li>
        );
      })}
    </ol>
  );
}

export function CreateProjectDialog({
  workspaceName,
  workspaceSlug,
  maxRetentionDays,
  defaultRateLimitPerMinute,
  existingProjects,
  onClose,
  onCreated,
  onViewIssues,
}: CreateProjectDialogProps) {
  const [step, setStep] = useState<WizardStep>(1);

  // Step 1 — platform
  const [platform, setPlatform] = useState<string>("");
  const [platformQuery, setPlatformQuery] = useState("");
  const [category, setCategory] = useState<PlatformCategory | "all">("all");

  // Step 2 — project details
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [slugTouched, setSlugTouched] = useState(false);
  const [tagsInput, setTagsInput] = useState("");
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [retentionOverride, setRetentionOverride] = useState("");
  const [rateLimit, setRateLimit] = useState("");
  const [selectedIcon, setSelectedIcon] = useState<ProjectIconId>("software-development");
  const [selectedColor, setSelectedColor] = useState(PROJECT_SWATCH_COLORS[0]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Step 3 — SDK setup & first event
  const [created, setCreated] = useState<ProjectView | null>(null);
  const [firstEvent, setFirstEvent] = useState<FirstEventState>({ kind: "waiting" });
  const [dsnCopied, setDsnCopied] = useState(false);

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  // Poll for the project's first error event or transaction.
  useEffect(() => {
    if (!created || firstEvent.kind === "received") return;
    let cancelled = false;
    let timer: number | undefined;

    async function check() {
      if (!created) return;
      try {
        const [issues, txns] = await Promise.all([
          fetchIssues({
            workspace_slug: created.workspace_slug,
            project_slug: created.slug,
            per_page: 1,
          }),
          fetchTransactions({
            workspace_slug: created.workspace_slug,
            project_slug: created.slug,
            hours: 24,
            per_page: 1,
          }).catch(() => null),
        ]);
        if (cancelled) return;
        if (issues.items.length > 0) {
          setFirstEvent({ kind: "received", what: "error", title: issues.items[0].title });
          return;
        }
        if (txns && txns.items.length > 0) {
          setFirstEvent({
            kind: "received",
            what: "transaction",
            title: txns.items[0].transaction_name,
          });
          return;
        }
      } catch {
        // Transient failure — keep polling.
      }
      if (!cancelled) timer = window.setTimeout(check, FIRST_EVENT_POLL_MS);
    }

    void check();
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [created, firstEvent.kind]);

  const existingTags = useMemo(() => {
    const set = new Set<string>();
    for (const p of existingProjects) {
      if (Array.isArray(p.tags)) {
        for (const t of p.tags) set.add(t);
      }
    }
    return [...set].sort((a, b) => a.localeCompare(b));
  }, [existingProjects]);

  const visiblePlatforms = useMemo(() => {
    const q = platformQuery.trim().toLowerCase();
    return ONBOARDING_PLATFORMS.filter(
      (p) =>
        (category === "all" || p.category === category) &&
        (!q || p.label.toLowerCase().includes(q) || p.language.toLowerCase().includes(q) || p.id.includes(q)),
    );
  }, [platformQuery, category]);

  const selectedPlatform = platform ? findPlatform(platform) : null;

  function choosePlatform(id: string) {
    setPlatform(id);
    const icon = defaultIconForPlatform(id);
    setSelectedIcon(icon);
    setSelectedColor(
      PROJECT_ICON_OPTIONS.find((o) => o.id === icon)?.defaultColor ?? PROJECT_SWATCH_COLORS[0],
    );
    if (!name.trim()) {
      const suggested = id === "other" ? "" : id;
      setName(suggested);
      if (!slugTouched) setSlug(slugifyProjectName(suggested));
    }
  }

  function handleNameChange(val: string) {
    setName(val);
    if (!slugTouched) setSlug(slugifyProjectName(val));
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
    if (existingProjects.some((p) => p.slug === cleanSlug)) {
      setError(`A project with slug "${cleanSlug}" already exists in this workspace.`);
      return;
    }

    const parsedRetention = retentionOverride.trim()
      ? Number.parseInt(retentionOverride.trim(), 10)
      : null;
    if (
      parsedRetention !== null &&
      (Number.isNaN(parsedRetention) || parsedRetention < 1 || parsedRetention > maxRetentionDays)
    ) {
      setError(`Retention override must be between 1 and ${maxRetentionDays} days.`);
      return;
    }

    const parsedRate = rateLimit.trim() ? Number.parseInt(rateLimit.trim(), 10) : null;
    if (parsedRate !== null && (Number.isNaN(parsedRate) || parsedRate < 1 || parsedRate > 1_000_000)) {
      setError("Rate limit must be between 1 and 1,000,000 envelopes/min.");
      return;
    }

    setBusy(true);
    setError(null);
    try {
      const project = await createProject({
        name: cleanName,
        slug: cleanSlug,
        platform: platform || "other",
        tags: parseTagsCsv(tagsInput),
        retention_days_override: parsedRetention,
        rate_limit_per_minute: parsedRate,
        workspace_slug: workspaceSlug,
      });
      saveProjectVisual(project.slug, selectedIcon, selectedColor);
      setCreated(project);
      setFirstEvent({ kind: "waiting" });
      setStep(3);
      setBusy(false);
      await onCreated(project);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to create project.");
      setBusy(false);
    }
  }

  const sdkSteps = created ? buildSdkSteps(created.platform, created.dsn) : [];

  // ---------------------------------------------------------------- render

  let body: ReactNode;
  let footer: ReactNode;

  if (step === 1) {
    body = (
      <div className="p-5 space-y-3">
        <div>
          <h3 className="text-sm font-semibold text-[var(--color-text)]">
            What are you monitoring?
          </h3>
          <p className="text-xs text-[var(--color-text-dim)]">
            Pick the platform this project runs on — we&apos;ll tailor the SDK setup to it.
          </p>
        </div>
        <div className="flex flex-col sm:flex-row sm:items-center gap-2">
          <div className="flex items-center gap-1">
            {(Object.keys(PLATFORM_CATEGORY_LABELS) as Array<PlatformCategory | "all">).map((c) => (
              <button
                key={c}
                type="button"
                onClick={() => setCategory(c)}
                className={`px-2.5 h-7 rounded-md text-xs font-semibold cursor-pointer ${
                  category === c
                    ? "bg-[var(--color-primary-light)] text-[var(--color-primary)]"
                    : "text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)]"
                }`}
              >
                {PLATFORM_CATEGORY_LABELS[c]}
              </button>
            ))}
          </div>
          <div className="relative sm:ml-auto sm:w-56">
            <Search
              size={13}
              className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--color-text-dim)] pointer-events-none"
            />
            <input
              type="text"
              value={platformQuery}
              onChange={(e) => setPlatformQuery(e.target.value)}
              placeholder="Filter platforms"
              aria-label="Filter platforms"
              className="pm-input !pl-8"
            />
          </div>
        </div>
        <div className="grid grid-cols-2 sm:grid-cols-3 gap-2">
          {visiblePlatforms.map((p) => {
            const active = platform === p.id;
            return (
              <button
                key={p.id}
                type="button"
                onClick={() => choosePlatform(p.id)}
                onDoubleClick={() => {
                  choosePlatform(p.id);
                  setStep(2);
                }}
                aria-pressed={active}
                className={`relative flex items-center gap-2.5 p-2.5 rounded-lg border text-left cursor-pointer transition ${
                  active
                    ? "border-[var(--color-primary)] bg-[var(--color-primary-light)] ring-1 ring-[var(--color-primary)]"
                    : "border-[var(--color-border)] bg-[var(--color-bg)] hover:bg-[var(--color-bg-hover)]"
                }`}
              >
                <PlatformLogo platform={p.id} />
                <span className="min-w-0">
                  <span className="block text-xs font-semibold text-[var(--color-text)] truncate">
                    {p.label}
                  </span>
                  <span className="block text-[10px] text-[var(--color-text-dim)] truncate">
                    {p.language}
                  </span>
                </span>
                {active && (
                  <CheckCircle2
                    size={14}
                    className="absolute top-1.5 right-1.5 text-[var(--color-primary)]"
                  />
                )}
              </button>
            );
          })}
          {visiblePlatforms.length === 0 && (
            <p className="col-span-full text-xs text-[var(--color-text-dim)] py-6 text-center">
              No platform matches. Choose <span className="font-semibold">Other</span> to use any Sentry SDK.
            </p>
          )}
        </div>
      </div>
    );
    footer = (
      <>
        <button
          type="button"
          onClick={onClose}
          className="h-8 px-3 rounded-md text-xs font-medium text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text)] cursor-pointer"
        >
          Cancel
        </button>
        <button
          type="button"
          disabled={!platform}
          onClick={() => setStep(2)}
          className="h-8 px-3.5 rounded-md bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-50 text-white text-xs font-semibold inline-flex items-center gap-1.5 cursor-pointer"
        >
          <span>Next</span>
          <ArrowRight size={13} />
        </button>
      </>
    );
  } else if (step === 2) {
    body = (
      <form
        id="ml-create-project-dialog-form"
        onSubmit={handleSubmit}
        className="p-5 space-y-4"
      >
        {selectedPlatform && (
          <div className="flex items-center gap-2.5 p-2.5 rounded-lg bg-[var(--color-bg-muted)] border border-[var(--color-border)]">
            <PlatformLogo platform={selectedPlatform.id} size={28} />
            <div className="flex-1 min-w-0">
              <p className="text-xs font-semibold text-[var(--color-text)]">{selectedPlatform.label}</p>
              <p className="text-[10px] text-[var(--color-text-dim)]">Platform</p>
            </div>
            <button
              type="button"
              onClick={() => setStep(1)}
              className="text-[11px] font-semibold text-[var(--color-primary)] hover:underline cursor-pointer"
            >
              Change
            </button>
          </div>
        )}

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
                    colorOverride={active ? selectedColor : opt.defaultColor}
                    size={22}
                  />
                </button>
              );
            })}
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
                    onClick={() => setTagsInput((cur) => toggleTagCsv(cur, t))}
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

        <div className="pt-2 border-t border-[var(--color-border-light)]">
          <button
            type="button"
            onClick={() => setShowAdvanced((v) => !v)}
            aria-expanded={showAdvanced}
            className="inline-flex items-center gap-1 text-xs font-semibold text-[var(--color-text-muted)] hover:text-[var(--color-text)] cursor-pointer"
          >
            {showAdvanced ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
            Advanced: retention &amp; rate limit
          </button>
          {showAdvanced && (
            <div className="mt-2 grid grid-cols-1 sm:grid-cols-2 gap-3">
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
          )}
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
    );
    footer = (
      <>
        <button
          type="button"
          onClick={() => setStep(1)}
          disabled={busy}
          className="h-8 px-3 rounded-md text-xs font-medium text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text)] inline-flex items-center gap-1 cursor-pointer mr-auto"
        >
          <ArrowLeft size={13} />
          <span>Back</span>
        </button>
        <button
          type="submit"
          form="ml-create-project-dialog-form"
          disabled={busy || !name.trim() || !slug.trim()}
          className="h-8 px-3.5 rounded-md bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-50 text-white text-xs font-semibold inline-flex items-center gap-1.5 cursor-pointer"
        >
          {busy ? <Loader2 size={13} className="animate-spin" /> : <Plus size={13} />}
          <span>{busy ? "Creating..." : "Create project"}</span>
        </button>
      </>
    );
  } else {
    const platformInfo = created ? findPlatform(created.platform) : null;
    body = created ? (
      <div className="p-5 space-y-4">
        <div className="flex items-center gap-2.5">
          {platformInfo && (
            <PlatformLogo platform={platformInfo.id} size={28} />
          )}
          <div className="min-w-0">
            <h3 className="text-sm font-semibold text-[var(--color-text)] truncate">
              Configure {platformInfo?.label ?? created.platform} SDK for{" "}
              <span className="pm-mono">{created.slug}</span>
            </h3>
            <p className="text-xs text-[var(--color-text-dim)]">
              Project created. Follow the steps below, then trigger an error to verify the setup.
            </p>
          </div>
        </div>

        <div>
          <label
            htmlFor="ml-dlg-proj-dsn"
            className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1"
          >
            Client DSN
          </label>
          <div className="flex items-center gap-1.5">
            <input
              id="ml-dlg-proj-dsn"
              type="text"
              readOnly
              value={created.dsn}
              onFocus={(e) => e.currentTarget.select()}
              className="pm-input pm-mono flex-1"
            />
            <button
              type="button"
              onClick={() => {
                navigator.clipboard?.writeText(created.dsn).catch(() => {});
                setDsnCopied(true);
                window.setTimeout(() => setDsnCopied(false), 1500);
              }}
              className="h-8 px-2.5 rounded-md border border-[var(--color-border)] text-xs font-semibold text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)] inline-flex items-center gap-1 cursor-pointer shrink-0"
            >
              {dsnCopied ? <Check size={12} /> : <Copy size={12} />}
              <span>{dsnCopied ? "Copied" : "Copy"}</span>
            </button>
          </div>
        </div>

        <ol className="space-y-3">
          {sdkSteps.map((s, idx) => (
            <li key={s.title} className="flex gap-2.5">
              <span className="w-5 h-5 mt-0.5 rounded-full bg-[var(--color-bg-muted)] border border-[var(--color-border)] text-[10px] font-bold text-[var(--color-text-muted)] inline-flex items-center justify-center shrink-0">
                {idx + 1}
              </span>
              <div className="flex-1 min-w-0 space-y-1">
                <p className="text-xs font-semibold text-[var(--color-text)]">{s.title}</p>
                {s.description && (
                  <p className="text-[11px] text-[var(--color-text-dim)]">{s.description}</p>
                )}
                <CodeBlock code={s.code} />
              </div>
            </li>
          ))}
        </ol>
      </div>
    ) : null;

    footer = (
      <>
        <div
          role="status"
          aria-live="polite"
          className="mr-auto min-w-0 flex items-center gap-2 text-xs font-medium"
        >
          {firstEvent.kind === "waiting" ? (
            <>
              <span className="relative flex w-2.5 h-2.5 shrink-0">
                <span className="absolute inline-flex w-full h-full rounded-full bg-[var(--color-primary)] opacity-60 animate-ping" />
                <span className="relative inline-flex w-2.5 h-2.5 rounded-full bg-[var(--color-primary)]" />
              </span>
              <span className="text-[var(--color-text-muted)] truncate">
                Waiting to receive first event…
              </span>
            </>
          ) : (
            <>
              <CheckCircle2 size={15} className="text-[var(--color-success)] shrink-0" />
              <span className="text-[var(--color-success-text)] truncate" title={firstEvent.title}>
                First {firstEvent.what} received: <span className="pm-mono">{firstEvent.title}</span>
              </span>
            </>
          )}
        </div>
        {firstEvent.kind === "waiting" ? (
          <button
            type="button"
            onClick={onClose}
            className="h-8 px-3 rounded-md text-xs font-medium text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text)] cursor-pointer shrink-0"
          >
            Skip for now
          </button>
        ) : (
          <button
            type="button"
            onClick={onClose}
            className="h-8 px-3 rounded-md text-xs font-medium text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text)] cursor-pointer shrink-0"
          >
            Close
          </button>
        )}
        <button
          type="button"
          onClick={() => created && onViewIssues(created)}
          disabled={!created}
          className={`h-8 px-3.5 rounded-md text-xs font-semibold inline-flex items-center gap-1.5 cursor-pointer shrink-0 ${
            firstEvent.kind === "received"
              ? "bg-[var(--color-success)] hover:opacity-90 text-white"
              : "border border-[var(--color-border)] text-[var(--color-text)] hover:bg-[var(--color-bg-hover)]"
          }`}
        >
          <span>{firstEvent.kind === "received" ? "Take me to my event" : "Go to issues"}</span>
          <ArrowRight size={13} />
        </button>
      </>
    );
  }

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-start justify-center pt-[5vh] px-4">
      <div
        className="absolute inset-0 bg-[rgba(9,30,66,0.54)] pm-fade-in"
        onClick={onClose}
      />
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="create-project-dialog-title"
        className="relative w-full max-w-[680px] max-h-[90vh] flex flex-col rounded-xl bg-[var(--color-bg-card)] shadow-[var(--pm-shadow-raised)] outline-none pm-fade-in"
      >
        <div className="px-5 pt-3.5 pb-3 shrink-0 border-b border-[var(--color-border)] space-y-2.5">
          <div className="flex items-center">
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
          <Stepper step={step} />
        </div>

        <div className="flex-1 overflow-y-auto">{body}</div>

        <div className="px-5 py-3 border-t border-[var(--color-border)] flex items-center justify-end gap-2 shrink-0">
          {footer}
        </div>
      </div>
    </div>,
    document.body,
  );
}

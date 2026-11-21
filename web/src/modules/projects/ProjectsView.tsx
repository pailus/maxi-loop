import { type FormEvent, useState } from "react";
import {
  AlertCircle,
  ArrowRight,
  Check,
  Copy,
  KeyRound,
  Plus,
  RefreshCw,
} from "lucide-react";
import {
  type ProjectView,
  createProject,
  rotateProjectDsn,
} from "../../lib/api";
import {
  PROJECT_ICON_OPTIONS,
  PROJECT_SWATCH_COLORS,
  ProjectIcon,
  type ProjectIconId,
  saveProjectVisual,
} from "./ProjectIcon";

interface ProjectsViewProps {
  workspaceName: string;
  workspaceSlug?: string;
  globalRetentionDays: number;
  projects: ProjectView[];
  canManageProjects: boolean;
  onProjectsChanged: () => Promise<void>;
  onOpenProjectIssues: (projectSlug: string) => void;
  onShowToast: (message: string) => void;
}

function slugifyName(raw: string): string {
  return raw
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 48);
}

export function ProjectsView({
  workspaceName,
  workspaceSlug,
  globalRetentionDays,
  projects,
  canManageProjects,
  onProjectsChanged,
  onOpenProjectIssues,
  onShowToast,
}: ProjectsViewProps) {
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [slugTouched, setSlugTouched] = useState(false);
  const [platform, setPlatform] = useState("python");
  const [retentionOverride, setRetentionOverride] = useState("");
  const [rateLimit, setRateLimit] = useState("1200");
  const [selectedIcon, setSelectedIcon] = useState<ProjectIconId>("software-development");
  const [selectedColor, setSelectedColor] = useState(PROJECT_SWATCH_COLORS[0]);

  const [submitting, setSubmitting] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const [copiedId, setCopiedId] = useState<number | null>(null);
  const [rotatingSlug, setRotatingSlug] = useState<string | null>(null);

  function handleNameChange(val: string) {
    setName(val);
    if (!slugTouched) {
      setSlug(slugifyName(val));
    }
  }

  async function handleCreateProject(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const cleanName = name.trim();
    const cleanSlug = slug.trim();
    if (!cleanName || !cleanSlug) {
      setFormError("Project name and URL-safe slug are required.");
      return;
    }

    const parsedRetention = retentionOverride.trim()
      ? Number.parseInt(retentionOverride.trim(), 10)
      : null;
    if (
      parsedRetention !== null &&
      (Number.isNaN(parsedRetention) ||
        parsedRetention < 1 ||
        parsedRetention > globalRetentionDays)
    ) {
      setFormError(
        `Retention override must be between 1 and ${globalRetentionDays} days.`,
      );
      return;
    }

    const parsedRate = rateLimit.trim()
      ? Number.parseInt(rateLimit.trim(), 10)
      : null;

    setSubmitting(true);
    setFormError(null);
    try {
      const created = await createProject({
        name: cleanName,
        slug: cleanSlug,
        platform,
        retention_days_override: parsedRetention,
        rate_limit_per_minute: parsedRate,
        workspace_slug: workspaceSlug,
      });
      saveProjectVisual(created.slug, selectedIcon, selectedColor);
      setName("");
      setSlug("");
      setSlugTouched(false);
      setRetentionOverride("");
      await onProjectsChanged();
      onShowToast(`Project ${created.slug} created and DSN cached`);
    } catch (err) {
      setFormError(
        err instanceof Error ? err.message : "Failed to create project.",
      );
    } finally {
      setSubmitting(false);
    }
  }

  function handleCopyDsn(proj: ProjectView) {
    navigator.clipboard?.writeText(proj.dsn).catch(() => {});
    setCopiedId(proj.id);
    onShowToast(`Copied Sentry DSN for ${proj.slug}`);
    setTimeout(() => {
      setCopiedId((cur) => (cur === proj.id ? null : cur));
    }, 1800);
  }

  async function handleRotateDsn(proj: ProjectView) {
    setRotatingSlug(proj.slug);
    try {
      await rotateProjectDsn(proj.slug, workspaceSlug);
      await onProjectsChanged();
      onShowToast(`Rotated DSN key for ${proj.slug}`);
    } catch (err) {
      onShowToast(
        err instanceof Error ? err.message : "Failed to rotate project DSN.",
      );
    } finally {
      setRotatingSlug(null);
    }
  }

  const sampleDsn =
    projects[0]?.dsn ?? "http://<public_key>@localhost:8080/1";

  return (
    <div className="flex-1 min-w-0 flex flex-col lg:flex-row overflow-hidden bg-[var(--color-bg)]">
      {/* Left/Center Projects List Pane */}
      <section className="flex-1 min-w-0 px-6 py-4 overflow-y-auto">
        <div className="text-xs text-[var(--color-text-dim)] flex items-center gap-1.5">
          <span>{workspaceName}</span>
          <span>›</span>
          <span className="font-medium text-[var(--color-text-muted)]">
            Projects &amp; DSN keys
          </span>
        </div>

        <h1 className="mt-1.5 text-2xl font-semibold text-[var(--color-text)] tracking-tight">
          Projects &amp; Sentry DSNs
        </h1>
        <p className="mt-1 text-xs text-[var(--color-text-muted)]">
          Point any official Sentry SDK to a project DSN below. Keys are cached in memory (0 DB queries per envelope).
        </p>

        <div className="mt-4 space-y-4 pb-8">
          {projects.length === 0 ? (
            <div className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-muted)] p-8 text-center">
              <div className="mx-auto w-12 h-12 rounded-full bg-[var(--color-primary-light)] text-[var(--color-primary)] flex items-center justify-center mb-3">
                <KeyRound size={22} />
              </div>
              <h3 className="text-sm font-semibold text-[var(--color-text)]">
                No projects created yet
              </h3>
              <p className="mt-1 text-xs text-[var(--color-text-dim)] max-w-md mx-auto">
                Use the Create project form on the right to provision your first project and generate an instant Sentry DSN.
              </p>
            </div>
          ) : (
            projects.map((p) => {
              const isCopied = copiedId === p.id;
              const isRotating = rotatingSlug === p.slug;
              return (
                <article
                  key={p.id}
                  className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-card)] p-5 shadow-[var(--pm-shadow-card)]"
                >
                  {/* Card Header */}
                  <div className="flex items-center justify-between gap-3 flex-wrap">
                    <div className="flex items-center gap-2.5 min-w-0">
                      <ProjectIcon slug={p.slug} platform={p.platform} size={26} />
                      <div>
                        <div className="flex items-center gap-2 flex-wrap">
                          <h2 className="text-base font-bold text-[var(--color-text)]">
                            {p.name}
                          </h2>
                          <span className="pm-mono text-xs text-[var(--color-text-dim)]">
                            ({p.slug})
                          </span>
                          <span className="pm-lozenge pm-lozenge-primary">
                            {p.platform}
                          </span>
                        </div>
                      </div>
                    </div>

                    <div className="flex items-center gap-2">
                      <button
                        type="button"
                        onClick={() => onOpenProjectIssues(p.slug)}
                        className="h-7 px-2.5 rounded bg-[var(--pm-col)] hover:bg-[var(--color-bg-hover)] text-xs font-medium text-[var(--color-text-muted)] inline-flex items-center gap-1 cursor-pointer"
                      >
                        <span>View issues</span>
                        <ArrowRight size={12} />
                      </button>

                      {canManageProjects && (
                        <button
                          type="button"
                          disabled={isRotating}
                          onClick={() => void handleRotateDsn(p)}
                          className="h-7 px-2.5 rounded border border-[#f87168] bg-[var(--color-danger-light)] hover:brightness-95 disabled:opacity-50 text-xs font-semibold text-[var(--color-danger-text)] inline-flex items-center gap-1 cursor-pointer"
                        >
                          <RefreshCw
                            size={12}
                            className={isRotating ? "animate-spin" : ""}
                          />
                          <span>{isRotating ? "Rotating..." : "Rotate DSN"}</span>
                        </button>
                      )}
                    </div>
                  </div>

                  {/* Copyable Sentry DSN Box */}
                  <div className="mt-3.5">
                    <label
                      htmlFor={`dsn-${p.id}`}
                      className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1"
                    >
                      Client DSN (128-bit CSPRNG key)
                    </label>
                    <div className="flex items-center gap-2">
                      <input
                        id={`dsn-${p.id}`}
                        type="text"
                        readOnly
                        value={p.dsn}
                        onFocus={(e) => e.currentTarget.select()}
                        className="pm-input pm-mono flex-1 h-9 text-xs"
                      />
                      <button
                        type="button"
                        onClick={() => handleCopyDsn(p)}
                        className="h-9 px-3 rounded-[6px] bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] text-white text-xs font-semibold inline-flex items-center gap-1.5 shrink-0 cursor-pointer"
                      >
                        {isCopied ? <Check size={13} /> : <Copy size={13} />}
                        <span>{isCopied ? "Copied!" : "Copy DSN"}</span>
                      </button>
                    </div>
                  </div>

                  {/* Metadata & Limits Grid */}
                  <dl className="mt-3.5 rounded-md bg-[var(--color-bg-muted)] border border-[var(--color-border-light)] p-3 grid grid-cols-1 sm:grid-cols-[140px_1fr] gap-y-1.5 gap-x-3 text-xs">
                    <dt className="font-semibold text-[var(--color-text-dim)]">
                      Envelope endpoint
                    </dt>
                    <dd className="pm-mono text-[var(--color-text)] break-all">
                      POST {p.ingest_endpoint}
                    </dd>

                    <dt className="font-semibold text-[var(--color-text-dim)]">
                      Effective retention
                    </dt>
                    <dd className="text-[var(--color-text)]">
                      {p.effective_retention_days} days{" "}
                      {p.retention_days_override
                        ? `(project override <= ${globalRetentionDays}d)`
                        : "(inherits workspace/global default)"}
                    </dd>

                    <dt className="font-semibold text-[var(--color-text-dim)]">
                      Governor rate limit
                    </dt>
                    <dd className="font-medium text-[var(--color-success-text)]">
                      {p.rate_limit_per_minute.toLocaleString()} envelopes/min (HTTP 429 + Retry-After on burst)
                    </dd>
                  </dl>
                </article>
              );
            })
          )}
        </div>
      </section>

      {/* Right Docked Drawer: Create Project + SDK Quickstart */}
      <aside className="w-full lg:w-[440px] shrink-0 bg-[var(--color-bg-card)] pm-card-panel p-5 overflow-y-auto max-h-[calc(100vh-56px)]">
        <div className="pb-3 border-b border-[var(--color-border)]">
          <h2 className="text-base font-semibold text-[var(--color-text)]">
            Create project
          </h2>
          <p className="mt-0.5 text-xs text-[var(--color-text-dim)]">
            Generates a new DSN and updates the in-memory ingest cache.
          </p>
        </div>

        {formError && (
          <div className="mt-3 p-2.5 rounded bg-[var(--color-danger-light)] border border-[#f87168] text-[var(--color-danger-text)] text-xs font-medium flex items-center gap-2">
            <AlertCircle size={14} className="shrink-0" />
            <span>{formError}</span>
          </div>
        )}

        <form onSubmit={handleCreateProject} className="mt-4 space-y-3.5">
          {/* MaxiBoard Project Icon & Color Picker */}
          <div>
            <span className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1.5">
              Project icon &amp; accent (MaxiBoard icons)
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
              htmlFor="proj-name"
              className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1"
            >
              Project name
            </label>
            <input
              id="proj-name"
              type="text"
              required
              disabled={!canManageProjects}
              value={name}
              onChange={(e) => handleNameChange(e.target.value)}
              placeholder="Checkout API"
              className="pm-input"
            />
          </div>

          <div>
            <label
              htmlFor="proj-slug"
              className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1"
            >
              Project slug (URL-safe)
            </label>
            <input
              id="proj-slug"
              type="text"
              required
              disabled={!canManageProjects}
              value={slug}
              onChange={(e) => {
                setSlugTouched(true);
                setSlug(slugifyName(e.target.value));
              }}
              placeholder="checkout-api"
              className="pm-input pm-mono"
            />
          </div>

          <div>
            <label
              htmlFor="proj-platform"
              className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1"
            >
              Platform / SDK
            </label>
            <select
              id="proj-platform"
              disabled={!canManageProjects}
              value={platform}
              onChange={(e) => setPlatform(e.target.value)}
              className="pm-input"
            >
              <option value="python">python (FastAPI / Django)</option>
              <option value="rust">rust (sentry + tracing)</option>
              <option value="go">go (sentry-go)</option>
              <option value="javascript-react">javascript-react (Browser)</option>
            </select>
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div>
              <label
                htmlFor="proj-retention"
                className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1"
              >
                Retention (max {globalRetentionDays}d)
              </label>
              <input
                id="proj-retention"
                type="number"
                min={1}
                max={globalRetentionDays}
                disabled={!canManageProjects}
                value={retentionOverride}
                onChange={(e) => setRetentionOverride(e.target.value)}
                placeholder={`${globalRetentionDays} (default)`}
                className="pm-input pm-mono"
              />
            </div>

            <div>
              <label
                htmlFor="proj-rate"
                className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1"
              >
                Rate limit (/min)
              </label>
              <input
                id="proj-rate"
                type="number"
                min={10}
                max={100000}
                disabled={!canManageProjects}
                value={rateLimit}
                onChange={(e) => setRateLimit(e.target.value)}
                className="pm-input pm-mono"
              />
            </div>
          </div>

          <button
            type="submit"
            disabled={submitting || !canManageProjects}
            className="h-9 px-4 rounded-[6px] bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-50 text-white text-xs font-semibold inline-flex items-center gap-1.5 cursor-pointer"
          >
            <Plus size={14} />
            <span>{submitting ? "Creating..." : "Create project"}</span>
          </button>
        </form>

        <hr className="my-5 border-[var(--color-border)]" />

        <div>
          <h3 className="text-xs font-bold text-[var(--color-text)]">
            Sentry SDK quickstart (drop-in DSN replacement)
          </h3>
          <p className="mt-0.5 text-xs text-[var(--color-text-dim)]">
            Official Python, Go, Rust, and React SDKs work without custom wrappers:
          </p>

          <pre className="mt-2.5 p-3 rounded-md bg-[var(--pm-col)] border border-[var(--color-border)] pm-mono text-xs leading-relaxed text-[var(--color-text)] overflow-x-auto">
{`# pip install sentry-sdk[fastapi]
import sentry_sdk

sentry_sdk.init(
    dsn="${sampleDsn}",
    environment="production",
    release="9f3c2a1",
    send_default_pii=False,
)`}
          </pre>
        </div>
      </aside>
    </div>
  );
}

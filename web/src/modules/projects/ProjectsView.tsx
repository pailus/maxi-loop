import { useMemo, useState } from "react";
import {
  ArrowRight,
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  Folder,
  KeyRound,
  Plus,
  RefreshCw,
  Tag,
} from "lucide-react";
import {
  type ProjectView,
  rotateProjectDsn,
  updateProjectSettings,
} from "../../lib/api";
import { ONBOARDING_PLATFORMS, buildSdkSteps } from "./onboardingPlatforms";
import { PlatformChip, PlatformLogo } from "./PlatformLogo";
import { SdkSetupSteps } from "./SdkSetupSteps";
import { ProjectIcon } from "./ProjectIcon";

interface ProjectsViewProps {
  workspaceName: string;
  workspaceSlug?: string;
  globalRetentionDays: number;
  projects: ProjectView[];
  canManageProjects: boolean;
  onOpenCreateProject?: () => void;
  onProjectsChanged: () => Promise<void>;
  onOpenProjectIssues: (projectSlug: string) => void;
  onShowToast: (message: string) => void;
}

function parseTagsCsv(raw: string): string[] {
  const seen = new Set<string>();
  for (const part of raw.split(",")) {
    const clean = part.trim().toLowerCase();
    if (clean) seen.add(clean);
  }
  return [...seen].sort();
}

function appendTagCsv(raw: string, tag: string): string {
  const cur = parseTagsCsv(raw);
  if (cur.includes(tag)) return cur.filter((t) => t !== tag).join(", ");
  return [...cur, tag].join(", ");
}

export function ProjectsView({
  workspaceName,
  workspaceSlug,
  globalRetentionDays,
  projects,
  canManageProjects,
  onOpenCreateProject,
  onProjectsChanged,
  onOpenProjectIssues,
  onShowToast,
}: ProjectsViewProps) {
  const [quickstartSlug, setQuickstartSlug] = useState<string>("");
  const [quickstartPlatform, setQuickstartPlatform] = useState<string | null>(null);
  const [editingSlug, setEditingSlug] = useState<string | null>(null);
  const [editTags, setEditTags] = useState("");
  const [editRetention, setEditRetention] = useState("");
  const [editRateLimit, setEditRateLimit] = useState("");
  const [savingEdit, setSavingEdit] = useState(false);

  const [activeTagFilter, setActiveTagFilter] = useState<string>("__all__");
  const [groupByTag, setGroupByTag] = useState(true);
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(() => new Set());

  const [copiedId, setCopiedId] = useState<number | null>(null);
  const [rotatingSlug, setRotatingSlug] = useState<string | null>(null);

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

  function startEditProject(proj: ProjectView) {
    setEditingSlug(proj.slug);
    setEditTags((proj.tags ?? []).join(", "));
    setEditRetention(
      proj.retention_days_override !== null
        ? String(proj.retention_days_override)
        : "",
    );
    setEditRateLimit(
      proj.rate_limit_per_minute_override !== null
        ? String(proj.rate_limit_per_minute_override)
        : "",
    );
  }

  async function handleSaveProjectEdit(proj: ProjectView) {
    setSavingEdit(true);
    try {
      const nextRetention = editRetention.trim()
        ? Number.parseInt(editRetention.trim(), 10)
        : null;
      const nextRate = editRateLimit.trim()
        ? Number.parseInt(editRateLimit.trim(), 10)
        : null;
      await updateProjectSettings(
        proj.slug,
        {
          tags: parseTagsCsv(editTags),
          retention_days_override: nextRetention,
          rate_limit_per_minute_override: nextRate,
        },
        proj.workspace_slug,
      );
      setEditingSlug(null);
      await onProjectsChanged();
      onShowToast(`Updated tags & settings for ${proj.slug}`);
    } catch (err) {
      onShowToast(
        err instanceof Error ? err.message : "Failed to update project settings",
      );
    } finally {
      setSavingEdit(false);
    }
  }

  const allTagCounts = useMemo(() => {
    const counts = new Map<string, number>();
    let untaggedCount = 0;
    for (const p of projects) {
      const tags = Array.isArray(p.tags) ? p.tags : [];
      if (tags.length === 0) {
        untaggedCount += 1;
      } else {
        for (const t of tags) {
          counts.set(t, (counts.get(t) ?? 0) + 1);
        }
      }
    }
    const sorted = [...counts.entries()].sort(([a], [b]) => a.localeCompare(b));
    return { sorted, untaggedCount };
  }, [projects]);

  const existingTags = useMemo(
    () => allTagCounts.sorted.map(([t]) => t),
    [allTagCounts],
  );

  const visibleProjects = useMemo(() => {
    if (activeTagFilter === "__all__") return projects;
    if (activeTagFilter === "__untagged__") {
      return projects.filter((p) => !p.tags || p.tags.length === 0);
    }
    return projects.filter((p) => (p.tags ?? []).includes(activeTagFilter));
  }, [projects, activeTagFilter]);

  const groupedSections = useMemo(() => {
    if (!groupByTag || activeTagFilter !== "__all__" || existingTags.length === 0) {
      return null;
    }
    const map = new Map<string, ProjectView[]>();
    const untagged: ProjectView[] = [];
    for (const p of visibleProjects) {
      const tags = Array.isArray(p.tags) ? p.tags : [];
      if (tags.length === 0) {
        untagged.push(p);
      } else {
        for (const t of tags) {
          const list = map.get(t) ?? [];
          list.push(p);
          map.set(t, list);
        }
      }
    }
    const sections = [...map.entries()]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([tag, items]) => ({ key: tag, label: tag, isUntagged: false, items }));
    if (untagged.length > 0) {
      sections.push({
        key: "__untagged__",
        label: "Ungrouped projects",
        isUntagged: true,
        items: untagged,
      });
    }
    return sections;
  }, [groupByTag, activeTagFilter, existingTags.length, visibleProjects]);

  function toggleGroupSection(key: string) {
    setCollapsedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }

  const quickstartProject =
    projects.find((p) => p.slug === quickstartSlug) ?? projects[0] ?? null;
  const sampleDsn = quickstartProject?.dsn ?? "http://<public_key>@localhost:8080/1";
  // Default the quickstart tab to the selected project's own platform.
  const activeQuickstartPlatform =
    quickstartPlatform ??
    (quickstartProject && ONBOARDING_PLATFORMS.some((o) => o.id === quickstartProject.platform)
      ? quickstartProject.platform
      : "python");

  const renderProjectCard = (p: ProjectView, keyPrefix: string) => {
    const isCopied = copiedId === p.id;
    const isRotating = rotatingSlug === p.slug;
    const projectTags = Array.isArray(p.tags) ? p.tags : [];
    return (
      <article
        key={`${keyPrefix}:${p.id}`}
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
                <PlatformChip platform={p.platform} />
                {projectTags.map((tag) => (
                  <button
                    key={tag}
                    type="button"
                    onClick={() =>
                      setActiveTagFilter((cur) => (cur === tag ? "__all__" : tag))
                    }
                    className="pm-lozenge pm-lozenge-purple hover:brightness-95 cursor-pointer"
                    title={`Filter by tag "${tag}"`}
                  >
                    <Tag size={10} />
                    <span>{tag}</span>
                  </button>
                ))}
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
            htmlFor={`dsn-${keyPrefix}-${p.id}`}
            className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1"
          >
            Client DSN (128-bit CSPRNG key)
          </label>
          <div className="flex items-center gap-2">
            <input
              id={`dsn-${keyPrefix}-${p.id}`}
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
            {p.effective_rate_limit_per_minute.toLocaleString()} envelopes/min{" "}
            <span className="font-normal text-[var(--color-text-dim)]">
              {p.rate_limit_per_minute_override !== null
                ? "(project override)"
                : "(inherited from workspace/tenant)"}
            </span>
          </dd>
        </dl>

        {canManageProjects && editingSlug === p.slug && (
          <div className="mt-3 p-3 rounded-md border border-[var(--color-border)] bg-[var(--color-bg-muted)] space-y-2.5">
            <div>
              <label className="block text-[11px] font-semibold text-[var(--color-text-dim)] mb-1">
                Group tags (comma-separated, e.g. backend, payments, e2e)
              </label>
              <input
                type="text"
                value={editTags}
                onChange={(e) => setEditTags(e.target.value)}
                placeholder="e.g. backend, core-api"
                className="pm-input pm-mono h-8 text-xs"
              />
              {existingTags.length > 0 && (
                <div className="mt-1.5 flex items-center gap-1 flex-wrap">
                  <span className="text-[10px] text-[var(--color-text-dim)] mr-1">
                    Existing groups:
                  </span>
                  {existingTags.map((t) => {
                    const selected = parseTagsCsv(editTags).includes(t);
                    return (
                      <button
                        key={t}
                        type="button"
                        onClick={() => setEditTags((cur) => appendTagCsv(cur, t))}
                        className={`px-1.5 py-0.5 rounded text-[10px] font-semibold cursor-pointer transition ${
                          selected
                            ? "bg-[var(--color-primary)] text-white"
                            : "bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)]"
                        }`}
                      >
                        {selected ? `✓ ${t}` : `+ ${t}`}
                      </button>
                    );
                  })}
                </div>
              )}
            </div>

            <div className="grid grid-cols-1 sm:grid-cols-[1fr_1fr_auto] gap-2.5 items-end">
              <div>
                <label className="block text-[11px] font-semibold text-[var(--color-text-dim)] mb-1">
                  Retention override (blank = inherit)
                </label>
                <input
                  type="number"
                  min={1}
                  max={globalRetentionDays}
                  value={editRetention}
                  onChange={(e) => setEditRetention(e.target.value)}
                  placeholder="Inherit"
                  className="pm-input pm-mono h-8 text-xs"
                />
              </div>
              <div>
                <label className="block text-[11px] font-semibold text-[var(--color-text-dim)] mb-1">
                  Rate limit override /min (blank = inherit)
                </label>
                <input
                  type="number"
                  min={1}
                  max={1000000}
                  value={editRateLimit}
                  onChange={(e) => setEditRateLimit(e.target.value)}
                  placeholder="Inherit"
                  className="pm-input pm-mono h-8 text-xs"
                />
              </div>
              <div className="flex items-center gap-1.5">
                <button
                  type="button"
                  disabled={savingEdit}
                  onClick={() => void handleSaveProjectEdit(p)}
                  className="h-8 px-3 rounded bg-[var(--color-primary)] text-white text-xs font-semibold cursor-pointer"
                >
                  Save
                </button>
                <button
                  type="button"
                  onClick={() => setEditingSlug(null)}
                  className="h-8 px-2.5 rounded border border-[var(--color-border)] bg-[var(--color-bg)] text-xs cursor-pointer"
                >
                  Cancel
                </button>
              </div>
            </div>
          </div>
        )}

        {canManageProjects && editingSlug !== p.slug && (
          <div className="mt-2.5 flex justify-end">
            <button
              type="button"
              onClick={() => startEditProject(p)}
              className="text-[11px] font-semibold text-[var(--color-link)] hover:underline cursor-pointer"
            >
              Edit tags &amp; overrides
            </button>
          </div>
        )}
      </article>
    );
  };

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

        <div className="mt-1.5 flex items-center justify-between gap-3 flex-wrap">
          <div>
            <h1 className="text-2xl font-semibold text-[var(--color-text)] tracking-tight">
              Projects &amp; Sentry DSNs
            </h1>
            <p className="mt-1 text-xs text-[var(--color-text-muted)]">
              Point any official Sentry SDK to a project DSN below. Group projects into sidebar folders using tags.
            </p>
          </div>

          <div className="flex items-center gap-2 flex-wrap">
            {existingTags.length > 0 && activeTagFilter === "__all__" && (
              <label className="inline-flex items-center gap-1.5 px-2.5 py-1.5 rounded-md border border-[var(--color-border)] bg-[var(--color-bg-muted)] text-xs font-medium text-[var(--color-text)] cursor-pointer select-none">
                <input
                  type="checkbox"
                  checked={groupByTag}
                  onChange={(e) => setGroupByTag(e.target.checked)}
                  className="accent-[var(--color-primary)]"
                />
                <span>Group by tag</span>
              </label>
            )}
            {canManageProjects && onOpenCreateProject && (
              <button
                type="button"
                onClick={onOpenCreateProject}
                className="h-8 px-3 rounded-[6px] bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] text-white text-xs font-semibold inline-flex items-center gap-1.5 cursor-pointer"
              >
                <Plus size={14} />
                <span>Create project</span>
              </button>
            )}
          </div>
        </div>

        {/* Tag Filter Pills */}
        {projects.length > 0 && existingTags.length > 0 && (
          <div className="mt-3 flex items-center gap-1.5 flex-wrap">
            <button
              type="button"
              onClick={() => setActiveTagFilter("__all__")}
              className={`h-7 px-2.5 rounded-md text-xs font-semibold inline-flex items-center gap-1.5 cursor-pointer transition ${
                activeTagFilter === "__all__"
                  ? "bg-[var(--color-primary-light)] text-[var(--color-primary)]"
                  : "bg-[var(--color-bg-muted)] text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)]"
              }`}
            >
              <span>All</span>
              <span className="text-[11px] opacity-80">{projects.length}</span>
            </button>
            {allTagCounts.sorted.map(([tag, count]) => (
              <button
                key={tag}
                type="button"
                onClick={() =>
                  setActiveTagFilter((cur) => (cur === tag ? "__all__" : tag))
                }
                className={`h-7 px-2.5 rounded-md text-xs font-semibold inline-flex items-center gap-1.5 cursor-pointer transition ${
                  activeTagFilter === tag
                    ? "bg-[var(--color-primary-light)] text-[var(--color-primary)]"
                    : "bg-[var(--color-bg-muted)] text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)]"
                }`}
              >
                <Folder size={12} />
                <span>{tag}</span>
                <span className="text-[11px] opacity-80">{count}</span>
              </button>
            ))}
            {allTagCounts.untaggedCount > 0 && (
              <button
                type="button"
                onClick={() =>
                  setActiveTagFilter((cur) =>
                    cur === "__untagged__" ? "__all__" : "__untagged__",
                  )
                }
                className={`h-7 px-2.5 rounded-md text-xs font-semibold inline-flex items-center gap-1.5 cursor-pointer transition ${
                  activeTagFilter === "__untagged__"
                    ? "bg-[var(--color-primary-light)] text-[var(--color-primary)]"
                    : "bg-[var(--color-bg-muted)] text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)]"
                }`}
              >
                <span>Ungrouped</span>
                <span className="text-[11px] opacity-80">
                  {allTagCounts.untaggedCount}
                </span>
              </button>
            )}
          </div>
        )}

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
                Provision your first project in {workspaceName} to generate an instant Sentry DSN.
              </p>
              {canManageProjects && onOpenCreateProject && (
                <button
                  type="button"
                  onClick={onOpenCreateProject}
                  className="mt-4 h-9 px-4 rounded-[6px] bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] text-white text-xs font-semibold inline-flex items-center gap-1.5 cursor-pointer"
                >
                  <Plus size={14} />
                  <span>Create project</span>
                </button>
              )}
            </div>
          ) : groupedSections ? (
            groupedSections.map((section) => {
              const isOpen = !collapsedGroups.has(section.key);
              return (
                <div key={section.key} className="space-y-3">
                  <button
                    type="button"
                    onClick={() => toggleGroupSection(section.key)}
                    aria-expanded={isOpen}
                    className="w-full flex items-center gap-2 py-1.5 px-2 rounded-md hover:bg-[var(--color-bg-hover)] text-left cursor-pointer transition"
                  >
                    {isOpen ? (
                      <ChevronDown
                        size={15}
                        className="text-[var(--color-text-dim)]"
                      />
                    ) : (
                      <ChevronRight
                        size={15}
                        className="text-[var(--color-text-dim)]"
                      />
                    )}
                    <Folder
                      size={15}
                      className={
                        section.isUntagged
                          ? "text-[var(--color-text-dim)]"
                          : "text-[var(--color-primary)]"
                      }
                    />
                    <span className="text-sm font-bold text-[var(--color-text)]">
                      {section.label}
                    </span>
                    <span className="pm-lozenge">{section.items.length}</span>
                  </button>
                  {isOpen && (
                    <div className="space-y-3 pl-2">
                      {section.items.map((p) =>
                        renderProjectCard(p, `grp-${section.key}`),
                      )}
                    </div>
                  )}
                </div>
              );
            })
          ) : (
            visibleProjects.map((p) => renderProjectCard(p, "flat"))
          )}
        </div>
      </section>

      {/* Right Docked Drawer: SDK Quickstart (project creation lives in the CreateProjectDialog wizard) */}
      <aside className="w-full lg:w-[440px] shrink-0 bg-[var(--color-bg-card)] pm-card-panel p-5 overflow-y-auto max-h-[calc(100vh-56px)]">
        <div className="space-y-3">
          <div>
            <h3 className="text-sm font-semibold text-[var(--color-text)]">
              Sentry SDK quickstart
            </h3>
            <p className="mt-0.5 text-xs text-[var(--color-text-dim)]">
              Any official Sentry SDK works without custom wrappers. Pick a project and platform:
            </p>
          </div>

          {projects.length > 0 && (
            <div>
              <label
                htmlFor="quickstart-project"
                className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1"
              >
                Project DSN
              </label>
              <select
                id="quickstart-project"
                value={quickstartProject?.slug ?? ""}
                onChange={(e) => {
                  setQuickstartSlug(e.target.value);
                  setQuickstartPlatform(null);
                }}
                className="pm-input"
              >
                {projects.map((p) => (
                  <option key={p.id} value={p.slug}>
                    {p.name} ({p.slug})
                  </option>
                ))}
              </select>
            </div>
          )}

          <div className="grid grid-cols-5 gap-1.5" role="tablist" aria-label="SDK platform">
            {ONBOARDING_PLATFORMS.filter((o) => o.id !== "other").map((o) => {
              const active = activeQuickstartPlatform === o.id;
              return (
                <button
                  key={o.id}
                  type="button"
                  role="tab"
                  aria-selected={active}
                  title={o.label}
                  onClick={() => setQuickstartPlatform(o.id)}
                  className={`flex flex-col items-center gap-1 p-1.5 rounded-md border cursor-pointer transition ${
                    active
                      ? "border-[var(--color-primary)] bg-[var(--color-primary-light)]"
                      : "border-transparent hover:bg-[var(--color-bg-hover)]"
                  }`}
                >
                  <PlatformLogo platform={o.id} size={28} />
                  <span
                    className={`text-[10px] font-semibold truncate max-w-full ${
                      active ? "text-[var(--color-primary)]" : "text-[var(--color-text-dim)]"
                    }`}
                  >
                    {o.label.replace("Browser JavaScript", "Browser JS")}
                  </span>
                </button>
              );
            })}
          </div>

          <SdkSetupSteps steps={buildSdkSteps(activeQuickstartPlatform, sampleDsn)} />
        </div>
      </aside>
    </div>
  );
}

import { useCallback, useEffect, useState } from "react";
import { GitCommit, Plus, RefreshCw, Tag } from "lucide-react";
import {
  ApiError,
  type ProjectView,
  type ReleaseView,
  fetchReleases,
  upsertRelease,
} from "../../lib/api";

interface ReleasesViewProps {
  workspaceName: string;
  projects: ProjectView[];
  canTriage: boolean;
  onOpenReleaseIssues: (projectSlug: string, releaseVersion: string) => void;
  onShowToast: (message: string) => void;
}

export function ReleasesView({
  workspaceName,
  projects,
  canTriage,
  onOpenReleaseIssues,
  onShowToast,
}: ReleasesViewProps) {
  const [filterProject, setFilterProject] = useState("");
  const [releases, setReleases] = useState<ReleaseView[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [projectSlug, setProjectSlug] = useState(projects[0]?.slug ?? "");
  const [version, setVersion] = useState("");
  const [commitSha, setCommitSha] = useState("");
  const [environment, setEnvironment] = useState("production");
  const [notes, setNotes] = useState("");
  const [submitting, setSubmitting] = useState(false);

  useEffect(() => {
    if (!projectSlug && projects.length > 0) {
      setProjectSlug(projects[0].slug);
    }
  }, [projects, projectSlug]);

  const loadReleases = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const items = await fetchReleases(filterProject || undefined);
      setReleases(items);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : "Failed to load releases");
    } finally {
      setLoading(false);
    }
  }, [filterProject]);

  useEffect(() => {
    void loadReleases();
  }, [loadReleases]);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!projectSlug || !version.trim()) return;
    setSubmitting(true);
    setError(null);
    try {
      await upsertRelease({
        project_slug: projectSlug,
        version: version.trim(),
        commit_sha: commitSha.trim() || undefined,
        environment: environment.trim() || undefined,
        notes: notes.trim() || undefined,
      });
      setVersion("");
      setCommitSha("");
      setNotes("");
      onShowToast("Release recorded");
      await loadReleases();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : "Failed to save release");
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="flex-1 min-w-0 flex overflow-hidden bg-[var(--color-bg)]">
      {/* Main Releases Table */}
      <section className="flex-1 min-w-0 flex flex-col overflow-hidden">
        <div className="px-6 py-4 border-b border-[var(--color-border)] flex items-center justify-between gap-3">
          <div>
            <div className="text-[11px] font-medium text-[var(--color-text-dim)]">
              {workspaceName} / Releases
            </div>
            <h1 className="text-lg font-bold text-[var(--color-text)] flex items-center gap-2">
              <Tag size={18} className="text-[var(--color-primary)]" />
              <span>Release Tracking</span>
            </h1>
          </div>

          <div className="flex items-center gap-2">
            <select
              value={filterProject}
              onChange={(e) => setFilterProject(e.target.value)}
              aria-label="Filter releases by project"
              className="pm-input h-8 text-xs w-44"
            >
              <option value="">All projects</option>
              {projects.map((p) => (
                <option key={p.id} value={p.slug}>
                  {p.slug}
                </option>
              ))}
            </select>
            <button
              type="button"
              onClick={() => void loadReleases()}
              className="h-8 px-2.5 rounded border border-[var(--color-border-strong)] bg-[var(--color-bg)] hover:bg-[var(--color-bg-hover)] text-xs font-medium text-[var(--color-text)] inline-flex items-center gap-1.5 cursor-pointer"
            >
              <RefreshCw size={13} />
              <span>Refresh</span>
            </button>
          </div>
        </div>

        {error && (
          <div className="mx-6 mt-3 px-3 py-2 rounded bg-[#ffeceb] text-[#ae2e24] text-xs font-medium">
            {error}
          </div>
        )}

        <div className="flex-1 overflow-y-auto p-6">
          {loading ? (
            <div className="text-xs text-[var(--color-text-dim)]">Loading releases...</div>
          ) : releases.length === 0 ? (
            <div className="pm-card-panel p-6 text-center space-y-1">
              <div className="text-sm font-semibold text-[var(--color-text)]">
                No releases tracked yet
              </div>
              <p className="text-xs text-[var(--color-text-dim)]">
                Releases appear automatically when Sentry SDK events include a{" "}
                <code className="pm-mono">release</code> tag, or you can register one with its Git
                commit SHA on the right.
              </p>
            </div>
          ) : (
            <div className="border border-[var(--color-border)] rounded-lg overflow-hidden bg-[var(--color-bg)]">
              <table className="w-full text-left border-collapse">
                <thead>
                  <tr className="bg-[var(--color-bg-muted)] border-b border-[var(--color-border)] text-[11px] font-bold uppercase tracking-wider text-[var(--color-text-dim)]">
                    <th className="py-2.5 px-3">Release</th>
                    <th className="py-2.5 px-3">Project</th>
                    <th className="py-2.5 px-3">Commit SHA</th>
                    <th className="py-2.5 px-3">Env</th>
                    <th className="py-2.5 px-3 text-right">Events</th>
                    <th className="py-2.5 px-3 text-right">New / Linked Issues</th>
                    <th className="py-2.5 px-3 text-right">Last seen</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-[var(--color-border)] text-xs">
                  {releases.map((rel) => (
                    <tr
                      key={rel.id}
                      onClick={() => onOpenReleaseIssues(rel.project_slug, rel.version)}
                      className="hover:bg-[var(--color-bg-hover)] cursor-pointer transition"
                    >
                      <td className="py-2.5 px-3 font-semibold text-[var(--color-text)] pm-mono">
                        <div>{rel.version}</div>
                        {rel.notes && (
                          <div className="font-sans font-normal text-[11px] text-[var(--color-text-dim)] truncate max-w-[260px]">
                            {rel.notes}
                          </div>
                        )}
                      </td>
                      <td className="py-2.5 px-3">
                        <span className="pm-lozenge">{rel.project_slug}</span>
                      </td>
                      <td className="py-2.5 px-3 pm-mono text-[11px] text-[var(--color-text-muted)]">
                        {rel.commit_sha ? (
                          <span className="inline-flex items-center gap-1">
                            <GitCommit size={12} />
                            {rel.commit_sha.slice(0, 10)}
                          </span>
                        ) : (
                          "—"
                        )}
                      </td>
                      <td className="py-2.5 px-3">
                        {rel.environment ? (
                          <span className="pm-lozenge pm-lozenge-info">{rel.environment}</span>
                        ) : (
                          "—"
                        )}
                      </td>
                      <td className="py-2.5 px-3 text-right font-semibold pm-mono">
                        {rel.event_count.toLocaleString()}
                      </td>
                      <td className="py-2.5 px-3 text-right pm-mono">
                        <span className="text-[#ae2e24] font-semibold">
                          +{rel.new_issues_count}
                        </span>{" "}
                        <span className="text-[var(--color-text-dim)]">
                          / {rel.linked_issues_count}
                        </span>
                      </td>
                      <td className="py-2.5 px-3 text-right text-[var(--color-text-dim)]">
                        {new Date(rel.last_seen).toLocaleString()}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      </section>

      {/* Right Drawer: Register / Annotate Release */}
      {canTriage && (
        <aside className="w-[340px] shrink-0 border-l border-[var(--color-border)] bg-[var(--color-bg-muted)] p-5 overflow-y-auto">
          <h2 className="text-sm font-bold text-[var(--color-text)] flex items-center gap-1.5 mb-1">
            <Plus size={15} />
            <span>Register or annotate release</span>
          </h2>
          <p className="text-xs text-[var(--color-text-dim)] mb-4">
            Map a release version to its Git commit SHA and deployment notes (also used by MCP AI
            triage).
          </p>

          <form onSubmit={(e) => void handleSubmit(e)} className="space-y-3">
            <div>
              <label className="block text-xs font-semibold text-[var(--color-text)] mb-1">
                Project
              </label>
              <select
                value={projectSlug}
                onChange={(e) => setProjectSlug(e.target.value)}
                required
                className="pm-input h-9 text-xs"
              >
                {projects.map((p) => (
                  <option key={p.id} value={p.slug}>
                    {p.name} ({p.slug})
                  </option>
                ))}
              </select>
            </div>

            <div>
              <label className="block text-xs font-semibold text-[var(--color-text)] mb-1">
                Version
              </label>
              <input
                type="text"
                value={version}
                onChange={(e) => setVersion(e.target.value)}
                placeholder="e.g. api@2.4.0 or a1b2c3d"
                required
                className="pm-input h-9 text-xs pm-mono"
              />
            </div>

            <div>
              <label className="block text-xs font-semibold text-[var(--color-text)] mb-1">
                Commit SHA (optional)
              </label>
              <input
                type="text"
                value={commitSha}
                onChange={(e) => setCommitSha(e.target.value)}
                placeholder="7-40 hex chars (auto-inferred if hex)"
                className="pm-input h-9 text-xs pm-mono"
              />
            </div>

            <div>
              <label className="block text-xs font-semibold text-[var(--color-text)] mb-1">
                Environment
              </label>
              <input
                type="text"
                value={environment}
                onChange={(e) => setEnvironment(e.target.value)}
                placeholder="production"
                className="pm-input h-9 text-xs"
              />
            </div>

            <div>
              <label className="block text-xs font-semibold text-[var(--color-text)] mb-1">
                Deployment notes
              </label>
              <textarea
                value={notes}
                onChange={(e) => setNotes(e.target.value)}
                rows={3}
                placeholder="Summary of changes in this release..."
                className="pm-input text-xs py-2"
              />
            </div>

            <button
              type="submit"
              disabled={submitting || projects.length === 0}
              className="w-full h-9 rounded bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-50 text-white text-xs font-semibold cursor-pointer"
            >
              {submitting ? "Saving..." : "Save release"}
            </button>
          </form>
        </aside>
      )}
    </div>
  );
}

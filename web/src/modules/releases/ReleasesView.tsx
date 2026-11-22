import { useCallback, useEffect, useState } from "react";
import {
  FileCode2,
  GitCommit,
  Plus,
  RefreshCw,
  Tag,
  Trash2,
  Upload,
} from "lucide-react";
import {
  ApiError,
  type ProjectView,
  type ReleaseView,
  type SourceMapView,
  deleteSourceMap,
  fetchReleases,
  fetchSourceMaps,
  uploadSourceMap,
  upsertRelease,
} from "../../lib/api";

interface ReleasesViewProps {
  workspaceName: string;
  projects: ProjectView[];
  canTriage: boolean;
  onOpenReleaseIssues: (projectSlug: string, releaseVersion: string) => void;
  onShowToast: (message: string) => void;
}

type ReleasesSubTab = "releases" | "sourcemaps";

function formatBytes(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${bytes} B`;
}

export function ReleasesView({
  workspaceName,
  projects,
  canTriage,
  onOpenReleaseIssues,
  onShowToast,
}: ReleasesViewProps) {
  const [activeTab, setActiveTab] = useState<ReleasesSubTab>("releases");
  const [filterProject, setFilterProject] = useState("");
  const [releases, setReleases] = useState<ReleaseView[]>([]);
  const [sourceMaps, setSourceMaps] = useState<SourceMapView[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Release form state
  const [projectSlug, setProjectSlug] = useState(projects[0]?.slug ?? "");
  const [version, setVersion] = useState("");
  const [commitSha, setCommitSha] = useState("");
  const [environment, setEnvironment] = useState("production");
  const [notes, setNotes] = useState("");
  const [submitting, setSubmitting] = useState(false);

  // Source map upload form state
  const [smProjectSlug, setSmProjectSlug] = useState(projects[0]?.slug ?? "");
  const [smRelease, setSmRelease] = useState("");
  const [smFilename, setSmFilename] = useState("");
  const [smDebugId, setSmDebugId] = useState("");
  const [smContent, setSmContent] = useState("");
  const [smFileNameDisplay, setSmFileNameDisplay] = useState("");
  const [smSubmitting, setSmSubmitting] = useState(false);

  useEffect(() => {
    if (!projectSlug && projects.length > 0) {
      setProjectSlug(projects[0].slug);
    }
    if (!smProjectSlug && projects.length > 0) {
      setSmProjectSlug(projects[0].slug);
    }
  }, [projects, projectSlug, smProjectSlug]);

  const loadData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [relItems, smItems] = await Promise.all([
        fetchReleases(filterProject || undefined),
        fetchSourceMaps(filterProject || undefined),
      ]);
      setReleases(relItems);
      setSourceMaps(smItems);
    } catch (err) {
      setError(
        err instanceof ApiError ? err.message : "Failed to load releases & source maps",
      );
    } finally {
      setLoading(false);
    }
  }, [filterProject]);

  useEffect(() => {
    void loadData();
  }, [loadData]);

  async function handleSubmitRelease(e: React.FormEvent) {
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
      await loadData();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : "Failed to save release");
    } finally {
      setSubmitting(false);
    }
  }

  function handleSelectMapFile(e: React.ChangeEvent<HTMLInputElement>) {
    const file = e.target.files?.[0];
    if (!file) return;
    setSmFileNameDisplay(file.name);
    if (!smFilename.trim()) {
      const inferred = file.name.endsWith(".map")
        ? `~/assets/${file.name.slice(0, -4)}`
        : `~/assets/${file.name}`;
      setSmFilename(inferred);
    }
    const reader = new FileReader();
    reader.onload = () => {
      if (typeof reader.result === "string") {
        setSmContent(reader.result);
      }
    };
    reader.readAsText(file);
  }

  async function handleUploadSourceMap(e: React.FormEvent) {
    e.preventDefault();
    if (!smProjectSlug || !smRelease.trim() || !smFilename.trim() || !smContent.trim()) {
      return;
    }
    setSmSubmitting(true);
    setError(null);
    try {
      await uploadSourceMap({
        project_slug: smProjectSlug,
        release_version: smRelease.trim(),
        filename: smFilename.trim(),
        debug_id: smDebugId.trim() || undefined,
        content: smContent,
      });
      setSmFilename("");
      setSmDebugId("");
      setSmContent("");
      setSmFileNameDisplay("");
      onShowToast("Source map uploaded & cached for ingest resolution");
      await loadData();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : "Failed to upload source map");
    } finally {
      setSmSubmitting(false);
    }
  }

  async function handleDeleteSourceMap(id: number) {
    setError(null);
    try {
      await deleteSourceMap(id);
      onShowToast("Source map deleted");
      await loadData();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : "Failed to delete source map");
    }
  }

  return (
    <div className="flex-1 min-w-0 flex overflow-hidden max-lg:flex-col max-lg:overflow-y-auto bg-[var(--color-bg)]">
      {/* Main Releases & Source Maps Area */}
      <section className="flex-1 min-w-0 flex flex-col overflow-hidden max-lg:flex-none max-lg:overflow-visible">
        <div className="px-4 md:px-6 pt-4 pb-3 border-b border-[var(--color-border)]">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="min-w-0">
              <div className="text-[11px] font-medium text-[var(--color-text-dim)]">
                {workspaceName} / Releases &amp; Source Maps
              </div>
              <h1 className="text-xl md:text-2xl font-semibold tracking-tight text-[var(--color-text)] flex items-center gap-2">
                <Tag size={18} className="shrink-0 text-[var(--color-primary)]" />
                <span>Release Tracking &amp; React Source Maps</span>
              </h1>
            </div>

            <div className="flex items-center gap-2">
              <select
                value={filterProject}
                onChange={(e) => setFilterProject(e.target.value)}
                aria-label="Filter by project"
                className="pm-input h-8 text-sm w-44"
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
                onClick={() => void loadData()}
                className="h-8 px-2.5 rounded border border-[var(--color-border-strong)] bg-[var(--color-bg)] hover:bg-[var(--color-bg-hover)] text-sm font-medium text-[var(--color-text)] inline-flex items-center gap-1.5 cursor-pointer"
              >
                <RefreshCw size={13} />
                <span>Refresh</span>
              </button>
            </div>
          </div>

          <div className="mt-3 flex items-center gap-4">
            <button
              type="button"
              onClick={() => setActiveTab("releases")}
              className={`pm-tab cursor-pointer ${
                activeTab === "releases"
                  ? "pm-tab-active text-[var(--color-primary)]"
                  : "text-[var(--color-text-muted)]"
              }`}
            >
              Releases ({releases.length})
            </button>
            <button
              type="button"
              onClick={() => setActiveTab("sourcemaps")}
              className={`pm-tab cursor-pointer inline-flex items-center gap-1.5 ${
                activeTab === "sourcemaps"
                  ? "pm-tab-active text-[var(--color-primary)]"
                  : "text-[var(--color-text-muted)]"
              }`}
            >
              <FileCode2 size={13} />
              <span>Source Maps ({sourceMaps.length})</span>
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
            <div className="text-xs text-[var(--color-text-dim)]">Loading...</div>
          ) : activeTab === "releases" ? (
            releases.length === 0 ? (
              <div className="pm-card-panel p-6 text-center space-y-1">
                <div className="text-sm font-semibold text-[var(--color-text)]">
                  No releases tracked yet
                </div>
                <p className="text-xs text-[var(--color-text-dim)]">
                  Releases appear automatically when Sentry SDK events include a{" "}
                  <code className="pm-mono">release</code> tag, or you can register one with its
                  Git commit SHA on the right.
                </p>
              </div>
            ) : (
              <div className="border border-[var(--color-border)] rounded-lg overflow-x-auto bg-[var(--color-bg)]">
                <table className="w-full max-md:min-w-[640px] text-left border-collapse">
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
                          <div className="whitespace-nowrap">{rel.version}</div>
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
            )
          ) : sourceMaps.length === 0 ? (
            <div className="pm-card-panel p-6 text-center space-y-1">
              <div className="text-sm font-semibold text-[var(--color-text)]">
                No source maps uploaded yet
              </div>
              <p className="text-xs text-[var(--color-text-dim)] max-w-md mx-auto">
                Upload Vite/Webpack <code className="pm-mono">.js.map</code> artifacts per release
                (matched by <code className="pm-mono">release + filename</code> or{" "}
                <code className="pm-mono">debug_id</code>). Ingest resolves minified React frames
                via a memory-bounded LRU cache before fingerprinting.
              </p>
            </div>
          ) : (
            <div className="border border-[var(--color-border)] rounded-lg overflow-x-auto bg-[var(--color-bg)]">
              <table className="w-full max-md:min-w-[640px] text-left border-collapse">
                <thead>
                  <tr className="bg-[var(--color-bg-muted)] border-b border-[var(--color-border)] text-[11px] font-bold uppercase tracking-wider text-[var(--color-text-dim)]">
                    <th className="py-2.5 px-3">Bundle Filename</th>
                    <th className="py-2.5 px-3">Release</th>
                    <th className="py-2.5 px-3">Project</th>
                    <th className="py-2.5 px-3">Debug ID</th>
                    <th className="py-2.5 px-3 text-right">Size (zstd)</th>
                    <th className="py-2.5 px-3 text-right">Updated</th>
                    {canTriage && <th className="py-2.5 px-3 text-right">Action</th>}
                  </tr>
                </thead>
                <tbody className="divide-y divide-[var(--color-border)] text-xs">
                  {sourceMaps.map((sm) => (
                    <tr key={sm.id} className="hover:bg-[var(--color-bg-hover)]">
                      <td className="py-2.5 px-3 pm-mono font-semibold text-[var(--color-text)]">
                        <div>{sm.filename}</div>
                        <div className="text-[10px] font-normal text-[var(--color-text-dim)]">
                          sha256: {sm.sha256.slice(0, 12)}
                        </div>
                      </td>
                      <td className="py-2.5 px-3 pm-mono text-[11px]">{sm.release_version}</td>
                      <td className="py-2.5 px-3">
                        <span className="pm-lozenge">{sm.project_slug}</span>
                      </td>
                      <td className="py-2.5 px-3 pm-mono text-[11px] text-[var(--color-text-muted)]">
                        {sm.debug_id ? sm.debug_id.slice(0, 18) : "—"}
                      </td>
                      <td className="py-2.5 px-3 text-right pm-mono text-[11px]">
                        <div>{formatBytes(sm.raw_size_bytes)}</div>
                        <div className="text-[10px] text-[var(--color-text-dim)]">
                          zstd: {formatBytes(sm.compressed_size_bytes)}
                        </div>
                      </td>
                      <td className="py-2.5 px-3 text-right text-[var(--color-text-dim)]">
                        {new Date(sm.updated_at).toLocaleString()}
                      </td>
                      {canTriage && (
                        <td className="py-2.5 px-3 text-right">
                          <button
                            type="button"
                            onClick={() => void handleDeleteSourceMap(sm.id)}
                            className="p-1.5 rounded text-[var(--color-text-dim)] hover:text-[var(--color-danger)] hover:bg-[#ffeceb] cursor-pointer"
                            title="Delete source map"
                          >
                            <Trash2 size={14} />
                          </button>
                        </td>
                      )}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      </section>

      {/* Right Drawer: Register Release OR Upload Source Map */}
      {canTriage && (
        <aside className="w-full lg:w-[360px] shrink-0 max-lg:border-t lg:border-l border-[var(--color-border)] bg-[var(--color-bg-muted)] p-5 overflow-y-auto">
          {activeTab === "releases" ? (
            <>
              <h2 className="text-sm font-bold text-[var(--color-text)] flex items-center gap-1.5 mb-1">
                <Plus size={15} />
                <span>Register or annotate release</span>
              </h2>
              <p className="text-xs text-[var(--color-text-dim)] mb-4">
                Map a release version to its Git commit SHA and deployment notes (also used by MCP
                AI triage).
              </p>

              <form onSubmit={(e) => void handleSubmitRelease(e)} className="space-y-3">
                <div>
                  <label className="block text-xs font-semibold text-[var(--color-text)] mb-1">
                    Project
                  </label>
                  <select
                    value={projectSlug}
                    onChange={(e) => setProjectSlug(e.target.value)}
                    required
                    className="pm-input h-9 text-sm"
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
                    placeholder="e.g. web@2.4.0 or a1b2c3d"
                    required
                    className="pm-input h-9 text-sm pm-mono"
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
                    className="pm-input h-9 text-sm pm-mono"
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
                    className="pm-input h-9 text-sm"
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
                  className="w-full h-9 rounded bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-50 text-white text-sm font-semibold cursor-pointer"
                >
                  {submitting ? "Saving..." : "Save release"}
                </button>
              </form>
            </>
          ) : (
            <>
              <h2 className="text-sm font-bold text-[var(--color-text)] flex items-center gap-1.5 mb-1">
                <Upload size={15} />
                <span>Upload React / JS Source Map</span>
              </h2>
              <p className="text-xs text-[var(--color-text-dim)] mb-4">
                Attach a Source Map v3 (<code className="pm-mono">.js.map</code>) to a release.
                Stored zstd-compressed and resolved lazily via bounded LRU during ingest.
              </p>

              <form onSubmit={(e) => void handleUploadSourceMap(e)} className="space-y-3">
                <div>
                  <label className="block text-xs font-semibold text-[var(--color-text)] mb-1">
                    Project
                  </label>
                  <select
                    value={smProjectSlug}
                    onChange={(e) => setSmProjectSlug(e.target.value)}
                    required
                    className="pm-input h-9 text-sm"
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
                    Release version
                  </label>
                  <input
                    type="text"
                    value={smRelease}
                    onChange={(e) => setSmRelease(e.target.value)}
                    placeholder="e.g. web@2.4.0"
                    required
                    className="pm-input h-9 text-sm pm-mono"
                  />
                </div>

                <div>
                  <label className="block text-xs font-semibold text-[var(--color-text)] mb-1">
                    Source Map file (.map)
                  </label>
                  <input
                    type="file"
                    accept=".map,.json,application/json"
                    onChange={handleSelectMapFile}
                    className="block w-full text-xs text-[var(--color-text-muted)] file:mr-2 file:py-1.5 file:px-3 file:rounded file:border file:border-[var(--color-border-strong)] file:text-xs file:font-semibold file:bg-[var(--color-bg)] file:cursor-pointer"
                  />
                  {smFileNameDisplay && (
                    <div className="mt-1 text-[11px] text-[var(--color-text-dim)] pm-mono truncate">
                      Loaded: {smFileNameDisplay} ({formatBytes(smContent.length)})
                    </div>
                  )}
                </div>

                <div>
                  <label className="block text-xs font-semibold text-[var(--color-text)] mb-1">
                    Bundle filename / URL path
                  </label>
                  <input
                    type="text"
                    value={smFilename}
                    onChange={(e) => setSmFilename(e.target.value)}
                    placeholder="~/assets/index-abc123.js"
                    required
                    className="pm-input h-9 text-sm pm-mono"
                  />
                </div>

                <div>
                  <label className="block text-xs font-semibold text-[var(--color-text)] mb-1">
                    Debug ID (optional — auto-detected from map)
                  </label>
                  <input
                    type="text"
                    value={smDebugId}
                    onChange={(e) => setSmDebugId(e.target.value)}
                    placeholder="UUID from Sentry bundler plugin"
                    className="pm-input h-9 text-sm pm-mono"
                  />
                </div>

                <button
                  type="submit"
                  disabled={smSubmitting || !smContent || projects.length === 0}
                  className="w-full h-9 rounded bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-50 text-white text-sm font-semibold cursor-pointer"
                >
                  {smSubmitting ? "Uploading..." : "Upload source map"}
                </button>
              </form>
            </>
          )}
        </aside>
      )}
    </div>
  );
}

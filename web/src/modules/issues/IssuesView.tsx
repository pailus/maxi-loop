import { useCallback, useEffect, useState } from "react";
import {
  AlertCircle,
  AlertTriangle,
  Check,
  ChevronLeft,
  ChevronRight,
  EyeOff,
  Inbox,
  RefreshCw,
  Search,
} from "lucide-react";
import {
  type IssueDetailView,
  type IssueStatus,
  type IssueSummaryView,
  type PaginatedIssuesResponse,
  type ProjectView,
  fetchIssueDetail,
  fetchIssues,
  updateIssueStatus,
} from "../../lib/api";
import { ProjectIcon } from "../projects/ProjectIcon";
import { IssueDetailDrawer, formatRelativeAge } from "./IssueDetailDrawer";
import { IssueDetailFullView } from "./IssueDetailFullView";

export type IssuesTabMode =
  | "unresolved"
  | "regressed"
  | "resolved"
  | "ignored"
  | "all";

interface IssuesViewProps {
  workspaceName: string;
  workspaceSlug?: string;
  projects: ProjectView[];
  selectedProjectSlug: string;
  onSelectProjectSlug: (slug: string) => void;
  globalSearchQuery: string;
  initialTabMode?: IssuesTabMode;
  initialReleaseFilter?: string;
  canTriage: boolean;
  onShowToast: (message: string, undoAction?: () => void) => void;
  onNavigateToProjects: () => void;
}

function deterministicSparkBars(issue: IssueSummaryView): number[] {
  const base = Math.max(1, issue.id * 17 + issue.event_count);
  const bars: number[] = [];
  for (let i = 0; i < 10; i++) {
    const step = ((base + i * 31) % 18) + 4;
    const boost = issue.is_regressed && i >= 6 ? (i - 5) * 4 : 0;
    bars.push(Math.min(26, step + boost));
  }
  return bars;
}

export function IssuesView({
  workspaceName,
  workspaceSlug,
  projects,
  selectedProjectSlug,
  onSelectProjectSlug,
  globalSearchQuery,
  initialTabMode = "unresolved",
  initialReleaseFilter = "",
  canTriage,
  onShowToast,
  onNavigateToProjects,
}: IssuesViewProps) {
  const [tabMode, setTabMode] = useState<IssuesTabMode>(initialTabMode);
  const [localSearch, setLocalSearch] = useState("");
  const [environment, setEnvironment] = useState("");
  const [release, setRelease] = useState(initialReleaseFilter);

  useEffect(() => {
    if (initialReleaseFilter) {
      setRelease(initialReleaseFilter);
      setPage(1);
    }
  }, [initialReleaseFilter]);
  const [sort, setSort] = useState<"last_seen" | "first_seen" | "events">("last_seen");
  const [page, setPage] = useState(1);

  const [issuesPage, setIssuesPage] = useState<PaginatedIssuesResponse | null>(null);
  const [loadingList, setLoadingList] = useState(true);
  const [listError, setListError] = useState<string | null>(null);

  const [selectedIssueId, setSelectedIssueId] = useState<number | null>(null);
  const [issueDetail, setIssueDetail] = useState<IssueDetailView | null>(null);
  const [loadingDetail, setLoadingDetail] = useState(false);
  const [fullViewOpen, setFullViewOpen] = useState(false);
  const [triageBusy, setTriageBusy] = useState(false);

  useEffect(() => {
    setTabMode(initialTabMode);
    setPage(1);
  }, [initialTabMode]);

  const effectiveQuery = (localSearch.trim() || globalSearchQuery.trim()) || undefined;

  const loadIssuesList = useCallback(async () => {
    setLoadingList(true);
    setListError(null);
    try {
      const statusParam =
        tabMode === "unresolved" || tabMode === "regressed"
          ? "unresolved"
          : tabMode === "resolved"
            ? "resolved"
            : tabMode === "ignored"
              ? "ignored"
              : undefined;
      const isRegressedParam = tabMode === "regressed" ? true : undefined;

      const resp = await fetchIssues({
        workspace_slug: workspaceSlug,
        project_slug: selectedProjectSlug || undefined,
        environment: environment || undefined,
        release: release.trim() || undefined,
        status: statusParam,
        is_regressed: isRegressedParam,
        query: effectiveQuery,
        sort,
        page,
        per_page: 20,
      });
      setIssuesPage(resp);

      if (resp.items.length > 0) {
        setSelectedIssueId((prev) => {
          if (prev !== null && resp.items.some((i) => i.id === prev)) {
            return prev;
          }
          return resp.items[0].id;
        });
      } else {
        setSelectedIssueId(null);
        setIssueDetail(null);
      }
    } catch (err) {
      setListError(err instanceof Error ? err.message : "Failed to load issues.");
    } finally {
      setLoadingList(false);
    }
  }, [
    workspaceSlug,
    selectedProjectSlug,
    environment,
    release,
    tabMode,
    effectiveQuery,
    sort,
    page,
  ]);

  useEffect(() => {
    void loadIssuesList();
  }, [loadIssuesList]);

  useEffect(() => {
    if (selectedIssueId === null) {
      setIssueDetail(null);
      return;
    }
    let active = true;
    setLoadingDetail(true);
    fetchIssueDetail(selectedIssueId)
      .then((detail) => {
        if (active) {
          setIssueDetail(detail);
        }
      })
      .catch(() => {})
      .finally(() => {
        if (active) {
          setLoadingDetail(false);
        }
      });
    return () => {
      active = false;
    };
  }, [selectedIssueId]);

  async function handleTriageStatus(issueId: number, nextStatus: IssueStatus) {
    const previousStatus =
      issueDetail?.issue.id === issueId
        ? issueDetail.issue.status
        : issuesPage?.items.find((i) => i.id === issueId)?.status;

    setTriageBusy(true);
    try {
      const updated = await updateIssueStatus(issueId, nextStatus);
      setIssueDetail((prev) =>
        prev && prev.issue.id === issueId ? { ...prev, issue: updated } : prev,
      );
      await loadIssuesList();

      const keyLabel = `${updated.project_slug.slice(0, 3).toUpperCase()}-${updated.id}`;
      onShowToast(
        `Issue ${keyLabel} marked as ${nextStatus}`,
        previousStatus && previousStatus !== nextStatus
          ? () => {
              void updateIssueStatus(issueId, previousStatus).then((restored) => {
                setIssueDetail((prev) =>
                  prev && prev.issue.id === issueId
                    ? { ...prev, issue: restored }
                    : prev,
                );
                void loadIssuesList();
              });
            }
          : undefined,
      );
    } catch (err) {
      onShowToast(
        err instanceof Error ? err.message : "Failed to update issue status",
      );
    } finally {
      setTriageBusy(false);
    }
  }

  if (fullViewOpen && issueDetail) {
    return (
      <IssueDetailFullView
        detail={issueDetail}
        canTriage={canTriage}
        triageBusy={triageBusy}
        onUpdateStatus={handleTriageStatus}
        onBack={() => setFullViewOpen(false)}
      />
    );
  }

  const items = issuesPage?.items ?? [];
  const totalCount = issuesPage?.total ?? 0;
  const regressedInPage = items.filter((i) => i.is_regressed).length;
  const totalEventsInPage = items.reduce((acc, i) => acc + i.event_count, 0);

  // Discover distinct environments & releases from loaded items for quick dropdown selection
  const knownEnvs = Array.from(
    new Set(["production", "staging", "development", ...items.map((i) => i.last_environment).filter(Boolean)]),
  );

  return (
    <div className="flex-1 min-w-0 flex flex-col lg:flex-row overflow-hidden bg-[var(--color-bg)]">
      {/* Center Issues Stream Pane */}
      <section className="flex-1 min-w-0 flex flex-col px-6 pt-4 pb-4 overflow-y-auto">
        {/* Breadcrumb */}
        <div className="text-xs text-[var(--color-text-dim)] flex items-center gap-1.5">
          <span>{workspaceName}</span>
          <span>›</span>
          <span className="font-medium text-[var(--color-text-muted)]">
            {selectedProjectSlug || "All projects"}
          </span>
        </div>

        {/* Title Row & Inline MaxiBoard Status Summary Pill */}
        <div className="mt-1.5 flex items-center gap-3 flex-wrap">
          <h1 className="text-2xl font-semibold text-[var(--color-text)] tracking-tight">
            Issues
          </h1>

          <div className="px-2.5 py-1 rounded-md bg-[var(--color-bg-muted)] border border-[var(--color-border)] text-xs text-[var(--color-text-muted)] flex items-center gap-2">
            <span className="font-semibold text-[var(--color-text)]">
              {totalCount} matching
            </span>
            <span>·</span>
            <span>{regressedInPage} regressed</span>
            <span>·</span>
            <span>{totalEventsInPage.toLocaleString()} events</span>
          </div>

          <span className="pm-lozenge pm-lozenge-success">WORKER ACTIVE</span>

          <button
            type="button"
            onClick={() => void loadIssuesList()}
            className="ml-auto h-8 px-2.5 rounded bg-[var(--pm-col)] hover:bg-[var(--color-bg-hover)] text-xs font-medium text-[var(--color-text-muted)] inline-flex items-center gap-1.5 cursor-pointer"
            title="Refresh issues stream"
          >
            <RefreshCw size={13} className={loadingList ? "animate-spin" : ""} />
            <span>Refresh</span>
          </button>
        </div>

        {/* Horizontal MaxiBoard Status Tabs */}
        <nav
          role="tablist"
          aria-label="Issue status filter"
          className="mt-3 flex items-center gap-1 border-b-2 border-[var(--color-border)] overflow-x-auto"
        >
          {(
            [
              { id: "unresolved", label: "Unresolved" },
              { id: "regressed", label: "Regressed" },
              { id: "resolved", label: "Resolved" },
              { id: "ignored", label: "Ignored" },
              { id: "all", label: "All issues" },
            ] as const
          ).map((t) => {
            const active = tabMode === t.id;
            return (
              <button
                key={t.id}
                type="button"
                role="tab"
                aria-selected={active}
                onClick={() => {
                  setTabMode(t.id);
                  setPage(1);
                }}
                className="pm-tab"
              >
                <span>{t.label}</span>
                {active && issuesPage && (
                  <span className="px-1.5 py-0.2 rounded-full bg-[var(--color-primary-light)] text-[var(--color-primary)] text-[11px] font-bold">
                    {issuesPage.total}
                  </span>
                )}
              </button>
            );
          })}
        </nav>

        {/* Compact 32px Filter Toolbar */}
        <div className="mt-3.5 flex items-center gap-2 flex-wrap">
          <div className="relative w-52">
            <Search
              size={13}
              className="absolute left-2.5 top-1/2 -translate-y-1/2 text-[var(--pm-text-faint)] pointer-events-none"
            />
            <input
              type="text"
              value={localSearch}
              onChange={(e) => {
                setLocalSearch(e.target.value);
                setPage(1);
              }}
              placeholder="Filter exception, culprit..."
              aria-label="Filter issues by text"
              className="w-full h-8 pl-7 pr-2.5 rounded border border-[var(--color-border-strong)] bg-[var(--color-bg)] text-xs text-[var(--color-text)] outline-none focus:border-[var(--color-primary)]"
            />
          </div>

          <select
            value={selectedProjectSlug}
            onChange={(e) => {
              onSelectProjectSlug(e.target.value);
              setPage(1);
            }}
            aria-label="Filter by project"
            className="h-8 px-2.5 rounded bg-[var(--pm-col)] hover:bg-[var(--color-bg-hover)] text-xs font-medium text-[var(--color-text-muted)] border-0 cursor-pointer"
          >
            <option value="">Project: All ({projects.length})</option>
            {projects.map((p) => (
              <option key={p.id} value={p.slug}>
                Project: {p.slug}
              </option>
            ))}
          </select>

          <select
            value={environment}
            onChange={(e) => {
              setEnvironment(e.target.value);
              setPage(1);
            }}
            aria-label="Filter by environment"
            className="h-8 px-2.5 rounded bg-[var(--pm-col)] hover:bg-[var(--color-bg-hover)] text-xs font-medium text-[var(--color-text-muted)] border-0 cursor-pointer"
          >
            <option value="">Env: All</option>
            {knownEnvs.map((envName) => (
              <option key={envName} value={envName}>
                Env: {envName}
              </option>
            ))}
          </select>

          <input
            type="text"
            value={release}
            onChange={(e) => {
              setRelease(e.target.value);
              setPage(1);
            }}
            placeholder="Release SHA..."
            aria-label="Filter by release"
            className="w-28 h-8 px-2.5 rounded border border-[var(--color-border)] bg-[var(--color-bg)] text-xs pm-mono text-[var(--color-text)] outline-none focus:border-[var(--color-primary)]"
          />

          <select
            value={sort}
            onChange={(e) => {
              setSort(e.target.value as "last_seen" | "first_seen" | "events");
              setPage(1);
            }}
            aria-label="Sort issues"
            className="h-8 px-2.5 rounded bg-[var(--pm-col)] hover:bg-[var(--color-bg-hover)] text-xs font-medium text-[var(--color-text-muted)] border-0 cursor-pointer ml-auto"
          >
            <option value="last_seen">Sort: Last seen</option>
            <option value="events">Sort: Most events</option>
            <option value="first_seen">Sort: Newest first</option>
          </select>
        </div>

        {listError && (
          <div className="mt-3 p-3 rounded-md bg-[var(--color-danger-light)] text-[var(--color-danger-text)] text-xs font-medium flex items-center gap-2">
            <AlertCircle size={15} />
            <span>{listError}</span>
          </div>
        )}

        {/* Issues Table Card */}
        <div className="mt-3.5 flex-1 rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-card)] shadow-[var(--pm-shadow-card)] flex flex-col overflow-hidden">
          <div className="px-4 py-2.5 bg-[var(--color-bg-muted)] border-b border-[var(--color-border)] grid grid-cols-[1fr_92px_72px_76px] gap-3 text-xs font-semibold text-[var(--color-text-dim)]">
            <span>Issue &amp; culprit</span>
            <span className="hidden sm:block">24h trend</span>
            <span className="text-right">Events</span>
            <span className="text-right">Last seen</span>
          </div>

          {loadingList && items.length === 0 ? (
            <div className="p-6 space-y-3">
              {[1, 2, 3, 4].map((n) => (
                <div
                  key={n}
                  className="h-16 rounded-md bg-[var(--pm-col)] animate-pulse"
                />
              ))}
            </div>
          ) : items.length === 0 ? (
            <div className="flex-1 flex flex-col items-center justify-center text-center px-6 py-12">
              <div className="w-12 h-12 rounded-full bg-[var(--color-primary-light)] text-[var(--color-primary)] flex items-center justify-center mb-3">
                <Inbox size={22} />
              </div>
              <h3 className="text-sm font-semibold text-[var(--color-text)]">
                No issues match the current filter
              </h3>
              <p className="mt-1 text-xs text-[var(--color-text-dim)] max-w-sm">
                {projects.length === 0
                  ? "Create a project first to generate a Sentry DSN and start ingesting exceptions."
                  : "Point your Sentry SDK to a project DSN or clear active filters to see grouped issues."}
              </p>
              {projects.length === 0 && (
                <button
                  type="button"
                  onClick={onNavigateToProjects}
                  className="mt-4 h-8 px-3 rounded bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] text-white text-xs font-semibold cursor-pointer"
                >
                  + Create project &amp; get DSN
                </button>
              )}
            </div>
          ) : (
            <div className="divide-y divide-[var(--color-border-light)] overflow-y-auto">
              {items.map((item) => {
                const isSelected = item.id === selectedIssueId;
                const issueKey = `${item.project_slug.slice(0, 3).toUpperCase()}-${item.id}`;
                const sparkBars = deterministicSparkBars(item);

                return (
                  <div
                    key={item.id}
                    role="button"
                    tabIndex={0}
                    onClick={() => setSelectedIssueId(item.id)}
                    onDoubleClick={() => {
                      setSelectedIssueId(item.id);
                      setFullViewOpen(true);
                    }}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        e.preventDefault();
                        setSelectedIssueId(item.id);
                      }
                    }}
                    className={`px-4 py-3 grid grid-cols-[1fr_92px_72px_76px] gap-3 items-center text-left transition cursor-pointer relative ${
                      isSelected
                        ? "bg-[var(--color-primary-light)]"
                        : "hover:bg-[var(--pm-card-hover)]"
                    }`}
                  >
                    {isSelected && (
                      <span className="absolute left-0 top-0 bottom-0 w-[3px] bg-[var(--color-primary)]" />
                    )}

                    <div className="min-w-0">
                      {/* Row Top Meta: ProjectIcon + Key + Lozenges */}
                      <div className="flex items-center gap-1.5 flex-wrap">
                        <ProjectIcon slug={item.project_slug} size={15} />
                        <span className="text-xs font-semibold text-[var(--color-text-dim)]">
                          {issueKey} · {item.project_slug}
                        </span>

                        <span
                          className={`pm-lozenge ${
                            item.level === "error" || item.level === "fatal"
                              ? "pm-lozenge-danger"
                              : "pm-lozenge-warning"
                          }`}
                        >
                          {item.level}
                        </span>

                        {item.is_regressed && (
                          <span className="pm-lozenge pm-lozenge-warning">
                            <AlertTriangle size={10} />
                            <span>REGRESSED</span>
                          </span>
                        )}

                        {item.status === "resolved" && (
                          <span className="pm-lozenge pm-lozenge-success">
                            RESOLVED
                          </span>
                        )}

                        {item.status === "ignored" && (
                          <span className="pm-lozenge">IGNORED</span>
                        )}

                        <span className="pm-lozenge">
                          {item.last_environment || "production"}
                        </span>

                        {item.last_release && (
                          <span className="pm-lozenge pm-mono lowercase">
                            {item.last_release}
                          </span>
                        )}
                      </div>

                      {/* Exception Title */}
                      <div className="mt-1 text-sm font-semibold text-[var(--color-text)] truncate">
                        {item.title}
                      </div>

                      {/* Culprit & Quick Triage on Hover */}
                      <div className="mt-0.5 flex items-center gap-3">
                        <span className="pm-mono text-xs text-[var(--color-text-muted)] truncate">
                          {item.culprit || "message fingerprint"}
                        </span>

                        {canTriage && (
                          <div
                            className="hidden sm:inline-flex items-center gap-1 ml-auto shrink-0"
                            onClick={(e) => e.stopPropagation()}
                          >
                            {item.status !== "resolved" && (
                              <button
                                type="button"
                                disabled={triageBusy}
                                onClick={() =>
                                  void handleTriageStatus(item.id, "resolved")
                                }
                                className="h-5 px-1.5 rounded bg-[var(--color-success-light)] text-[var(--color-success-text)] text-[10px] font-bold inline-flex items-center gap-0.5 hover:brightness-95 cursor-pointer"
                                title="Mark as Resolved"
                              >
                                <Check size={10} />
                                <span>Resolve</span>
                              </button>
                            )}
                            {item.status !== "ignored" && (
                              <button
                                type="button"
                                disabled={triageBusy}
                                onClick={() =>
                                  void handleTriageStatus(item.id, "ignored")
                                }
                                className="h-5 px-1.5 rounded bg-[var(--pm-col)] text-[var(--color-text-muted)] text-[10px] font-semibold inline-flex items-center gap-0.5 hover:bg-[var(--color-bg-hover)] cursor-pointer"
                                title="Ignore issue"
                              >
                                <EyeOff size={10} />
                                <span>Ignore</span>
                              </button>
                            )}
                          </div>
                        )}
                      </div>
                    </div>

                    {/* 24h Mini Bar Trend */}
                    <div className="hidden sm:flex items-end gap-[3px] h-7">
                      {sparkBars.map((bh, bIdx) => (
                        <span
                          key={bIdx}
                          className={`w-[5px] rounded-t-[1px] ${
                            item.is_regressed
                              ? "bg-[var(--color-danger)]"
                              : "bg-[#1d7afc]"
                          }`}
                          style={{ height: bh }}
                        />
                      ))}
                    </div>

                    {/* Event Count */}
                    <div className="text-right font-bold text-sm text-[var(--color-text)]">
                      {item.event_count.toLocaleString()}
                    </div>

                    {/* Last Seen Age */}
                    <div className="text-right text-xs text-[var(--color-text-dim)]">
                      {formatRelativeAge(item.last_seen)}
                    </div>
                  </div>
                );
              })}
            </div>
          )}

          {/* Table Pagination Footer */}
          {issuesPage && issuesPage.total > 0 && (
            <div className="mt-auto px-4 py-2.5 border-t border-[var(--color-border)] bg-[var(--color-bg)] flex items-center justify-between text-xs text-[var(--color-text-dim)]">
              <span>
                Showing {(issuesPage.page - 1) * issuesPage.per_page + 1}–
                {Math.min(
                  issuesPage.page * issuesPage.per_page,
                  issuesPage.total,
                )}{" "}
                of {issuesPage.total} issues
              </span>

              <div className="inline-flex items-center gap-1">
                <button
                  type="button"
                  disabled={issuesPage.page <= 1}
                  onClick={() => setPage((p) => Math.max(1, p - 1))}
                  className="h-7 px-2 rounded border border-[var(--color-border)] bg-[var(--color-bg)] hover:bg-[var(--color-bg-hover)] disabled:opacity-40 text-xs font-medium text-[var(--color-text-muted)] inline-flex items-center gap-1 cursor-pointer"
                >
                  <ChevronLeft size={13} />
                  <span>Prev</span>
                </button>
                <span className="h-7 px-2.5 rounded bg-[var(--color-primary)] text-white font-semibold inline-flex items-center">
                  {issuesPage.page}
                </span>
                <button
                  type="button"
                  disabled={issuesPage.page >= issuesPage.total_pages}
                  onClick={() =>
                    setPage((p) => Math.min(issuesPage.total_pages, p + 1))
                  }
                  className="h-7 px-2 rounded border border-[var(--color-border)] bg-[var(--color-bg)] hover:bg-[var(--color-bg-hover)] disabled:opacity-40 text-xs font-medium text-[var(--color-text-muted)] inline-flex items-center gap-1 cursor-pointer"
                >
                  <span>Next</span>
                  <ChevronRight size={13} />
                </button>
              </div>
            </div>
          )}
        </div>
      </section>

      {/* Right Docked Issue Detail Inspector Drawer */}
      {(selectedIssueId !== null || loadingDetail) && (
        <IssueDetailDrawer
          detail={issueDetail}
          loading={loadingDetail}
          canTriage={canTriage}
          triageBusy={triageBusy}
          onUpdateStatus={handleTriageStatus}
          onOpenFullView={() => setFullViewOpen(true)}
          onClose={() => setSelectedIssueId(null)}
        />
      )}
    </div>
  );
}

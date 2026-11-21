import { useCallback, useEffect, useRef, useState } from "react";
import {
  Activity,
  AlertTriangle,
  Bell,
  Bot,
  FolderKanban,
  Gauge,
  Grid,
  Inbox,
  LogOut,
  Plus,
  RotateCcw,
  Search,
  Tag,
  Users,
} from "lucide-react";
import {
  type AuthSessionView,
  type HealthPayload,
  type ProjectView,
  type WorkspaceView,
  fetchCurrentSession,
  fetchHealth,
  fetchProjects,
  fetchWorkspaces,
  logoutSession,
} from "../lib/api";
import { TeamAndSettingsView } from "../modules/admin/TeamAndSettingsView";
import { AlertsView } from "../modules/alerts/AlertsView";
import { LoginView } from "../modules/auth/LoginView";
import { HealthCard } from "../modules/health/HealthCard";
import { IssuesView, type IssuesTabMode } from "../modules/issues/IssuesView";
import { McpView } from "../modules/mcp/McpView";
import { PerformanceView } from "../modules/performance/PerformanceView";
import { ProjectIcon } from "../modules/projects/ProjectIcon";
import { ProjectsView } from "../modules/projects/ProjectsView";
import { ReleasesView } from "../modules/releases/ReleasesView";

type ActiveNav =
  | "issues"
  | "regressed"
  | "performance"
  | "releases"
  | "alerts"
  | "mcp"
  | "projects"
  | "team"
  | "health";

interface ToastState {
  message: string;
  undoAction?: () => void;
}

function getInitials(name: string, email: string): string {
  const src = name.trim() || email.trim() || "AD";
  const parts = src.split(/\s+/).filter(Boolean);
  if (parts.length >= 2) {
    return `${parts[0][0]}${parts[1][0]}`.toUpperCase();
  }
  return src.slice(0, 2).toUpperCase();
}

export function App() {
  const [health, setHealth] = useState<HealthPayload | null>(null);
  const [session, setSession] = useState<AuthSessionView | null>(null);
  const [bootstrapping, setBootstrapping] = useState(true);

  const [workspaces, setWorkspaces] = useState<WorkspaceView[]>([]);
  const [activeWorkspaceSlug, setActiveWorkspaceSlug] = useState<string>("");
  const [projects, setProjects] = useState<ProjectView[]>([]);
  const [selectedProjectSlug, setSelectedProjectSlug] = useState<string>(() => {
    const params = new URLSearchParams(window.location.search);
    return params.get("project") ?? "";
  });
  const [selectedReleaseFilter, setSelectedReleaseFilter] = useState<string>("");
  const [deepLinkTraceId, setDeepLinkTraceId] = useState<string>(() => {
    const params = new URLSearchParams(window.location.search);
    return params.get("trace") ?? "";
  });

  const [activeNav, setActiveNav] = useState<ActiveNav>(() => {
    const params = new URLSearchParams(window.location.search);
    if (params.get("view") === "performance" || params.get("trace")) {
      return "performance";
    }
    return "issues";
  });
  const [globalSearch, setGlobalSearch] = useState("");
  const [toast, setToast] = useState<ToastState | null>(null);
  const searchInputRef = useRef<HTMLInputElement | null>(null);

  const showToast = useCallback((message: string, undoAction?: () => void) => {
    setToast({ message, undoAction });
  }, []);

  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(null), 4500);
    return () => clearTimeout(timer);
  }, [toast]);

  // '/' keyboard shortcut focuses the global search box in the topbar
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (
        e.key === "/" &&
        document.activeElement?.tagName !== "INPUT" &&
        document.activeElement?.tagName !== "TEXTAREA" &&
        document.activeElement?.tagName !== "SELECT"
      ) {
        e.preventDefault();
        searchInputRef.current?.focus();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, []);

  useEffect(() => {
    let active = true;
    Promise.all([
      fetchHealth().catch(() => null),
      fetchCurrentSession().catch(() => null),
    ]).then(([healthData, sessionData]) => {
      if (!active) return;
      if (healthData) setHealth(healthData);
      if (sessionData) {
        setSession(sessionData);
        setActiveWorkspaceSlug(sessionData.default_workspace.slug);
      }
      setBootstrapping(false);
    });
    return () => {
      active = false;
    };
  }, []);

  const loadWorkspaceResources = useCallback(
    async (wsSlug?: string) => {
      if (!session) return;
      try {
        const [projList, wsList] = await Promise.all([
          fetchProjects(wsSlug || undefined),
          session.single_tenant_mode ? Promise.resolve([]) : fetchWorkspaces(),
        ]);
        setProjects(projList);
        if (!session.single_tenant_mode) {
          setWorkspaces(wsList);
        }
      } catch {
        // Keep current list on transient error
      }
    },
    [session],
  );

  useEffect(() => {
    if (session) {
      void loadWorkspaceResources(
        activeWorkspaceSlug || session.default_workspace.slug,
      );
    }
  }, [session, activeWorkspaceSlug, loadWorkspaceResources]);

  async function handleLogout() {
    try {
      await logoutSession();
    } catch {
      // Clear local session state regardless
    }
    setSession(null);
    setProjects([]);
    setSelectedProjectSlug("");
  }

  if (bootstrapping) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-[var(--color-bg-muted)] text-xs text-[var(--color-text-dim)]">
        Loading MaxiLoop workspace...
      </div>
    );
  }

  if (!session) {
    return (
      <LoginView
        health={health}
        onAuthenticated={(newSession) => {
          setSession(newSession);
          setActiveWorkspaceSlug(newSession.default_workspace.slug);
        }}
      />
    );
  }

  const dbHealthy = health?.database === "ok" && health?.status === "ok";
  const canTriage = session.role !== "viewer";
  const canManageProjects =
    session.role === "tenant_owner" ||
    session.role === "tenant_admin" ||
    session.role === "workspace_admin";
  const currentWorkspaceName =
    workspaces.find((w) => w.slug === activeWorkspaceSlug)?.name ??
    session.default_workspace.name;
  const userInitials = getInitials(session.user.name, session.user.email);

  const issuesTabMode: IssuesTabMode =
    activeNav === "regressed" ? "regressed" : "unresolved";

  return (
    <div className="h-screen flex flex-col overflow-hidden bg-[var(--color-bg)] text-[var(--color-text)]">
      {/* 56px MaxiBoard Topbar */}
      <header className="h-14 px-3 bg-[var(--color-bg)] border-b border-[var(--color-border)] flex items-center gap-3 shrink-0">
        <span
          className="w-8 h-8 rounded inline-flex items-center justify-center text-[var(--color-text-muted)]"
          title="MaxiLoop Observability"
        >
          <Grid size={18} />
        </span>

        <button
          type="button"
          onClick={() => {
            setActiveNav("issues");
            setSelectedProjectSlug("");
          }}
          className="flex items-center gap-2 font-bold text-base text-[var(--color-text)] cursor-pointer"
        >
          <span className="w-7 h-7 rounded-[6px] bg-[var(--color-primary)] text-white text-sm font-bold inline-flex items-center justify-center">
            M
          </span>
          <span>MaxiLoop</span>
        </button>

        {/* Global Search Box */}
        <div className="flex-1 max-w-[540px] ml-2 relative">
          <Search
            size={15}
            className="absolute left-3 top-1/2 -translate-y-1/2 text-[var(--color-text-dim)] pointer-events-none"
          />
          <input
            ref={searchInputRef}
            type="text"
            value={globalSearch}
            onChange={(e) => {
              setGlobalSearch(e.target.value);
              if (activeNav !== "issues" && activeNav !== "regressed") {
                setActiveNav("issues");
              }
            }}
            placeholder="Search issues, exception types, projects..."
            aria-label="Global search"
            className="w-full h-9 pl-9 pr-8 rounded border border-[var(--color-border-strong)] bg-[var(--color-bg)] text-xs text-[var(--color-text)] outline-none focus:border-[var(--color-primary)]"
          />
          <kbd className="absolute right-2.5 top-1/2 -translate-y-1/2 px-1.5 py-0.5 rounded border border-[var(--color-border)] bg-[var(--color-bg-muted)] text-[10px] pm-mono text-[var(--color-text-dim)] pointer-events-none">
            /
          </kbd>
        </div>

        <button
          type="button"
          onClick={() => setActiveNav("projects")}
          className="h-8 px-3 rounded bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] text-white text-xs font-semibold inline-flex items-center gap-1.5 shrink-0 cursor-pointer"
        >
          <Plus size={14} />
          <span>New project</span>
        </button>

        <div className="ml-auto flex items-center gap-2">
          <span
            className={`hidden md:inline-flex pm-lozenge ${
              dbHealthy ? "pm-lozenge-success" : "pm-lozenge-warning"
            }`}
          >
            {dbHealthy ? "POSTGRES 16 OK" : "DB DEGRADED"}
          </span>

          <span className="hidden sm:inline-flex pm-lozenge">
            {session.single_tenant_mode
              ? "SINGLE-TENANT"
              : session.tenant.slug.toUpperCase()}
          </span>

          <span
            className="w-7 h-7 rounded-full bg-[#1d7afc] text-white text-xs font-bold inline-flex items-center justify-center select-none"
            title={`${session.user.name} (${session.user.email})`}
          >
            {userInitials}
          </span>
        </div>
      </header>

      {/* Main Content Shell: 240px Sidebar + Active View */}
      <div className="flex-1 min-h-0 flex overflow-hidden">
        {/* 240px MaxiBoard Sunken Sidebar */}
        <aside className="w-60 shrink-0 bg-[var(--color-bg-sidebar)] border-r border-[var(--color-border)] p-2.5 flex flex-col justify-between overflow-y-auto">
          <div>
            {/* Workspace Mark Header (hides tenant switcher when single_tenant_mode is true per F-ADM-10) */}
            <div className="p-2 rounded-md mb-2 flex items-center gap-2.5">
              <span className="w-8 h-8 rounded-[6px] bg-[var(--color-success)] text-white font-bold text-sm inline-flex items-center justify-center shrink-0">
                {currentWorkspaceName.slice(0, 1).toUpperCase()}
              </span>
              <div className="min-w-0 flex-1">
                <div className="text-sm font-bold text-[var(--color-text)] truncate">
                  {currentWorkspaceName}
                </div>
                <div className="text-xs text-[var(--color-text-dim)] truncate">
                  {session.single_tenant_mode
                    ? `Single-tenant · ${projects.length} projects`
                    : `${session.tenant.name} · ${projects.length} projects`}
                </div>
              </div>
            </div>

            {!session.single_tenant_mode && workspaces.length > 1 && (
              <div className="px-2 mb-2">
                <select
                  value={activeWorkspaceSlug}
                  onChange={(e) => {
                    setActiveWorkspaceSlug(e.target.value);
                    setSelectedProjectSlug("");
                  }}
                  aria-label="Switch workspace"
                  className="pm-input h-8 text-xs"
                >
                  {workspaces.map((w) => (
                    <option key={w.id} value={w.slug}>
                      Workspace: {w.name}
                    </option>
                  ))}
                </select>
              </div>
            )}

            {/* Primary Nav Items */}
            <nav className="space-y-0.5" aria-label="Main navigation">
              <button
                type="button"
                onClick={() => setActiveNav("issues")}
                className={`w-full h-9 px-2.5 rounded flex items-center gap-2.5 text-xs font-medium cursor-pointer transition ${
                  activeNav === "issues"
                    ? "bg-[var(--color-primary-light)] text-[var(--color-primary)] font-semibold shadow-[inset_3px_0_0_var(--color-primary)]"
                    : "text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)]"
                }`}
              >
                <Inbox size={16} />
                <span>Issues</span>
              </button>

              <button
                type="button"
                onClick={() => setActiveNav("regressed")}
                className={`w-full h-9 px-2.5 rounded flex items-center gap-2.5 text-xs font-medium cursor-pointer transition ${
                  activeNav === "regressed"
                    ? "bg-[var(--color-primary-light)] text-[var(--color-primary)] font-semibold shadow-[inset_3px_0_0_var(--color-primary)]"
                    : "text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)]"
                }`}
              >
                <AlertTriangle size={16} />
                <span>Regressions</span>
              </button>

              <button
                type="button"
                onClick={() => setActiveNav("performance")}
                className={`w-full h-9 px-2.5 rounded flex items-center gap-2.5 text-xs font-medium cursor-pointer transition ${
                  activeNav === "performance"
                    ? "bg-[var(--color-primary-light)] text-[var(--color-primary)] font-semibold shadow-[inset_3px_0_0_var(--color-primary)]"
                    : "text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)]"
                }`}
              >
                <Gauge size={16} />
                <span>Performance</span>
              </button>

              <button
                type="button"
                onClick={() => setActiveNav("releases")}
                className={`w-full h-9 px-2.5 rounded flex items-center gap-2.5 text-xs font-medium cursor-pointer transition ${
                  activeNav === "releases"
                    ? "bg-[var(--color-primary-light)] text-[var(--color-primary)] font-semibold shadow-[inset_3px_0_0_var(--color-primary)]"
                    : "text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)]"
                }`}
              >
                <Tag size={16} />
                <span>Releases</span>
              </button>

              <button
                type="button"
                onClick={() => setActiveNav("alerts")}
                className={`w-full h-9 px-2.5 rounded flex items-center gap-2.5 text-xs font-medium cursor-pointer transition ${
                  activeNav === "alerts"
                    ? "bg-[var(--color-primary-light)] text-[var(--color-primary)] font-semibold shadow-[inset_3px_0_0_var(--color-primary)]"
                    : "text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)]"
                }`}
              >
                <Bell size={16} />
                <span>Alerts &amp; PII</span>
              </button>

              <button
                type="button"
                onClick={() => setActiveNav("mcp")}
                className={`w-full h-9 px-2.5 rounded flex items-center gap-2.5 text-xs font-medium cursor-pointer transition ${
                  activeNav === "mcp"
                    ? "bg-[var(--color-primary-light)] text-[var(--color-primary)] font-semibold shadow-[inset_3px_0_0_var(--color-primary)]"
                    : "text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)]"
                }`}
              >
                <Bot size={16} />
                <span>MCP &amp; AI</span>
              </button>

              <button
                type="button"
                onClick={() => setActiveNav("projects")}
                className={`w-full h-9 px-2.5 rounded flex items-center gap-2.5 text-xs font-medium cursor-pointer transition ${
                  activeNav === "projects"
                    ? "bg-[var(--color-primary-light)] text-[var(--color-primary)] font-semibold shadow-[inset_3px_0_0_var(--color-primary)]"
                    : "text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)]"
                }`}
              >
                <FolderKanban size={16} />
                <span>Projects &amp; DSN</span>
                <span className="ml-auto px-1.5 py-0.5 rounded-full bg-[var(--color-bg-hover)] text-[var(--color-text-muted)] text-[11px] font-bold">
                  {projects.length}
                </span>
              </button>

              <button
                type="button"
                onClick={() => setActiveNav("team")}
                className={`w-full h-9 px-2.5 rounded flex items-center gap-2.5 text-xs font-medium cursor-pointer transition ${
                  activeNav === "team"
                    ? "bg-[var(--color-primary-light)] text-[var(--color-primary)] font-semibold shadow-[inset_3px_0_0_var(--color-primary)]"
                    : "text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)]"
                }`}
              >
                <Users size={16} />
                <span>Team &amp; settings</span>
              </button>

              <button
                type="button"
                onClick={() => setActiveNav("health")}
                className={`w-full h-9 px-2.5 rounded flex items-center gap-2.5 text-xs font-medium cursor-pointer transition ${
                  activeNav === "health"
                    ? "bg-[var(--color-primary-light)] text-[var(--color-primary)] font-semibold shadow-[inset_3px_0_0_var(--color-primary)]"
                    : "text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)]"
                }`}
              >
                <Activity size={16} />
                <span>Retention &amp; health</span>
              </button>
            </nav>

            {/* Projects Tree Section with MaxiBoard Doodle SVG Icons */}
            <div className="mt-4 pt-3 border-t border-[var(--color-border)]">
              <div className="px-2.5 pb-1.5 flex items-center justify-between text-[11px] font-bold uppercase tracking-wider text-[var(--color-text-dim)]">
                <span>Projects</span>
                <button
                  type="button"
                  onClick={() => setActiveNav("projects")}
                  className="hover:text-[var(--color-text)] cursor-pointer"
                  title="Manage or create projects"
                >
                  <Plus size={14} />
                </button>
              </div>

              {projects.length === 0 ? (
                <div className="px-2.5 py-2 text-xs text-[var(--color-text-dim)]">
                  No projects yet
                </div>
              ) : (
                <ul className="space-y-0.5">
                  {projects.map((p) => {
                    const isActiveProj =
                      (activeNav === "issues" || activeNav === "regressed") &&
                      selectedProjectSlug === p.slug;
                    return (
                      <li key={p.id}>
                        <button
                          type="button"
                          onClick={() => {
                            setSelectedProjectSlug((cur) =>
                              cur === p.slug ? "" : p.slug,
                            );
                            if (
                              activeNav !== "issues" &&
                              activeNav !== "regressed"
                            ) {
                              setActiveNav("issues");
                            }
                          }}
                          className={`w-full h-8 px-2.5 rounded flex items-center gap-2 text-xs cursor-pointer transition ${
                            isActiveProj
                              ? "bg-[var(--color-primary-light)] text-[var(--color-primary)] font-semibold"
                              : "text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)]"
                          }`}
                        >
                          <ProjectIcon
                            slug={p.slug}
                            platform={p.platform}
                            size={18}
                          />
                          <span className="truncate flex-1 text-left">
                            {p.slug}
                          </span>
                        </button>
                      </li>
                    );
                  })}
                </ul>
              )}
            </div>
          </div>

          {/* Sidebar Runtime & User Session Footer */}
          <div className="pt-3 border-t border-[var(--color-border)] px-2 space-y-1 text-[11px] text-[var(--color-text-dim)]">
            <div>Ingest worker: 100ms batch</div>
            <div>Retention: {session.tenant.retention_days}d daily partitions</div>
            <div className="pt-1.5 font-semibold text-[var(--color-text-muted)] truncate">
              {session.user.email} ({session.role})
            </div>
            <button
              type="button"
              onClick={() => void handleLogout()}
              className="inline-flex items-center gap-1 text-xs font-medium text-[var(--color-link)] hover:underline cursor-pointer pt-0.5"
            >
              <LogOut size={12} />
              <span>Sign out</span>
            </button>
          </div>
        </aside>

        {/* Active View Area */}
        {(activeNav === "issues" || activeNav === "regressed") && (
          <IssuesView
            workspaceName={currentWorkspaceName}
            workspaceSlug={activeWorkspaceSlug || undefined}
            projects={projects}
            selectedProjectSlug={selectedProjectSlug}
            onSelectProjectSlug={setSelectedProjectSlug}
            globalSearchQuery={globalSearch}
            initialTabMode={issuesTabMode}
            initialReleaseFilter={selectedReleaseFilter}
            canTriage={canTriage}
            onShowToast={showToast}
            onNavigateToProjects={() => setActiveNav("projects")}
          />
        )}

        {activeNav === "performance" && (
          <PerformanceView
            workspaceName={currentWorkspaceName}
            workspaceSlug={activeWorkspaceSlug || undefined}
            projects={projects}
            selectedProjectSlug={selectedProjectSlug}
            onSelectProjectSlug={setSelectedProjectSlug}
            initialTraceId={deepLinkTraceId || undefined}
            onClearInitialTraceId={() => setDeepLinkTraceId("")}
            onShowToast={(msg) => showToast(msg)}
          />
        )}

        {activeNav === "releases" && (
          <ReleasesView
            workspaceName={currentWorkspaceName}
            projects={projects}
            canTriage={canTriage}
            onOpenReleaseIssues={(projSlug, relVersion) => {
              setSelectedProjectSlug(projSlug);
              setSelectedReleaseFilter(relVersion);
              setActiveNav("issues");
            }}
            onShowToast={(msg) => showToast(msg)}
          />
        )}

        {activeNav === "alerts" && (
          <AlertsView
            workspaceName={currentWorkspaceName}
            projects={projects}
            canManage={canManageProjects}
            onShowToast={(msg) => showToast(msg)}
          />
        )}

        {activeNav === "mcp" && (
          <McpView
            workspaceName={currentWorkspaceName}
            projects={projects}
            canTriage={canTriage}
            onShowToast={(msg) => showToast(msg)}
          />
        )}

        {activeNav === "projects" && (
          <ProjectsView
            workspaceName={currentWorkspaceName}
            workspaceSlug={activeWorkspaceSlug || undefined}
            globalRetentionDays={session.tenant.retention_days}
            projects={projects}
            canManageProjects={canManageProjects}
            onProjectsChanged={() =>
              loadWorkspaceResources(
                activeWorkspaceSlug || session.default_workspace.slug,
              )
            }
            onOpenProjectIssues={(slug) => {
              setSelectedProjectSlug(slug);
              setActiveNav("issues");
            }}
            onShowToast={(msg) => showToast(msg)}
          />
        )}

        {activeNav === "team" && (
          <TeamAndSettingsView
            session={session}
            projects={projects}
            onSettingsChanged={async () => {
              const refreshed = await fetchCurrentSession().catch(() => null);
              if (refreshed) setSession(refreshed);
              await loadWorkspaceResources(
                activeWorkspaceSlug || session.default_workspace.slug,
              );
            }}
            onShowToast={(msg) => showToast(msg)}
          />
        )}

        {activeNav === "health" && <HealthCard />}
      </div>

      {/* MaxiBoard Bottom Toast (#172B4D) */}
      {toast && (
        <div
          role="status"
          className="fixed left-64 bottom-5 z-50 bg-[#172b4d] text-white rounded-md px-3.5 py-2.5 shadow-[var(--pm-shadow-raised)] flex items-center gap-3 text-xs font-medium"
        >
          <span>{toast.message}</span>
          {toast.undoAction && (
            <button
              type="button"
              onClick={() => {
                toast.undoAction?.();
                setToast(null);
              }}
              className="text-[#85b8ff] hover:underline font-bold inline-flex items-center gap-1 cursor-pointer"
            >
              <RotateCcw size={12} />
              <span>Undo</span>
            </button>
          )}
        </div>
      )}
    </div>
  );
}

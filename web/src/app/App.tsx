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
  type LucideIcon,
  Menu,
  MoreHorizontal,
  Plus,
  Radar,
  RotateCcw,
  Search,
  Tag,
  Users,
  X,
} from "lucide-react";
import {
  type AuthSessionView,
  type HealthPayload,
  type ProjectView,
  type Role,
  type WorkspaceView,
  fetchCurrentSession,
  fetchHealth,
  fetchProjects,
  fetchWorkspaces,
  logoutSession,
} from "../lib/api";
import { colorFor, initials } from "../lib/avatar";
import {
  type AdminTab,
  TeamAndSettingsView,
} from "../modules/admin/TeamAndSettingsView";
import { AlertsView } from "../modules/alerts/AlertsView";
import { LoginView } from "../modules/auth/LoginView";
import { HealthCard } from "../modules/health/HealthCard";
import { getUrlParam, setUrlParams } from "../lib/urlState";
import { IssuesView, type IssuesTabMode } from "../modules/issues/IssuesView";
import { McpView } from "../modules/mcp/McpView";
import { PerformanceView } from "../modules/performance/PerformanceView";
import { CreateProjectDialog } from "../modules/projects/CreateProjectDialog";
import { ProjectTree } from "../modules/projects/ProjectTree";
import { ProjectsView } from "../modules/projects/ProjectsView";
import { ReleasesView } from "../modules/releases/ReleasesView";
import { CreateWorkspaceDialog } from "../modules/workspaces/CreateWorkspaceDialog";
import { WorkspaceSwitcher } from "../modules/workspaces/WorkspaceSwitcher";
import {
  getLastActiveWorkspaceSlug,
  recordWorkspaceOpened,
} from "../modules/workspaces/workspacePrefs";

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

const NAV_IDS: readonly ActiveNav[] = [
  "issues",
  "regressed",
  "performance",
  "releases",
  "alerts",
  "mcp",
  "projects",
  "team",
  "health",
];

/** Params owned by a single view; dropped when the menu changes (D014). */
const VIEW_SCOPED_PARAMS = ["tab", "status", "issue", "full", "detail", "trace"] as const;

function readNavFromUrl(): ActiveNav {
  const view = getUrlParam("view");
  if (view && (NAV_IDS as readonly string[]).includes(view)) return view as ActiveNav;
  if (getUrlParam("trace")) return "performance";
  return "issues";
}

interface ToastState {
  message: string;
  undoAction?: () => void;
}

const NAV_ITEMS: { id: ActiveNav; label: string; icon: LucideIcon }[] = [
  { id: "issues", label: "Issues", icon: Inbox },
  { id: "regressed", label: "Regressions", icon: AlertTriangle },
  { id: "performance", label: "Performance", icon: Gauge },
  { id: "releases", label: "Releases", icon: Tag },
  { id: "alerts", label: "Alerts & PII", icon: Bell },
  { id: "mcp", label: "MCP & AI", icon: Bot },
  { id: "projects", label: "Projects & DSN", icon: FolderKanban },
  { id: "team", label: "Team & settings", icon: Users },
  { id: "health", label: "Retention & health", icon: Activity },
];

/** Mobile bottom tab bar (MaxiBoard TabBar): 4 primary destinations + "More" opens the drawer. */
const TAB_ITEMS: { id: ActiveNav; label: string; icon: LucideIcon }[] = [
  { id: "issues", label: "Issues", icon: Inbox },
  { id: "performance", label: "Performance", icon: Gauge },
  { id: "releases", label: "Releases", icon: Tag },
  { id: "alerts", label: "Alerts", icon: Bell },
];

function NavItem({
  icon: Icon,
  label,
  active,
  count,
  onClick,
}: {
  icon: LucideIcon;
  label: string;
  active: boolean;
  count?: number;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-current={active ? "page" : undefined}
      className={`w-full flex items-center gap-3 px-2.5 min-h-11 md:min-h-9 rounded-md text-sm text-left cursor-pointer transition ${
        active
          ? "bg-[var(--color-primary-light)] text-[var(--color-primary)] font-semibold"
          : "text-[var(--color-text-muted)] font-medium hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text)]"
      }`}
    >
      <Icon size={18} strokeWidth={1.85} className="shrink-0" />
      <span className="flex-1 truncate">{label}</span>
      {count !== undefined && count > 0 && (
        <span className="min-w-5 h-5 px-1.5 rounded-full bg-[var(--pm-col)] text-[var(--color-text-muted)] text-[11px] font-semibold leading-5 text-center">
          {count > 99 ? "99+" : count}
        </span>
      )}
    </button>
  );
}

const ROLE_RANK: Record<Role, number> = {
  tenant_owner: 5,
  tenant_admin: 4,
  workspace_admin: 3,
  member: 2,
  viewer: 1,
};

function resolveEffectiveWorkspaceRole(
  session: AuthSessionView,
  workspaceId: number | null,
): Role {
  const tenantRole = session.tenant_role;
  const wsRole =
    workspaceId !== null ? session.workspace_roles[String(workspaceId)] : undefined;
  if (tenantRole && wsRole) {
    return ROLE_RANK[wsRole] > ROLE_RANK[tenantRole] ? wsRole : tenantRole;
  }
  return tenantRole ?? wsRole ?? session.role;
}

function resolveInitialWorkspaceSlug(session: AuthSessionView): string {
  const params = new URLSearchParams(window.location.search);
  const urlWs = params.get("workspace")?.trim();
  if (urlWs) return urlWs;
  return (
    getLastActiveWorkspaceSlug(session.tenant.slug) ??
    session.default_workspace.slug
  );
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
  const [activeNav, setActiveNavState] = useState<ActiveNav>(readNavFromUrl);

  // Menu changes get their own history entry; the URL is updated before the next
  // render so the newly mounted view never reads the previous view's `tab`.
  const setActiveNav = useCallback((nav: ActiveNav) => {
    if (readNavFromUrl() !== nav) {
      const cleared = Object.fromEntries(VIEW_SCOPED_PARAMS.map((k) => [k, null]));
      setUrlParams({ ...cleared, view: nav === "issues" ? null : nav }, "push");
    }
    setActiveNavState(nav);
  }, []);
  const [globalSearch, setGlobalSearch] = useState("");
  const [toast, setToast] = useState<ToastState | null>(null);
  const [createWsInitialName, setCreateWsInitialName] = useState<string | null>(
    null,
  );
  const [createProjectOpen, setCreateProjectOpen] = useState(false);
  const [teamTabRequest, setTeamTabRequest] = useState<{
    tab: AdminTab;
    nonce: number;
  }>({ tab: "members", nonce: 0 });
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [mobileSearchOpen, setMobileSearchOpen] = useState(false);
  const searchInputRef = useRef<HTMLInputElement | null>(null);

  const goTo = useCallback(
    (nav: ActiveNav) => {
      setActiveNav(nav);
      setDrawerOpen(false);
    },
    [setActiveNav],
  );

  // Keep project / workspace selection in the URL so a refresh keeps them.
  useEffect(() => {
    setUrlParams({ project: selectedProjectSlug || null });
  }, [selectedProjectSlug]);

  useEffect(() => {
    if (activeWorkspaceSlug) setUrlParams({ workspace: activeWorkspaceSlug });
  }, [activeWorkspaceSlug]);

  // Back / forward: restore the menu and selection from the URL.
  useEffect(() => {
    const onPop = () => {
      setActiveNavState(readNavFromUrl());
      setSelectedProjectSlug(getUrlParam("project") ?? "");
      setDeepLinkTraceId(getUrlParam("trace") ?? "");
      const ws = getUrlParam("workspace");
      if (ws) setActiveWorkspaceSlug(ws);
    };
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);

  // Close the mobile drawer on Escape and when the viewport grows to desktop.
  useEffect(() => {
    if (!drawerOpen) return;
    const mq = window.matchMedia("(min-width: 768px)");
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setDrawerOpen(false);
    };
    const onMq = () => {
      if (mq.matches) setDrawerOpen(false);
    };
    window.addEventListener("keydown", onKey);
    mq.addEventListener("change", onMq);
    return () => {
      window.removeEventListener("keydown", onKey);
      mq.removeEventListener("change", onMq);
    };
  }, [drawerOpen]);

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
        setActiveWorkspaceSlug(resolveInitialWorkspaceSlug(sessionData));
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
        const wsList = await fetchWorkspaces();
        setWorkspaces(wsList);

        let targetSlug = wsSlug || session.default_workspace.slug;
        if (
          wsList.length > 0 &&
          !wsList.some((w) => w.slug === targetSlug)
        ) {
          targetSlug = wsList[0].slug;
          setActiveWorkspaceSlug(targetSlug);
          setSelectedProjectSlug("");
        }

        const projList = await fetchProjects(targetSlug || undefined);
        setProjects(projList);
        if (targetSlug) {
          recordWorkspaceOpened(session.tenant.slug, targetSlug);
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

  const handleSelectWorkspace = useCallback(
    (nextSlug: string) => {
      setActiveWorkspaceSlug(nextSlug);
      setSelectedProjectSlug("");
      setSelectedReleaseFilter("");
      if (session) {
        recordWorkspaceOpened(session.tenant.slug, nextSlug);
      }
    },
    [session],
  );

  const openTeamTab = useCallback(
    (tab: AdminTab) => {
      setTeamTabRequest((prev) => ({ tab, nonce: prev.nonce + 1 }));
      setActiveNav("team");
      setDrawerOpen(false);
    },
    [setActiveNav],
  );

  async function handleLogout() {
    try {
      await logoutSession();
    } catch {
      // Clear local session state regardless
    }
    setSession(null);
    setWorkspaces([]);
    setActiveWorkspaceSlug("");
    setProjects([]);
    setSelectedProjectSlug("");
    setCreateWsInitialName(null);
    setCreateProjectOpen(false);
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
          setActiveWorkspaceSlug(resolveInitialWorkspaceSlug(newSession));
        }}
      />
    );
  }

  const dbHealthy = health?.database === "ok" && health?.status === "ok";
  const activeWorkspace =
    workspaces.find((w) => w.slug === activeWorkspaceSlug) ??
    workspaces.find((w) => w.slug === session.default_workspace.slug) ??
    null;
  const activeWorkspaceRole = resolveEffectiveWorkspaceRole(
    session,
    activeWorkspace?.id ?? session.default_workspace.id,
  );
  const canTriage = activeWorkspaceRole !== "viewer";
  const canManageProjects =
    activeWorkspaceRole === "tenant_owner" ||
    activeWorkspaceRole === "tenant_admin" ||
    activeWorkspaceRole === "workspace_admin";
  const canCreateWorkspace =
    session.tenant_role === "tenant_owner" ||
    session.tenant_role === "tenant_admin";
  const currentWorkspaceName =
    activeWorkspace?.name ?? session.default_workspace.name;
  const userInitials = initials(session.user.email || session.user.name);

  const issuesTabMode: IssuesTabMode =
    activeNav === "regressed" ? "regressed" : "unresolved";

  return (
    <div className="h-[100dvh] flex flex-col overflow-hidden bg-[var(--color-bg)] text-[var(--color-text)]">
      {/* 56px MaxiBoard Topbar */}
      <header className="relative h-14 px-2 md:px-3 bg-[var(--color-bg)] border-b border-[var(--color-border)] flex items-center gap-1 md:gap-3 shrink-0 z-30">
        {/* Mobile: hamburger opens the sidebar drawer. Desktop: app grid mark. */}
        <button
          type="button"
          onClick={() => setDrawerOpen(true)}
          aria-label="Open navigation"
          aria-expanded={drawerOpen}
          className="md:hidden w-11 h-11 rounded-md inline-flex items-center justify-center text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)] cursor-pointer"
        >
          <Menu size={22} />
        </button>
        <span
          className="max-md:hidden w-8 h-8 rounded-md inline-flex items-center justify-center text-[var(--color-text-muted)]"
          title="MaxiLoop Observability"
        >
          <Grid size={18} />
        </span>

        <button
          type="button"
          onClick={() => {
            goTo("issues");
            setSelectedProjectSlug("");
          }}
          className="flex items-center gap-2 px-1 min-w-0 cursor-pointer"
        >
          <span className="w-8 h-8 rounded-md bg-[var(--color-primary)] text-white inline-flex items-center justify-center shrink-0">
            <Radar size={17} />
          </span>
          <span className="text-[15px] font-bold tracking-tight truncate">MaxiLoop</span>
        </button>

        {/* Global Search Box: inline on desktop, drop-down row on mobile */}
        <div
          className={`md:flex-1 md:max-w-xl md:ml-6 md:relative ${
            mobileSearchOpen
              ? "max-md:absolute max-md:inset-x-0 max-md:top-full max-md:px-3 max-md:py-2 max-md:bg-[var(--color-bg)] max-md:border-b max-md:border-[var(--color-border)] max-md:shadow-[var(--pm-shadow-raised)]"
              : "max-md:hidden"
          }`}
        >
          <div className="relative">
            <Search
              size={16}
              className="absolute left-3 top-1/2 -translate-y-1/2 text-[var(--color-text-dim)] pointer-events-none"
            />
            <input
              ref={searchInputRef}
              type="search"
              value={globalSearch}
              onChange={(e) => {
                setGlobalSearch(e.target.value);
                if (activeNav !== "issues" && activeNav !== "regressed") {
                  setActiveNav("issues");
                }
              }}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  setMobileSearchOpen(false);
                  e.currentTarget.blur();
                }
              }}
              placeholder="Search issues, exception types, projects..."
              aria-label="Global search"
              className="w-full h-11 md:h-9 pl-9 pr-9 rounded-md border border-[var(--color-border-strong)] bg-[var(--color-bg)] text-base md:text-sm text-[var(--color-text)] outline-none placeholder:text-[var(--color-text-dim)] focus:border-[var(--color-primary)] focus:shadow-[0_0_0_1px_var(--color-primary)]"
            />
            <kbd className="max-md:hidden absolute right-2.5 top-1/2 -translate-y-1/2 min-w-6 text-center rounded border border-[var(--color-border)] bg-[var(--color-bg-muted)] text-[11px] leading-5 pm-mono text-[var(--color-text-dim)] pointer-events-none">
              /
            </kbd>
          </div>
        </div>

        <button
          type="button"
          onClick={() => {
            if (canManageProjects) {
              setCreateProjectOpen(true);
            } else {
              goTo("projects");
            }
          }}
          className="max-md:hidden h-9 px-3 rounded-md bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] text-[var(--color-primary-text)] text-sm font-medium inline-flex items-center gap-1.5 shrink-0 cursor-pointer transition"
        >
          <Plus size={16} />
          <span>New project</span>
        </button>

        <div className="ml-auto flex items-center gap-1 md:gap-2 shrink-0">
          <span
            className={`max-lg:hidden pm-lozenge ${
              dbHealthy ? "pm-lozenge-success" : "pm-lozenge-warning"
            }`}
          >
            {dbHealthy ? "POSTGRES 16 OK" : "DB DEGRADED"}
          </span>

          <span className="max-xl:hidden pm-lozenge">
            {session.single_tenant_mode
              ? "SINGLE-TENANT"
              : session.tenant.slug.toUpperCase()}
          </span>

          <button
            type="button"
            onClick={() => {
              setMobileSearchOpen((o) => !o);
              setTimeout(() => searchInputRef.current?.focus(), 0);
            }}
            aria-label="Search"
            aria-expanded={mobileSearchOpen}
            className="md:hidden w-11 h-11 rounded-md inline-flex items-center justify-center text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)] cursor-pointer"
          >
            {mobileSearchOpen ? <X size={20} /> : <Search size={20} />}
          </button>
          <button
            type="button"
            onClick={() => {
              if (canManageProjects) {
                setCreateProjectOpen(true);
              } else {
                goTo("projects");
              }
            }}
            aria-label="New project"
            className="md:hidden w-11 h-11 rounded-md inline-flex items-center justify-center text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)] cursor-pointer"
          >
            <Plus size={22} />
          </button>

          <span
            className="max-md:hidden ml-1 w-8 h-8 rounded-full text-white text-[13px] font-semibold inline-flex items-center justify-center shrink-0 select-none"
            style={{ background: colorFor(session.user.email || "?") }}
            title={`${session.user.name} (${session.user.email})`}
          >
            {userInitials}
          </span>
        </div>
      </header>

      {/* Main Content Shell: 240px Sidebar + Active View */}
      <div className="flex-1 min-h-0 flex overflow-hidden">
        {/* Mobile drawer backdrop */}
        {drawerOpen && (
          <div
            className="md:hidden fixed inset-0 z-40 bg-[rgba(9,30,66,0.54)] pm-fade-in"
            onClick={() => setDrawerOpen(false)}
            aria-hidden="true"
          />
        )}

        {/* 240px MaxiBoard Sidebar — static on desktop, off-canvas drawer on mobile */}
        <aside
          className={`w-60 shrink-0 bg-[var(--color-bg-sidebar)] border-r border-[var(--color-border)] p-2 pt-3 flex flex-col justify-between overflow-y-auto max-md:fixed max-md:inset-y-0 max-md:left-0 max-md:z-50 max-md:w-[min(18rem,85vw)] max-md:shadow-[var(--pm-shadow-raised)] max-md:transition-transform max-md:duration-200 ${
            drawerOpen ? "" : "max-md:-translate-x-full max-md:invisible"
          }`}
          aria-label="Sidebar"
        >
          <div>
            <div className="md:hidden flex items-center justify-between px-1 pb-2">
              <span className="text-[15px] font-bold tracking-tight">MaxiLoop</span>
              <button
                type="button"
                onClick={() => setDrawerOpen(false)}
                aria-label="Close navigation"
                className="w-11 h-11 rounded-md inline-flex items-center justify-center text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)] cursor-pointer"
              >
                <X size={20} />
              </button>
            </div>
            {/* Top-Left MaxiBoard Workspace Switcher & Dropdown */}
            <WorkspaceSwitcher
              session={session}
              workspaces={workspaces}
              activeWorkspace={activeWorkspace}
              activeWorkspaceSlug={
                activeWorkspaceSlug || session.default_workspace.slug
              }
              activeWorkspaceRole={activeWorkspaceRole}
              projectCount={projects.length}
              canCreateWorkspace={canCreateWorkspace}
              canManageWorkspace={canManageProjects}
              onSelectWorkspace={handleSelectWorkspace}
              onOpenCreateWorkspace={(initialName = "") =>
                setCreateWsInitialName(initialName)
              }
              onOpenWorkspaceSettings={() => openTeamTab("settings")}
              onOpenWorkspaceMembers={() => openTeamTab("members")}
            />

            {/* Primary Nav Items (MaxiBoard NavLink spec: 14px, 18px icons) */}
            <nav className="space-y-0.5" aria-label="Main navigation">
              {NAV_ITEMS.map((item) => (
                <NavItem
                  key={item.id}
                  icon={item.icon}
                  label={item.label}
                  active={activeNav === item.id}
                  count={item.id === "projects" ? projects.length : undefined}
                  onClick={() => goTo(item.id)}
                />
              ))}
            </nav>

            {/* Projects Tree Section with MaxiBoard Doodle SVG Icons */}
            <div className="mt-3">
              <div className="px-2 pt-2 pb-1.5 flex items-center justify-between text-[11px] font-bold uppercase tracking-wider text-[var(--color-text-dim)]">
                <span>Projects</span>
                {canManageProjects && (
                  <button
                    type="button"
                    onClick={() => setCreateProjectOpen(true)}
                    className="w-6 h-6 -mr-1 flex items-center justify-center rounded text-[var(--color-text-dim)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text)] cursor-pointer transition"
                    title="Create project"
                    aria-label="Create project"
                  >
                    <Plus size={15} />
                  </button>
                )}
              </div>

              {projects.length === 0 ? (
                <div className="px-2.5 py-2 text-sm text-[var(--color-text-dim)]">
                  No projects yet
                </div>
              ) : (
                <ProjectTree
                  projects={projects}
                  selectedProjectSlug={selectedProjectSlug}
                  highlightSelection={
                    activeNav === "issues" ||
                    activeNav === "regressed" ||
                    activeNav === "performance"
                  }
                  onSelectProject={(slug) => {
                    setSelectedProjectSlug((cur) => (cur === slug ? "" : slug));
                    if (
                      activeNav !== "issues" &&
                      activeNav !== "regressed" &&
                      activeNav !== "performance"
                    ) {
                      setActiveNav("issues");
                    }
                    setDrawerOpen(false);
                  }}
                />
              )}
            </div>
          </div>

          {/* Sidebar Runtime & User Session Footer */}
          <div className="mt-4 pt-3 border-t border-[var(--color-border)] px-2 pb-1 space-y-1 text-xs text-[var(--color-text-dim)]">
            <div>Ingest worker: 100ms batch</div>
            <div>
              Retention:{" "}
              {activeWorkspace?.effective_retention_days ??
                session.tenant.retention_days}
              d ·{" "}
              {(
                activeWorkspace?.effective_rate_limit_per_minute ??
                session.tenant.default_rate_limit_per_minute
              ).toLocaleString()}
              /m
            </div>
            <div
              className="pt-1.5 font-semibold text-[var(--color-text-muted)] truncate"
              title={`${session.user.email} (${activeWorkspaceRole})`}
            >
              {session.user.email} ({activeWorkspaceRole})
            </div>
            <button
              type="button"
              onClick={() => void handleLogout()}
              className="inline-flex items-center gap-1.5 min-h-9 text-sm font-medium text-[var(--color-link)] hover:underline cursor-pointer"
            >
              <LogOut size={14} />
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
            onNavigateToProjects={() => {
              setActiveNav("projects");
              if (canManageProjects) setCreateProjectOpen(true);
            }}
            onOpenTrace={(traceId) => {
              setDeepLinkTraceId(traceId);
              goTo("performance");
            }}
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
            workspaceSlug={activeWorkspaceSlug || session.default_workspace.slug}
            projects={projects}
            canTriage={canTriage}
            onShowToast={(msg) => showToast(msg)}
          />
        )}

        {activeNav === "projects" && (
          <ProjectsView
            workspaceName={currentWorkspaceName}
            workspaceSlug={activeWorkspaceSlug || undefined}
            globalRetentionDays={
              activeWorkspace?.effective_retention_days ??
              session.tenant.retention_days
            }
            projects={projects}
            canManageProjects={canManageProjects}
            onOpenCreateProject={
              canManageProjects ? () => setCreateProjectOpen(true) : undefined
            }
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
            initialTab={teamTabRequest.tab}
            tabNonce={teamTabRequest.nonce}
            activeWorkspaceSlug={
              activeWorkspaceSlug || session.default_workspace.slug
            }
            onOpenCreateWorkspace={
              canCreateWorkspace
                ? () => setCreateWsInitialName("")
                : undefined
            }
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

      {/* Mobile bottom tab bar (MaxiBoard TabBar): in-flow so it tracks 100dvh exactly */}
      <nav
        className="md:hidden shrink-0 grid grid-cols-5 bg-[var(--color-bg)] border-t border-[var(--color-border)]"
        style={{ paddingBottom: "env(safe-area-inset-bottom, 0px)" }}
        aria-label="Primary"
      >
        {TAB_ITEMS.map((tab) => {
          const Icon = tab.icon;
          const active =
            activeNav === tab.id ||
            (tab.id === "issues" && activeNav === "regressed");
          return (
            <button
              key={tab.id}
              type="button"
              onClick={() => goTo(tab.id)}
              aria-current={active ? "page" : undefined}
              className={`flex flex-col items-center justify-center gap-0.5 h-[60px] text-[11px] font-medium cursor-pointer transition ${
                active
                  ? "text-[var(--color-primary)]"
                  : "text-[var(--color-text-muted)]"
              }`}
            >
              <Icon size={22} strokeWidth={active ? 2.1 : 1.8} />
              <span className="truncate max-w-full px-1">{tab.label}</span>
            </button>
          );
        })}
        <button
          type="button"
          onClick={() => setDrawerOpen(true)}
          aria-current={
            !TAB_ITEMS.some((t) => t.id === activeNav) && activeNav !== "regressed"
              ? "page"
              : undefined
          }
          className={`flex flex-col items-center justify-center gap-0.5 h-[60px] text-[11px] font-medium cursor-pointer transition ${
            !TAB_ITEMS.some((t) => t.id === activeNav) && activeNav !== "regressed"
              ? "text-[var(--color-primary)]"
              : "text-[var(--color-text-muted)]"
          }`}
        >
          <MoreHorizontal size={22} strokeWidth={1.8} />
          <span>More</span>
        </button>
      </nav>

      {/* Create Workspace Modal (MaxiBoard style) */}
      {createWsInitialName !== null && (
        <CreateWorkspaceDialog
          tenant={session.tenant}
          initialName={createWsInitialName}
          onClose={() => setCreateWsInitialName(null)}
          onCreated={(created) => {
            setCreateWsInitialName(null);
            setWorkspaces((prev) =>
              prev.some((w) => w.id === created.id)
                ? prev
                : [...prev, created],
            );
            handleSelectWorkspace(created.slug);
            showToast(`Workspace ${created.name} created`);
          }}
        />
      )}

      {/* Create Project Modal (MaxiBoard style, triggered from sidebar '+' or topbar) */}
      {createProjectOpen && (
        <CreateProjectDialog
          workspaceName={currentWorkspaceName}
          workspaceSlug={activeWorkspaceSlug || session.default_workspace.slug}
          maxRetentionDays={
            activeWorkspace?.effective_retention_days ??
            session.tenant.retention_days
          }
          defaultRateLimitPerMinute={
            activeWorkspace?.effective_rate_limit_per_minute ??
            session.tenant.default_rate_limit_per_minute
          }
          existingProjects={projects}
          onClose={() => setCreateProjectOpen(false)}
          onCreated={async (created) => {
            // Keep the dialog open: it moves on to the SDK setup / first-event step.
            await loadWorkspaceResources(
              activeWorkspaceSlug || session.default_workspace.slug,
            );
            setActiveNav("projects");
            showToast(`Project ${created.slug} created and DSN cached`);
          }}
          onViewIssues={(created) => {
            setCreateProjectOpen(false);
            setSelectedProjectSlug(created.slug);
            setActiveNav("issues");
          }}
        />
      )}

      {/* Toast: MaxiBoard uses sonner (light theme, bottom-center, above the mobile tab bar) */}
      {toast && (
        <div
          role="status"
          className="fixed z-50 left-1/2 -translate-x-1/2 bottom-[calc(72px+env(safe-area-inset-bottom,0px))] md:bottom-4 w-[356px] max-w-[calc(100vw-2rem)] bg-[var(--color-bg-card)] text-[var(--color-text)] border border-[var(--color-border)] rounded-lg p-4 shadow-[0_4px_12px_rgba(0,0,0,0.1)] flex items-center gap-3 text-[13px] font-medium pm-fade-in"
        >
          <span className="flex-1 min-w-0">{toast.message}</span>
          {toast.undoAction && (
            <button
              type="button"
              onClick={() => {
                toast.undoAction?.();
                setToast(null);
              }}
              className="shrink-0 h-6 px-2 rounded bg-[var(--color-text)] text-[var(--color-bg)] text-xs font-medium inline-flex items-center gap-1 cursor-pointer hover:opacity-90"
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

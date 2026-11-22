import {
  type KeyboardEvent as ReactKeyboardEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import {
  Check,
  ChevronDown,
  Clock,
  Gauge,
  Plus,
  Search,
  Settings2,
  Star,
  Users,
} from "lucide-react";
import type { AuthSessionView, Role, WorkspaceView } from "../../lib/api";
import { WorkspaceMark } from "./WorkspaceMark";
import {
  SEARCH_MIN,
  arrangeWorkspaces,
  formatRoleLabel,
  getTenantWorkspacePrefs,
  matchRange,
  toggleWorkspaceStarred,
} from "./workspacePrefs";

interface WorkspaceSwitcherProps {
  session: AuthSessionView;
  workspaces: WorkspaceView[];
  activeWorkspace: WorkspaceView | null;
  activeWorkspaceSlug: string;
  activeWorkspaceRole: Role;
  projectCount: number;
  canCreateWorkspace: boolean;
  canManageWorkspace: boolean;
  onSelectWorkspace: (slug: string) => void;
  onOpenCreateWorkspace: (initialName?: string) => void;
  onOpenWorkspaceSettings: () => void;
  onOpenWorkspaceMembers: () => void;
}

function isTypingElement(el: Element | null): boolean {
  if (!el) return false;
  if ((el as HTMLElement).isContentEditable) return true;
  return /^(INPUT|TEXTAREA|SELECT)$/.test(el.tagName);
}

function HighlightMatch({ text, query }: { text: string; query: string }) {
  const range = matchRange(text, query);
  if (!range) return <>{text}</>;
  return (
    <>
      {text.slice(0, range[0])}
      <mark className="bg-transparent text-inherit font-semibold underline decoration-[var(--color-primary)] underline-offset-2">
        {text.slice(range[0], range[1])}
      </mark>
      {text.slice(range[1])}
    </>
  );
}

const SECTION_LABELS: Record<"starred" | "recent" | "all", string> = {
  starred: "Starred",
  recent: "Recently opened",
  all: "All workspaces",
};

export function WorkspaceSwitcher({
  session,
  workspaces,
  activeWorkspace,
  activeWorkspaceSlug,
  activeWorkspaceRole,
  projectCount,
  canCreateWorkspace,
  canManageWorkspace,
  onSelectWorkspace,
  onOpenCreateWorkspace,
  onOpenWorkspaceSettings,
  onOpenWorkspaceMembers,
}: WorkspaceSwitcherProps) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [activeIdx, setActiveIdx] = useState(0);
  const [keyed, setKeyed] = useState(false);
  const [viaKey, setViaKey] = useState(false);
  const [prefsVersion, setPrefsVersion] = useState(0);
  const [menuPos, setMenuPos] = useState<{ top: number; left: number }>({
    top: 64,
    left: 10,
  });

  const containerRef = useRef<HTMLDivElement | null>(null);
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const popoverRef = useRef<HTMLDivElement | null>(null);
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);

  const tenantSlug = session.tenant.slug;

  const updateMenuPos = useCallback(() => {
    const rect = triggerRef.current?.getBoundingClientRect();
    if (!rect) return;
    const width = 296;
    const maxLeft = Math.max(8, window.innerWidth - width - 8);
    setMenuPos({
      top: Math.round(rect.bottom + 4),
      left: Math.round(Math.max(8, Math.min(rect.left, maxLeft))),
    });
  }, []);

  const close = useCallback(() => {
    setOpen(false);
    setQuery("");
    setActiveIdx(0);
    setKeyed(false);
    setViaKey(false);
  }, []);

  // Ctrl/Cmd+K opens the workspace switcher (matches MaxiBoard PM36).
  useEffect(() => {
    function handleShortcut(e: KeyboardEvent) {
      if (
        e.key.toLowerCase() !== "k" ||
        !(e.metaKey || e.ctrlKey) ||
        e.altKey ||
        e.shiftKey
      ) {
        return;
      }
      if (isTypingElement(document.activeElement)) return;
      e.preventDefault();
      updateMenuPos();
      setViaKey(true);
      setKeyed(true);
      setOpen((prev) => !prev);
    }
    document.addEventListener("keydown", handleShortcut);
    return () => document.removeEventListener("keydown", handleShortcut);
  }, [updateMenuPos]);

  // Keep anchored position updated and close on outside click or Escape.
  useEffect(() => {
    if (!open) return;
    updateMenuPos();
    function handlePointerDown(e: PointerEvent) {
      const target = e.target as Node;
      const inTrigger = containerRef.current?.contains(target) ?? false;
      const inPopover = popoverRef.current?.contains(target) ?? false;
      if (!inTrigger && !inPopover) {
        close();
      }
    }
    function handleEscape(e: KeyboardEvent) {
      if (e.key === "Escape") close();
    }
    const timer = window.setTimeout(
      () => document.addEventListener("pointerdown", handlePointerDown),
      0,
    );
    document.addEventListener("keydown", handleEscape);
    window.addEventListener("resize", updateMenuPos);
    return () => {
      window.clearTimeout(timer);
      document.removeEventListener("pointerdown", handlePointerDown);
      document.removeEventListener("keydown", handleEscape);
      window.removeEventListener("resize", updateMenuPos);
    };
  }, [open, close, updateMenuPos]);

  const effectiveWorkspaces: WorkspaceView[] = useMemo(() => {
    if (workspaces.length > 0) return workspaces;
    const dw = session.default_workspace;
    return [
      {
        id: dw.id,
        tenant_id: session.tenant.id,
        tenant_slug: session.tenant.slug,
        slug: dw.slug,
        name: dw.name,
        retention_days_override: dw.retention_days_override,
        effective_retention_days:
          dw.retention_days_override ?? session.tenant.retention_days,
        default_rate_limit_per_minute_override:
          dw.default_rate_limit_per_minute_override,
        effective_rate_limit_per_minute:
          dw.default_rate_limit_per_minute_override ??
          session.tenant.default_rate_limit_per_minute,
        is_default: dw.is_default,
        created_at: "",
        updated_at: "",
      },
    ];
  }, [workspaces, session]);

  const prefs = useMemo(
    () => getTenantWorkspacePrefs(tenantSlug),
    // prefsVersion triggers recomputation after star/color updates.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [tenantSlug, prefsVersion, open],
  );
  const starredSet = useMemo(() => new Set(prefs.starred), [prefs.starred]);

  const sections = useMemo(
    () =>
      arrangeWorkspaces(
        effectiveWorkspaces,
        query,
        starredSet,
        prefs.lastOpenedAt,
      ),
    [effectiveWorkspaces, query, starredSet, prefs.lastOpenedAt],
  );
  const flatItems = useMemo(
    () => sections.flatMap((s) => s.items),
    [sections],
  );

  const searchable = effectiveWorkspaces.length >= SEARCH_MIN;

  useEffect(() => {
    if (open && viaKey && !searchable) {
      bodyRef.current?.focus();
    }
  }, [open, viaKey, searchable]);

  useEffect(() => {
    setActiveIdx(0);
  }, [query]);

  useEffect(() => {
    listRef.current
      ?.querySelector(`[data-ws-idx="${activeIdx}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [activeIdx]);

  function handleSelect(ws: WorkspaceView) {
    close();
    if (ws.slug !== activeWorkspaceSlug) {
      onSelectWorkspace(ws.slug);
    }
  }

  function handleToggleStar(wsSlug: string) {
    toggleWorkspaceStarred(tenantSlug, wsSlug);
    setPrefsVersion((v) => v + 1);
  }

  function handleMenuKeyDown(e: ReactKeyboardEvent<HTMLDivElement>) {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      setKeyed(true);
      if (flatItems.length === 0) return;
      const delta = e.key === "ArrowDown" ? 1 : -1;
      setActiveIdx((i) => (i + delta + flatItems.length) % flatItems.length);
    } else if (
      e.key === "Enter" &&
      ((e.target as HTMLElement).tagName === "INPUT" ||
        e.target === e.currentTarget)
    ) {
      e.preventDefault();
      if (flatItems[activeIdx]) {
        handleSelect(flatItems[activeIdx]);
      } else if (query.trim() && canCreateWorkspace) {
        const initial = query.trim();
        close();
        onOpenCreateWorkspace(initial);
      }
    }
  }

  const currentWs =
    activeWorkspace ??
    effectiveWorkspaces.find((w) => w.slug === activeWorkspaceSlug) ??
    effectiveWorkspaces[0];

  const currentName = currentWs?.name ?? session.default_workspace.name;
  const currentSlug = currentWs?.slug ?? session.default_workspace.slug;
  const effectiveRetention =
    currentWs?.effective_retention_days ?? session.tenant.retention_days;
  const effectiveRpm =
    currentWs?.effective_rate_limit_per_minute ??
    session.tenant.default_rate_limit_per_minute;

  let itemIdx = -1;

  return (
    <div ref={containerRef} className="relative mb-2">
      {/* Top-Left Clickable Workspace Trigger (MaxiBoard style) */}
      <button
        ref={triggerRef}
        type="button"
        onClick={() => {
          updateMenuPos();
          setOpen((o) => !o);
        }}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={`Switch workspace (${currentName})`}
        className={`w-full p-2 rounded-md flex items-center gap-2.5 text-left cursor-pointer transition ${
          open
            ? "bg-[var(--color-bg-hover)]"
            : "hover:bg-[var(--color-bg-hover)]"
        }`}
      >
        <WorkspaceMark
          name={currentName}
          slug={currentSlug}
          tenantSlug={tenantSlug}
          size={34}
        />
        <span className="min-w-0 flex-1">
          <span className="block text-sm font-semibold text-[var(--color-text)] truncate">
            {currentName}
          </span>
          <span className="block text-xs text-[var(--color-text-dim)] truncate">
            Workspace · {projectCount}{" "}
            {projectCount === 1 ? "project" : "projects"}
          </span>
        </span>
        <ChevronDown
          size={16}
          className={`shrink-0 text-[var(--color-text-dim)] transition-transform ${
            open ? "rotate-180" : ""
          }`}
        />
      </button>

      {/* Portaled Anchored Popover Dropdown (never clipped by sidebar scroll) */}
      {open &&
        createPortal(
          <div
            ref={popoverRef}
            role="menu"
            aria-label="Workspace menu"
            style={{ top: menuPos.top, left: menuPos.left }}
            className="fixed z-50 w-[296px] max-w-[calc(100vw-1rem)] max-h-[min(76vh,34rem)] flex flex-col rounded-lg bg-[var(--color-bg-card)] border border-[var(--color-border)] py-1.5 shadow-[var(--pm-shadow-raised)] pm-fade-in"
          >
          <div
            ref={bodyRef}
            tabIndex={-1}
            onKeyDown={handleMenuKeyDown}
            className="flex flex-col min-h-0 outline-none"
          >
            {/* Header row */}
            <div className="px-3 pt-1 pb-1.5 flex items-center justify-between gap-2">
              <span className="text-[11px] font-semibold uppercase tracking-wide text-[var(--color-text-dim)]">
                Switch workspace
              </span>
              <span className="text-[10px] pm-mono text-[var(--color-text-dim)] border border-[var(--color-border)] bg-[var(--color-bg-muted)] rounded px-1.5 py-0.5">
                ⌘K
              </span>
            </div>

            {/* Search input (appears automatically from SEARCH_MIN workspaces) */}
            {searchable && (
              <div className="px-2 pb-1.5">
                <label className="flex items-center gap-2 h-8 px-2.5 rounded-md border border-[var(--color-border)] bg-[var(--color-bg)] focus-within:border-[var(--color-primary)]">
                  <Search
                    size={14}
                    className="shrink-0 text-[var(--color-text-dim)]"
                  />
                  <input
                    type="text"
                    value={query}
                    onChange={(e) => setQuery(e.target.value)}
                    autoFocus
                    placeholder="Search workspaces..."
                    aria-label="Search workspaces"
                    className="flex-1 min-w-0 bg-transparent text-base md:text-sm text-[var(--color-text)] outline-none placeholder:text-[var(--color-text-dim)]"
                  />
                </label>
              </div>
            )}

            {/* Scrollable Workspace List */}
            <div
              ref={listRef}
              className="min-h-0 flex-1 overflow-y-auto overscroll-contain"
            >
              {sections.map((section) => (
                <div
                  key={section.key ?? "list"}
                  role="group"
                  aria-label={
                    section.key && section.key !== "results"
                      ? SECTION_LABELS[section.key]
                      : undefined
                  }
                >
                  {section.key &&
                    section.key !== "results" &&
                    sections.length > 1 && (
                      <div className="px-3 pt-2 pb-1 text-[11px] font-medium text-[var(--color-text-dim)]">
                        {SECTION_LABELS[section.key]}
                      </div>
                    )}

                  {section.items.map((w) => {
                    const idx = ++itemIdx;
                    const isCurrent = w.slug === currentSlug;
                    const isStarred = starredSet.has(w.slug);
                    return (
                      <div
                        key={w.id}
                        className={`group flex items-center hover:bg-[var(--color-bg-hover)] ${
                          (keyed || query.trim()) && idx === activeIdx
                            ? "bg-[var(--color-bg-hover)]"
                            : ""
                        }`}
                        onMouseMove={() => setActiveIdx(idx)}
                      >
                        <button
                          type="button"
                          role="menuitem"
                          data-ws-idx={idx}
                          onClick={() => handleSelect(w)}
                          className="flex-1 min-w-0 flex items-center gap-2.5 pl-3 py-2 text-left cursor-pointer"
                        >
                          <WorkspaceMark
                            name={w.name}
                            slug={w.slug}
                            tenantSlug={tenantSlug}
                            size={28}
                          />
                          <span className="flex-1 min-w-0">
                            <span className="flex items-center gap-1.5">
                              <span className="text-sm font-semibold text-[var(--color-text)] truncate">
                                <HighlightMatch text={w.name} query={query} />
                              </span>
                              {w.is_default && (
                                <span className="px-1 py-0.25 rounded bg-[var(--pm-col)] text-[9px] font-bold uppercase text-[var(--color-text-dim)] shrink-0">
                                  Default
                                </span>
                              )}
                            </span>
                            <span className="block text-xs text-[var(--color-text-dim)] truncate">
                              <span className="pm-mono">{w.slug}</span> ·{" "}
                              {w.effective_retention_days}d ·{" "}
                              {w.effective_rate_limit_per_minute.toLocaleString()}
                              /m
                            </span>
                          </span>
                          {isCurrent && (
                            <Check
                              size={15}
                              className="text-[var(--color-primary)] shrink-0"
                            />
                          )}
                        </button>

                        <button
                          type="button"
                          onClick={(e) => {
                            e.stopPropagation();
                            handleToggleStar(w.slug);
                          }}
                          aria-pressed={isStarred}
                          title={isStarred ? "Unstar workspace" : "Star workspace"}
                          aria-label={
                            isStarred ? "Unstar workspace" : "Star workspace"
                          }
                          className={`shrink-0 w-8 h-8 mr-1 rounded-md flex items-center justify-center cursor-pointer hover:bg-[var(--color-bg-card)] focus-visible:opacity-100 ${
                            isStarred
                              ? "text-[#E2B203]"
                              : "text-[var(--color-text-dim)] opacity-0 group-hover:opacity-100"
                          }`}
                        >
                          <Star
                            size={14}
                            fill={isStarred ? "currentColor" : "none"}
                          />
                        </button>
                      </div>
                    );
                  })}
                </div>
              ))}

              {query.trim() && flatItems.length === 0 && (
                <div className="px-3 py-3 text-sm text-[var(--color-text-muted)]">
                  No workspace matches &ldquo;{query.trim()}&rdquo;
                </div>
              )}
            </div>
          </div>

          {/* Active Workspace Parameters Strip */}
          <div className="shrink-0">
            <div className="border-t border-[var(--color-border-light)] my-1.5" />
            <div className="mx-2 px-2.5 py-2 rounded-md bg-[var(--color-bg-muted)] border border-[var(--color-border-light)] space-y-1">
              <div className="flex items-center justify-between gap-2 text-[11px]">
                <span className="font-semibold text-[var(--color-text-muted)] truncate">
                  {session.single_tenant_mode
                    ? "Single-tenant"
                    : `Tenant: ${session.tenant.name}`}
                </span>
                <span className="pm-lozenge pm-lozenge-primary text-[10px]">
                  {formatRoleLabel(activeWorkspaceRole)}
                </span>
              </div>
              <div className="flex items-center gap-3 text-[11px] text-[var(--color-text-dim)]">
                <span className="inline-flex items-center gap-1">
                  <Clock size={11} />
                  <span>
                    {effectiveRetention}d
                    {currentWs?.retention_days_override != null
                      ? " (override)"
                      : " (inherited)"}
                  </span>
                </span>
                <span className="inline-flex items-center gap-1">
                  <Gauge size={11} />
                  <span>{effectiveRpm.toLocaleString()}/min</span>
                </span>
              </div>
            </div>

            {/* Fixed Bottom Actions (never pushed off-screen) */}
            <div className="border-t border-[var(--color-border)] my-1.5" />

            {canManageWorkspace && (
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  close();
                  onOpenWorkspaceSettings();
                }}
                className="w-full flex items-center gap-2.5 px-3 min-h-11 md:min-h-9 text-sm font-medium text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text)] cursor-pointer"
              >
                <span className="w-7 h-7 rounded-md flex items-center justify-center">
                  <Settings2 size={15} />
                </span>
                <span>Workspace settings &amp; limits</span>
              </button>
            )}

            <button
              type="button"
              role="menuitem"
              onClick={() => {
                close();
                onOpenWorkspaceMembers();
              }}
              className="w-full flex items-center gap-2.5 px-3 min-h-11 md:min-h-9 text-sm font-medium text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text)] cursor-pointer"
            >
              <span className="w-7 h-7 rounded-md flex items-center justify-center">
                <Users size={15} />
              </span>
              <span>Members &amp; access</span>
            </button>

            {canCreateWorkspace && (
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  const initial =
                    query.trim() && flatItems.length === 0 ? query.trim() : "";
                  close();
                  onOpenCreateWorkspace(initial);
                }}
                className="w-full flex items-center gap-2.5 px-3 min-h-11 md:min-h-9 text-sm font-medium text-[var(--color-text-muted)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text)] cursor-pointer"
              >
                <span className="w-7 h-7 rounded-md border border-dashed border-[var(--color-border-strong)] flex items-center justify-center">
                  <Plus size={14} />
                </span>
                <span className="truncate">
                  {query.trim() && flatItems.length === 0
                    ? `Create workspace “${query.trim()}”`
                    : "Create workspace"}
                </span>
              </button>
            )}
          </div>
        </div>,
        document.body,
      )}
    </div>
  );
}

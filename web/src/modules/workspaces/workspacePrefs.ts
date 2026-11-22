import type { Role, WorkspaceView } from "../../lib/api";

// MaxiBoard 9-color workspace palette (matches maxi-code BOARD_COLORS).
export const WORKSPACE_COLORS = [
  "#1D7AFC",
  "#22A06B",
  "#E56910",
  "#6E5DC6",
  "#C9372C",
  "#D97008",
  "#2898BD",
  "#AE4787",
  "#626F86",
] as const;

// MaxiBoard switcher thresholds (PM35).
export const RECENT_MAX = 5;
export const SEARCH_MIN = 6;

const PREFS_STORAGE_KEY = "maxiloop_workspace_prefs_v1";

interface TenantWorkspacePrefs {
  colors: Record<string, string>;
  starred: string[];
  lastOpenedAt: Record<string, number>;
  lastActiveSlug?: string;
}

type AllWorkspacePrefs = Record<string, TenantWorkspacePrefs>;

function emptyTenantPrefs(): TenantWorkspacePrefs {
  return {
    colors: {},
    starred: [],
    lastOpenedAt: {},
  };
}

function readAllPrefs(): AllWorkspacePrefs {
  try {
    const raw = localStorage.getItem(PREFS_STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== "object") return {};
    return parsed as AllWorkspacePrefs;
  } catch {
    return {};
  }
}

function writeAllPrefs(all: AllWorkspacePrefs): void {
  try {
    localStorage.setItem(PREFS_STORAGE_KEY, JSON.stringify(all));
  } catch {
    // Ignore storage quota or private-browsing errors
  }
}

export function getTenantWorkspacePrefs(tenantSlug: string): TenantWorkspacePrefs {
  const all = readAllPrefs();
  const entry = all[tenantSlug];
  if (!entry) return emptyTenantPrefs();
  return {
    colors: entry.colors && typeof entry.colors === "object" ? entry.colors : {},
    starred: Array.isArray(entry.starred) ? entry.starred : [],
    lastOpenedAt:
      entry.lastOpenedAt && typeof entry.lastOpenedAt === "object"
        ? entry.lastOpenedAt
        : {},
    lastActiveSlug:
      typeof entry.lastActiveSlug === "string" ? entry.lastActiveSlug : undefined,
  };
}

export function saveWorkspaceColor(
  tenantSlug: string,
  workspaceSlug: string,
  color: string,
): void {
  const all = readAllPrefs();
  const current = getTenantWorkspacePrefs(tenantSlug);
  current.colors[workspaceSlug] = color;
  all[tenantSlug] = current;
  writeAllPrefs(all);
}

export function resolveWorkspaceColor(
  tenantSlug: string,
  workspaceSlug: string,
): string {
  const prefs = getTenantWorkspacePrefs(tenantSlug);
  const saved = prefs.colors[workspaceSlug];
  if (saved) return saved;
  if (workspaceSlug === "default") return "#22A06B";
  let hash = 0;
  for (let i = 0; i < workspaceSlug.length; i++) {
    hash = (hash * 31 + workspaceSlug.charCodeAt(i)) >>> 0;
  }
  return WORKSPACE_COLORS[hash % WORKSPACE_COLORS.length];
}

export function toggleWorkspaceStarred(
  tenantSlug: string,
  workspaceSlug: string,
): boolean {
  const all = readAllPrefs();
  const current = getTenantWorkspacePrefs(tenantSlug);
  const exists = current.starred.includes(workspaceSlug);
  current.starred = exists
    ? current.starred.filter((s) => s !== workspaceSlug)
    : [...current.starred, workspaceSlug];
  all[tenantSlug] = current;
  writeAllPrefs(all);
  return !exists;
}

export function recordWorkspaceOpened(
  tenantSlug: string,
  workspaceSlug: string,
): void {
  if (!tenantSlug || !workspaceSlug) return;
  const all = readAllPrefs();
  const current = getTenantWorkspacePrefs(tenantSlug);
  current.lastOpenedAt[workspaceSlug] = Date.now();
  current.lastActiveSlug = workspaceSlug;
  all[tenantSlug] = current;
  writeAllPrefs(all);
}

export function getLastActiveWorkspaceSlug(tenantSlug: string): string | null {
  return getTenantWorkspacePrefs(tenantSlug).lastActiveSlug ?? null;
}

export function slugifyWorkspaceName(raw: string): string {
  return raw
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 64);
}

export function formatRoleLabel(role: Role | null | undefined): string {
  switch (role) {
    case "tenant_owner":
      return "Tenant Owner";
    case "tenant_admin":
      return "Tenant Admin";
    case "workspace_admin":
      return "Workspace Admin";
    case "member":
      return "Member";
    case "viewer":
      return "Viewer";
    default:
      return "Member";
  }
}

// Lowercase without accents, matching MaxiBoard workspaceSwitcher.js.
export function foldText(s: string): string {
  return (s || "")
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase();
}

function compareByName(a: WorkspaceView, b: WorkspaceView): number {
  return (
    foldText(a.name).localeCompare(foldText(b.name)) ||
    String(a.id).localeCompare(String(b.id))
  );
}

export function matchRank(w: WorkspaceView, q: string): number {
  const name = foldText(w.name);
  if (name.startsWith(q)) return 0;
  if (name.split(/[^a-z0-9]+/).some((word) => word.startsWith(q))) return 1;
  if (name.includes(q)) return 2;
  if (foldText(w.slug).includes(q)) return 3;
  return -1;
}

export function matchRange(name: string, query: string): [number, number] | null {
  const q = foldText(query.trim());
  if (!q) return null;
  const n = foldText(name);
  if (n.length !== name.length) return null;
  const i = n.indexOf(q);
  return i < 0 ? null : [i, i + q.length];
}

export interface WorkspaceSection {
  key: "results" | "starred" | "recent" | "all" | null;
  items: WorkspaceView[];
}

// Arrange workspaces into MaxiBoard sections:
// - While searching: ranked results
// - Short list (<= RECENT_MAX): starred first, then recent/default order (no section headers)
// - Long list (> RECENT_MAX): Starred (A-Z), Recently opened, All workspaces (A-Z)
export function arrangeWorkspaces(
  list: WorkspaceView[],
  query: string,
  starredSlugs: Set<string>,
  lastOpenedAt: Record<string, number>,
): WorkspaceSection[] {
  const orderedByRecent = [...list].sort((a, b) => {
    const ta = lastOpenedAt[a.slug] ?? 0;
    const tb = lastOpenedAt[b.slug] ?? 0;
    if (ta !== tb) return tb - ta;
    if (a.is_default !== b.is_default) return a.is_default ? -1 : 1;
    return compareByName(a, b);
  });

  const q = foldText(query.trim());
  if (q) {
    const hits = orderedByRecent
      .map((w, i) => ({ w, i, r: matchRank(w, q) }))
      .filter((x) => x.r >= 0);
    hits.sort(
      (a, b) =>
        a.r - b.r ||
        (starredSlugs.has(b.w.slug) ? 1 : 0) -
          (starredSlugs.has(a.w.slug) ? 1 : 0) ||
        a.i - b.i,
    );
    return [{ key: "results", items: hits.map((x) => x.w) }];
  }

  if (orderedByRecent.length <= RECENT_MAX) {
    return [
      {
        key: null,
        items: [
          ...orderedByRecent.filter((w) => starredSlugs.has(w.slug)),
          ...orderedByRecent.filter((w) => !starredSlugs.has(w.slug)),
        ],
      },
    ];
  }

  const starred = orderedByRecent
    .filter((w) => starredSlugs.has(w.slug))
    .sort(compareByName);
  const recent = orderedByRecent
    .filter((w) => !starredSlugs.has(w.slug) && (lastOpenedAt[w.slug] ?? 0) > 0)
    .slice(0, RECENT_MAX);
  const recentSet = new Set(recent.map((w) => w.id));
  const rest = orderedByRecent
    .filter((w) => !starredSlugs.has(w.slug) && !recentSet.has(w.id))
    .sort(compareByName);

  const sections: WorkspaceSection[] = [];
  if (starred.length > 0) sections.push({ key: "starred", items: starred });
  if (recent.length > 0) sections.push({ key: "recent", items: recent });
  if (rest.length > 0) sections.push({ key: "all", items: rest });
  return sections;
}

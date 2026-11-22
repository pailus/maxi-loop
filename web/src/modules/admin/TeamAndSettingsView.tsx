import { type FormEvent, useCallback, useEffect, useState } from "react";
import { useUrlEnum } from "../../lib/urlState";
import {
  AlertCircle,
  Check,
  Copy,
  KeyRound,
  Plus,
  Settings2,
  ShieldCheck,
  Trash2,
  Users,
} from "lucide-react";
import {
  type ApiTokenView,
  type AuthSessionView,
  type ProjectView,
  type Role,
  type TenantMemberView,
  type WorkspaceView,
  createApiToken,
  createMember,
  deleteMember,
  fetchApiTokens,
  fetchMembers,
  fetchWorkspaces,
  revokeApiToken,
  updateMemberRole,
  updateTenantSettings,
  updateWorkspaceSettings,
} from "../../lib/api";
import { WorkspaceMark } from "../workspaces/WorkspaceMark";
import {
  WORKSPACE_COLORS,
  resolveWorkspaceColor,
  saveWorkspaceColor,
} from "../workspaces/workspacePrefs";

export type AdminTab = "members" | "settings" | "api_tokens";

const ADMIN_TABS: readonly AdminTab[] = ["members", "settings", "api_tokens"];

interface TeamAndSettingsViewProps {
  session: AuthSessionView;
  projects: ProjectView[];
  initialTab?: AdminTab;
  tabNonce?: number;
  activeWorkspaceSlug?: string;
  onOpenCreateWorkspace?: () => void;
  onSettingsChanged: () => Promise<void>;
  onShowToast: (message: string) => void;
}

const TENANT_ROLES: Role[] = ["tenant_owner", "tenant_admin", "member", "viewer"];
const WORKSPACE_ROLES: Role[] = ["workspace_admin", "member", "viewer"];

export function TeamAndSettingsView({
  session,
  projects,
  initialTab = "members",
  tabNonce = 0,
  activeWorkspaceSlug,
  onOpenCreateWorkspace,
  onSettingsChanged,
  onShowToast,
}: TeamAndSettingsViewProps) {
  const [tab, setTab] = useUrlEnum<AdminTab>("tab", ADMIN_TABS, "members", initialTab);

  // Explicit tab requests from elsewhere in the app (nonce > 0); on mount the
  // URL wins so a refresh keeps the open tab.
  useEffect(() => {
    if (tabNonce > 0) setTab(initialTab);
  }, [initialTab, tabNonce, setTab]);

  // Members state
  const [members, setMembers] = useState<TenantMemberView[]>([]);
  const [workspaces, setWorkspaces] = useState<WorkspaceView[]>([]);
  const [loadingMembers, setLoadingMembers] = useState(true);
  const [memberError, setMemberError] = useState<string | null>(null);

  const [newEmail, setNewEmail] = useState("");
  const [newName, setNewName] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [newScopeWs, setNewScopeWs] = useState("");
  const [newRole, setNewRole] = useState<Role>("member");
  const [creatingMember, setCreatingMember] = useState(false);

  // Tenant settings state
  const [tenantName, setTenantName] = useState(session.tenant.name);
  const [tenantRetention, setTenantRetention] = useState(
    String(session.tenant.retention_days),
  );
  const [tenantDefaultRpm, setTenantDefaultRpm] = useState(
    String(session.tenant.default_rate_limit_per_minute),
  );
  const [tenantAggRpm, setTenantAggRpm] = useState(
    String(session.tenant.aggregate_rate_limit_per_minute),
  );
  const [savingTenant, setSavingTenant] = useState(false);
  const [settingsError, setSettingsError] = useState<string | null>(null);

  // Workspace settings editor state
  const [wsEdits, setWsEdits] = useState<
    Record<string, { name: string; color: string; retention: string; rpm: string }>
  >({});

  // Per-project API tokens state
  const [apiTokens, setApiTokens] = useState<ApiTokenView[]>([]);
  const [loadingTokens, setLoadingTokens] = useState(false);
  const [tokenError, setTokenError] = useState<string | null>(null);
  const [tokName, setTokName] = useState("");
  const [tokProjectSlug, setTokProjectSlug] = useState(projects[0]?.slug ?? "");
  const [tokCanWrite, setTokCanWrite] = useState(false);
  const [tokExpiryDays, setTokExpiryDays] = useState("90");
  const [creatingToken, setCreatingToken] = useState(false);
  const [newlyCreatedToken, setNewlyCreatedToken] = useState<{
    raw: string;
    projectSlug: string;
  } | null>(null);
  const [copiedRaw, setCopiedRaw] = useState(false);

  const canManageTenant =
    session.tenant_role === "tenant_owner" ||
    session.tenant_role === "tenant_admin";
  const isTenantOwner = session.tenant_role === "tenant_owner";
  const canManageAnyWorkspace =
    canManageTenant ||
    Object.values(session.workspace_roles).some((r) => r === "workspace_admin");

  useEffect(() => {
    setTenantName(session.tenant.name);
    setTenantRetention(String(session.tenant.retention_days));
    setTenantDefaultRpm(String(session.tenant.default_rate_limit_per_minute));
    setTenantAggRpm(String(session.tenant.aggregate_rate_limit_per_minute));
  }, [session]);

  useEffect(() => {
    if (!tokProjectSlug && projects.length > 0) {
      setTokProjectSlug(projects[0].slug);
    }
  }, [projects, tokProjectSlug]);

  const loadMembersAndWorkspaces = useCallback(async () => {
    setLoadingMembers(true);
    setMemberError(null);
    try {
      const [mList, wList] = await Promise.all([
        fetchMembers(),
        fetchWorkspaces(),
      ]);
      setMembers(mList);
      setWorkspaces(wList);
      const nextEdits: Record<
        string,
        { name: string; color: string; retention: string; rpm: string }
      > = {};
      for (const w of wList) {
        nextEdits[w.slug] = {
          name: w.name,
          color: resolveWorkspaceColor(session.tenant.slug, w.slug),
          retention:
            w.retention_days_override !== null
              ? String(w.retention_days_override)
              : "",
          rpm:
            w.default_rate_limit_per_minute_override !== null
              ? String(w.default_rate_limit_per_minute_override)
              : "",
        };
      }
      setWsEdits(nextEdits);
    } catch (err) {
      setMemberError(
        err instanceof Error ? err.message : "Failed to load members",
      );
    } finally {
      setLoadingMembers(false);
    }
  }, [session.tenant.slug]);

  const loadApiTokens = useCallback(async () => {
    setLoadingTokens(true);
    setTokenError(null);
    try {
      const list = await fetchApiTokens();
      setApiTokens(list);
    } catch (err) {
      setTokenError(
        err instanceof Error ? err.message : "Failed to load API tokens",
      );
    } finally {
      setLoadingTokens(false);
    }
  }, []);

  useEffect(() => {
    void loadMembersAndWorkspaces();
  }, [loadMembersAndWorkspaces, activeWorkspaceSlug]);

  useEffect(() => {
    if (tab === "api_tokens") {
      void loadApiTokens();
    }
  }, [tab, loadApiTokens]);

  async function handleAddMember(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    setCreatingMember(true);
    setMemberError(null);
    try {
      await createMember({
        email: newEmail.trim(),
        name: newName.trim() || undefined,
        password: newPassword || undefined,
        role: newRole,
        workspace_slug: newScopeWs || null,
      });
      setNewEmail("");
      setNewName("");
      setNewPassword("");
      await loadMembersAndWorkspaces();
      onShowToast("Member added");
    } catch (err) {
      setMemberError(
        err instanceof Error ? err.message : "Failed to add member",
      );
    } finally {
      setCreatingMember(false);
    }
  }

  async function handleRoleChange(m: TenantMemberView, role: Role) {
    setMemberError(null);
    try {
      await updateMemberRole(m.membership_id, role);
      await loadMembersAndWorkspaces();
      onShowToast(`Updated ${m.email} role to ${role}`);
    } catch (err) {
      setMemberError(
        err instanceof Error ? err.message : "Failed to update role",
      );
    }
  }

  async function handleDeleteMember(m: TenantMemberView) {
    setMemberError(null);
    try {
      await deleteMember(m.membership_id);
      await loadMembersAndWorkspaces();
      onShowToast(`Removed membership for ${m.email}`);
    } catch (err) {
      setMemberError(
        err instanceof Error ? err.message : "Failed to remove membership",
      );
    }
  }

  async function handleSaveTenantSettings(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    setSavingTenant(true);
    setSettingsError(null);
    try {
      await updateTenantSettings({
        name: tenantName.trim(),
        retention_days: Number.parseInt(tenantRetention, 10),
        default_rate_limit_per_minute: Number.parseInt(tenantDefaultRpm, 10),
        aggregate_rate_limit_per_minute: Number.parseInt(tenantAggRpm, 10),
      });
      await onSettingsChanged();
      await loadMembersAndWorkspaces();
      onShowToast("Tenant defaults & aggregate rate limit updated live");
    } catch (err) {
      setSettingsError(
        err instanceof Error ? err.message : "Failed to update tenant settings",
      );
    } finally {
      setSavingTenant(false);
    }
  }

  async function handleSaveWorkspaceSettings(wsSlug: string) {
    setSettingsError(null);
    const edit = wsEdits[wsSlug];
    if (!edit) return;
    try {
      const cleanName = edit.name.trim();
      if (!cleanName) {
        setSettingsError("Workspace name cannot be empty.");
        return;
      }
      const retentionOverride = edit.retention.trim()
        ? Number.parseInt(edit.retention.trim(), 10)
        : null;
      const rpmOverride = edit.rpm.trim()
        ? Number.parseInt(edit.rpm.trim(), 10)
        : null;
      await updateWorkspaceSettings(wsSlug, {
        name: cleanName,
        retention_days_override: retentionOverride,
        default_rate_limit_per_minute_override: rpmOverride,
      });
      saveWorkspaceColor(session.tenant.slug, wsSlug, edit.color);
      await onSettingsChanged();
      await loadMembersAndWorkspaces();
      onShowToast(`Workspace ${cleanName} settings updated`);
    } catch (err) {
      setSettingsError(
        err instanceof Error
          ? err.message
          : "Failed to update workspace settings",
      );
    }
  }

  async function handleCreateToken(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    if (!tokProjectSlug) {
      setTokenError("Select a project first.");
      return;
    }
    setCreatingToken(true);
    setTokenError(null);
    try {
      const parsedExp = tokExpiryDays.trim()
        ? Number.parseInt(tokExpiryDays.trim(), 10)
        : null;
      const res = await createApiToken({
        project_slug: tokProjectSlug,
        name: tokName.trim(),
        can_write: tokCanWrite,
        expires_in_days: parsedExp,
      });
      setNewlyCreatedToken({
        raw: res.raw_token,
        projectSlug: res.token.project_slug,
      });
      setTokName("");
      await loadApiTokens();
      onShowToast(`Created project API token for ${res.token.project_slug}`);
    } catch (err) {
      setTokenError(
        err instanceof Error ? err.message : "Failed to create API token",
      );
    } finally {
      setCreatingToken(false);
    }
  }

  async function handleRevokeToken(tok: ApiTokenView) {
    setTokenError(null);
    try {
      await revokeApiToken(tok.id);
      await loadApiTokens();
      onShowToast(`Revoked API token ${tok.name}`);
    } catch (err) {
      setTokenError(
        err instanceof Error ? err.message : "Failed to revoke API token",
      );
    }
  }

  const availableRolesForNew = newScopeWs
    ? WORKSPACE_ROLES
    : isTenantOwner
      ? TENANT_ROLES
      : TENANT_ROLES.filter((r) => r !== "tenant_owner");

  return (
    <div className="flex-1 min-w-0 flex flex-col overflow-hidden bg-[var(--color-bg)]">
      {/* Header & Sub-tabs */}
      <div className="px-4 md:px-6 pt-4 border-b border-[var(--color-border)] bg-[var(--color-bg)] shrink-0">
        <div className="text-xs text-[var(--color-text-dim)] flex items-center gap-1.5">
          <span>{session.tenant.name}</span>
          <span>›</span>
          <span className="font-medium text-[var(--color-text-muted)]">
            Team, Settings &amp; API Tokens
          </span>
        </div>

        <h1 className="mt-1.5 text-xl md:text-2xl font-semibold text-[var(--color-text)] tracking-tight">
          Access Control, Inherited Settings &amp; Project Tokens
        </h1>

        <div className="mt-3 flex items-center gap-5">
          <button
            type="button"
            onClick={() => setTab("members")}
            className={`pm-tab inline-flex items-center gap-1.5 cursor-pointer ${
              tab === "members" ? "pm-tab-active" : ""
            }`}
          >
            <Users size={14} />
            <span>Members &amp; RBAC</span>
          </button>

          <button
            type="button"
            onClick={() => setTab("settings")}
            className={`pm-tab inline-flex items-center gap-1.5 cursor-pointer ${
              tab === "settings" ? "pm-tab-active" : ""
            }`}
          >
            <Settings2 size={14} />
            <span>Inherited Settings &amp; Limits</span>
          </button>

          <button
            type="button"
            onClick={() => setTab("api_tokens")}
            className={`pm-tab inline-flex items-center gap-1.5 cursor-pointer ${
              tab === "api_tokens" ? "pm-tab-active" : ""
            }`}
          >
            <KeyRound size={14} />
            <span>Project API Tokens</span>
          </button>
        </div>
      </div>

      {/* Tab Body */}
      <div className="flex-1 min-h-0 overflow-y-auto p-6">
        {tab === "members" && (
          <div className="grid grid-cols-1 lg:grid-cols-[1fr_360px] gap-6">
            <div>
              <h2 className="text-sm font-bold text-[var(--color-text)]">
                Tenant &amp; Workspace Memberships
              </h2>
              <p className="mt-0.5 text-xs text-[var(--color-text-dim)]">
                Roles inherit downward: tenant roles apply across all workspaces; workspace roles apply to all projects inside that workspace.
              </p>

              {memberError && (
                <div className="mt-3 p-2.5 rounded bg-[var(--color-danger-light)] border border-[#f87168] text-[var(--color-danger-text)] text-xs font-medium flex items-center gap-2">
                  <AlertCircle size={14} className="shrink-0" />
                  <span>{memberError}</span>
                </div>
              )}

              <div className="mt-4 rounded-lg border border-[var(--color-border)] overflow-x-auto">
                <table className="w-full max-md:min-w-[640px] text-left border-collapse text-[13px]">
                  <thead>
                    <tr className="bg-[var(--color-bg-muted)] border-b border-[var(--color-border)] text-[var(--color-text-dim)] uppercase text-[11px]">
                      <th className="py-2.5 px-3 font-semibold">User</th>
                      <th className="py-2.5 px-3 font-semibold">Scope</th>
                      <th className="py-2.5 px-3 font-semibold">Role</th>
                      <th className="py-2.5 px-3 font-semibold text-right">
                        Actions
                      </th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-[var(--color-border-light)]">
                    {loadingMembers ? (
                      <tr>
                        <td
                          colSpan={4}
                          className="py-6 text-center text-[var(--color-text-dim)]"
                        >
                          Loading memberships...
                        </td>
                      </tr>
                    ) : members.length === 0 ? (
                      <tr>
                        <td
                          colSpan={4}
                          className="py-6 text-center text-[var(--color-text-dim)]"
                        >
                          No memberships found.
                        </td>
                      </tr>
                    ) : (
                      members.map((m) => {
                        const roleChoices =
                          m.scope === "workspace"
                            ? WORKSPACE_ROLES
                            : isTenantOwner
                              ? TENANT_ROLES
                              : TENANT_ROLES.filter((r) => r !== "tenant_owner");
                        const canEditRow =
                          m.scope === "tenant"
                            ? canManageTenant &&
                              (m.role !== "tenant_owner" || isTenantOwner)
                            : canManageAnyWorkspace;
                        return (
                          <tr
                            key={m.membership_id}
                            className="hover:bg-[var(--color-bg-muted)]"
                          >
                            <td className="py-2.5 px-3">
                              <div className="font-semibold text-[var(--color-text)]">
                                {m.name}
                              </div>
                              <div className="pm-mono text-[11px] text-[var(--color-text-dim)]">
                                {m.email}
                              </div>
                            </td>
                            <td className="py-2.5 px-3">
                              {m.scope === "tenant" ? (
                                <span className="pm-lozenge pm-lozenge-primary">
                                  TENANT
                                </span>
                              ) : (
                                <span className="pm-lozenge">
                                  WS: {m.workspace_slug}
                                </span>
                              )}
                            </td>
                            <td className="py-2.5 px-3">
                              {canEditRow ? (
                                <select
                                  value={m.role}
                                  onChange={(e) =>
                                    void handleRoleChange(
                                      m,
                                      e.target.value as Role,
                                    )
                                  }
                                  aria-label={`Role for ${m.email}`}
                                  className="pm-input h-7 text-xs py-0 px-2 w-40"
                                >
                                  {roleChoices.map((r) => (
                                    <option key={r} value={r}>
                                      {r}
                                    </option>
                                  ))}
                                </select>
                              ) : (
                                <span className="pm-lozenge">{m.role}</span>
                              )}
                            </td>
                            <td className="py-2.5 px-3 text-right">
                              {canEditRow && (
                                <button
                                  type="button"
                                  onClick={() => void handleDeleteMember(m)}
                                  className="h-7 px-2 rounded text-[var(--color-danger-text)] hover:bg-[var(--color-danger-light)] inline-flex items-center gap-1 cursor-pointer"
                                  title="Remove membership"
                                >
                                  <Trash2 size={13} />
                                  <span>Remove</span>
                                </button>
                              )}
                            </td>
                          </tr>
                        );
                      })
                    )}
                  </tbody>
                </table>
              </div>
            </div>

            {/* Add Member Form */}
            <aside className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-card)] p-4 h-fit">
              <div className="flex items-center gap-2 text-sm font-bold text-[var(--color-text)]">
                <ShieldCheck size={16} className="text-[var(--color-primary)]" />
                <span>Add Member / Role</span>
              </div>
              <p className="mt-1 text-xs text-[var(--color-text-dim)]">
                Creates a new Argon2id user account or grants an additional workspace role to an existing user.
              </p>

              <form onSubmit={handleAddMember} className="mt-3 space-y-3">
                <div>
                  <label className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1">
                    Email
                  </label>
                  <input
                    type="email"
                    required
                    disabled={!canManageAnyWorkspace}
                    value={newEmail}
                    onChange={(e) => setNewEmail(e.target.value)}
                    placeholder="engineer@company.io"
                    className="pm-input"
                  />
                </div>

                <div>
                  <label className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1">
                    Display name (new users)
                  </label>
                  <input
                    type="text"
                    disabled={!canManageAnyWorkspace}
                    value={newName}
                    onChange={(e) => setNewName(e.target.value)}
                    placeholder="Alex Rivera"
                    className="pm-input"
                  />
                </div>

                <div>
                  <label className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1">
                    Initial password (min 12 chars, new users)
                  </label>
                  <input
                    type="password"
                    disabled={!canManageAnyWorkspace}
                    value={newPassword}
                    onChange={(e) => setNewPassword(e.target.value)}
                    placeholder="Optional if user already exists"
                    className="pm-input"
                  />
                </div>

                <div>
                  <label className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1">
                    Scope
                  </label>
                  <select
                    disabled={!canManageAnyWorkspace}
                    value={newScopeWs}
                    onChange={(e) => {
                      const val = e.target.value;
                      setNewScopeWs(val);
                      if (val && (newRole === "tenant_owner" || newRole === "tenant_admin")) {
                        setNewRole("workspace_admin");
                      } else if (!val && newRole === "workspace_admin") {
                        setNewRole("member");
                      }
                    }}
                    className="pm-input"
                  >
                    {canManageTenant && (
                      <option value="">Tenant-wide (all workspaces)</option>
                    )}
                    {workspaces.map((w) => (
                      <option key={w.id} value={w.slug}>
                        Workspace: {w.name} ({w.slug})
                      </option>
                    ))}
                  </select>
                </div>

                <div>
                  <label className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1">
                    Role
                  </label>
                  <select
                    disabled={!canManageAnyWorkspace}
                    value={newRole}
                    onChange={(e) => setNewRole(e.target.value as Role)}
                    className="pm-input"
                  >
                    {availableRolesForNew.map((r) => (
                      <option key={r} value={r}>
                        {r}
                      </option>
                    ))}
                  </select>
                </div>

                <button
                  type="submit"
                  disabled={creatingMember || !canManageAnyWorkspace}
                  className="w-full h-9 rounded-[6px] bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-50 text-white text-sm font-semibold inline-flex items-center justify-center gap-1.5 cursor-pointer"
                >
                  <Plus size={14} />
                  <span>{creatingMember ? "Adding..." : "Grant membership"}</span>
                </button>
              </form>
            </aside>
          </div>
        )}

        {tab === "settings" && (
          <div className="space-y-6">
            {settingsError && (
              <div className="p-2.5 rounded bg-[var(--color-danger-light)] border border-[#f87168] text-[var(--color-danger-text)] text-xs font-medium flex items-center gap-2">
                <AlertCircle size={14} className="shrink-0" />
                <span>{settingsError}</span>
              </div>
            )}

            {/* Tenant Defaults & Aggregate Limit */}
            <section className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-card)] p-5">
              <h2 className="text-sm font-bold text-[var(--color-text)]">
                Tenant Defaults &amp; Aggregate Rate Limit ({session.tenant.slug})
              </h2>
              <p className="mt-0.5 text-xs text-[var(--color-text-dim)]">
                Retention and default project rate limit cascade down to workspaces and projects unless overridden. The aggregate tenant rate limit protects shared capacity across all projects in this tenant (`X-Sentry-Rate-Limits: organization`).
              </p>

              <form
                onSubmit={handleSaveTenantSettings}
                className="mt-4 grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-3"
              >
                <div>
                  <label className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1">
                    Tenant name
                  </label>
                  <input
                    type="text"
                    disabled={!canManageTenant}
                    value={tenantName}
                    onChange={(e) => setTenantName(e.target.value)}
                    className="pm-input"
                  />
                </div>
                <div>
                  <label className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1">
                    Default retention (days)
                  </label>
                  <input
                    type="number"
                    min={1}
                    max={3650}
                    disabled={!canManageTenant}
                    value={tenantRetention}
                    onChange={(e) => setTenantRetention(e.target.value)}
                    className="pm-input pm-mono"
                  />
                </div>
                <div>
                  <label className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1">
                    Default project limit (/min)
                  </label>
                  <input
                    type="number"
                    min={1}
                    max={1000000}
                    disabled={!canManageTenant}
                    value={tenantDefaultRpm}
                    onChange={(e) => setTenantDefaultRpm(e.target.value)}
                    className="pm-input pm-mono"
                  />
                </div>
                <div>
                  <label className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1">
                    Aggregate tenant cap (/min)
                  </label>
                  <input
                    type="number"
                    min={1}
                    max={10000000}
                    disabled={!canManageTenant}
                    value={tenantAggRpm}
                    onChange={(e) => setTenantAggRpm(e.target.value)}
                    className="pm-input pm-mono"
                  />
                </div>

                <div className="sm:col-span-2 lg:col-span-4 flex justify-end">
                  <button
                    type="submit"
                    disabled={savingTenant || !canManageTenant}
                    className="h-8 px-4 rounded-[6px] bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-50 text-white text-sm font-semibold cursor-pointer"
                  >
                    {savingTenant ? "Saving..." : "Save tenant defaults"}
                  </button>
                </div>
              </form>
            </section>

            {/* Workspace Overrides Table */}
            <section className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-card)] p-5">
              <div className="flex items-start justify-between gap-3 flex-wrap">
                <div>
                  <h2 className="text-sm font-bold text-[var(--color-text)]">
                    Workspaces &amp; Inheritance Overrides ({workspaces.length})
                  </h2>
                  <p className="mt-0.5 text-xs text-[var(--color-text-dim)]">
                    Rename workspaces, pick badge colors, or override tenant defaults. Leave override fields blank to inherit the tenant default. Changes propagate immediately to the in-memory ingest rate limiter.
                  </p>
                </div>
                {canManageTenant && onOpenCreateWorkspace && (
                  <button
                    type="button"
                    onClick={onOpenCreateWorkspace}
                    className="h-8 px-3 rounded-[6px] bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] text-white text-sm font-semibold inline-flex items-center gap-1.5 shrink-0 cursor-pointer"
                  >
                    <Plus size={14} />
                    <span>Create workspace</span>
                  </button>
                )}
              </div>

              <div className="mt-4 space-y-3">
                {workspaces.map((ws) => {
                  const edit = wsEdits[ws.slug] ?? {
                    name: ws.name,
                    color: resolveWorkspaceColor(session.tenant.slug, ws.slug),
                    retention: "",
                    rpm: "",
                  };
                  const isCurrentWs = ws.slug === activeWorkspaceSlug;
                  return (
                    <div
                      key={ws.id}
                      className={`rounded-md border p-3.5 space-y-3 ${
                        isCurrentWs
                          ? "border-[var(--color-primary)] bg-[var(--color-primary-light)]/30"
                          : "border-[var(--color-border-light)] bg-[var(--color-bg-muted)]"
                      }`}
                    >
                      <div className="flex items-center justify-between gap-3 flex-wrap">
                        <div className="flex items-center gap-2.5 min-w-0">
                          <WorkspaceMark
                            name={edit.name || ws.name}
                            slug={ws.slug}
                            tenantSlug={session.tenant.slug}
                            colorOverride={edit.color}
                            size={30}
                          />
                          <div className="min-w-0">
                            <div className="text-xs font-bold text-[var(--color-text)] flex items-center gap-1.5 flex-wrap">
                              <span>{ws.name}</span>
                              <span className="pm-mono font-normal text-[var(--color-text-dim)]">
                                ({ws.slug})
                              </span>
                              {ws.is_default && (
                                <span className="pm-lozenge">DEFAULT</span>
                              )}
                              {isCurrentWs && (
                                <span className="pm-lozenge pm-lozenge-primary">
                                  ACTIVE
                                </span>
                              )}
                            </div>
                            <div className="mt-0.5 text-[11px] text-[var(--color-text-muted)]">
                              Effective:{" "}
                              <strong>{ws.effective_retention_days}d</strong>{" "}
                              retention ·{" "}
                              <strong>
                                {ws.effective_rate_limit_per_minute.toLocaleString()}
                                /min
                              </strong>{" "}
                              default project limit
                            </div>
                          </div>
                        </div>

                        <div className="flex items-center gap-1">
                          {WORKSPACE_COLORS.map((c) => {
                            const selected = edit.color === c;
                            return (
                              <button
                                key={c}
                                type="button"
                                disabled={!canManageAnyWorkspace}
                                onClick={() =>
                                  setWsEdits((prev) => ({
                                    ...prev,
                                    [ws.slug]: { ...edit, color: c },
                                  }))
                                }
                                title={`Workspace color ${c}`}
                                aria-label={`Workspace color ${c}`}
                                className="w-5 h-5 rounded flex items-center justify-center cursor-pointer disabled:opacity-50"
                                style={{
                                  backgroundColor: c,
                                  boxShadow: selected
                                    ? `0 0 0 1.5px var(--color-bg-card), 0 0 0 3px ${c}`
                                    : undefined,
                                }}
                              >
                                {selected && (
                                  <Check
                                    size={11}
                                    className="text-white"
                                    strokeWidth={3}
                                  />
                                )}
                              </button>
                            );
                          })}
                        </div>
                      </div>

                      <div className="grid grid-cols-1 md:grid-cols-[1fr_170px_190px_auto] gap-3 items-end">
                        <div>
                          <label className="block text-[11px] font-semibold text-[var(--color-text-dim)] mb-1">
                            Workspace name
                          </label>
                          <input
                            type="text"
                            maxLength={128}
                            disabled={!canManageAnyWorkspace}
                            value={edit.name}
                            onChange={(e) =>
                              setWsEdits((prev) => ({
                                ...prev,
                                [ws.slug]: { ...edit, name: e.target.value },
                              }))
                            }
                            className="pm-input h-8 text-sm"
                          />
                        </div>

                        <div>
                          <label className="block text-[11px] font-semibold text-[var(--color-text-dim)] mb-1">
                            Retention override (d)
                          </label>
                          <input
                            type="number"
                            min={1}
                            max={session.tenant.retention_days}
                            disabled={!canManageAnyWorkspace}
                            value={edit.retention}
                            placeholder={`Inherit (${session.tenant.retention_days}d)`}
                            onChange={(e) =>
                              setWsEdits((prev) => ({
                                ...prev,
                                [ws.slug]: { ...edit, retention: e.target.value },
                              }))
                            }
                            className="pm-input pm-mono h-8 text-sm"
                          />
                        </div>

                        <div>
                          <label className="block text-[11px] font-semibold text-[var(--color-text-dim)] mb-1">
                            Default limit override (/min)
                          </label>
                          <input
                            type="number"
                            min={1}
                            max={1000000}
                            disabled={!canManageAnyWorkspace}
                            value={edit.rpm}
                            placeholder={`Inherit (${session.tenant.default_rate_limit_per_minute})`}
                            onChange={(e) =>
                              setWsEdits((prev) => ({
                                ...prev,
                                [ws.slug]: { ...edit, rpm: e.target.value },
                              }))
                            }
                            className="pm-input pm-mono h-8 text-sm"
                          />
                        </div>

                        <button
                          type="button"
                          disabled={!canManageAnyWorkspace}
                          onClick={() =>
                            void handleSaveWorkspaceSettings(ws.slug)
                          }
                          className="h-8 px-3 rounded bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-50 text-white text-sm font-semibold cursor-pointer"
                        >
                          Apply
                        </button>
                      </div>
                    </div>
                  );
                })}
              </div>
            </section>
          </div>
        )}

        {tab === "api_tokens" && (
          <div className="grid grid-cols-1 lg:grid-cols-[1fr_380px] gap-6">
            <div>
              <h2 className="text-sm font-bold text-[var(--color-text)]">
                Per-Project API Tokens (`mlapi_...`)
              </h2>
              <p className="mt-0.5 text-xs text-[var(--color-text-dim)]">
                Scoped strictly to a single project for CI/CD source map uploads, release registration, and project issue/performance queries. Stored only as SHA-256 hashes.
              </p>

              {tokenError && (
                <div className="mt-3 p-2.5 rounded bg-[var(--color-danger-light)] border border-[#f87168] text-[var(--color-danger-text)] text-xs font-medium flex items-center gap-2">
                  <AlertCircle size={14} className="shrink-0" />
                  <span>{tokenError}</span>
                </div>
              )}

              {newlyCreatedToken && (
                <div className="mt-4 rounded-lg border border-[#4bce97] bg-[var(--color-success-light)] p-4">
                  <div className="text-xs font-bold text-[var(--color-success-text)]">
                    Copy your new API token now — it will never be shown again!
                  </div>
                  <div className="mt-2 flex items-center gap-2">
                    <input
                      type="text"
                      readOnly
                      value={newlyCreatedToken.raw}
                      onFocus={(e) => e.currentTarget.select()}
                      className="pm-input pm-mono text-xs flex-1 bg-white"
                    />
                    <button
                      type="button"
                      onClick={() => {
                        navigator.clipboard
                          ?.writeText(newlyCreatedToken.raw)
                          .catch(() => {});
                        setCopiedRaw(true);
                        setTimeout(() => setCopiedRaw(false), 1800);
                      }}
                      className="h-8 px-3 rounded bg-[var(--color-primary)] text-white text-sm font-semibold inline-flex items-center gap-1 cursor-pointer"
                    >
                      {copiedRaw ? <Check size={13} /> : <Copy size={13} />}
                      <span>{copiedRaw ? "Copied" : "Copy"}</span>
                    </button>
                  </div>
                  <pre className="mt-2.5 p-2.5 rounded bg-white border border-[var(--color-border)] pm-mono text-[11px] overflow-x-auto text-[var(--color-text)]">
{`# CI Source Map Upload Example
curl -X POST -H "Authorization: Bearer ${newlyCreatedToken.raw}" \\
  --data-binary @dist/assets/app.js.map \\
  "${window.location.origin}/api/v1/projects/${newlyCreatedToken.projectSlug}/releases/1.0.0/sourcemaps?filename=assets/app.js"`}
                  </pre>
                </div>
              )}

              <div className="mt-4 rounded-lg border border-[var(--color-border)] overflow-x-auto">
                <table className="w-full max-md:min-w-[640px] text-left border-collapse text-[13px]">
                  <thead>
                    <tr className="bg-[var(--color-bg-muted)] border-b border-[var(--color-border)] text-[var(--color-text-dim)] uppercase text-[11px]">
                      <th className="py-2.5 px-3 font-semibold">Name / Prefix</th>
                      <th className="py-2.5 px-3 font-semibold">Project</th>
                      <th className="py-2.5 px-3 font-semibold">Scope</th>
                      <th className="py-2.5 px-3 font-semibold">Status</th>
                      <th className="py-2.5 px-3 font-semibold text-right">
                        Actions
                      </th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-[var(--color-border-light)]">
                    {loadingTokens ? (
                      <tr>
                        <td
                          colSpan={5}
                          className="py-6 text-center text-[var(--color-text-dim)]"
                        >
                          Loading API tokens...
                        </td>
                      </tr>
                    ) : apiTokens.length === 0 ? (
                      <tr>
                        <td
                          colSpan={5}
                          className="py-6 text-center text-[var(--color-text-dim)]"
                        >
                          No project API tokens created yet.
                        </td>
                      </tr>
                    ) : (
                      apiTokens.map((t) => (
                        <tr
                          key={t.id}
                          className="hover:bg-[var(--color-bg-muted)]"
                        >
                          <td className="py-2.5 px-3">
                            <div className="font-semibold text-[var(--color-text)]">
                              {t.name}
                            </div>
                            <div className="pm-mono text-[11px] text-[var(--color-text-dim)]">
                              {t.token_prefix}…
                            </div>
                          </td>
                          <td className="py-2.5 px-3 pm-mono">
                            {t.project_slug}
                          </td>
                          <td className="py-2.5 px-3">
                            <span
                              className={`pm-lozenge ${
                                t.can_write
                                  ? "pm-lozenge-warning"
                                  : "pm-lozenge-primary"
                              }`}
                            >
                              {t.can_write ? "READ + WRITE" : "READ ONLY"}
                            </span>
                          </td>
                          <td className="py-2.5 px-3">
                            {t.revoked_at ? (
                              <span className="pm-lozenge pm-lozenge-danger">
                                REVOKED
                              </span>
                            ) : (
                              <span className="pm-lozenge pm-lozenge-success">
                                ACTIVE
                              </span>
                            )}
                          </td>
                          <td className="py-2.5 px-3 text-right">
                            {!t.revoked_at && canManageAnyWorkspace && (
                              <button
                                type="button"
                                onClick={() => void handleRevokeToken(t)}
                                className="h-7 px-2 rounded text-[var(--color-danger-text)] hover:bg-[var(--color-danger-light)] inline-flex items-center gap-1 cursor-pointer"
                              >
                                <Trash2 size={13} />
                                <span>Revoke</span>
                              </button>
                            )}
                          </td>
                        </tr>
                      ))
                    )}
                  </tbody>
                </table>
              </div>
            </div>

            <aside className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-card)] p-4 h-fit">
              <h3 className="text-sm font-bold text-[var(--color-text)]">
                Mint Project API Token
              </h3>
              <p className="mt-1 text-xs text-[var(--color-text-dim)]">
                Token access is strictly limited to one project and cannot call admin/settings endpoints.
              </p>

              <form onSubmit={handleCreateToken} className="mt-3 space-y-3">
                <div>
                  <label className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1">
                    Project
                  </label>
                  <select
                    disabled={!canManageAnyWorkspace}
                    value={tokProjectSlug}
                    onChange={(e) => setTokProjectSlug(e.target.value)}
                    className="pm-input"
                  >
                    {projects.map((p) => (
                      <option key={p.id} value={p.slug}>
                        {p.name} ({p.slug})
                      </option>
                    ))}
                  </select>
                </div>

                <div>
                  <label className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1">
                    Token label
                  </label>
                  <input
                    type="text"
                    required
                    disabled={!canManageAnyWorkspace}
                    value={tokName}
                    onChange={(e) => setTokName(e.target.value)}
                    placeholder="github-actions-sourcemaps"
                    className="pm-input"
                  />
                </div>

                <div>
                  <label className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1">
                    Expires in (days, optional)
                  </label>
                  <input
                    type="number"
                    min={1}
                    max={365}
                    disabled={!canManageAnyWorkspace}
                    value={tokExpiryDays}
                    onChange={(e) => setTokExpiryDays(e.target.value)}
                    placeholder="90"
                    className="pm-input pm-mono"
                  />
                </div>

                <label className="flex items-start gap-2 text-xs text-[var(--color-text-muted)] cursor-pointer">
                  <input
                    type="checkbox"
                    disabled={!canManageAnyWorkspace}
                    checked={tokCanWrite}
                    onChange={(e) => setTokCanWrite(e.target.checked)}
                    className="mt-0.5"
                  />
                  <span>
                    Allow write actions (upload/delete source maps, register releases, triage issues)
                  </span>
                </label>

                <button
                  type="submit"
                  disabled={
                    creatingToken ||
                    !canManageAnyWorkspace ||
                    projects.length === 0
                  }
                  className="w-full h-9 rounded-[6px] bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-50 text-white text-sm font-semibold inline-flex items-center justify-center gap-1.5 cursor-pointer"
                >
                  <KeyRound size={14} />
                  <span>
                    {creatingToken ? "Generating..." : "Generate API token"}
                  </span>
                </button>
              </form>
            </aside>
          </div>
        )}
      </div>
    </div>
  );
}

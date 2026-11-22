import { type FormEvent, useCallback, useEffect, useState } from "react";
import {
  Bot,
  Check,
  Copy,
  KeyRound,
  Plus,
  RefreshCw,
  ShieldAlert,
  ShieldCheck,
  Trash2,
} from "lucide-react";
import {
  ApiError,
  type CreatedMcpTokenResponse,
  type McpAccessScope,
  type McpAuditLogView,
  type McpTokenView,
  type ProjectView,
  createMcpToken,
  fetchMcpAuditLogs,
  fetchMcpTokens,
  revokeMcpToken,
} from "../../lib/api";

interface McpViewProps {
  workspaceName: string;
  workspaceSlug?: string;
  projects: ProjectView[];
  canTriage: boolean;
  onShowToast: (message: string) => void;
}

export function McpView({
  workspaceName,
  workspaceSlug,
  projects,
  canTriage,
  onShowToast,
}: McpViewProps) {
  const [tab, setTab] = useState<"tokens" | "audit">("tokens");
  const [tokens, setTokens] = useState<McpTokenView[]>([]);
  const [auditLogs, setAuditLogs] = useState<McpAuditLogView[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [name, setName] = useState("");
  const [accessScope, setAccessScope] = useState<McpAccessScope>("tenant");
  const [projectSlug, setProjectSlug] = useState("");
  const [allowTriage, setAllowTriage] = useState(false);
  const [rateLimit, setRateLimit] = useState("60");
  const [expiresInDays, setExpiresInDays] = useState("90");
  const [submitting, setSubmitting] = useState(false);

  const [createdToken, setCreatedToken] =
    useState<CreatedMcpTokenResponse | null>(null);
  const [copiedKey, setCopiedKey] = useState<"token" | "config" | null>(null);

  const loadData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [toks, logs] = await Promise.all([
        fetchMcpTokens(),
        fetchMcpAuditLogs(100),
      ]);
      setTokens(toks);
      setAuditLogs(logs);
    } catch (err) {
      setError(
        err instanceof ApiError ? err.message : "Failed to load MCP data",
      );
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadData();
  }, [loadData]);

  async function handleCreateToken(e: FormEvent) {
    e.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      const rpm = Number.parseInt(rateLimit, 10);
      const exp = expiresInDays.trim()
        ? Number.parseInt(expiresInDays, 10)
        : undefined;
      const res = await createMcpToken({
        name: name.trim(),
        access_scope: accessScope,
        workspace_slug: accessScope === "workspace" ? workspaceSlug : undefined,
        project_slug: accessScope === "project" ? projectSlug : undefined,
        can_triage: canTriage && allowTriage,
        rate_limit_per_minute: Number.isFinite(rpm) ? rpm : 60,
        expires_in_days: exp && Number.isFinite(exp) ? exp : undefined,
      });
      setCreatedToken(res);
      setName("");
      setAllowTriage(false);
      onShowToast("MCP Bearer token created (shown once — copy it now)");
      await loadData();
    } catch (err) {
      setError(
        err instanceof ApiError ? err.message : "Failed to create MCP token",
      );
    } finally {
      setSubmitting(false);
    }
  }

  async function handleRevoke(tokenId: number, label: string) {
    try {
      await revokeMcpToken(tokenId);
      onShowToast(`Revoked MCP token "${label}"`);
      await loadData();
    } catch (err) {
      setError(
        err instanceof ApiError ? err.message : "Failed to revoke token",
      );
    }
  }

  async function copyText(text: string, which: "token" | "config") {
    try {
      await navigator.clipboard.writeText(text);
      setCopiedKey(which);
      setTimeout(() => setCopiedKey(null), 2000);
    } catch {
      onShowToast("Clipboard copy failed — please copy manually");
    }
  }

  const mcpEndpointUrl = `${window.location.origin}/mcp`;
  const clientConfigSnippet = JSON.stringify(
    {
      mcpServers: {
        "maxi-loop": {
          type: "http",
          url: mcpEndpointUrl,
          headers: {
            Authorization: `Bearer ${createdToken?.raw_token ?? "<YOUR_MLMCP_TOKEN>"}`,
          },
        },
      },
    },
    null,
    2,
  );

  return (
    <div className="flex-1 min-w-0 bg-[var(--color-bg)] flex flex-col overflow-hidden">
      {/* Header */}
      <div className="px-6 pt-5 pb-3 border-b border-[var(--color-border)] flex items-center justify-between gap-4 shrink-0">
        <div>
          <div className="text-xs text-[var(--color-text-dim)]">
            {workspaceName} › AI &amp; Automation
          </div>
          <h1 className="mt-0.5 text-xl font-semibold text-[var(--color-text)] flex items-center gap-2">
            <Bot size={20} className="text-[var(--color-primary)]" />
            <span>Embedded MCP Server</span>
            <span className="pm-lozenge pm-lozenge-primary pm-mono">
              POST /mcp
            </span>
          </h1>
          <p className="mt-1 text-xs text-[var(--color-text-muted)]">
            Connect AI coding agents (Claude Code, Cursor, Codex) over Streamable
            HTTP with revocable scoped tokens, PII output scrubbing, untrusted
            event wrapping, and full audit logging.
          </p>
        </div>

        <button
          type="button"
          onClick={() => void loadData()}
          className="h-8 px-3 rounded border border-[var(--color-border)] bg-[var(--color-bg)] hover:bg-[var(--color-bg-hover)] text-xs font-medium text-[var(--color-text-muted)] inline-flex items-center gap-1.5 cursor-pointer"
        >
          <RefreshCw size={13} />
          <span>Refresh</span>
        </button>
      </div>

      {/* Tabs */}
      <div className="px-6 flex items-center gap-2 border-b border-[var(--color-border)] bg-[var(--color-bg-muted)] shrink-0">
        <button
          type="button"
          role="tab"
          aria-selected={tab === "tokens"}
          onClick={() => setTab("tokens")}
          className="pm-tab"
        >
          <KeyRound size={14} />
          <span>Bearer Tokens ({tokens.length})</span>
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === "audit"}
          onClick={() => setTab("audit")}
          className="pm-tab"
        >
          <ShieldCheck size={14} />
          <span>Audit Log ({auditLogs.length})</span>
        </button>
      </div>

      <div className="flex-1 overflow-y-auto px-6 py-5 space-y-5">
        {error && (
          <div className="rounded-md border border-[#f87168] bg-[#ffeceb] px-3.5 py-2.5 text-xs text-[#ae2e24]">
            {error}
          </div>
        )}

        {createdToken && (
          <div className="rounded-lg border border-[var(--color-primary)] bg-[var(--color-primary-light)] p-4 space-y-3">
            <div className="flex items-start justify-between gap-3">
              <div>
                <div className="text-xs font-bold text-[var(--color-primary)]">
                  New MCP Bearer Token Created — Copy Now (Shown Once)
                </div>
                <p className="mt-0.5 text-xs text-[var(--color-text-muted)]">
                  MaxiLoop stores only a SHA-256 hash. You will not be able to
                  view this raw token again after closing this panel.
                </p>
              </div>
              <button
                type="button"
                onClick={() => setCreatedToken(null)}
                className="text-xs text-[var(--color-text-dim)] hover:text-[var(--color-text)] cursor-pointer"
              >
                Dismiss
              </button>
            </div>

            <div className="flex items-center gap-2">
              <code className="flex-1 px-3 py-2 rounded bg-[var(--color-bg)] border border-[var(--color-border)] pm-mono text-xs text-[var(--color-text)] break-all">
                {createdToken.raw_token}
              </code>
              <button
                type="button"
                onClick={() => void copyText(createdToken.raw_token, "token")}
                className="h-9 px-3 rounded bg-[var(--color-primary)] text-white text-xs font-semibold inline-flex items-center gap-1.5 cursor-pointer shrink-0"
              >
                {copiedKey === "token" ? <Check size={14} /> : <Copy size={14} />}
                <span>{copiedKey === "token" ? "Copied" : "Copy token"}</span>
              </button>
            </div>
          </div>
        )}

        {tab === "tokens" && (
          <div className="grid grid-cols-1 xl:grid-cols-[1fr_380px] gap-5">
            {/* Left: Existing tokens + MCP Config */}
            <div className="space-y-4">
              <section className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-card)] overflow-hidden">
                <div className="px-4 py-3 bg-[var(--color-bg-muted)] border-b border-[var(--color-border)] flex items-center justify-between">
                  <h2 className="text-xs font-bold text-[var(--color-text)]">
                    Active &amp; Revoked MCP Tokens
                  </h2>
                  <span className="text-[11px] text-[var(--color-text-dim)]">
                    Read-only by default · Triage opt-in
                  </span>
                </div>

                {loading ? (
                  <div className="p-5 text-xs text-[var(--color-text-dim)]">
                    Loading tokens...
                  </div>
                ) : tokens.length === 0 ? (
                  <div className="p-6 text-xs text-[var(--color-text-dim)]">
                    No MCP tokens yet. Create a scoped token on the right to
                    connect an AI coding agent.
                  </div>
                ) : (
                  <div className="divide-y divide-[var(--color-border-light)]">
                    {tokens.map((tok) => {
                      const isRevoked = Boolean(tok.revoked_at);
                      return (
                        <div
                          key={tok.id}
                          className="px-4 py-3 flex items-center justify-between gap-3 text-xs"
                        >
                          <div className="min-w-0 space-y-1">
                            <div className="flex items-center gap-2 flex-wrap">
                              <span className="font-semibold text-[var(--color-text)]">
                                {tok.name}
                              </span>
                              <span className="pm-lozenge pm-mono">
                                {tok.token_prefix}…
                              </span>
                              <span className="pm-lozenge pm-lozenge-primary">
                                {tok.access_scope}
                                {tok.project_slug
                                  ? `: ${tok.project_slug}`
                                  : tok.workspace_slug
                                    ? `: ${tok.workspace_slug}`
                                    : ""}
                              </span>
                              <span
                                className={`pm-lozenge ${
                                  tok.can_triage
                                    ? "pm-lozenge-warning"
                                    : "pm-lozenge-success"
                                }`}
                              >
                                {tok.can_triage ? "READ + TRIAGE" : "READ ONLY"}
                              </span>
                              {isRevoked && (
                                <span className="pm-lozenge pm-lozenge-danger">
                                  REVOKED
                                </span>
                              )}
                            </div>
                            <div className="text-[11px] text-[var(--color-text-dim)]">
                              Owner: {tok.user_email} · Limit:{" "}
                              {tok.rate_limit_per_minute}/min · Last used:{" "}
                              {tok.last_used_at
                                ? new Date(tok.last_used_at).toLocaleString()
                                : "never"}
                            </div>
                          </div>

                          {!isRevoked && (
                            <button
                              type="button"
                              onClick={() => void handleRevoke(tok.id, tok.name)}
                              className="h-7 px-2.5 rounded border border-[var(--color-border)] hover:bg-[#ffeceb] hover:text-[#ae2e24] hover:border-[#f87168] text-[var(--color-text-muted)] text-xs inline-flex items-center gap-1 cursor-pointer shrink-0"
                            >
                              <Trash2 size={12} />
                              <span>Revoke</span>
                            </button>
                          )}
                        </div>
                      );
                    })}
                  </div>
                )}
              </section>

              <section className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-card)] p-4 space-y-2.5">
                <div className="flex items-center justify-between gap-2">
                  <h2 className="text-xs font-bold text-[var(--color-text)]">
                    MCP Client Configuration (Streamable HTTP)
                  </h2>
                  <button
                    type="button"
                    onClick={() => void copyText(clientConfigSnippet, "config")}
                    className="h-7 px-2.5 rounded border border-[var(--color-border)] hover:bg-[var(--color-bg-hover)] text-xs text-[var(--color-text-muted)] inline-flex items-center gap-1 cursor-pointer"
                  >
                    {copiedKey === "config" ? (
                      <Check size={12} />
                    ) : (
                      <Copy size={12} />
                    )}
                    <span>
                      {copiedKey === "config" ? "Copied" : "Copy JSON"}
                    </span>
                  </button>
                </div>
                <pre className="p-3 rounded bg-[var(--pm-col)] border border-[var(--color-border)] pm-mono text-xs text-[var(--color-text)] overflow-x-auto">
                  {clientConfigSnippet}
                </pre>
                <div className="flex items-start gap-2 text-[11px] text-[var(--color-text-dim)]">
                  <ShieldAlert size={14} className="shrink-0 mt-0.5 text-[#a54800]" />
                  <span>
                    Tools exposed: <code>list_projects</code>,{" "}
                    <code>list_issues</code>, <code>get_issue</code>,{" "}
                    <code>get_event</code>, <code>get_issue_stats</code>{" "}
                    (read), plus <code>update_issue_status</code> &amp;{" "}
                    <code>add_issue_note</code> (triage opt-in). Destructive
                    delete/config tools are never exposed.
                  </span>
                </div>
              </section>
            </div>

            {/* Right: Create Token Form */}
            <section className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-card)] p-4 h-fit">
              <h2 className="text-xs font-bold text-[var(--color-text)] pb-2.5 border-b border-[var(--color-border-light)]">
                Create Scoped MCP Token
              </h2>
              <form onSubmit={handleCreateToken} className="mt-3 space-y-3">
                <div>
                  <label className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1">
                    Token name
                  </label>
                  <input
                    type="text"
                    required
                    maxLength={128}
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    placeholder="e.g. Claude Code — Payments"
                    className="pm-input"
                  />
                </div>

                <div>
                  <label className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1">
                    Access scope
                  </label>
                  <select
                    value={accessScope}
                    onChange={(e) =>
                      setAccessScope(e.target.value as McpAccessScope)
                    }
                    className="pm-input"
                  >
                    <option value="tenant">Tenant (all accessible projects)</option>
                    <option value="workspace">
                      Current workspace ({workspaceName})
                    </option>
                    <option value="project">Single project (least privilege)</option>
                  </select>
                </div>

                {accessScope === "project" && (
                  <div>
                    <label className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1">
                      Project
                    </label>
                    <select
                      required
                      value={projectSlug}
                      onChange={(e) => setProjectSlug(e.target.value)}
                      className="pm-input"
                    >
                      <option value="">Select a project...</option>
                      {projects.map((p) => (
                        <option key={p.id} value={p.slug}>
                          {p.name} ({p.slug})
                        </option>
                      ))}
                    </select>
                  </div>
                )}

                <div className="grid grid-cols-2 gap-2.5">
                  <div>
                    <label className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1">
                      Rate limit (/min)
                    </label>
                    <input
                      type="number"
                      min={1}
                      max={6000}
                      value={rateLimit}
                      onChange={(e) => setRateLimit(e.target.value)}
                      className="pm-input"
                    />
                  </div>
                  <div>
                    <label className="block text-xs font-semibold text-[var(--color-text-muted)] mb-1">
                      Expires (days)
                    </label>
                    <input
                      type="number"
                      min={1}
                      max={365}
                      value={expiresInDays}
                      onChange={(e) => setExpiresInDays(e.target.value)}
                      placeholder="Never"
                      className="pm-input"
                    />
                  </div>
                </div>

                <label className="flex items-start gap-2 pt-1 text-xs text-[var(--color-text)] cursor-pointer">
                  <input
                    type="checkbox"
                    disabled={!canTriage}
                    checked={canTriage && allowTriage}
                    onChange={(e) => setAllowTriage(e.target.checked)}
                    className="mt-0.5"
                  />
                  <span>
                    <span className="font-semibold">
                      Allow triage writes (`triage` scope)
                    </span>
                    <span className="block text-[11px] text-[var(--color-text-dim)]">
                      Opt-in: enables <code>update_issue_status</code> and{" "}
                      <code>add_issue_note</code>. Leave unchecked for read-only
                      analysis.
                    </span>
                  </span>
                </label>

                <button
                  type="submit"
                  disabled={submitting}
                  className="w-full h-9 rounded bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-50 text-white text-xs font-semibold inline-flex items-center justify-center gap-1.5 cursor-pointer"
                >
                  <Plus size={14} />
                  <span>{submitting ? "Creating..." : "Create MCP Token"}</span>
                </button>
              </form>
            </section>
          </div>
        )}

        {tab === "audit" && (
          <section className="rounded-lg border border-[var(--color-border)] bg-[var(--color-bg-card)] overflow-hidden">
            <div className="px-4 py-3 bg-[var(--color-bg-muted)] border-b border-[var(--color-border)] flex items-center justify-between">
              <h2 className="text-xs font-bold text-[var(--color-text)]">
                MCP Tool Invocation Audit Log
              </h2>
              <span className="text-[11px] text-[var(--color-text-dim)]">
                Newest 100 calls · sensitive arguments auto-scrubbed
              </span>
            </div>

            {auditLogs.length === 0 ? (
              <div className="p-6 text-xs text-[var(--color-text-dim)]">
                No MCP tool invocations recorded yet.
              </div>
            ) : (
              <div className="overflow-x-auto">
                <table className="w-full text-left border-collapse text-xs">
                  <thead>
                    <tr className="border-b border-[var(--color-border)] bg-[var(--color-bg-muted)] text-[11px] text-[var(--color-text-dim)] uppercase">
                      <th className="py-2 px-3">Time</th>
                      <th className="py-2 px-3">User / Token</th>
                      <th className="py-2 px-3">Tool</th>
                      <th className="py-2 px-3">Status</th>
                      <th className="py-2 px-3">Duration</th>
                      <th className="py-2 px-3">Arguments</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-[var(--color-border-light)]">
                    {auditLogs.map((log) => (
                      <tr key={log.id} className="hover:bg-[var(--pm-card-hover)]">
                        <td className="py-2 px-3 pm-mono whitespace-nowrap text-[var(--color-text-dim)]">
                          {new Date(log.created_at).toLocaleString()}
                        </td>
                        <td className="py-2 px-3">
                          <div className="font-medium text-[var(--color-text)]">
                            {log.token_name}
                          </div>
                          <div className="text-[11px] text-[var(--color-text-dim)]">
                            {log.user_email}
                          </div>
                        </td>
                        <td className="py-2 px-3 pm-mono font-semibold text-[var(--color-text)]">
                          {log.tool_name}
                        </td>
                        <td className="py-2 px-3">
                          <span
                            className={`pm-lozenge ${
                              log.status === "ok"
                                ? "pm-lozenge-success"
                                : log.status === "denied" ||
                                    log.status === "rate_limited"
                                  ? "pm-lozenge-warning"
                                  : "pm-lozenge-danger"
                            }`}
                          >
                            {log.status}
                          </span>
                        </td>
                        <td className="py-2 px-3 pm-mono text-[var(--color-text-dim)]">
                          {log.duration_ms}ms
                        </td>
                        <td className="py-2 px-3 pm-mono text-[11px] text-[var(--color-text-muted)] max-w-[320px] truncate">
                          {JSON.stringify(log.arguments_json)}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </section>
        )}
      </div>
    </div>
  );
}

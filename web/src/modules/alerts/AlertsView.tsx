import { useCallback, useEffect, useState } from "react";
import { Bell, Lock, Plus, RefreshCw, Shield, Trash2, Webhook } from "lucide-react";
import {
  type AlertChannelType,
  type AlertChannelView,
  type AlertDeliveryView,
  type AlertRuleView,
  type AlertTriggerType,
  ApiError,
  type PiiScrubRuleType,
  type PiiScrubRuleView,
  type ProjectView,
  createAlertChannel,
  createAlertRule,
  createPiiRule,
  deleteAlertChannel,
  deleteAlertRule,
  deletePiiRule,
  fetchAlertChannels,
  fetchAlertDeliveries,
  fetchAlertRules,
  fetchPiiRules,
  toggleAlertRule,
  togglePiiRule,
} from "../../lib/api";

interface AlertsViewProps {
  workspaceName: string;
  projects: ProjectView[];
  canManage: boolean;
  onShowToast: (message: string) => void;
}

type AlertsTab = "rules" | "channels" | "pii";

export function AlertsView({
  workspaceName,
  projects,
  canManage,
  onShowToast,
}: AlertsViewProps) {
  const [tab, setTab] = useState<AlertsTab>("rules");
  const [channels, setChannels] = useState<AlertChannelView[]>([]);
  const [rules, setRules] = useState<AlertRuleView[]>([]);
  const [deliveries, setDeliveries] = useState<AlertDeliveryView[]>([]);
  const [piiRules, setPiiRules] = useState<PiiScrubRuleView[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Create Channel form state (Webhook prioritized first per D002)
  const [chanName, setChanName] = useState("");
  const [chanType, setChanType] = useState<AlertChannelType>("webhook");
  const [chanProject, setChanProject] = useState("");
  const [webhookUrl, setWebhookUrl] = useState("");
  const [webhookSecret, setWebhookSecret] = useState("");
  const [tgBotToken, setTgBotToken] = useState("");
  const [tgChatId, setTgChatId] = useState("");
  const [emailTo, setEmailTo] = useState("");
  const [emailPrefix, setEmailPrefix] = useState("[MaxiLoop]");

  // Create Alert Rule form state
  const [ruleName, setRuleName] = useState("");
  const [ruleChannelId, setRuleChannelId] = useState<string>("");
  const [ruleTrigger, setRuleTrigger] = useState<AlertTriggerType>("new_issue");
  const [ruleProject, setRuleProject] = useState("");
  const [ruleEnv, setRuleEnv] = useState("");
  const [spikeThreshold, setSpikeThreshold] = useState("10");
  const [spikeWindow, setSpikeWindow] = useState("60");
  const [cooldownMinutes, setCooldownMinutes] = useState("15");

  // Create PII Rule form state
  const [piiName, setPiiName] = useState("");
  const [piiType, setPiiType] = useState<PiiScrubRuleType>("field_name");
  const [piiPattern, setPiiPattern] = useState("");
  const [piiReplacement, setPiiReplacement] = useState("[Filtered]");
  const [piiProject, setPiiProject] = useState("");

  const [submitting, setSubmitting] = useState(false);

  const loadAll = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [c, r, d, p] = await Promise.all([
        fetchAlertChannels(),
        fetchAlertRules(),
        fetchAlertDeliveries(50),
        fetchPiiRules(),
      ]);
      setChannels(c);
      setRules(r);
      setDeliveries(d);
      setPiiRules(p);
      if (c.length > 0) {
        setRuleChannelId((prev) => prev || String(c[0].id));
      }
    } catch (err) {
      setError(err instanceof ApiError ? err.message : "Failed to load alerts & PII settings");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadAll();
  }, [loadAll]);

  async function handleCreateChannel(e: React.FormEvent) {
    e.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      let config: Record<string, unknown>;
      if (chanType === "webhook") {
        config = {
          url: webhookUrl.trim(),
          ...(webhookSecret.trim() ? { secret: webhookSecret.trim() } : {}),
        };
      } else if (chanType === "telegram") {
        config = { bot_token: tgBotToken.trim(), chat_id: tgChatId.trim() };
      } else {
        const recipients = emailTo
          .split(",")
          .map((s) => s.trim())
          .filter(Boolean);
        config = {
          to: recipients,
          ...(emailPrefix.trim() ? { subject_prefix: emailPrefix.trim() } : {}),
        };
      }

      await createAlertChannel({
        name: chanName.trim(),
        channel_type: chanType,
        config,
        project_slug: chanProject || undefined,
      });
      setChanName("");
      setWebhookUrl("");
      setWebhookSecret("");
      setTgBotToken("");
      setTgChatId("");
      setEmailTo("");
      onShowToast("Alert channel created");
      await loadAll();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : "Failed to create channel");
    } finally {
      setSubmitting(false);
    }
  }

  async function handleCreateRule(e: React.FormEvent) {
    e.preventDefault();
    if (!ruleChannelId) return;
    setSubmitting(true);
    setError(null);
    try {
      await createAlertRule({
        name: ruleName.trim(),
        channel_id: Number(ruleChannelId),
        trigger_type: ruleTrigger,
        project_slug: ruleProject || undefined,
        environment_filter: ruleEnv.trim() || undefined,
        spike_threshold: ruleTrigger === "spike" ? Number(spikeThreshold) : undefined,
        spike_window_minutes: ruleTrigger === "spike" ? Number(spikeWindow) : undefined,
        cooldown_minutes: Number(cooldownMinutes) || 15,
      });
      setRuleName("");
      setRuleEnv("");
      onShowToast("Alert rule created");
      await loadAll();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : "Failed to create alert rule");
    } finally {
      setSubmitting(false);
    }
  }

  async function handleCreatePiiRule(e: React.FormEvent) {
    e.preventDefault();
    setSubmitting(true);
    setError(null);
    try {
      await createPiiRule({
        name: piiName.trim(),
        rule_type: piiType,
        pattern: piiPattern.trim(),
        replacement: piiReplacement.trim() || "[Filtered]",
        project_slug: piiProject || undefined,
      });
      setPiiName("");
      setPiiPattern("");
      setPiiReplacement("[Filtered]");
      onShowToast("PII scrub rule saved & active in ingest worker");
      await loadAll();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : "Failed to create PII rule");
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="flex-1 min-w-0 flex overflow-hidden max-lg:flex-col max-lg:overflow-y-auto bg-[var(--color-bg)]">
      {/* Main Content Area */}
      <section className="flex-1 min-w-0 flex flex-col overflow-hidden max-lg:flex-none max-lg:overflow-visible">
        <div className="px-4 md:px-6 pt-4 pb-0 border-b border-[var(--color-border)]">
          <div className="flex flex-wrap items-center justify-between gap-3 mb-3">
            <div className="min-w-0">
              <div className="text-[11px] font-medium text-[var(--color-text-dim)]">
                {workspaceName} / Operations
              </div>
              <h1 className="text-xl md:text-2xl font-semibold tracking-tight text-[var(--color-text)] flex items-center gap-2">
                <Bell size={18} className="shrink-0 text-[var(--color-primary)]" />
                <span>Alerts &amp; PII Scrubbing</span>
              </h1>
            </div>
            <button
              type="button"
              onClick={() => void loadAll()}
              className="h-8 px-2.5 rounded border border-[var(--color-border-strong)] bg-[var(--color-bg)] hover:bg-[var(--color-bg-hover)] text-sm font-medium text-[var(--color-text)] inline-flex items-center gap-1.5 cursor-pointer"
            >
              <RefreshCw size={13} />
              <span>Refresh</span>
            </button>
          </div>

          <div className="flex items-center gap-5">
            <button
              type="button"
              onClick={() => setTab("rules")}
              className={`pm-tab cursor-pointer ${
                tab === "rules" ? "pm-tab-active" : ""
              }`}
            >
              Alert Rules ({rules.length})
            </button>
            <button
              type="button"
              onClick={() => setTab("channels")}
              className={`pm-tab cursor-pointer ${
                tab === "channels" ? "pm-tab-active" : ""
              }`}
            >
              Channels &amp; Delivery Log ({channels.length})
            </button>
            <button
              type="button"
              onClick={() => setTab("pii")}
              className={`pm-tab cursor-pointer ${
                tab === "pii" ? "pm-tab-active" : ""
              }`}
            >
              PII Scrubbing ({piiRules.length})
            </button>
          </div>
        </div>

        {error && (
          <div className="mx-6 mt-3 px-3 py-2 rounded bg-[#ffeceb] text-[#ae2e24] text-xs font-medium">
            {error}
          </div>
        )}

        <div className="flex-1 overflow-y-auto p-6 space-y-5">
          {loading ? (
            <div className="text-xs text-[var(--color-text-dim)]">Loading configuration...</div>
          ) : tab === "rules" ? (
            rules.length === 0 ? (
              <div className="pm-card-panel p-6 text-center space-y-1">
                <div className="text-sm font-semibold text-[var(--color-text)]">
                  No alert rules configured
                </div>
                <p className="text-xs text-[var(--color-text-dim)]">
                  Create a Webhook, Telegram, or Email channel first, then attach rules for new
                  issues, regressions, or event spikes.
                </p>
              </div>
            ) : (
              <div className="space-y-2.5">
                {rules.map((r) => (
                  <div
                    key={r.id}
                    className="pm-card-panel p-3.5 flex items-center justify-between gap-3"
                  >
                    <div className="min-w-0 space-y-1">
                      <div className="flex items-center gap-2 flex-wrap">
                        <span className="font-semibold text-xs text-[var(--color-text)]">
                          {r.name}
                        </span>
                        <span className="pm-lozenge pm-lozenge-info">{r.trigger_type}</span>
                        <span className="pm-lozenge">{r.channel_type}</span>
                        <span className="pm-lozenge">
                          {r.project_slug ? `project:${r.project_slug}` : `scope:${r.scope}`}
                        </span>
                      </div>
                      <div className="text-[11px] text-[var(--color-text-dim)]">
                        Channel: <strong>{r.channel_name}</strong>
                        {r.environment_filter ? ` · Env: ${r.environment_filter}` : ""}
                        {r.trigger_type === "spike"
                          ? ` · Threshold: >= ${r.spike_threshold} in ${r.spike_window_minutes}m`
                          : ""}
                        {` · Cooldown: ${r.cooldown_minutes}m`}
                      </div>
                    </div>

                    {canManage && (
                      <div className="flex items-center gap-2 shrink-0">
                        <button
                          type="button"
                          onClick={async () => {
                            await toggleAlertRule(r.id, !r.is_enabled);
                            onShowToast(r.is_enabled ? "Rule paused" : "Rule enabled");
                            await loadAll();
                          }}
                          className={`h-7 px-2.5 rounded text-[11px] font-semibold cursor-pointer ${
                            r.is_enabled
                              ? "bg-[#dcfff1] text-[#216e4e]"
                              : "bg-[var(--color-bg-hover)] text-[var(--color-text-dim)]"
                          }`}
                        >
                          {r.is_enabled ? "Enabled" : "Paused"}
                        </button>
                        <button
                          type="button"
                          onClick={async () => {
                            await deleteAlertRule(r.id);
                            onShowToast("Alert rule deleted");
                            await loadAll();
                          }}
                          className="h-7 w-7 rounded inline-flex items-center justify-center text-[var(--color-text-dim)] hover:text-[#ae2e24] hover:bg-[#ffeceb] cursor-pointer"
                          title="Delete rule"
                        >
                          <Trash2 size={14} />
                        </button>
                      </div>
                    )}
                  </div>
                ))}
              </div>
            )
          ) : tab === "channels" ? (
            <>
              <div className="space-y-2.5">
                <h2 className="text-xs font-bold uppercase tracking-wider text-[var(--color-text-dim)]">
                  Configured Channels (Secrets Masked)
                </h2>
                {channels.length === 0 ? (
                  <div className="pm-card-panel p-5 text-xs text-[var(--color-text-dim)]">
                    No alert channels yet. Add a Webhook, Telegram, or Email channel on the right.
                  </div>
                ) : (
                  channels.map((ch) => (
                    <div
                      key={ch.id}
                      className="pm-card-panel p-3.5 flex items-center justify-between gap-3"
                    >
                      <div className="min-w-0 space-y-1">
                        <div className="flex items-center gap-2">
                          <Webhook size={14} className="text-[var(--color-primary)]" />
                          <span className="font-semibold text-xs text-[var(--color-text)]">
                            {ch.name}
                          </span>
                          <span className="pm-lozenge pm-lozenge-info">{ch.channel_type}</span>
                          <span className="pm-lozenge">
                            {ch.project_slug ? `project:${ch.project_slug}` : `scope:${ch.scope}`}
                          </span>
                        </div>
                        <div className="text-[11px] pm-mono text-[var(--color-text-dim)] truncate">
                          {JSON.stringify(ch.config_summary)}
                        </div>
                      </div>
                      {canManage && (
                        <button
                          type="button"
                          onClick={async () => {
                            await deleteAlertChannel(ch.id);
                            onShowToast("Channel deleted");
                            await loadAll();
                          }}
                          className="h-7 w-7 rounded inline-flex items-center justify-center text-[var(--color-text-dim)] hover:text-[#ae2e24] hover:bg-[#ffeceb] cursor-pointer"
                          title="Delete channel"
                        >
                          <Trash2 size={14} />
                        </button>
                      )}
                    </div>
                  ))
                )}
              </div>

              <div className="space-y-2">
                <h2 className="text-xs font-bold uppercase tracking-wider text-[var(--color-text-dim)]">
                  Recent Deliveries ({deliveries.length})
                </h2>
                {deliveries.length === 0 ? (
                  <div className="text-xs text-[var(--color-text-dim)]">
                    No alerts have fired yet.
                  </div>
                ) : (
                  <div className="border border-[var(--color-border)] rounded-lg overflow-x-auto">
                    <table className="w-full max-md:min-w-[640px] text-left border-collapse text-[13px]">
                      <thead>
                        <tr className="bg-[var(--color-bg-muted)] border-b border-[var(--color-border)] text-[11px] uppercase text-[var(--color-text-dim)]">
                          <th className="py-2 px-3">Status</th>
                          <th className="py-2 px-3">Trigger</th>
                          <th className="py-2 px-3">Channel</th>
                          <th className="py-2 px-3">Issue</th>
                          <th className="py-2 px-3 text-right">Time</th>
                        </tr>
                      </thead>
                      <tbody className="divide-y divide-[var(--color-border)]">
                        {deliveries.map((d) => (
                          <tr key={d.id}>
                            <td className="py-2 px-3">
                              <span
                                className={`pm-lozenge ${
                                  d.status === "sent"
                                    ? "pm-lozenge-success"
                                    : "pm-lozenge-danger"
                                }`}
                              >
                                {d.status}
                                {d.http_status ? ` (${d.http_status})` : ""}
                              </span>
                            </td>
                            <td className="py-2 px-3 pm-mono text-[11px]">{d.trigger_type}</td>
                            <td className="py-2 px-3">
                              {d.channel_name} ({d.channel_type})
                            </td>
                            <td className="py-2 px-3 truncate max-w-[260px]">
                              <div className="font-medium text-[var(--color-text)] truncate">
                                {d.project_slug} #{d.issue_id}: {d.issue_title}
                              </div>
                              {d.error_message && (
                                <div className="text-[11px] text-[#ae2e24] truncate">
                                  {d.error_message}
                                </div>
                              )}
                            </td>
                            <td className="py-2 px-3 text-right text-[var(--color-text-dim)]">
                              {new Date(d.created_at).toLocaleTimeString()}
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}
              </div>
            </>
          ) : (
            <>
              <div className="rounded-lg border border-[#b3d4ff] bg-[#e9f2ff] p-3.5 flex items-start gap-2.5 text-xs text-[#0055cc]">
                <Lock size={15} className="shrink-0 mt-0.5" />
                <div>
                  <div className="font-bold">Mandatory Baseline Scrubbing (Always Active)</div>
                  <div className="mt-0.5 text-[#172b4d]">
                    Headers (<code className="pm-mono">Authorization</code>,{" "}
                    <code className="pm-mono">Cookie</code>,{" "}
                    <code className="pm-mono">X-Api-Key</code>) and sensitive fields (
                    <code className="pm-mono">password</code>,{" "}
                    <code className="pm-mono">token</code>,{" "}
                    <code className="pm-mono">secret</code>,{" "}
                    <code className="pm-mono">api_key</code>) are always scrubbed to{" "}
                    <code className="pm-mono">[Filtered]</code> before persistence. Custom rules
                    below are additive and inherited from tenant → workspace → project.
                  </div>
                </div>
              </div>

              {piiRules.length === 0 ? (
                <div className="pm-card-panel p-5 text-xs text-[var(--color-text-dim)]">
                  No custom PII scrub rules added yet. Add field, header, or literal substring
                  rules on the right.
                </div>
              ) : (
                <div className="space-y-2.5">
                  {piiRules.map((rule) => (
                    <div
                      key={rule.id}
                      className="pm-card-panel p-3.5 flex items-center justify-between gap-3"
                    >
                      <div className="min-w-0 space-y-1">
                        <div className="flex items-center gap-2 flex-wrap">
                          <Shield size={14} className="text-[var(--color-primary)]" />
                          <span className="font-semibold text-xs text-[var(--color-text)]">
                            {rule.name}
                          </span>
                          <span className="pm-lozenge pm-lozenge-info">{rule.rule_type}</span>
                          <span className="pm-lozenge">
                            {rule.project_slug
                              ? `project:${rule.project_slug}`
                              : rule.workspace_slug
                                ? `ws:${rule.workspace_slug}`
                                : "tenant-wide"}
                          </span>
                        </div>
                        <div className="text-[11px] pm-mono text-[var(--color-text-dim)]">
                          match: <strong>{rule.pattern}</strong> → {rule.replacement}
                        </div>
                      </div>
                      {canManage && (
                        <div className="flex items-center gap-2 shrink-0">
                          <button
                            type="button"
                            onClick={async () => {
                              await togglePiiRule(rule.id, !rule.is_enabled);
                              onShowToast(
                                rule.is_enabled ? "PII rule paused" : "PII rule enabled",
                              );
                              await loadAll();
                            }}
                            className={`h-7 px-2.5 rounded text-[11px] font-semibold cursor-pointer ${
                              rule.is_enabled
                                ? "bg-[#dcfff1] text-[#216e4e]"
                                : "bg-[var(--color-bg-hover)] text-[var(--color-text-dim)]"
                            }`}
                          >
                            {rule.is_enabled ? "Enabled" : "Paused"}
                          </button>
                          <button
                            type="button"
                            onClick={async () => {
                              await deletePiiRule(rule.id);
                              onShowToast("PII rule deleted");
                              await loadAll();
                            }}
                            className="h-7 w-7 rounded inline-flex items-center justify-center text-[var(--color-text-dim)] hover:text-[#ae2e24] hover:bg-[#ffeceb] cursor-pointer"
                            title="Delete PII rule"
                          >
                            <Trash2 size={14} />
                          </button>
                        </div>
                      )}
                    </div>
                  ))}
                </div>
              )}
            </>
          )}
        </div>
      </section>

      {/* Right Action Panel */}
      {canManage && (
        <aside className="w-full lg:w-[340px] shrink-0 max-lg:border-t lg:border-l border-[var(--color-border)] bg-[var(--color-bg-muted)] p-5 overflow-y-auto">
          {tab === "rules" && (
            <form onSubmit={(e) => void handleCreateRule(e)} className="space-y-3">
              <h2 className="text-sm font-bold text-[var(--color-text)] flex items-center gap-1.5">
                <Plus size={15} />
                <span>Create alert rule</span>
              </h2>

              <div>
                <label className="block text-xs font-semibold mb-1">Rule name</label>
                <input
                  type="text"
                  value={ruleName}
                  onChange={(e) => setRuleName(e.target.value)}
                  placeholder="Production regressions"
                  required
                  className="pm-input h-9 text-sm"
                />
              </div>

              <div>
                <label className="block text-xs font-semibold mb-1">Trigger</label>
                <select
                  value={ruleTrigger}
                  onChange={(e) => setRuleTrigger(e.target.value as AlertTriggerType)}
                  className="pm-input h-9 text-sm"
                >
                  <option value="new_issue">New issue</option>
                  <option value="regression">Regression (resolved issue reopened)</option>
                  <option value="spike">Event spike (threshold in window)</option>
                </select>
              </div>

              <div>
                <label className="block text-xs font-semibold mb-1">Delivery channel</label>
                <select
                  value={ruleChannelId}
                  onChange={(e) => setRuleChannelId(e.target.value)}
                  required
                  className="pm-input h-9 text-sm"
                >
                  {channels.length === 0 ? (
                    <option value="">Create a channel first</option>
                  ) : (
                    channels.map((c) => (
                      <option key={c.id} value={c.id}>
                        {c.name} ({c.channel_type})
                      </option>
                    ))
                  )}
                </select>
              </div>

              <div>
                <label className="block text-xs font-semibold mb-1">Scope (optional project)</label>
                <select
                  value={ruleProject}
                  onChange={(e) => setRuleProject(e.target.value)}
                  className="pm-input h-9 text-sm"
                >
                  <option value="">All projects (tenant-wide)</option>
                  {projects.map((p) => (
                    <option key={p.id} value={p.slug}>
                      {p.slug}
                    </option>
                  ))}
                </select>
              </div>

              <div>
                <label className="block text-xs font-semibold mb-1">
                  Environment filter (optional)
                </label>
                <input
                  type="text"
                  value={ruleEnv}
                  onChange={(e) => setRuleEnv(e.target.value)}
                  placeholder="e.g. production"
                  className="pm-input h-9 text-sm"
                />
              </div>

              {ruleTrigger === "spike" && (
                <div className="grid grid-cols-2 gap-2">
                  <div>
                    <label className="block text-xs font-semibold mb-1">Events &gt;=</label>
                    <input
                      type="number"
                      min={2}
                      value={spikeThreshold}
                      onChange={(e) => setSpikeThreshold(e.target.value)}
                      required
                      className="pm-input h-9 text-sm"
                    />
                  </div>
                  <div>
                    <label className="block text-xs font-semibold mb-1">Window (min)</label>
                    <input
                      type="number"
                      min={1}
                      max={1440}
                      value={spikeWindow}
                      onChange={(e) => setSpikeWindow(e.target.value)}
                      required
                      className="pm-input h-9 text-sm"
                    />
                  </div>
                </div>
              )}

              <div>
                <label className="block text-xs font-semibold mb-1">Cooldown (minutes)</label>
                <input
                  type="number"
                  min={1}
                  max={1440}
                  value={cooldownMinutes}
                  onChange={(e) => setCooldownMinutes(e.target.value)}
                  className="pm-input h-9 text-sm"
                />
              </div>

              <button
                type="submit"
                disabled={submitting || channels.length === 0}
                className="w-full h-9 rounded bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-50 text-white text-sm font-semibold cursor-pointer"
              >
                {submitting ? "Creating..." : "Create rule"}
              </button>
            </form>
          )}

          {tab === "channels" && (
            <form onSubmit={(e) => void handleCreateChannel(e)} className="space-y-3">
              <h2 className="text-sm font-bold text-[var(--color-text)] flex items-center gap-1.5">
                <Plus size={15} />
                <span>Add alert channel</span>
              </h2>

              <div>
                <label className="block text-xs font-semibold mb-1">Channel name</label>
                <input
                  type="text"
                  value={chanName}
                  onChange={(e) => setChanName(e.target.value)}
                  placeholder="Ops Webhook"
                  required
                  className="pm-input h-9 text-sm"
                />
              </div>

              <div>
                <label className="block text-xs font-semibold mb-1">Channel type</label>
                <select
                  value={chanType}
                  onChange={(e) => setChanType(e.target.value as AlertChannelType)}
                  className="pm-input h-9 text-sm"
                >
                  <option value="webhook">Webhook (HTTPS JSON + HMAC-SHA256)</option>
                  <option value="telegram">Telegram Bot</option>
                  <option value="email">Email (Server SMTP)</option>
                </select>
              </div>

              <div>
                <label className="block text-xs font-semibold mb-1">Scope (optional project)</label>
                <select
                  value={chanProject}
                  onChange={(e) => setChanProject(e.target.value)}
                  className="pm-input h-9 text-sm"
                >
                  <option value="">Tenant-wide</option>
                  {projects.map((p) => (
                    <option key={p.id} value={p.slug}>
                      {p.slug}
                    </option>
                  ))}
                </select>
              </div>

              {chanType === "webhook" && (
                <>
                  <div>
                    <label className="block text-xs font-semibold mb-1">Webhook URL</label>
                    <input
                      type="url"
                      value={webhookUrl}
                      onChange={(e) => setWebhookUrl(e.target.value)}
                      placeholder="https://hooks.example.com/maxi-loop"
                      required
                      className="pm-input h-9 text-sm pm-mono"
                    />
                  </div>
                  <div>
                    <label className="block text-xs font-semibold mb-1">
                      Signing secret (optional)
                    </label>
                    <input
                      type="password"
                      value={webhookSecret}
                      onChange={(e) => setWebhookSecret(e.target.value)}
                      placeholder="HMAC-SHA256 secret"
                      className="pm-input h-9 text-sm"
                    />
                  </div>
                </>
              )}

              {chanType === "telegram" && (
                <>
                  <div>
                    <label className="block text-xs font-semibold mb-1">Bot Token</label>
                    <input
                      type="password"
                      value={tgBotToken}
                      onChange={(e) => setTgBotToken(e.target.value)}
                      placeholder="123456:ABC..."
                      required
                      className="pm-input h-9 text-sm pm-mono"
                    />
                  </div>
                  <div>
                    <label className="block text-xs font-semibold mb-1">Chat ID</label>
                    <input
                      type="text"
                      value={tgChatId}
                      onChange={(e) => setTgChatId(e.target.value)}
                      placeholder="-1001234567890"
                      required
                      className="pm-input h-9 text-sm pm-mono"
                    />
                  </div>
                </>
              )}

              {chanType === "email" && (
                <>
                  <div>
                    <label className="block text-xs font-semibold mb-1">
                      Recipients (comma-separated)
                    </label>
                    <input
                      type="text"
                      value={emailTo}
                      onChange={(e) => setEmailTo(e.target.value)}
                      placeholder="oncall@example.com, dev@example.com"
                      required
                      className="pm-input h-9 text-sm"
                    />
                  </div>
                  <div>
                    <label className="block text-xs font-semibold mb-1">Subject prefix</label>
                    <input
                      type="text"
                      value={emailPrefix}
                      onChange={(e) => setEmailPrefix(e.target.value)}
                      className="pm-input h-9 text-sm"
                    />
                  </div>
                </>
              )}

              <button
                type="submit"
                disabled={submitting}
                className="w-full h-9 rounded bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-50 text-white text-sm font-semibold cursor-pointer"
              >
                {submitting ? "Saving..." : "Add channel"}
              </button>
            </form>
          )}

          {tab === "pii" && (
            <form onSubmit={(e) => void handleCreatePiiRule(e)} className="space-y-3">
              <h2 className="text-sm font-bold text-[var(--color-text)] flex items-center gap-1.5">
                <Plus size={15} />
                <span>Add custom PII rule</span>
              </h2>

              <div>
                <label className="block text-xs font-semibold mb-1">Rule name</label>
                <input
                  type="text"
                  value={piiName}
                  onChange={(e) => setPiiName(e.target.value)}
                  placeholder="Mask National ID"
                  required
                  className="pm-input h-9 text-sm"
                />
              </div>

              <div>
                <label className="block text-xs font-semibold mb-1">Match type</label>
                <select
                  value={piiType}
                  onChange={(e) => setPiiType(e.target.value as PiiScrubRuleType)}
                  className="pm-input h-9 text-sm"
                >
                  <option value="field_name">Field / query param name</option>
                  <option value="header_name">HTTP header name</option>
                  <option value="value_substring">Literal value substring</option>
                </select>
              </div>

              <div>
                <label className="block text-xs font-semibold mb-1">Pattern</label>
                <input
                  type="text"
                  value={piiPattern}
                  onChange={(e) => setPiiPattern(e.target.value)}
                  placeholder="e.g. national_id or X-Employee-Id"
                  required
                  className="pm-input h-9 text-sm pm-mono"
                />
              </div>

              <div>
                <label className="block text-xs font-semibold mb-1">Replacement</label>
                <input
                  type="text"
                  value={piiReplacement}
                  onChange={(e) => setPiiReplacement(e.target.value)}
                  className="pm-input h-9 text-sm pm-mono"
                />
              </div>

              <div>
                <label className="block text-xs font-semibold mb-1">Scope</label>
                <select
                  value={piiProject}
                  onChange={(e) => setPiiProject(e.target.value)}
                  className="pm-input h-9 text-sm"
                >
                  <option value="">Tenant-wide (inherited by all projects)</option>
                  {projects.map((p) => (
                    <option key={p.id} value={p.slug}>
                      Project: {p.slug}
                    </option>
                  ))}
                </select>
              </div>

              <button
                type="submit"
                disabled={submitting}
                className="w-full h-9 rounded bg-[var(--color-primary)] hover:bg-[var(--color-primary-hover)] disabled:opacity-50 text-white text-sm font-semibold cursor-pointer"
              >
                {submitting ? "Saving..." : "Add PII rule"}
              </button>
            </form>
          )}
        </aside>
      )}
    </div>
  );
}

export interface HealthPayload {
  status: string;
  service: string;
  version: string;
  database: string;
}

export type Role =
  | "tenant_owner"
  | "tenant_admin"
  | "workspace_admin"
  | "member"
  | "viewer";

export interface AuthenticatedUser {
  id: number;
  email: string;
  name: string;
}

export interface TenantSummary {
  id: number;
  slug: string;
  name: string;
  retention_days: number;
  default_rate_limit_per_minute: number;
  aggregate_rate_limit_per_minute: number;
  is_default: boolean;
}

export interface WorkspaceSummary {
  id: number;
  slug: string;
  name: string;
  retention_days_override: number | null;
  default_rate_limit_per_minute_override: number | null;
  is_default: boolean;
}

export interface AuthSessionView {
  user: AuthenticatedUser;
  tenant: TenantSummary;
  default_workspace: WorkspaceSummary;
  role: Role;
  tenant_role: Role | null;
  workspace_roles: Record<string, Role>;
  csrf_token: string;
  single_tenant_mode: boolean;
}

export interface LoginInput {
  email: string;
  password: string;
  tenant_slug?: string;
}

export interface WorkspaceView {
  id: number;
  tenant_id: number;
  tenant_slug: string;
  slug: string;
  name: string;
  retention_days_override: number | null;
  effective_retention_days: number;
  default_rate_limit_per_minute_override: number | null;
  effective_rate_limit_per_minute: number;
  is_default: boolean;
  created_at: string;
  updated_at: string;
}

export interface CreateWorkspaceInput {
  slug: string;
  name: string;
  retention_days_override?: number | null;
  default_rate_limit_per_minute_override?: number | null;
}

export interface ProjectView {
  id: number;
  tenant_id: number;
  tenant_slug: string;
  workspace_id: number;
  workspace_slug: string;
  slug: string;
  name: string;
  platform: string;
  dsn_public_key: string;
  dsn: string;
  ingest_endpoint: string;
  retention_days_override: number | null;
  effective_retention_days: number;
  rate_limit_per_minute_override: number | null;
  rate_limit_per_minute: number;
  effective_rate_limit_per_minute: number;
  created_at: string;
  updated_at: string;
}

export interface CreateProjectInput {
  slug: string;
  name: string;
  platform?: string;
  retention_days_override?: number | null;
  rate_limit_per_minute?: number | null;
  workspace_slug?: string;
}

export type IssueStatus = "unresolved" | "resolved" | "ignored";

export interface IssueListFilter {
  workspace_slug?: string;
  project_id?: number;
  project_slug?: string;
  environment?: string;
  release?: string;
  status?: string;
  is_regressed?: boolean;
  query?: string;
  sort?: "last_seen" | "first_seen" | "events";
  page?: number;
  per_page?: number;
}

export interface IssueSummaryView {
  id: number;
  tenant_id: number;
  workspace_id: number;
  workspace_slug: string;
  project_id: number;
  project_slug: string;
  fingerprint: string;
  title: string;
  exception_type: string;
  exception_value: string;
  culprit: string;
  level: string;
  status: IssueStatus;
  is_regressed: boolean;
  event_count: number;
  stored_full_events: number;
  first_seen: string;
  last_seen: string;
  last_environment: string;
  first_release: string | null;
  last_release: string | null;
  sample_event_id: string | null;
  created_at: string;
  updated_at: string;
}

export interface PaginatedIssuesResponse {
  items: IssueSummaryView[];
  total: number;
  page: number;
  per_page: number;
  total_pages: number;
}

export interface StackFrameItem {
  filename?: string;
  module?: string;
  function?: string;
  lineno?: number;
  colno?: number;
  in_app?: boolean;
  context_line?: string;
  pre_context?: string[];
  post_context?: string[];
}

export interface BreadcrumbView {
  timestamp?: string;
  type: string;
  category: string;
  level: string;
  message?: string;
  data?: Record<string, unknown> | unknown;
}

export interface IssueFingerprintView {
  fingerprint: string;
  original_title: string;
  original_culprit: string;
  is_primary: boolean;
  merged_at: string;
  stored_event_count: number;
}

export interface IssueEventView {
  event_id: string;
  issue_id: number;
  project_id: number;
  fingerprint?: string | null;
  received_at: string;
  environment: string;
  release: string | null;
  level: string;
  platform: string;
  exception_type: string;
  exception_value: string;
  stack_frames: StackFrameItem[] | unknown;
  tags: Record<string, string> | Array<[string, string]> | unknown;
  breadcrumbs?: BreadcrumbView[];
  payload: Record<string, unknown> | unknown;
  is_sample: boolean;
}

export interface CounterBucketPoint {
  bucket: string;
  event_count: number;
}

export interface IssueStatsResponse {
  issue_id: number;
  project_id: number;
  interval: string;
  series: CounterBucketPoint[];
}

export interface IssueDetailView {
  issue: IssueSummaryView;
  stack_frames: StackFrameItem[] | unknown;
  tags: Record<string, string> | Array<[string, string]> | unknown;
  breadcrumbs?: BreadcrumbView[];
  merged_fingerprints?: IssueFingerprintView[];
  sample_event: IssueEventView | null;
  hourly_series: CounterBucketPoint[];
  daily_series: CounterBucketPoint[];
}

export interface MergeIssuesInput {
  target_issue_id: number;
  source_issue_ids: number[];
}

export interface MergeIssuesResponse {
  target_issue: IssueSummaryView;
  merged_issue_ids: number[];
  merged_fingerprints: IssueFingerprintView[];
}

export interface SplitIssueInput {
  fingerprints?: string[];
  event_ids?: string[];
}

export interface SplitIssueResponse {
  source_issue: IssueSummaryView;
  created_issues: IssueSummaryView[];
}

export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

let activeCsrfToken: string | null = null;

export function setCsrfToken(token: string | null): void {
  activeCsrfToken = token;
}

export function getCsrfToken(): string | null {
  return activeCsrfToken;
}

async function requestJson<T>(path: string, init: RequestInit = {}): Promise<T> {
  const method = (init.method ?? "GET").toUpperCase();
  const headers = new Headers(init.headers);

  if (init.body && !headers.has("Content-Type")) {
    headers.set("Content-Type", "application/json");
  }
  if (method !== "GET" && method !== "HEAD" && method !== "OPTIONS" && activeCsrfToken) {
    headers.set("X-CSRF-Token", activeCsrfToken);
  }

  const res = await fetch(path, {
    ...init,
    method,
    headers,
    credentials: "same-origin",
  });

  if (!res.ok) {
    let message = `Request failed (${res.status})`;
    try {
      const errBody = (await res.json()) as { error?: string };
      if (errBody && typeof errBody.error === "string") {
        message = errBody.error;
      }
    } catch {
      // Ignore non-JSON error body
    }
    throw new ApiError(res.status, message);
  }

  return (await res.json()) as T;
}

export async function fetchHealth(): Promise<HealthPayload> {
  return requestJson<HealthPayload>("/api/v1/health");
}

export async function fetchCurrentSession(): Promise<AuthSessionView | null> {
  try {
    const session = await requestJson<AuthSessionView>("/api/v1/auth/me");
    setCsrfToken(session.csrf_token);
    return session;
  } catch (err) {
    if (err instanceof ApiError && err.status === 401) {
      setCsrfToken(null);
      return null;
    }
    throw err;
  }
}

export async function loginWithPassword(input: LoginInput): Promise<AuthSessionView> {
  const payload: Record<string, string> = {
    email: input.email.trim(),
    password: input.password,
  };
  if (input.tenant_slug && input.tenant_slug.trim()) {
    payload.tenant_slug = input.tenant_slug.trim();
  }
  const session = await requestJson<AuthSessionView>("/api/v1/auth/login", {
    method: "POST",
    body: JSON.stringify(payload),
  });
  setCsrfToken(session.csrf_token);
  return session;
}

export async function logoutSession(): Promise<void> {
  await requestJson<{ status: string }>("/api/v1/auth/logout", {
    method: "POST",
  });
  setCsrfToken(null);
}

export async function fetchWorkspaces(): Promise<WorkspaceView[]> {
  return requestJson<WorkspaceView[]>("/api/v1/workspaces");
}

export async function createWorkspace(input: CreateWorkspaceInput): Promise<WorkspaceView> {
  return requestJson<WorkspaceView>("/api/v1/workspaces", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function fetchProjects(workspaceSlug?: string): Promise<ProjectView[]> {
  const qs = workspaceSlug ? `?workspace_slug=${encodeURIComponent(workspaceSlug)}` : "";
  return requestJson<ProjectView[]>(`/api/v1/projects${qs}`);
}

export async function createProject(input: CreateProjectInput): Promise<ProjectView> {
  return requestJson<ProjectView>("/api/v1/projects", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function rotateProjectDsn(
  projectSlug: string,
  workspaceSlug?: string,
): Promise<ProjectView> {
  const qs = workspaceSlug ? `?workspace_slug=${encodeURIComponent(workspaceSlug)}` : "";
  return requestJson<ProjectView>(
    `/api/v1/projects/${encodeURIComponent(projectSlug)}/rotate-dsn${qs}`,
    {
      method: "POST",
    },
  );
}

export async function fetchIssues(filter: IssueListFilter = {}): Promise<PaginatedIssuesResponse> {
  const params = new URLSearchParams();
  if (filter.workspace_slug) params.set("workspace_slug", filter.workspace_slug);
  if (filter.project_id !== undefined) params.set("project_id", String(filter.project_id));
  if (filter.project_slug) params.set("project_slug", filter.project_slug);
  if (filter.environment) params.set("environment", filter.environment);
  if (filter.release) params.set("release", filter.release);
  if (filter.status) params.set("status", filter.status);
  if (filter.is_regressed !== undefined) params.set("is_regressed", String(filter.is_regressed));
  if (filter.query && filter.query.trim()) params.set("query", filter.query.trim());
  if (filter.sort) params.set("sort", filter.sort);
  if (filter.page !== undefined) params.set("page", String(filter.page));
  if (filter.per_page !== undefined) params.set("per_page", String(filter.per_page));

  const qs = params.toString();
  return requestJson<PaginatedIssuesResponse>(`/api/v1/issues${qs ? `?${qs}` : ""}`);
}

export async function fetchIssueDetail(issueId: number): Promise<IssueDetailView> {
  return requestJson<IssueDetailView>(`/api/v1/issues/${issueId}`);
}

export async function fetchIssueStats(
  issueId: number,
  interval: "1h" | "1d",
  hours?: number,
): Promise<IssueStatsResponse> {
  const params = new URLSearchParams({ interval });
  if (hours !== undefined) params.set("hours", String(hours));
  return requestJson<IssueStatsResponse>(`/api/v1/issues/${issueId}/stats?${params.toString()}`);
}

export async function fetchIssueEvents(
  issueId: number,
  limit = 20,
): Promise<IssueEventView[]> {
  return requestJson<IssueEventView[]>(`/api/v1/issues/${issueId}/events?limit=${limit}`);
}

export async function updateIssueStatus(
  issueId: number,
  status: IssueStatus,
): Promise<IssueSummaryView> {
  return requestJson<IssueSummaryView>(`/api/v1/issues/${issueId}/status`, {
    method: "PATCH",
    body: JSON.stringify({ status }),
  });
}

export async function mergeIssues(
  input: MergeIssuesInput,
): Promise<MergeIssuesResponse> {
  return requestJson<MergeIssuesResponse>("/api/v1/issues/merge", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function splitIssue(
  issueId: number,
  input: SplitIssueInput,
): Promise<SplitIssueResponse> {
  return requestJson<SplitIssueResponse>(`/api/v1/issues/${issueId}/split`, {
    method: "POST",
    body: JSON.stringify(input),
  });
}

// --- Phase 2: Configurable PII Scrubbing ---

export type PiiScrubRuleType = "field_name" | "header_name" | "value_substring";

export interface PiiScrubRuleView {
  id: number;
  tenant_id: number;
  workspace_id: number | null;
  workspace_slug: string | null;
  project_id: number | null;
  project_slug: string | null;
  scope: "tenant" | "workspace" | "project";
  name: string;
  rule_type: PiiScrubRuleType;
  pattern: string;
  replacement: string;
  is_enabled: boolean;
  created_at: string;
  updated_at: string;
}

export interface CreatePiiScrubRuleInput {
  name: string;
  rule_type: PiiScrubRuleType;
  pattern: string;
  replacement?: string;
  workspace_slug?: string;
  project_slug?: string;
}

export async function fetchPiiRules(): Promise<PiiScrubRuleView[]> {
  return requestJson<PiiScrubRuleView[]>("/api/v1/pii-rules");
}

export async function createPiiRule(
  input: CreatePiiScrubRuleInput,
): Promise<PiiScrubRuleView> {
  return requestJson<PiiScrubRuleView>("/api/v1/pii-rules", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function togglePiiRule(
  ruleId: number,
  isEnabled: boolean,
): Promise<PiiScrubRuleView> {
  return requestJson<PiiScrubRuleView>(`/api/v1/pii-rules/${ruleId}`, {
    method: "PATCH",
    body: JSON.stringify({ is_enabled: isEnabled }),
  });
}

export async function deletePiiRule(ruleId: number): Promise<void> {
  const headers = new Headers();
  if (activeCsrfToken) headers.set("X-CSRF-Token", activeCsrfToken);
  const res = await fetch(`/api/v1/pii-rules/${ruleId}`, {
    method: "DELETE",
    headers,
    credentials: "same-origin",
  });
  if (!res.ok) {
    throw new ApiError(res.status, `Failed to delete PII rule (${res.status})`);
  }
}

// --- Phase 2: Release Tracking ---

export interface ReleaseView {
  id: number;
  tenant_id: number;
  workspace_id: number;
  workspace_slug: string;
  project_id: number;
  project_slug: string;
  version: string;
  commit_sha: string | null;
  environment: string | null;
  notes: string;
  first_seen: string;
  last_seen: string;
  deployed_at: string | null;
  event_count: number;
  new_issues_count: number;
  linked_issues_count: number;
  created_at: string;
  updated_at: string;
}

export interface UpsertReleaseInput {
  project_slug: string;
  workspace_slug?: string;
  version: string;
  commit_sha?: string;
  environment?: string;
  notes?: string;
}

export async function fetchReleases(projectSlug?: string): Promise<ReleaseView[]> {
  const qs = projectSlug ? `?project_slug=${encodeURIComponent(projectSlug)}` : "";
  return requestJson<ReleaseView[]>(`/api/v1/releases${qs}`);
}

export async function upsertRelease(input: UpsertReleaseInput): Promise<ReleaseView> {
  return requestJson<ReleaseView>("/api/v1/releases", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

// --- Phase 2: Alerts (Webhook, Telegram, Email) ---

export type AlertChannelType = "webhook" | "telegram" | "email";
export type AlertTriggerType = "new_issue" | "regression" | "spike";

export interface AlertChannelView {
  id: number;
  tenant_id: number;
  workspace_id: number | null;
  workspace_slug: string | null;
  project_id: number | null;
  project_slug: string | null;
  scope: "tenant" | "workspace" | "project";
  name: string;
  channel_type: AlertChannelType;
  config_summary: Record<string, unknown>;
  is_enabled: boolean;
  created_at: string;
  updated_at: string;
}

export interface CreateAlertChannelInput {
  name: string;
  channel_type: AlertChannelType;
  config: Record<string, unknown>;
  workspace_slug?: string;
  project_slug?: string;
}

export interface AlertRuleView {
  id: number;
  tenant_id: number;
  workspace_id: number | null;
  workspace_slug: string | null;
  project_id: number | null;
  project_slug: string | null;
  scope: "tenant" | "workspace" | "project";
  channel_id: number;
  channel_name: string;
  channel_type: AlertChannelType;
  name: string;
  trigger_type: AlertTriggerType;
  environment_filter: string | null;
  spike_threshold: number | null;
  spike_window_minutes: number | null;
  cooldown_minutes: number;
  is_enabled: boolean;
  created_at: string;
  updated_at: string;
}

export interface CreateAlertRuleInput {
  name: string;
  channel_id: number;
  trigger_type: AlertTriggerType;
  project_slug?: string;
  environment_filter?: string;
  spike_threshold?: number;
  spike_window_minutes?: number;
  cooldown_minutes?: number;
}

export interface AlertDeliveryView {
  id: number;
  tenant_id: number;
  project_id: number;
  project_slug: string;
  rule_id: number;
  rule_name: string;
  channel_id: number;
  channel_name: string;
  issue_id: number;
  issue_title: string;
  trigger_type: AlertTriggerType;
  channel_type: AlertChannelType;
  status: "sent" | "failed" | "skipped";
  http_status: number | null;
  error_message: string | null;
  created_at: string;
}

export async function fetchAlertChannels(): Promise<AlertChannelView[]> {
  return requestJson<AlertChannelView[]>("/api/v1/alerts/channels");
}

export async function createAlertChannel(
  input: CreateAlertChannelInput,
): Promise<AlertChannelView> {
  return requestJson<AlertChannelView>("/api/v1/alerts/channels", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function deleteAlertChannel(channelId: number): Promise<void> {
  const headers = new Headers();
  if (activeCsrfToken) headers.set("X-CSRF-Token", activeCsrfToken);
  const res = await fetch(`/api/v1/alerts/channels/${channelId}`, {
    method: "DELETE",
    headers,
    credentials: "same-origin",
  });
  if (!res.ok) {
    throw new ApiError(res.status, `Failed to delete channel (${res.status})`);
  }
}

export async function fetchAlertRules(): Promise<AlertRuleView[]> {
  return requestJson<AlertRuleView[]>("/api/v1/alerts/rules");
}

export async function createAlertRule(
  input: CreateAlertRuleInput,
): Promise<AlertRuleView> {
  return requestJson<AlertRuleView>("/api/v1/alerts/rules", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function toggleAlertRule(
  ruleId: number,
  isEnabled: boolean,
): Promise<AlertRuleView> {
  return requestJson<AlertRuleView>(`/api/v1/alerts/rules/${ruleId}`, {
    method: "PATCH",
    body: JSON.stringify({ is_enabled: isEnabled }),
  });
}

export async function deleteAlertRule(ruleId: number): Promise<void> {
  const headers = new Headers();
  if (activeCsrfToken) headers.set("X-CSRF-Token", activeCsrfToken);
  const res = await fetch(`/api/v1/alerts/rules/${ruleId}`, {
    method: "DELETE",
    headers,
    credentials: "same-origin",
  });
  if (!res.ok) {
    throw new ApiError(res.status, `Failed to delete rule (${res.status})`);
  }
}

export async function fetchAlertDeliveries(limit = 50): Promise<AlertDeliveryView[]> {
  return requestJson<AlertDeliveryView[]>(`/api/v1/alerts/deliveries?limit=${limit}`);
}

// --- Phase 2b: Issue Notes & Embedded MCP Server ---

export interface IssueNoteView {
  id: number;
  issue_id: number;
  project_id: number;
  author_user_id: number | null;
  author_email: string;
  source: "ui" | "mcp";
  body: string;
  commit_ref: string | null;
  status_change: IssueStatus | null;
  created_at: string;
}

export async function fetchIssueNotes(issueId: number): Promise<IssueNoteView[]> {
  return requestJson<IssueNoteView[]>(`/api/v1/issues/${issueId}/notes`);
}

export async function createIssueNote(
  issueId: number,
  body: string,
  commitRef?: string,
): Promise<IssueNoteView> {
  return requestJson<IssueNoteView>(`/api/v1/issues/${issueId}/notes`, {
    method: "POST",
    body: JSON.stringify({
      body,
      commit_ref: commitRef?.trim() || undefined,
    }),
  });
}

export type McpAccessScope = "tenant" | "workspace" | "project";

export interface McpTokenView {
  id: number;
  tenant_id: number;
  user_id: number;
  user_email: string;
  name: string;
  token_prefix: string;
  access_scope: McpAccessScope;
  workspace_id: number | null;
  workspace_slug: string | null;
  project_id: number | null;
  project_slug: string | null;
  can_read: boolean;
  can_triage: boolean;
  rate_limit_per_minute: number;
  expires_at: string | null;
  revoked_at: string | null;
  last_used_at: string | null;
  created_at: string;
}

export interface CreatedMcpTokenResponse {
  token: McpTokenView;
  raw_token: string;
}

export interface CreateMcpTokenInput {
  name: string;
  access_scope?: McpAccessScope;
  workspace_slug?: string;
  project_slug?: string;
  can_triage?: boolean;
  rate_limit_per_minute?: number;
  expires_in_days?: number;
}

export interface McpAuditLogView {
  id: number;
  tenant_id: number;
  token_id: number | null;
  user_id: number;
  user_email: string;
  token_name: string;
  tool_name: string;
  arguments_json: Record<string, unknown>;
  status: "ok" | "error" | "denied" | "rate_limited";
  error_code: string | null;
  duration_ms: number;
  created_at: string;
}

export async function fetchMcpTokens(): Promise<McpTokenView[]> {
  return requestJson<McpTokenView[]>("/api/v1/mcp/tokens");
}

export async function createMcpToken(
  input: CreateMcpTokenInput,
): Promise<CreatedMcpTokenResponse> {
  return requestJson<CreatedMcpTokenResponse>("/api/v1/mcp/tokens", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function revokeMcpToken(tokenId: number): Promise<void> {
  const headers = new Headers();
  if (activeCsrfToken) headers.set("X-CSRF-Token", activeCsrfToken);
  const res = await fetch(`/api/v1/mcp/tokens/${tokenId}`, {
    method: "DELETE",
    headers,
    credentials: "same-origin",
  });
  if (!res.ok) {
    throw new ApiError(res.status, `Failed to revoke MCP token (${res.status})`);
  }
}

export async function fetchMcpAuditLogs(limit = 100): Promise<McpAuditLogView[]> {
  return requestJson<McpAuditLogView[]>(`/api/v1/mcp/audit-logs?limit=${limit}`);
}

// --- Phase 3: Performance Monitoring (Transactions, Histograms, Sampled Traces & Waterfall) ---

export type TransactionSort =
  | "p95"
  | "p99"
  | "p50"
  | "throughput"
  | "error_rate"
  | "avg"
  | "name";

export interface TransactionListFilter {
  workspace_slug?: string;
  project_slug?: string;
  environment?: string;
  release?: string;
  op?: string;
  query?: string;
  hours?: number;
  sort?: TransactionSort;
  page?: number;
  per_page?: number;
}

export interface TransactionSummaryView {
  project_id: number;
  project_slug: string;
  workspace_id: number;
  workspace_slug: string;
  transaction_name: string;
  op: string;
  total_count: number;
  error_count: number;
  error_rate: number;
  throughput_rpm: number;
  p50_ms: number;
  p95_ms: number;
  p99_ms: number;
  avg_ms: number;
  min_ms: number;
  max_ms: number;
  last_seen: string;
}

export interface PaginatedTransactionsResponse {
  items: TransactionSummaryView[];
  total: number;
  page: number;
  per_page: number;
  total_pages: number;
  window_minutes: number;
  since: string;
  until: string;
}

export interface TransactionTimeBucket {
  bucket: string;
  total_count: number;
  error_count: number;
  p50_ms: number;
  p95_ms: number;
  p99_ms: number;
  avg_ms: number;
}

export interface TraceSummaryView {
  trace_id: string;
  event_id: string;
  project_id: number;
  project_slug: string;
  workspace_slug: string;
  transaction_name: string;
  op: string;
  status: string;
  is_error: boolean;
  sample_reason: "slow" | "error" | "random" | string;
  environment: string;
  release: string | null;
  platform: string;
  duration_ms: number;
  span_count: number;
  start_timestamp: string;
  received_at: string;
}

export interface TransactionDetailView {
  summary: TransactionSummaryView;
  interval: string;
  series: TransactionTimeBucket[];
  recent_traces: TraceSummaryView[];
}

export interface WaterfallSpanNode {
  span_id: string;
  parent_span_id: string | null;
  project_id: number;
  project_slug: string;
  platform: string;
  environment: string;
  release: string | null;
  op: string;
  description: string;
  status: string;
  is_error: boolean;
  is_transaction_root: boolean;
  depth: number;
  start_timestamp_ms: number;
  end_timestamp_ms: number;
  offset_ms: number;
  duration_ms: number;
  self_duration_ms: number;
  pct_of_trace: number;
  is_slow: boolean;
  tags: Record<string, unknown>;
  data: Record<string, unknown>;
}

export interface SlowSpanHighlight {
  span_id: string;
  project_slug: string;
  op: string;
  description: string;
  duration_ms: number;
  self_duration_ms: number;
  pct_of_trace: number;
  is_error: boolean;
}

export interface TraceWaterfallView {
  trace_id: string;
  root_transaction: string;
  status: string;
  is_error: boolean;
  projects: string[];
  services_count: number;
  sample_reasons: string[];
  start_timestamp: string;
  end_timestamp: string;
  total_duration_ms: number;
  span_count: number;
  slowest_spans: SlowSpanHighlight[];
  spans: WaterfallSpanNode[];
}

export interface TraceListFilter {
  project_slug?: string;
  transaction_name?: string;
  environment?: string;
  release?: string;
  sample_reason?: string;
  is_error?: boolean;
  hours?: number;
  limit?: number;
}

export async function fetchTransactions(
  filter: TransactionListFilter = {},
): Promise<PaginatedTransactionsResponse> {
  const params = new URLSearchParams();
  if (filter.workspace_slug) params.set("workspace_slug", filter.workspace_slug);
  if (filter.project_slug) params.set("project_slug", filter.project_slug);
  if (filter.environment) params.set("environment", filter.environment);
  if (filter.release) params.set("release", filter.release);
  if (filter.op) params.set("op", filter.op);
  if (filter.query && filter.query.trim()) params.set("query", filter.query.trim());
  if (filter.hours !== undefined) params.set("hours", String(filter.hours));
  if (filter.sort) params.set("sort", filter.sort);
  if (filter.page !== undefined) params.set("page", String(filter.page));
  if (filter.per_page !== undefined) params.set("per_page", String(filter.per_page));
  const qs = params.toString();
  return requestJson<PaginatedTransactionsResponse>(
    `/api/v1/performance/transactions${qs ? `?${qs}` : ""}`,
  );
}

export async function fetchTransactionDetail(params: {
  project_slug: string;
  transaction_name: string;
  op?: string;
  environment?: string;
  release?: string;
  hours?: number;
}): Promise<TransactionDetailView> {
  const qs = new URLSearchParams({
    project_slug: params.project_slug,
    transaction_name: params.transaction_name,
  });
  if (params.op) qs.set("op", params.op);
  if (params.environment) qs.set("environment", params.environment);
  if (params.release) qs.set("release", params.release);
  if (params.hours !== undefined) qs.set("hours", String(params.hours));
  return requestJson<TransactionDetailView>(
    `/api/v1/performance/transactions/detail?${qs.toString()}`,
  );
}

export async function fetchTraces(
  filter: TraceListFilter = {},
): Promise<TraceSummaryView[]> {
  const params = new URLSearchParams();
  if (filter.project_slug) params.set("project_slug", filter.project_slug);
  if (filter.transaction_name) params.set("transaction_name", filter.transaction_name);
  if (filter.environment) params.set("environment", filter.environment);
  if (filter.release) params.set("release", filter.release);
  if (filter.sample_reason) params.set("sample_reason", filter.sample_reason);
  if (filter.is_error !== undefined) params.set("is_error", String(filter.is_error));
  if (filter.hours !== undefined) params.set("hours", String(filter.hours));
  if (filter.limit !== undefined) params.set("limit", String(filter.limit));
  const qs = params.toString();
  return requestJson<TraceSummaryView[]>(
    `/api/v1/performance/traces${qs ? `?${qs}` : ""}`,
  );
}

export async function fetchTraceWaterfall(traceId: string): Promise<TraceWaterfallView> {
  return requestJson<TraceWaterfallView>(
    `/api/v1/performance/traces/${encodeURIComponent(traceId)}`,
  );
}

// --- P2 Performance: Slow Queries/Operations & Compare Releases ---

export type SlowOperationSort =
  | "p95"
  | "total_ms"
  | "avg"
  | "p50"
  | "p99"
  | "count"
  | "error_rate";

export interface SlowOperationListFilter {
  workspace_slug?: string;
  project_slug?: string;
  environment?: string;
  release?: string;
  op?: string;
  db_only?: boolean;
  query?: string;
  hours?: number;
  sort?: SlowOperationSort;
  page?: number;
  per_page?: number;
}

export interface SlowOperationSummaryView {
  project_id: number;
  project_slug: string;
  workspace_id: number;
  workspace_slug: string;
  op: string;
  description_hash: string;
  normalized_description: string;
  total_count: number;
  error_count: number;
  error_rate: number;
  total_ms: number;
  avg_ms: number;
  p50_ms: number;
  p95_ms: number;
  p99_ms: number;
  min_ms: number;
  max_ms: number;
  sample_trace_id: string | null;
  last_seen: string;
}

export interface PaginatedSlowOperationsResponse {
  items: SlowOperationSummaryView[];
  total: number;
  page: number;
  per_page: number;
  total_pages: number;
  window_minutes: number;
  since: string;
  until: string;
}

export async function fetchSlowOperations(
  filter: SlowOperationListFilter = {},
): Promise<PaginatedSlowOperationsResponse> {
  const params = new URLSearchParams();
  if (filter.workspace_slug) params.set("workspace_slug", filter.workspace_slug);
  if (filter.project_slug) params.set("project_slug", filter.project_slug);
  if (filter.environment) params.set("environment", filter.environment);
  if (filter.release) params.set("release", filter.release);
  if (filter.op) params.set("op", filter.op);
  if (filter.db_only !== undefined) params.set("db_only", String(filter.db_only));
  if (filter.query && filter.query.trim()) params.set("query", filter.query.trim());
  if (filter.hours !== undefined) params.set("hours", String(filter.hours));
  if (filter.sort) params.set("sort", filter.sort);
  if (filter.page !== undefined) params.set("page", String(filter.page));
  if (filter.per_page !== undefined) params.set("per_page", String(filter.per_page));
  const qs = params.toString();
  return requestJson<PaginatedSlowOperationsResponse>(
    `/api/v1/performance/slow-operations${qs ? `?${qs}` : ""}`,
  );
}

export interface ReleasePerfMetrics {
  total_count: number;
  error_count: number;
  error_rate: number;
  active_minutes: number;
  throughput_rpm: number;
  p50_ms: number;
  p95_ms: number;
  p99_ms: number;
  avg_ms: number;
  min_ms: number;
  max_ms: number;
}

export interface ReleaseIssueMetrics {
  linked_issues_count: number;
  new_issues_count: number;
  regressed_issues_count: number;
  unresolved_issues_count: number;
  issue_events_count: number;
}

export interface ReleaseComparisonSide {
  version: string;
  commit_sha: string | null;
  first_seen: string | null;
  last_seen: string | null;
  deployed_at: string | null;
  performance: ReleasePerfMetrics;
  issues: ReleaseIssueMetrics;
}

export interface ReleaseComparisonDelta {
  p50_delta_ms: number;
  p50_delta_pct: number | null;
  p95_delta_ms: number;
  p95_delta_pct: number | null;
  p99_delta_ms: number;
  p99_delta_pct: number | null;
  avg_delta_ms: number;
  avg_delta_pct: number | null;
  error_rate_delta: number;
  throughput_rpm_delta: number;
  throughput_rpm_delta_pct: number | null;
  linked_issues_delta: number;
  new_issues_delta: number;
  regressed_issues_delta: number;
}

export interface ReleaseNewIssueItem {
  id: number;
  title: string;
  exception_type: string;
  level: string;
  status: string;
  is_regressed: boolean;
  event_count: number;
  first_seen: string;
  last_seen: string;
}

export interface TransactionComparisonRow {
  transaction_name: string;
  op: string;
  status: "regressed" | "improved" | "new" | "removed" | "stable" | string;
  base: ReleasePerfMetrics | null;
  target: ReleasePerfMetrics | null;
  p50_delta_ms: number | null;
  p50_delta_pct: number | null;
  p95_delta_ms: number | null;
  p95_delta_pct: number | null;
  p99_delta_ms: number | null;
  p99_delta_pct: number | null;
  error_rate_delta: number | null;
  throughput_rpm_delta: number | null;
}

export interface ReleaseComparisonView {
  project_id: number;
  project_slug: string;
  workspace_id: number;
  workspace_slug: string;
  environment: string | null;
  since: string;
  until: string;
  verdict: "regressed" | "improved" | "stable" | "insufficient_data" | string;
  base: ReleaseComparisonSide;
  target: ReleaseComparisonSide;
  delta: ReleaseComparisonDelta;
  new_issues_in_target: ReleaseNewIssueItem[];
  transactions: TransactionComparisonRow[];
  total_transactions_compared: number;
}

export interface CompareReleasesParams {
  project_slug: string;
  base_release: string;
  target_release: string;
  environment?: string;
  hours?: number;
  limit?: number;
}

export async function fetchCompareReleases(
  params: CompareReleasesParams,
): Promise<ReleaseComparisonView> {
  const qs = new URLSearchParams({
    project_slug: params.project_slug,
    base_release: params.base_release,
    target_release: params.target_release,
  });
  if (params.environment && params.environment.trim()) {
    qs.set("environment", params.environment.trim());
  }
  if (params.hours !== undefined) qs.set("hours", String(params.hours));
  if (params.limit !== undefined) qs.set("limit", String(params.limit));
  return requestJson<ReleaseComparisonView>(
    `/api/v1/performance/compare-releases?${qs.toString()}`,
  );
}

// --- Phase 4: React / JS Source Maps ---

export interface SourceMapView {
  id: number;
  tenant_id: number;
  workspace_id: number;
  workspace_slug: string;
  project_id: number;
  project_slug: string;
  release_version: string;
  filename: string;
  debug_id: string | null;
  sha256: string;
  raw_size_bytes: number;
  compressed_size_bytes: number;
  uploaded_by_user_id: number | null;
  created_at: string;
  updated_at: string;
}

export interface UploadSourceMapInput {
  project_slug: string;
  release_version: string;
  filename: string;
  debug_id?: string;
  content: string;
}

export async function fetchSourceMaps(
  projectSlug?: string,
  releaseVersion?: string,
): Promise<SourceMapView[]> {
  const params = new URLSearchParams();
  if (projectSlug) params.set("project_slug", projectSlug);
  if (releaseVersion) params.set("release_version", releaseVersion);
  const qs = params.toString();
  return requestJson<SourceMapView[]>(`/api/v1/sourcemaps${qs ? `?${qs}` : ""}`);
}

export async function uploadSourceMap(
  input: UploadSourceMapInput,
): Promise<SourceMapView> {
  return requestJson<SourceMapView>("/api/v1/sourcemaps", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function deleteSourceMap(sourcemapId: number): Promise<void> {
  const headers = new Headers();
  if (activeCsrfToken) headers.set("X-CSRF-Token", activeCsrfToken);
  const res = await fetch(`/api/v1/sourcemaps/${sourcemapId}`, {
    method: "DELETE",
    headers,
    credentials: "same-origin",
  });
  if (!res.ok) {
    throw new ApiError(res.status, `Failed to delete source map (${res.status})`);
  }
}

// --- Phase 5: Multi-User RBAC, Inherited Settings & Per-Project API Tokens ---

export interface TenantMemberView {
  membership_id: number;
  tenant_id: number;
  user_id: number;
  email: string;
  name: string;
  is_active: boolean;
  scope: "tenant" | "workspace";
  workspace_id: number | null;
  workspace_slug: string | null;
  workspace_name: string | null;
  role: Role;
  created_at: string;
}

export interface CreateMemberInput {
  email: string;
  name?: string;
  password?: string;
  role: Role;
  workspace_slug?: string | null;
}

export async function fetchMembers(): Promise<TenantMemberView[]> {
  return requestJson<TenantMemberView[]>("/api/v1/members");
}

export async function createMember(
  input: CreateMemberInput,
): Promise<TenantMemberView> {
  return requestJson<TenantMemberView>("/api/v1/members", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function updateMemberRole(
  membershipId: number,
  role: Role,
): Promise<TenantMemberView> {
  return requestJson<TenantMemberView>(`/api/v1/members/${membershipId}`, {
    method: "PATCH",
    body: JSON.stringify({ role }),
  });
}

export async function deleteMember(membershipId: number): Promise<void> {
  const headers = new Headers();
  if (activeCsrfToken) headers.set("X-CSRF-Token", activeCsrfToken);
  const res = await fetch(`/api/v1/members/${membershipId}`, {
    method: "DELETE",
    headers,
    credentials: "same-origin",
  });
  if (!res.ok) {
    const errText = await res.text().catch(() => "");
    throw new ApiError(res.status, errText || `Failed to remove member (${res.status})`);
  }
}

export interface TenantOverview {
  tenant: TenantSummary;
  default_workspace: WorkspaceSummary;
  single_tenant_mode: boolean;
}

export interface UpdateTenantSettingsInput {
  name?: string;
  retention_days?: number;
  default_rate_limit_per_minute?: number;
  aggregate_rate_limit_per_minute?: number;
}

export interface UpdateWorkspaceSettingsInput {
  name?: string;
  retention_days_override?: number | null;
  default_rate_limit_per_minute_override?: number | null;
}

export interface UpdateProjectSettingsInput {
  name?: string;
  platform?: string;
  retention_days_override?: number | null;
  rate_limit_per_minute_override?: number | null;
}

export async function updateTenantSettings(
  input: UpdateTenantSettingsInput,
): Promise<TenantOverview> {
  return requestJson<TenantOverview>("/api/v1/tenants/current/settings", {
    method: "PATCH",
    body: JSON.stringify(input),
  });
}

export async function updateWorkspaceSettings(
  workspaceSlug: string,
  input: UpdateWorkspaceSettingsInput,
): Promise<WorkspaceView> {
  return requestJson<WorkspaceView>(
    `/api/v1/workspaces/${encodeURIComponent(workspaceSlug)}/settings`,
    {
      method: "PATCH",
      body: JSON.stringify(input),
    },
  );
}

export async function updateProjectSettings(
  projectSlug: string,
  input: UpdateProjectSettingsInput,
  workspaceSlug?: string,
): Promise<ProjectView> {
  const qs = workspaceSlug
    ? `?workspace_slug=${encodeURIComponent(workspaceSlug)}`
    : "";
  return requestJson<ProjectView>(
    `/api/v1/projects/${encodeURIComponent(projectSlug)}/settings${qs}`,
    {
      method: "PATCH",
      body: JSON.stringify(input),
    },
  );
}

export interface ApiTokenView {
  id: number;
  tenant_id: number;
  workspace_id: number;
  workspace_slug: string;
  project_id: number;
  project_slug: string;
  created_by_user_id: number;
  created_by_email: string;
  name: string;
  token_prefix: string;
  can_read: boolean;
  can_write: boolean;
  expires_at: string | null;
  revoked_at: string | null;
  last_used_at: string | null;
  created_at: string;
}

export interface CreatedApiTokenResponse {
  token: ApiTokenView;
  raw_token: string;
}

export interface CreateApiTokenInput {
  project_slug: string;
  workspace_slug?: string;
  name: string;
  can_write?: boolean;
  expires_in_days?: number | null;
}

export async function fetchApiTokens(
  projectSlug?: string,
): Promise<ApiTokenView[]> {
  const qs = projectSlug ? `?project_slug=${encodeURIComponent(projectSlug)}` : "";
  return requestJson<ApiTokenView[]>(`/api/v1/api-tokens${qs}`);
}

export async function createApiToken(
  input: CreateApiTokenInput,
): Promise<CreatedApiTokenResponse> {
  return requestJson<CreatedApiTokenResponse>("/api/v1/api-tokens", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function revokeApiToken(tokenId: number): Promise<void> {
  const headers = new Headers();
  if (activeCsrfToken) headers.set("X-CSRF-Token", activeCsrfToken);
  const res = await fetch(`/api/v1/api-tokens/${tokenId}`, {
    method: "DELETE",
    headers,
    credentials: "same-origin",
  });
  if (!res.ok) {
    throw new ApiError(res.status, `Failed to revoke API token (${res.status})`);
  }
}

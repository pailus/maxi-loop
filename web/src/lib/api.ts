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
  is_default: boolean;
}

export interface WorkspaceSummary {
  id: number;
  slug: string;
  name: string;
  retention_days_override: number | null;
  is_default: boolean;
}

export interface AuthSessionView {
  user: AuthenticatedUser;
  tenant: TenantSummary;
  default_workspace: WorkspaceSummary;
  role: Role;
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
  is_default: boolean;
  created_at: string;
  updated_at: string;
}

export interface CreateWorkspaceInput {
  slug: string;
  name: string;
  retention_days_override?: number | null;
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
  rate_limit_per_minute: number;
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

export interface IssueEventView {
  event_id: string;
  issue_id: number;
  project_id: number;
  received_at: string;
  environment: string;
  release: string | null;
  level: string;
  platform: string;
  exception_type: string;
  exception_value: string;
  stack_frames: StackFrameItem[] | unknown;
  tags: Record<string, string> | Array<[string, string]> | unknown;
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
  sample_event: IssueEventView | null;
  hourly_series: CounterBucketPoint[];
  daily_series: CounterBucketPoint[];
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

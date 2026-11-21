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

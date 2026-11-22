use chrono::{DateTime, Utc};
use maxi_loop_auth::mcp::{McpAccessScope, McpAuthContext};
use maxi_loop_issues::model::{IssueListFilter, UpdateIssueStatusInput};
use maxi_loop_issues::notes::{CreateIssueNoteInput, NoteActor};
use maxi_loop_issues::service::{IssueScope, IssuesService};
use maxi_loop_org::service::OrgService;
use maxi_loop_performance::model::{
    CompareReleasesFilter, PerformanceScope, SlowOperationListFilter, TransactionListFilter,
};
use maxi_loop_performance::service::PerformanceService;
use maxi_loop_platform::error::AppError;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::sanitize::{
    UNTRUSTED_SECURITY_NOTICE, compare_releases_ui_url, format_stack_frames, issue_ui_url,
    performance_ui_url, project_ui_url, scrub_and_bound_json, slow_queries_ui_url, trace_ui_url,
    wrap_untrusted_text,
};

pub struct ToolServices<'a> {
    pub org: &'a OrgService,
    pub issues: &'a IssuesService,
    pub performance: &'a PerformanceService,
    pub public_base_url: &'a str,
}

pub struct ToolOutput {
    pub structured: Value,
    pub text: String,
}

impl ToolOutput {
    fn from_json(structured: Value) -> Self {
        let text = serde_json::to_string_pretty(&structured).unwrap_or_else(|_| "{}".to_owned());
        Self { structured, text }
    }
}

/// Return MCP Tool definitions filtered by the token's granted action scopes (least privilege).
pub fn list_tool_definitions(can_read: bool, can_triage: bool) -> Vec<Value> {
    let mut tools = Vec::new();

    if can_read {
        tools.push(json!({
            "name": "list_projects",
            "description": "List projects accessible to this MCP token (scoped to tenant, workspace, or project). Never exposes DSN credentials.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true
            }
        }));

        tools.push(json!({
            "name": "list_issues",
            "description": "List grouped error issues with filters (project, environment, release, status, regression, time range, search query, sort order) and pagination. Returns token-efficient summaries with UI links.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_slug": { "type": "string", "description": "Optional project slug filter." },
                    "environment": { "type": "string", "description": "Optional environment filter (e.g. production, staging)." },
                    "release": { "type": "string", "description": "Optional release version filter." },
                    "status": { "type": "string", "enum": ["unresolved", "resolved", "ignored"] },
                    "is_regressed": { "type": "boolean", "description": "Filter by regression status." },
                    "since": { "type": "string", "format": "date-time", "description": "ISO-8601 UTC lower bound on last_seen." },
                    "until": { "type": "string", "format": "date-time", "description": "ISO-8601 UTC upper bound on last_seen." },
                    "query": { "type": "string", "description": "Substring search on issue title or culprit." },
                    "sort": { "type": "string", "enum": ["last_seen", "first_seen", "events", "regressed"], "default": "last_seen" },
                    "page": { "type": "integer", "minimum": 1, "default": 1 },
                    "per_page": { "type": "integer", "minimum": 1, "maximum": 50, "default": 20 }
                },
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true
            }
        }));

        tools.push(json!({
            "name": "get_issue",
            "description": "Get detailed issue context for root-cause analysis: stack trace with surrounding source code (defaults to in-app frames only), release-to-commit SHA mapping, scrubbed tags, triage notes, compact sample event, and UI link. External event content is wrapped as untrusted data.",
            "inputSchema": {
                "type": "object",
                "required": ["issue_id"],
                "properties": {
                    "issue_id": { "type": "integer", "minimum": 1 },
                    "in_app_only": { "type": "boolean", "default": true, "description": "Return only in-app stack frames when present (defaults to true)." },
                    "include_sample_event": { "type": "boolean", "default": true },
                    "hours": { "type": "integer", "minimum": 1, "maximum": 720, "default": 24 }
                },
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true
            }
        }));

        tools.push(json!({
            "name": "get_event",
            "description": "Get PII-scrubbed details of a single event (request, breadcrumbs, user, tags, stack frames, extra) for an issue. Defaults to the issue's sample event when event_id is omitted. Event content is wrapped as untrusted data and truncated for token efficiency.",
            "inputSchema": {
                "type": "object",
                "required": ["issue_id"],
                "properties": {
                    "issue_id": { "type": "integer", "minimum": 1 },
                    "event_id": { "type": "string", "description": "Optional specific Sentry event_id; omits to return the sample event." }
                },
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true
            }
        }));

        tools.push(json!({
            "name": "get_issue_stats",
            "description": "Get hourly (1h) or daily (1d) occurrence time series from aggregated counter tables for an issue.",
            "inputSchema": {
                "type": "object",
                "required": ["issue_id"],
                "properties": {
                    "issue_id": { "type": "integer", "minimum": 1 },
                    "interval": { "type": "string", "enum": ["1h", "1d"], "default": "1h" },
                    "hours": { "type": "integer", "minimum": 1, "maximum": 2160 }
                },
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true
            }
        }));

        tools.push(json!({
            "name": "list_transactions",
            "description": "List aggregated performance transactions with p50/p95/p99 latency (ms), throughput (rpm), and error rate from 1-minute duration histograms. Filter by project, environment, release, op, query, and time window.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_slug": { "type": "string", "description": "Optional project slug filter." },
                    "environment": { "type": "string", "description": "Optional environment filter (e.g. production)." },
                    "release": { "type": "string", "description": "Optional release version filter." },
                    "op": { "type": "string", "description": "Optional span operation filter (e.g. http.server, pageload)." },
                    "query": { "type": "string", "description": "Substring search on transaction name." },
                    "since": { "type": "string", "format": "date-time", "description": "ISO-8601 UTC window start." },
                    "until": { "type": "string", "format": "date-time", "description": "ISO-8601 UTC window end." },
                    "hours": { "type": "integer", "minimum": 1, "maximum": 2160, "default": 24 },
                    "sort": { "type": "string", "enum": ["p95", "p99", "p50", "throughput", "error_rate", "avg", "name"], "default": "p95" },
                    "page": { "type": "integer", "minimum": 1, "default": 1 },
                    "per_page": { "type": "integer", "minimum": 1, "maximum": 50, "default": 20 }
                },
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true
            }
        }));

        tools.push(json!({
            "name": "get_trace",
            "description": "Get a compact distributed trace waterfall by trace_id across services/projects in your authorized scope, with slowest spans explicitly flagged (`is_slow: true` + `slowest_spans`), PII scrubbed, and untrusted span content wrapped.",
            "inputSchema": {
                "type": "object",
                "required": ["trace_id"],
                "properties": {
                    "trace_id": { "type": "string", "minLength": 1, "maxLength": 64, "description": "Distributed trace ID (hex string)." },
                    "max_spans": { "type": "integer", "minimum": 1, "maximum": 200, "default": 60 },
                    "slow_only": { "type": "boolean", "default": false, "description": "When true, return only slow/error spans and transaction roots." }
                },
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true
            }
        }));

        tools.push(json!({
            "name": "list_slow_queries",
            "description": "List aggregated slowest normalized DB queries or backend operations (p50/p95/p99/avg/total_ms/count/error_rate) with sample_trace_id for jumping directly to `get_trace`. Defaults to DB spans (`db_only: true`); set `db_only: false` or pass `op` for HTTP/cache/other spans.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_slug": { "type": "string", "description": "Optional project slug filter." },
                    "environment": { "type": "string", "description": "Optional environment filter (e.g. production)." },
                    "release": { "type": "string", "description": "Optional release version filter." },
                    "op": { "type": "string", "description": "Optional span op filter (exact or prefix ending with '.' or '*'; e.g. db.sql.query, http.client, cache.)." },
                    "db_only": { "type": "boolean", "default": true, "description": "When true (default) and `op` is omitted, restricts results to database spans (`db`, `db.*`)." },
                    "query": { "type": "string", "description": "Substring search on normalized query/operation description." },
                    "since": { "type": "string", "format": "date-time", "description": "ISO-8601 UTC window start." },
                    "until": { "type": "string", "format": "date-time", "description": "ISO-8601 UTC window end." },
                    "hours": { "type": "integer", "minimum": 1, "maximum": 2160, "default": 24 },
                    "sort": { "type": "string", "enum": ["p95", "total_ms", "avg", "p50", "p99", "count", "error_rate"], "default": "p95" },
                    "page": { "type": "integer", "minimum": 1, "default": 1 },
                    "per_page": { "type": "integer", "minimum": 1, "maximum": 50, "default": 20 }
                },
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true
            }
        }));

        tools.push(json!({
            "name": "compare_releases",
            "description": "Compare two releases (`base_release` vs `target_release`) for a project across p50/p95/p99 latency, throughput (rpm), error rate, per-transaction regressions/improvements, and issue counts (including newly introduced issues).",
            "inputSchema": {
                "type": "object",
                "required": ["base_release", "target_release"],
                "properties": {
                    "project_slug": { "type": "string", "description": "Project slug (optional when token is scoped to a single project)." },
                    "base_release": { "type": "string", "minLength": 1, "maxLength": 128, "description": "Baseline release version." },
                    "target_release": { "type": "string", "minLength": 1, "maxLength": 128, "description": "Target/new release version to compare against baseline." },
                    "environment": { "type": "string", "description": "Optional environment filter (e.g. production)." },
                    "hours": { "type": "integer", "minimum": 1, "maximum": 2160, "default": 720 },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "default": 20 }
                },
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true
            }
        }));
    }

    if can_triage {
        tools.push(json!({
            "name": "update_issue_status",
            "description": "Update an issue's triage status (resolved, ignored, or unresolved) with an optional analysis note and commit reference (e.g. fix commit SHA). Requires 'triage' scope.",
            "inputSchema": {
                "type": "object",
                "required": ["issue_id", "status"],
                "properties": {
                    "issue_id": { "type": "integer", "minimum": 1 },
                    "status": { "type": "string", "enum": ["unresolved", "resolved", "ignored"] },
                    "note": { "type": "string", "maxLength": 4000, "description": "Optional explanation or root-cause summary recorded on the issue." },
                    "commit_ref": { "type": "string", "maxLength": 128, "description": "Optional fix commit SHA or PR reference." }
                },
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": true
            }
        }));

        tools.push(json!({
            "name": "add_issue_note",
            "description": "Add an AI analysis or triage note (with optional commit reference) to an issue without changing its status. Requires 'triage' scope.",
            "inputSchema": {
                "type": "object",
                "required": ["issue_id", "body"],
                "properties": {
                    "issue_id": { "type": "integer", "minimum": 1 },
                    "body": { "type": "string", "minLength": 1, "maxLength": 4000 },
                    "commit_ref": { "type": "string", "maxLength": 128 }
                },
                "additionalProperties": false
            },
            "annotations": {
                "readOnlyHint": false,
                "destructiveHint": false,
                "idempotentHint": false
            }
        }));
    }

    tools
}

pub fn is_known_tool(name: &str) -> bool {
    matches!(
        name,
        "list_projects"
            | "list_issues"
            | "get_issue"
            | "get_event"
            | "get_issue_stats"
            | "list_transactions"
            | "get_trace"
            | "list_slow_queries"
            | "compare_releases"
            | "update_issue_status"
            | "add_issue_note"
    )
}

pub async fn execute_tool(
    services: &ToolServices<'_>,
    auth: &McpAuthContext,
    tool_name: &str,
    arguments: Value,
) -> Result<ToolOutput, AppError> {
    match tool_name {
        "list_projects" => {
            require_read(auth)?;
            tool_list_projects(services, auth).await
        }
        "list_issues" => {
            require_read(auth)?;
            let args: ListIssuesArgs = parse_args(arguments)?;
            tool_list_issues(services, auth, args).await
        }
        "get_issue" => {
            require_read(auth)?;
            let args: GetIssueArgs = parse_args(arguments)?;
            tool_get_issue(services, auth, args).await
        }
        "get_event" => {
            require_read(auth)?;
            let args: GetEventArgs = parse_args(arguments)?;
            tool_get_event(services, auth, args).await
        }
        "get_issue_stats" => {
            require_read(auth)?;
            let args: GetIssueStatsArgs = parse_args(arguments)?;
            tool_get_issue_stats(services, auth, args).await
        }
        "list_transactions" => {
            require_read(auth)?;
            let args: ListTransactionsArgs = parse_args(arguments)?;
            tool_list_transactions(services, auth, args).await
        }
        "get_trace" => {
            require_read(auth)?;
            let args: GetTraceArgs = parse_args(arguments)?;
            tool_get_trace(services, auth, args).await
        }
        "list_slow_queries" => {
            require_read(auth)?;
            let args: ListSlowQueriesArgs = parse_args(arguments)?;
            tool_list_slow_queries(services, auth, args).await
        }
        "compare_releases" => {
            require_read(auth)?;
            let args: CompareReleasesArgs = parse_args(arguments)?;
            tool_compare_releases(services, auth, args).await
        }
        "update_issue_status" => {
            require_triage(auth)?;
            let args: UpdateIssueStatusArgs = parse_args(arguments)?;
            tool_update_issue_status(services, auth, args).await
        }
        "add_issue_note" => {
            require_triage(auth)?;
            let args: AddIssueNoteArgs = parse_args(arguments)?;
            tool_add_issue_note(services, auth, args).await
        }
        _ => Err(AppError::Validation(format!("unknown tool: {tool_name}"))),
    }
}

fn require_read(auth: &McpAuthContext) -> Result<(), AppError> {
    if !auth.can_read {
        return Err(AppError::Forbidden("token lacks 'read' scope"));
    }
    Ok(())
}

fn require_triage(auth: &McpAuthContext) -> Result<(), AppError> {
    if !auth.can_triage {
        return Err(AppError::Forbidden("token lacks 'triage' scope"));
    }
    Ok(())
}

fn parse_args<T: for<'de> Deserialize<'de>>(arguments: Value) -> Result<T, AppError> {
    let normalized = if arguments.is_null() {
        json!({})
    } else {
        arguments
    };
    serde_json::from_value(normalized)
        .map_err(|err| AppError::Validation(format!("invalid tool arguments: {err}")))
}

fn build_issue_scope<'a>(
    auth: &'a McpAuthContext,
    requested_project_slug: Option<&'a str>,
) -> Result<IssueScope<'a>, AppError> {
    match &auth.access_scope {
        McpAccessScope::Tenant => Ok(IssueScope {
            tenant_slug: &auth.tenant_slug,
            workspace_slug: None,
            project_slug: requested_project_slug,
        }),
        McpAccessScope::Workspace { workspace_slug, .. } => Ok(IssueScope {
            tenant_slug: &auth.tenant_slug,
            workspace_slug: Some(workspace_slug.as_str()),
            project_slug: requested_project_slug,
        }),
        McpAccessScope::Project {
            workspace_slug,
            project_slug,
            ..
        } => {
            if let Some(req_slug) = requested_project_slug
                && req_slug.trim() != project_slug.as_str()
            {
                return Err(AppError::Forbidden(
                    "token is scoped to a different project",
                ));
            }
            Ok(IssueScope {
                tenant_slug: &auth.tenant_slug,
                workspace_slug: Some(workspace_slug.as_str()),
                project_slug: Some(project_slug.as_str()),
            })
        }
    }
}

async fn tool_list_projects(
    services: &ToolServices<'_>,
    auth: &McpAuthContext,
) -> Result<ToolOutput, AppError> {
    let ws_filter = auth.access_scope.workspace_slug();
    let projects = services
        .org
        .list_projects(auth.tenant_ctx, &auth.tenant_slug, ws_filter)
        .await?;

    let scoped_project_id = auth.access_scope.project_id();
    let items: Vec<Value> = projects
        .into_iter()
        .filter(|p| scoped_project_id.is_none_or(|allowed| p.id == allowed))
        .map(|p| {
            // Deliberately omit `dsn_public_key` and `dsn` credentials from MCP output.
            json!({
                "id": p.id,
                "slug": p.slug,
                "name": p.name,
                "workspace_slug": p.workspace_slug,
                "platform": p.platform,
                "effective_retention_days": p.effective_retention_days,
                "rate_limit_per_minute": p.rate_limit_per_minute,
                "ui_url": project_ui_url(services.public_base_url, &p.slug),
            })
        })
        .collect();

    Ok(ToolOutput::from_json(json!({
        "scope": auth.access_scope.as_str(),
        "total": items.len(),
        "projects": items,
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListIssuesArgs {
    project_slug: Option<String>,
    environment: Option<String>,
    release: Option<String>,
    status: Option<String>,
    is_regressed: Option<bool>,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
    query: Option<String>,
    sort: Option<String>,
    page: Option<u32>,
    per_page: Option<u32>,
}

async fn tool_list_issues(
    services: &ToolServices<'_>,
    auth: &McpAuthContext,
    args: ListIssuesArgs,
) -> Result<ToolOutput, AppError> {
    let per_page = args.per_page.unwrap_or(20);
    if !(1..=50).contains(&per_page) {
        return Err(AppError::Validation(
            "per_page must be between 1 and 50 for MCP responses".to_owned(),
        ));
    }

    let req_proj_slug = args
        .project_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let scope = build_issue_scope(auth, req_proj_slug)?;

    let filter = IssueListFilter {
        workspace_slug: scope.workspace_slug.map(str::to_owned),
        project_id: auth.access_scope.project_id(),
        project_slug: scope.project_slug.map(str::to_owned),
        environment: args.environment,
        release: args.release,
        status: args.status,
        is_regressed: args.is_regressed,
        since: args.since,
        until: args.until,
        query: args.query,
        sort: args.sort,
        page: args.page,
        per_page: Some(per_page),
    };

    let page_resp = services
        .issues
        .list_issues(auth.tenant_ctx, scope, filter)
        .await?;

    let rules = services
        .org
        .list_effective_pii_rules(
            auth.tenant_ctx,
            auth.access_scope.workspace_id(),
            auth.access_scope.project_id(),
        )
        .await?;

    let mut any_truncated = page_resp.page < page_resp.total_pages;
    let mut items = Vec::with_capacity(page_resp.items.len());
    for issue in page_resp.items {
        let (title_wrapped, t1) = wrap_untrusted_text(&issue.title, &rules, 240);
        let (culprit_wrapped, t2) = wrap_untrusted_text(&issue.culprit, &rules, 180);
        any_truncated |= t1 || t2;

        let commit_sha = match issue.last_release.as_deref() {
            Some(ver) => services
                .issues
                .resolve_release_commit_sha(auth.tenant_ctx, issue.project_id, ver)
                .await
                .unwrap_or(None),
            None => None,
        };

        items.push(json!({
            "id": issue.id,
            "project_slug": issue.project_slug,
            "workspace_slug": issue.workspace_slug,
            "status": issue.status,
            "is_regressed": issue.is_regressed,
            "level": issue.level,
            "event_count": issue.event_count,
            "first_seen": issue.first_seen,
            "last_seen": issue.last_seen,
            "last_environment": issue.last_environment,
            "first_release": issue.first_release,
            "last_release": issue.last_release,
            "last_release_commit_sha": commit_sha,
            "ui_url": issue_ui_url(services.public_base_url, &issue.project_slug, issue.id),
            "untrusted_event_data": {
                "title": title_wrapped,
                "culprit": culprit_wrapped,
                "exception_type": issue.exception_type,
            }
        }));
    }

    Ok(ToolOutput::from_json(json!({
        "security_notice": UNTRUSTED_SECURITY_NOTICE,
        "page": page_resp.page,
        "per_page": page_resp.per_page,
        "total": page_resp.total,
        "total_pages": page_resp.total_pages,
        "has_more": page_resp.page < page_resp.total_pages,
        "truncated": any_truncated,
        "items": items,
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetIssueArgs {
    issue_id: i64,
    in_app_only: Option<bool>,
    include_sample_event: Option<bool>,
    hours: Option<u32>,
}

async fn tool_get_issue(
    services: &ToolServices<'_>,
    auth: &McpAuthContext,
    args: GetIssueArgs,
) -> Result<ToolOutput, AppError> {
    let scope = build_issue_scope(auth, None)?;
    let detail = services
        .issues
        .get_issue_detail(
            auth.tenant_ctx,
            scope.clone(),
            args.issue_id,
            Some(args.hours.unwrap_or(24).clamp(1, 720)),
        )
        .await?;

    let rules = services
        .org
        .list_effective_pii_rules(
            auth.tenant_ctx,
            Some(detail.issue.workspace_id),
            Some(detail.issue.project_id),
        )
        .await?;

    let in_app_only = args.in_app_only.unwrap_or(true);
    let include_sample_event = args.include_sample_event.unwrap_or(true);

    let (title_wrapped, t_title) = wrap_untrusted_text(&detail.issue.title, &rules, 300);
    let (exc_val_wrapped, t_exc) = wrap_untrusted_text(&detail.issue.exception_value, &rules, 500);
    let (culprit_wrapped, t_culprit) = wrap_untrusted_text(&detail.issue.culprit, &rules, 200);
    let (frames, t_frames, omitted_non_in_app) =
        format_stack_frames(&detail.stack_frames, in_app_only, &rules, 20);
    let (tags, t_tags) = scrub_and_bound_json(detail.tags, &rules, 200, 30, 30);

    let mut truncated = t_title || t_exc || t_culprit || t_frames || t_tags;

    let sample_summary = if include_sample_event {
        if let Some(ev) = detail.sample_event {
            let (compact_payload, t_ev) = extract_compact_event_fields(&ev.payload, &rules);
            truncated |= t_ev;
            Some(json!({
                "event_id": ev.event_id,
                "received_at": ev.received_at,
                "environment": ev.environment,
                "release": ev.release,
                "platform": ev.platform,
                "context": compact_payload,
            }))
        } else {
            None
        }
    } else {
        None
    };

    let first_release_sha = match detail.issue.first_release.as_deref() {
        Some(ver) => services
            .issues
            .resolve_release_commit_sha(auth.tenant_ctx, detail.issue.project_id, ver)
            .await
            .unwrap_or(None),
        None => None,
    };
    let last_release_sha = match detail.issue.last_release.as_deref() {
        Some(ver) => services
            .issues
            .resolve_release_commit_sha(auth.tenant_ctx, detail.issue.project_id, ver)
            .await
            .unwrap_or(None),
        None => None,
    };

    let notes = services
        .issues
        .list_issue_notes(auth.tenant_ctx, scope, detail.issue.id, Some(15))
        .await
        .unwrap_or_default();

    Ok(ToolOutput::from_json(json!({
        "security_notice": UNTRUSTED_SECURITY_NOTICE,
        "truncated": truncated,
        "ui_url": issue_ui_url(services.public_base_url, &detail.issue.project_slug, detail.issue.id),
        "issue": {
            "id": detail.issue.id,
            "project_slug": detail.issue.project_slug,
            "workspace_slug": detail.issue.workspace_slug,
            "status": detail.issue.status,
            "is_regressed": detail.issue.is_regressed,
            "level": detail.issue.level,
            "event_count": detail.issue.event_count,
            "first_seen": detail.issue.first_seen,
            "last_seen": detail.issue.last_seen,
            "last_environment": detail.issue.last_environment,
            "sample_event_id": detail.issue.sample_event_id,
        },
        "releases": {
            "first_release": detail.issue.first_release,
            "first_release_commit_sha": first_release_sha,
            "last_release": detail.issue.last_release,
            "last_release_commit_sha": last_release_sha,
        },
        "notes": notes,
        "untrusted_event_data": {
            "title": title_wrapped,
            "exception_type": detail.issue.exception_type,
            "exception_value": exc_val_wrapped,
            "culprit": culprit_wrapped,
            "stack_frames": frames,
            "in_app_only": in_app_only,
            "omitted_non_in_app_frames": omitted_non_in_app,
            "tags": tags,
            "sample_event": sample_summary,
        }
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetEventArgs {
    issue_id: i64,
    event_id: Option<String>,
}

async fn tool_get_event(
    services: &ToolServices<'_>,
    auth: &McpAuthContext,
    args: GetEventArgs,
) -> Result<ToolOutput, AppError> {
    let scope = build_issue_scope(auth, None)?;
    let detail = services
        .issues
        .get_issue_detail(auth.tenant_ctx, scope.clone(), args.issue_id, Some(24))
        .await?;

    let event = match args
        .event_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(ev_id) => {
            services
                .issues
                .get_issue_event(auth.tenant_ctx, scope, args.issue_id, ev_id)
                .await?
        }
        None => detail.sample_event.ok_or(AppError::NotFound)?,
    };

    let rules = services
        .org
        .list_effective_pii_rules(
            auth.tenant_ctx,
            Some(detail.issue.workspace_id),
            Some(detail.issue.project_id),
        )
        .await?;

    let (exc_val_wrapped, t_exc) = wrap_untrusted_text(&event.exception_value, &rules, 600);
    let (frames, t_frames, _) = format_stack_frames(&event.stack_frames, false, &rules, 30);
    let (tags, t_tags) = scrub_and_bound_json(event.tags, &rules, 240, 40, 40);
    let (context_fields, t_ctx) = extract_compact_event_fields(&event.payload, &rules);
    let commit_sha = match event.release.as_deref() {
        Some(ver) => services
            .issues
            .resolve_release_commit_sha(auth.tenant_ctx, event.project_id, ver)
            .await
            .unwrap_or(None),
        None => None,
    };

    let truncated = t_exc || t_frames || t_tags || t_ctx;

    Ok(ToolOutput::from_json(json!({
        "security_notice": UNTRUSTED_SECURITY_NOTICE,
        "truncated": truncated,
        "ui_url": issue_ui_url(services.public_base_url, &detail.issue.project_slug, detail.issue.id),
        "event": {
            "event_id": event.event_id,
            "issue_id": event.issue_id,
            "project_slug": detail.issue.project_slug,
            "received_at": event.received_at,
            "environment": event.environment,
            "release": event.release,
            "release_commit_sha": commit_sha,
            "level": event.level,
            "platform": event.platform,
            "is_sample": event.is_sample,
        },
        "untrusted_event_data": {
            "exception_type": event.exception_type,
            "exception_value": exc_val_wrapped,
            "stack_frames": frames,
            "tags": tags,
            "request": context_fields.get("request").cloned().unwrap_or(Value::Null),
            "user": context_fields.get("user").cloned().unwrap_or(Value::Null),
            "breadcrumbs": context_fields.get("breadcrumbs").cloned().unwrap_or(Value::Null),
            "extra": context_fields.get("extra").cloned().unwrap_or(Value::Null),
        }
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetIssueStatsArgs {
    issue_id: i64,
    interval: Option<String>,
    hours: Option<u32>,
}

async fn tool_get_issue_stats(
    services: &ToolServices<'_>,
    auth: &McpAuthContext,
    args: GetIssueStatsArgs,
) -> Result<ToolOutput, AppError> {
    let scope = build_issue_scope(auth, None)?;
    let interval_str = args.interval.as_deref().unwrap_or("1h");
    let default_hours = if interval_str == "1d" { 168 } else { 24 };
    let hours = args.hours.unwrap_or(default_hours);

    let detail = services
        .issues
        .get_issue_detail(auth.tenant_ctx, scope.clone(), args.issue_id, Some(24))
        .await?;
    let stats = services
        .issues
        .get_issue_stats(
            auth.tenant_ctx,
            scope,
            args.issue_id,
            Some(interval_str),
            Some(hours),
        )
        .await?;

    let total_in_window: i64 = stats.series.iter().map(|p| p.event_count).sum();
    let peak = stats
        .series
        .iter()
        .max_by_key(|p| p.event_count)
        .map(|p| json!({"bucket": p.bucket, "event_count": p.event_count}));

    let truncated = stats.series.len() > 200;
    let slice_start = stats.series.len().saturating_sub(200);
    let bounded_series = &stats.series[slice_start..];

    Ok(ToolOutput::from_json(json!({
        "issue_id": stats.issue_id,
        "project_slug": detail.issue.project_slug,
        "interval": stats.interval,
        "hours": hours,
        "total_in_window": total_in_window,
        "peak_bucket": peak,
        "truncated": truncated,
        "ui_url": issue_ui_url(services.public_base_url, &detail.issue.project_slug, detail.issue.id),
        "series": bounded_series,
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateIssueStatusArgs {
    issue_id: i64,
    status: String,
    note: Option<String>,
    commit_ref: Option<String>,
}

async fn tool_update_issue_status(
    services: &ToolServices<'_>,
    auth: &McpAuthContext,
    args: UpdateIssueStatusArgs,
) -> Result<ToolOutput, AppError> {
    let scope = build_issue_scope(auth, None)?;
    let (updated, created_note) = services
        .issues
        .update_issue_status_with_actor(
            auth.tenant_ctx,
            auth.role,
            scope,
            args.issue_id,
            NoteActor {
                user_id: auth.user_id,
                email: &auth.user_email,
                source: "mcp",
            },
            UpdateIssueStatusInput {
                status: args.status,
                note: args.note,
                commit_ref: args.commit_ref,
            },
        )
        .await?;

    Ok(ToolOutput::from_json(json!({
        "updated": true,
        "issue_id": updated.id,
        "project_slug": updated.project_slug,
        "status": updated.status,
        "is_regressed": updated.is_regressed,
        "note": created_note,
        "ui_url": issue_ui_url(services.public_base_url, &updated.project_slug, updated.id),
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddIssueNoteArgs {
    issue_id: i64,
    body: String,
    commit_ref: Option<String>,
}

async fn tool_add_issue_note(
    services: &ToolServices<'_>,
    auth: &McpAuthContext,
    args: AddIssueNoteArgs,
) -> Result<ToolOutput, AppError> {
    let scope = build_issue_scope(auth, None)?;
    let detail = services
        .issues
        .get_issue_detail(auth.tenant_ctx, scope.clone(), args.issue_id, Some(24))
        .await?;

    let created = services
        .issues
        .add_issue_note(
            auth.tenant_ctx,
            auth.role,
            scope,
            args.issue_id,
            NoteActor {
                user_id: auth.user_id,
                email: &auth.user_email,
                source: "mcp",
            },
            CreateIssueNoteInput {
                body: args.body,
                commit_ref: args.commit_ref,
            },
        )
        .await?;

    Ok(ToolOutput::from_json(json!({
        "created": true,
        "note": created,
        "ui_url": issue_ui_url(services.public_base_url, &detail.issue.project_slug, detail.issue.id),
    })))
}

fn extract_compact_event_fields(
    payload: &Value,
    rules: &[maxi_loop_platform::scrub_notify::PiiScrubRuleEntry],
) -> (Value, bool) {
    let mut picked = serde_json::Map::new();
    for key in ["request", "user", "breadcrumbs", "extra", "contexts"] {
        if let Some(val) = payload.get(key) {
            picked.insert(key.to_owned(), val.clone());
        }
    }
    scrub_and_bound_json(Value::Object(picked), rules, 260, 20, 25)
}

fn build_performance_scope<'a>(
    auth: &'a McpAuthContext,
    requested_project_slug: Option<&'a str>,
) -> Result<PerformanceScope<'a>, AppError> {
    match &auth.access_scope {
        McpAccessScope::Tenant => Ok(PerformanceScope {
            tenant_slug: &auth.tenant_slug,
            workspace_slug: None,
            project_slug: requested_project_slug,
        }),
        McpAccessScope::Workspace { workspace_slug, .. } => Ok(PerformanceScope {
            tenant_slug: &auth.tenant_slug,
            workspace_slug: Some(workspace_slug.as_str()),
            project_slug: requested_project_slug,
        }),
        McpAccessScope::Project {
            workspace_slug,
            project_slug,
            ..
        } => {
            if let Some(req_slug) = requested_project_slug
                && req_slug.trim() != project_slug.as_str()
            {
                return Err(AppError::Forbidden(
                    "token is scoped to a different project",
                ));
            }
            Ok(PerformanceScope {
                tenant_slug: &auth.tenant_slug,
                workspace_slug: Some(workspace_slug.as_str()),
                project_slug: Some(project_slug.as_str()),
            })
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListTransactionsArgs {
    project_slug: Option<String>,
    environment: Option<String>,
    release: Option<String>,
    op: Option<String>,
    query: Option<String>,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
    hours: Option<u32>,
    sort: Option<String>,
    page: Option<u32>,
    per_page: Option<u32>,
}

async fn tool_list_transactions(
    services: &ToolServices<'_>,
    auth: &McpAuthContext,
    args: ListTransactionsArgs,
) -> Result<ToolOutput, AppError> {
    let per_page = args.per_page.unwrap_or(20);
    if !(1..=50).contains(&per_page) {
        return Err(AppError::Validation(
            "per_page must be between 1 and 50 for MCP responses".to_owned(),
        ));
    }

    let req_proj_slug = args
        .project_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let scope = build_performance_scope(auth, req_proj_slug)?;

    let filter = TransactionListFilter {
        workspace_slug: scope.workspace_slug.map(str::to_owned),
        project_id: auth.access_scope.project_id(),
        project_slug: scope.project_slug.map(str::to_owned),
        environment: args.environment,
        release: args.release,
        op: args.op,
        query: args.query,
        since: args.since,
        until: args.until,
        hours: args.hours,
        sort: args.sort,
        page: args.page,
        per_page: Some(per_page),
    };

    let page_resp = services
        .performance
        .list_transactions(auth.tenant_ctx, scope, filter)
        .await?;

    let rules = services
        .org
        .list_effective_pii_rules(
            auth.tenant_ctx,
            auth.access_scope.workspace_id(),
            auth.access_scope.project_id(),
        )
        .await?;

    let mut any_truncated = page_resp.page < page_resp.total_pages;
    let mut items = Vec::with_capacity(page_resp.items.len());
    for tx in page_resp.items {
        let (tx_name_wrapped, t_name) = wrap_untrusted_text(&tx.transaction_name, &rules, 240);
        any_truncated |= t_name;
        items.push(json!({
            "project_slug": tx.project_slug,
            "workspace_slug": tx.workspace_slug,
            "op": tx.op,
            "total_count": tx.total_count,
            "error_count": tx.error_count,
            "error_rate": tx.error_rate,
            "throughput_rpm": tx.throughput_rpm,
            "p50_ms": tx.p50_ms,
            "p95_ms": tx.p95_ms,
            "p99_ms": tx.p99_ms,
            "avg_ms": tx.avg_ms,
            "min_ms": tx.min_ms,
            "max_ms": tx.max_ms,
            "last_seen": tx.last_seen,
            "ui_url": performance_ui_url(services.public_base_url, &tx.project_slug),
            "untrusted_event_data": {
                "transaction_name": tx_name_wrapped,
            }
        }));
    }

    Ok(ToolOutput::from_json(json!({
        "security_notice": UNTRUSTED_SECURITY_NOTICE,
        "window_minutes": page_resp.window_minutes,
        "since": page_resp.since,
        "until": page_resp.until,
        "page": page_resp.page,
        "per_page": page_resp.per_page,
        "total": page_resp.total,
        "total_pages": page_resp.total_pages,
        "has_more": page_resp.page < page_resp.total_pages,
        "truncated": any_truncated,
        "items": items,
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetTraceArgs {
    trace_id: String,
    max_spans: Option<u32>,
    slow_only: Option<bool>,
}

async fn tool_get_trace(
    services: &ToolServices<'_>,
    auth: &McpAuthContext,
    args: GetTraceArgs,
) -> Result<ToolOutput, AppError> {
    let max_spans = usize::try_from(args.max_spans.unwrap_or(60).clamp(1, 200)).unwrap_or(60);
    let slow_only = args.slow_only.unwrap_or(false);
    let scope = build_performance_scope(auth, None)?;

    let rules = services
        .org
        .list_effective_pii_rules(
            auth.tenant_ctx,
            auth.access_scope.workspace_id(),
            auth.access_scope.project_id(),
        )
        .await?;

    let wf = services
        .performance
        .get_trace(auth.tenant_ctx, scope, &args.trace_id, &rules)
        .await?;

    let (root_tx_wrapped, mut truncated) = wrap_untrusted_text(&wf.root_transaction, &rules, 240);

    let filtered_spans: Vec<_> = if slow_only {
        wf.spans
            .iter()
            .filter(|s| s.is_slow || s.is_error || s.is_transaction_root)
            .collect()
    } else {
        wf.spans.iter().collect()
    };

    let omitted_spans = filtered_spans.len().saturating_sub(max_spans);
    if omitted_spans > 0 {
        truncated = true;
    }

    let mut compact_spans = Vec::with_capacity(filtered_spans.len().min(max_spans));
    for span in filtered_spans.into_iter().take(max_spans) {
        let (desc_wrapped, t_desc) = wrap_untrusted_text(&span.description, &rules, 240);
        let (tags_bounded, t_tags) = scrub_and_bound_json(span.tags.clone(), &rules, 160, 15, 20);
        let (data_bounded, t_data) = scrub_and_bound_json(span.data.clone(), &rules, 200, 15, 20);
        truncated |= t_desc || t_tags || t_data;

        compact_spans.push(json!({
            "span_id": span.span_id,
            "parent_span_id": span.parent_span_id,
            "project_slug": span.project_slug,
            "platform": span.platform,
            "op": span.op,
            "status": span.status,
            "is_error": span.is_error,
            "is_transaction_root": span.is_transaction_root,
            "is_slow": span.is_slow,
            "depth": span.depth,
            "offset_ms": span.offset_ms,
            "duration_ms": span.duration_ms,
            "self_duration_ms": span.self_duration_ms,
            "pct_of_trace": span.pct_of_trace,
            "untrusted_event_data": {
                "description": desc_wrapped,
                "tags": tags_bounded,
                "data": data_bounded,
            }
        }));
    }

    let slowest_spans: Vec<Value> = wf
        .slowest_spans
        .iter()
        .map(|s| {
            let (desc_wrapped, t_desc) = wrap_untrusted_text(&s.description, &rules, 200);
            truncated |= t_desc;
            json!({
                "span_id": s.span_id,
                "project_slug": s.project_slug,
                "op": s.op,
                "duration_ms": s.duration_ms,
                "self_duration_ms": s.self_duration_ms,
                "pct_of_trace": s.pct_of_trace,
                "is_error": s.is_error,
                "untrusted_event_data": {
                    "description": desc_wrapped,
                }
            })
        })
        .collect();

    Ok(ToolOutput::from_json(json!({
        "security_notice": UNTRUSTED_SECURITY_NOTICE,
        "truncated": truncated,
        "omitted_spans": omitted_spans,
        "ui_url": trace_ui_url(services.public_base_url, &wf.trace_id),
        "trace": {
            "trace_id": wf.trace_id,
            "status": wf.status,
            "is_error": wf.is_error,
            "projects": wf.projects,
            "services_count": wf.services_count,
            "sample_reasons": wf.sample_reasons,
            "start_timestamp": wf.start_timestamp,
            "end_timestamp": wf.end_timestamp,
            "total_duration_ms": wf.total_duration_ms,
            "total_spans": wf.span_count,
            "untrusted_event_data": {
                "root_transaction": root_tx_wrapped,
            }
        },
        "slowest_spans": slowest_spans,
        "waterfall": compact_spans,
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListSlowQueriesArgs {
    project_slug: Option<String>,
    environment: Option<String>,
    release: Option<String>,
    op: Option<String>,
    db_only: Option<bool>,
    query: Option<String>,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
    hours: Option<u32>,
    sort: Option<String>,
    page: Option<u32>,
    per_page: Option<u32>,
}

async fn tool_list_slow_queries(
    services: &ToolServices<'_>,
    auth: &McpAuthContext,
    args: ListSlowQueriesArgs,
) -> Result<ToolOutput, AppError> {
    let per_page = args.per_page.unwrap_or(20);
    if !(1..=50).contains(&per_page) {
        return Err(AppError::Validation(
            "per_page must be between 1 and 50 for MCP responses".to_owned(),
        ));
    }

    let req_proj_slug = args
        .project_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let scope = build_performance_scope(auth, req_proj_slug)?;
    let db_only = match args.db_only {
        Some(v) => v,
        None => args
            .op
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_none(),
    };

    let filter = SlowOperationListFilter {
        workspace_slug: scope.workspace_slug.map(str::to_owned),
        project_id: auth.access_scope.project_id(),
        project_slug: scope.project_slug.map(str::to_owned),
        environment: args.environment,
        release: args.release,
        op: args.op,
        db_only: Some(db_only),
        query: args.query,
        since: args.since,
        until: args.until,
        hours: args.hours,
        sort: args.sort,
        page: args.page,
        per_page: Some(per_page),
    };

    let page_resp = services
        .performance
        .list_slow_operations(auth.tenant_ctx, scope, filter)
        .await?;

    let rules = services
        .org
        .list_effective_pii_rules(
            auth.tenant_ctx,
            auth.access_scope.workspace_id(),
            auth.access_scope.project_id(),
        )
        .await?;

    let mut any_truncated = page_resp.page < page_resp.total_pages;
    let mut items = Vec::with_capacity(page_resp.items.len());
    for op_row in page_resp.items {
        let (desc_wrapped, t_desc) =
            wrap_untrusted_text(&op_row.normalized_description, &rules, 320);
        any_truncated |= t_desc;
        let sample_trace_ui = op_row
            .sample_trace_id
            .as_deref()
            .map(|tid| trace_ui_url(services.public_base_url, tid));

        items.push(json!({
            "project_slug": op_row.project_slug,
            "workspace_slug": op_row.workspace_slug,
            "op": op_row.op,
            "description_hash": op_row.description_hash,
            "total_count": op_row.total_count,
            "error_count": op_row.error_count,
            "error_rate": op_row.error_rate,
            "total_ms": op_row.total_ms,
            "avg_ms": op_row.avg_ms,
            "p50_ms": op_row.p50_ms,
            "p95_ms": op_row.p95_ms,
            "p99_ms": op_row.p99_ms,
            "min_ms": op_row.min_ms,
            "max_ms": op_row.max_ms,
            "sample_trace_id": op_row.sample_trace_id,
            "sample_trace_ui_url": sample_trace_ui,
            "last_seen": op_row.last_seen,
            "ui_url": slow_queries_ui_url(services.public_base_url, &op_row.project_slug),
            "untrusted_event_data": {
                "normalized_description": desc_wrapped,
            }
        }));
    }

    Ok(ToolOutput::from_json(json!({
        "security_notice": UNTRUSTED_SECURITY_NOTICE,
        "db_only": db_only,
        "window_minutes": page_resp.window_minutes,
        "since": page_resp.since,
        "until": page_resp.until,
        "page": page_resp.page,
        "per_page": page_resp.per_page,
        "total": page_resp.total,
        "total_pages": page_resp.total_pages,
        "has_more": page_resp.page < page_resp.total_pages,
        "truncated": any_truncated,
        "items": items,
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompareReleasesArgs {
    project_slug: Option<String>,
    base_release: String,
    target_release: String,
    environment: Option<String>,
    hours: Option<u32>,
    limit: Option<u32>,
}

async fn tool_compare_releases(
    services: &ToolServices<'_>,
    auth: &McpAuthContext,
    args: CompareReleasesArgs,
) -> Result<ToolOutput, AppError> {
    let limit = args.limit.unwrap_or(20);
    if !(1..=50).contains(&limit) {
        return Err(AppError::Validation(
            "limit must be between 1 and 50 for MCP responses".to_owned(),
        ));
    }

    let req_proj_slug = args
        .project_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let scope = build_performance_scope(auth, req_proj_slug)?;
    let effective_project_slug = scope
        .project_slug
        .ok_or_else(|| {
            AppError::Validation(
                "project_slug is required when using a tenant or workspace scoped token".to_owned(),
            )
        })?
        .to_owned();

    let cmp = services
        .performance
        .compare_releases(
            auth.tenant_ctx,
            scope,
            CompareReleasesFilter {
                project_slug: effective_project_slug.clone(),
                base_release: args.base_release,
                target_release: args.target_release,
                environment: args.environment,
                since: None,
                until: None,
                hours: args.hours,
                limit: Some(limit),
            },
        )
        .await?;

    let rules = services
        .org
        .list_effective_pii_rules(
            auth.tenant_ctx,
            Some(cmp.workspace_id),
            Some(cmp.project_id),
        )
        .await?;

    let mut truncated = cmp.total_transactions_compared > cmp.transactions.len();
    let mut tx_items = Vec::with_capacity(cmp.transactions.len());
    for row in cmp.transactions {
        let (tx_name_wrapped, t_name) = wrap_untrusted_text(&row.transaction_name, &rules, 240);
        truncated |= t_name;
        tx_items.push(json!({
            "op": row.op,
            "status": row.status,
            "base": row.base,
            "target": row.target,
            "p50_delta_ms": row.p50_delta_ms,
            "p50_delta_pct": row.p50_delta_pct,
            "p95_delta_ms": row.p95_delta_ms,
            "p95_delta_pct": row.p95_delta_pct,
            "p99_delta_ms": row.p99_delta_ms,
            "p99_delta_pct": row.p99_delta_pct,
            "error_rate_delta": row.error_rate_delta,
            "throughput_rpm_delta": row.throughput_rpm_delta,
            "untrusted_event_data": {
                "transaction_name": tx_name_wrapped,
            }
        }));
    }

    let mut new_issues = Vec::with_capacity(cmp.new_issues_in_target.len());
    for issue in cmp.new_issues_in_target {
        let (title_wrapped, t_title) = wrap_untrusted_text(&issue.title, &rules, 240);
        truncated |= t_title;
        new_issues.push(json!({
            "id": issue.id,
            "exception_type": issue.exception_type,
            "level": issue.level,
            "status": issue.status,
            "is_regressed": issue.is_regressed,
            "event_count": issue.event_count,
            "first_seen": issue.first_seen,
            "last_seen": issue.last_seen,
            "ui_url": issue_ui_url(services.public_base_url, &cmp.project_slug, issue.id),
            "untrusted_event_data": {
                "title": title_wrapped,
            }
        }));
    }

    Ok(ToolOutput::from_json(json!({
        "security_notice": UNTRUSTED_SECURITY_NOTICE,
        "project_slug": cmp.project_slug,
        "workspace_slug": cmp.workspace_slug,
        "environment": cmp.environment,
        "since": cmp.since,
        "until": cmp.until,
        "verdict": cmp.verdict,
        "base": cmp.base,
        "target": cmp.target,
        "delta": cmp.delta,
        "new_issues_in_target": new_issues,
        "transactions": tx_items,
        "total_transactions_compared": cmp.total_transactions_compared,
        "truncated": truncated,
        "ui_url": compare_releases_ui_url(services.public_base_url, &effective_project_slug),
    })))
}

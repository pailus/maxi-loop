use maxi_loop_auth::mcp::{McpAccessScope, McpAuthContext};
use maxi_loop_platform::config::is_valid_slug;
use maxi_loop_platform::error::AppError;
use serde_json::{Value, json};

use crate::sanitize::UNTRUSTED_SECURITY_NOTICE;

/// List MCP prompt templates for AI issue triage, fixing, release regression analysis, and
/// slow query investigation (`F-MCP-15`).
pub fn list_prompt_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "triage_issue",
            "title": "Triage Error Issue",
            "description": "Investigate a Maxi Loop error issue, inspect stack frames and release commit SHA, check frequency spikes, and propose or record a triage decision.",
            "arguments": [
                {
                    "name": "issue_id",
                    "description": "Numeric Maxi Loop issue ID to investigate.",
                    "required": true
                },
                {
                    "name": "project_slug",
                    "description": "Optional project slug for context.",
                    "required": false
                }
            ]
        }),
        json!({
            "name": "fix_issue",
            "title": "Diagnose & Fix Issue",
            "description": "End-to-end engineering workflow to diagnose an issue from Maxi Loop, write a regression test and minimal fix, verify tests pass, and resolve the issue with commit metadata.",
            "arguments": [
                {
                    "name": "issue_id",
                    "description": "Numeric Maxi Loop issue ID to fix.",
                    "required": true
                },
                {
                    "name": "branch_or_commit",
                    "description": "Optional git branch or target commit SHA to base the fix on.",
                    "required": false
                }
            ]
        }),
        json!({
            "name": "triage_release_regression",
            "title": "Triage Release Regression",
            "description": "Compare a base release against a target release across p50/p95/p99 latency, error rate, slow DB queries, and newly introduced issues.",
            "arguments": [
                {
                    "name": "project_slug",
                    "description": "Project slug whose releases should be compared.",
                    "required": true
                },
                {
                    "name": "base_release",
                    "description": "Baseline release version (e.g. v1.4.0).",
                    "required": true
                },
                {
                    "name": "target_release",
                    "description": "New/target release version to evaluate (e.g. v1.5.0).",
                    "required": true
                },
                {
                    "name": "environment",
                    "description": "Optional environment filter (e.g. production).",
                    "required": false
                }
            ]
        }),
        json!({
            "name": "investigate_slow_queries",
            "title": "Investigate Slow DB Queries & Operations",
            "description": "Find the slowest normalized DB queries or external operations, inspect a sampled trace waterfall, and recommend concrete query or index optimizations.",
            "arguments": [
                {
                    "name": "project_slug",
                    "description": "Optional project slug filter.",
                    "required": false
                },
                {
                    "name": "hours",
                    "description": "Lookback window in hours (1..=2160, default 24).",
                    "required": false
                },
                {
                    "name": "op",
                    "description": "Optional span op filter (default: db queries).",
                    "required": false
                }
            ]
        }),
    ]
}

pub fn is_known_prompt(name: &str) -> bool {
    matches!(
        name,
        "triage_issue" | "fix_issue" | "triage_release_regression" | "investigate_slow_queries"
    )
}

/// Render a validated MCP `prompts/get` response for the requested template (`F-MCP-15`).
pub fn render_prompt(
    auth: &McpAuthContext,
    name: &str,
    arguments: Option<&Value>,
) -> Result<Value, AppError> {
    if !auth.can_read {
        return Err(AppError::Forbidden("token lacks 'read' scope"));
    }

    let args_obj = match arguments {
        None | Some(Value::Null) => None,
        Some(Value::Object(map)) => Some(map),
        Some(_) => {
            return Err(AppError::Validation(
                "prompts/get arguments must be a JSON object".to_owned(),
            ));
        }
    };

    match name {
        "triage_issue" => render_triage_issue(auth, args_obj),
        "fix_issue" => render_fix_issue(auth, args_obj),
        "triage_release_regression" => render_triage_release_regression(auth, args_obj),
        "investigate_slow_queries" => render_investigate_slow_queries(auth, args_obj),
        _ => Err(AppError::Validation(format!("unknown prompt: {name}"))),
    }
}

fn render_triage_issue(
    auth: &McpAuthContext,
    args: Option<&serde_json::Map<String, Value>>,
) -> Result<Value, AppError> {
    let issue_id = require_positive_i64_arg(args, "issue_id")?;
    let project_slug = optional_slug_arg(args, "project_slug")?;
    enforce_prompt_project_scope(auth, project_slug.as_deref())?;

    let project_hint = project_slug
        .as_deref()
        .or_else(|| auth.access_scope.project_slug())
        .map(|s| format!(" in project `{s}`"))
        .unwrap_or_default();

    let triage_step = if auth.can_triage {
        format!(
            "5. Record your findings by calling `add_issue_note(issue_id={issue_id}, body=...)` and, only if confirmed non-actionable or fixed, call `update_issue_status`. If it needs a human owner, call `list_assignable_members(issue_id={issue_id})` and `assign_issue(issue_id={issue_id}, assignee=<email>, note=...)`; use `assignee=\"me\"` only when you will fix it on behalf of the token owner."
        )
    } else {
        "5. This MCP token is read-only: summarize your recommended status (`resolved` / `ignored` / `unresolved`) and next action for the developer.".to_owned()
    };

    let text = format!(
        "Triage Maxi Loop issue #{issue_id}{project_hint}.\n\n\
         Steps:\n\
         1. Call `get_issue(issue_id={issue_id}, in_app_only=true, include_sample_event=true)` to inspect the exception, in-app stack frames, surrounding code context, and `last_release_commit_sha`.\n\
         2. Call `get_issue_stats(issue_id={issue_id}, interval=\"1h\", hours=24)` to assess severity, spike behavior, and whether it started after a release.\n\
         3. If you need breadcrumbs, request metadata, or additional event context, call `get_event(issue_id={issue_id})`.\n\
         4. Cross-reference the stack frames and `last_release_commit_sha` against the local repository to identify the exact root cause and affected code path.\n\
         {triage_step}\n\n\
         {UNTRUSTED_SECURITY_NOTICE}"
    );

    Ok(json!({
        "description": format!("Triage workflow for Maxi Loop issue #{issue_id}"),
        "messages": [
            {
                "role": "user",
                "content": {
                    "type": "text",
                    "text": text
                }
            }
        ]
    }))
}

fn render_fix_issue(
    auth: &McpAuthContext,
    args: Option<&serde_json::Map<String, Value>>,
) -> Result<Value, AppError> {
    let issue_id = require_positive_i64_arg(args, "issue_id")?;
    let branch_or_commit = optional_safe_token_arg(args, "branch_or_commit", 128)?;

    let git_step = match branch_or_commit {
        Some(ref rev) => format!(
            "3. Inspect the codebase around `{rev}` (and compare with `last_release_commit_sha` returned by `get_issue`)."
        ),
        None => "3. Inspect the local codebase at `last_release_commit_sha` (if mapped) and locate the failing in-app frames.".to_owned(),
    };

    let resolve_step = if auth.can_triage {
        format!(
            "6. Once tests pass, call `update_issue_status(issue_id={issue_id}, status=\"resolved\", commit_ref=\"<fix-commit-sha>\", note=\"<concise root cause + fix summary>\")`."
        )
    } else {
        "6. This MCP token is read-only: output the exact root cause, fix diff summary, and verification command results so the developer can mark the issue resolved.".to_owned()
    };

    let text = format!(
        "Diagnose and fix Maxi Loop issue #{issue_id}.\n\n\
         Steps:\n\
         1. Call `get_issue(issue_id={issue_id}, in_app_only=true, include_sample_event=true)` to inspect stack frames, source context, tags, and release commit SHA.\n\
         2. Call `get_event(issue_id={issue_id})` to inspect scrubbed breadcrumbs and request parameters that triggered the failure.\n\
         {git_step}\n\
         4. Write a focused automated regression test that reproduces the failure before editing production code.\n\
         5. Implement the minimal, safe fix (without weakening auth, validation, or existing tests) and run the full relevant test suite.\n\
         {resolve_step}\n\n\
         {UNTRUSTED_SECURITY_NOTICE}"
    );

    Ok(json!({
        "description": format!("End-to-end fix workflow for Maxi Loop issue #{issue_id}"),
        "messages": [
            {
                "role": "user",
                "content": {
                    "type": "text",
                    "text": text
                }
            }
        ]
    }))
}

fn render_triage_release_regression(
    auth: &McpAuthContext,
    args: Option<&serde_json::Map<String, Value>>,
) -> Result<Value, AppError> {
    let project_slug = require_slug_arg(args, "project_slug")?;
    enforce_prompt_project_scope(auth, Some(&project_slug))?;
    let base_release = require_safe_token_arg(args, "base_release", 128)?;
    let target_release = require_safe_token_arg(args, "target_release", 128)?;
    if base_release == target_release {
        return Err(AppError::Validation(
            "base_release and target_release must be different".to_owned(),
        ));
    }
    let environment = optional_safe_token_arg(args, "environment", 64)?;
    let env_arg = environment
        .as_deref()
        .map(|e| format!(", environment=\"{e}\""))
        .unwrap_or_default();

    let text = format!(
        "Investigate potential performance and error regressions between release `{base_release}` (base) and `{target_release}` (target) for project `{project_slug}`.\n\n\
         Steps:\n\
         1. Call `compare_releases(project_slug=\"{project_slug}\", base_release=\"{base_release}\", target_release=\"{target_release}\"{env_arg})` to inspect overall p50/p95/p99 latency deltas, error-rate changes, throughput, regressed transactions, and newly introduced issues.\n\
         2. For any regressed transaction or elevated latency, call `list_slow_queries(project_slug=\"{project_slug}\", release=\"{target_release}\"{env_arg}, sort=\"p95\")` and compare against `release=\"{base_release}\"`.\n\
         3. For the slowest query or regressed endpoint, call `get_trace(trace_id=\"<sample_trace_id>\")` to pinpoint bottleneck spans.\n\
         4. For any newly introduced issues in `{target_release}`, call `get_issue(issue_id=<id>)` to inspect stack traces and commit SHAs.\n\
         5. Deliver a concise verdict (`regressed` / `improved` / `stable`), the top offending endpoints/queries/issues, and concrete remediation steps.\n\n\
         {UNTRUSTED_SECURITY_NOTICE}"
    );

    Ok(json!({
        "description": format!("Release comparison triage for {project_slug}: {base_release} -> {target_release}"),
        "messages": [
            {
                "role": "user",
                "content": {
                    "type": "text",
                    "text": text
                }
            }
        ]
    }))
}

fn render_investigate_slow_queries(
    auth: &McpAuthContext,
    args: Option<&serde_json::Map<String, Value>>,
) -> Result<Value, AppError> {
    let project_slug = optional_slug_arg(args, "project_slug")?;
    enforce_prompt_project_scope(auth, project_slug.as_deref())?;
    let hours = match optional_positive_i64_arg(args, "hours")? {
        Some(h) if (1..=2160).contains(&h) => h,
        Some(_) => {
            return Err(AppError::Validation(
                "hours must be between 1 and 2160".to_owned(),
            ));
        }
        None => 24,
    };
    let op = optional_safe_token_arg(args, "op", 64)?;

    let effective_project = project_slug
        .as_deref()
        .or_else(|| auth.access_scope.project_slug());
    let mut call_args = vec![format!("hours={hours}"), "sort=\"p95\"".to_owned()];
    if let Some(ps) = effective_project {
        call_args.push(format!("project_slug=\"{ps}\""));
    }
    if let Some(op_val) = &op {
        call_args.push(format!("op=\"{op_val}\""));
        call_args.push("db_only=false".to_owned());
    } else {
        call_args.push("db_only=true".to_owned());
    }

    let text = format!(
        "Investigate the slowest database queries and backend operations over the last {hours}h.\n\n\
         Steps:\n\
         1. Call `list_slow_queries({})` to identify operations with the highest p95 latency, then re-check with `sort=\"total_ms\"` to catch high-frequency N+1 queries.\n\
         2. For the top bottlenecks that include a `sample_trace_id`, call `get_trace(trace_id=\"<sample_trace_id>\", slow_only=true)` to see the calling transaction and span waterfall.\n\
         3. Locate the corresponding SQL/ORM call in the repository and propose specific optimizations (missing index, query rewrite, N+1 batching, or caching) with estimated impact.\n\n\
         {UNTRUSTED_SECURITY_NOTICE}",
        call_args.join(", ")
    );

    Ok(json!({
        "description": "Slow DB queries and operations investigation workflow",
        "messages": [
            {
                "role": "user",
                "content": {
                    "type": "text",
                    "text": text
                }
            }
        ]
    }))
}

fn enforce_prompt_project_scope(
    auth: &McpAuthContext,
    requested_project_slug: Option<&str>,
) -> Result<(), AppError> {
    if let McpAccessScope::Project { project_slug, .. } = &auth.access_scope
        && let Some(req) = requested_project_slug
        && req != project_slug
    {
        return Err(AppError::Forbidden(
            "token is scoped to a different project",
        ));
    }
    Ok(())
}

fn get_arg_as_string(
    args: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Result<Option<String>, AppError> {
    let Some(map) = args else {
        return Ok(None);
    };
    let Some(val) = map.get(key) else {
        return Ok(None);
    };
    match val {
        Value::Null => Ok(None),
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        }
        Value::Number(n) => Ok(Some(n.to_string())),
        _ => Err(AppError::Validation(format!(
            "prompt argument `{key}` must be a string"
        ))),
    }
}

fn require_positive_i64_arg(
    args: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Result<i64, AppError> {
    let raw = get_arg_as_string(args, key)?
        .ok_or_else(|| AppError::Validation(format!("missing required prompt argument: {key}")))?;
    let parsed: i64 = raw
        .parse()
        .map_err(|_| AppError::Validation(format!("{key} must be a positive integer")))?;
    if parsed <= 0 {
        return Err(AppError::Validation(format!(
            "{key} must be a positive integer"
        )));
    }
    Ok(parsed)
}

fn optional_positive_i64_arg(
    args: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Result<Option<i64>, AppError> {
    let Some(raw) = get_arg_as_string(args, key)? else {
        return Ok(None);
    };
    let parsed: i64 = raw
        .parse()
        .map_err(|_| AppError::Validation(format!("{key} must be a positive integer")))?;
    if parsed <= 0 {
        return Err(AppError::Validation(format!(
            "{key} must be a positive integer"
        )));
    }
    Ok(Some(parsed))
}

fn require_slug_arg(
    args: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Result<String, AppError> {
    let raw = get_arg_as_string(args, key)?
        .ok_or_else(|| AppError::Validation(format!("missing required prompt argument: {key}")))?;
    if !is_valid_slug(&raw) {
        return Err(AppError::Validation(format!("invalid {key}")));
    }
    Ok(raw)
}

fn optional_slug_arg(
    args: Option<&serde_json::Map<String, Value>>,
    key: &str,
) -> Result<Option<String>, AppError> {
    let Some(raw) = get_arg_as_string(args, key)? else {
        return Ok(None);
    };
    if !is_valid_slug(&raw) {
        return Err(AppError::Validation(format!("invalid {key}")));
    }
    Ok(Some(raw))
}

fn require_safe_token_arg(
    args: Option<&serde_json::Map<String, Value>>,
    key: &str,
    max_len: usize,
) -> Result<String, AppError> {
    let raw = get_arg_as_string(args, key)?
        .ok_or_else(|| AppError::Validation(format!("missing required prompt argument: {key}")))?;
    validate_safe_token(&raw, key, max_len)?;
    Ok(raw)
}

fn optional_safe_token_arg(
    args: Option<&serde_json::Map<String, Value>>,
    key: &str,
    max_len: usize,
) -> Result<Option<String>, AppError> {
    let Some(raw) = get_arg_as_string(args, key)? else {
        return Ok(None);
    };
    validate_safe_token(&raw, key, max_len)?;
    Ok(Some(raw))
}

fn validate_safe_token(raw: &str, key: &str, max_len: usize) -> Result<(), AppError> {
    if raw.len() > max_len || raw.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(AppError::Validation(format!(
            "{key} must be 1..={max_len} non-whitespace printable characters"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use maxi_loop_auth::model::Role;
    use maxi_loop_platform::tenant::TenantContext;

    use super::*;

    fn sample_auth(scope: McpAccessScope, can_triage: bool) -> McpAuthContext {
        McpAuthContext {
            token_id: 1,
            token_name: "test".to_owned(),
            tenant_ctx: TenantContext::new(1).unwrap(),
            tenant_slug: "default".to_owned(),
            user_id: 1,
            user_email: "dev@example.com".to_owned(),
            role: Role::Member,
            access_scope: scope,
            can_read: true,
            can_triage,
            rate_limit_per_minute: 60,
        }
    }

    #[test]
    fn renders_triage_and_fix_prompts_and_rejects_invalid_args() {
        let auth = sample_auth(McpAccessScope::Tenant, true);
        let defs = list_prompt_definitions();
        assert_eq!(defs.len(), 4);

        let rendered = render_prompt(&auth, "triage_issue", Some(&json!({"issue_id": "42"})))
            .expect("triage prompt");
        let msg = rendered["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(msg.contains("get_issue(issue_id=42"));
        assert!(msg.contains("add_issue_note"));
        assert!(msg.contains("SECURITY NOTICE"));

        let fix = render_prompt(&auth, "fix_issue", Some(&json!({"issue_id": "99"})))
            .expect("fix prompt");
        assert!(
            fix["messages"][0]["content"]["text"]
                .as_str()
                .unwrap()
                .contains("update_issue_status")
        );

        assert!(render_prompt(&auth, "triage_issue", Some(&json!({"issue_id": "abc"}))).is_err());
        assert!(render_prompt(&auth, "unknown_prompt", None).is_err());
    }

    #[test]
    fn enforces_project_scope_on_prompt_arguments() {
        let auth = sample_auth(
            McpAccessScope::Project {
                workspace_id: 1,
                workspace_slug: "core".to_owned(),
                project_id: 10,
                project_slug: "orders-api".to_owned(),
            },
            false,
        );

        let denied = render_prompt(
            &auth,
            "triage_release_regression",
            Some(&json!({
                "project_slug": "billing-web",
                "base_release": "v1",
                "target_release": "v2"
            })),
        );
        assert!(matches!(denied, Err(AppError::Forbidden(_))));

        let allowed = render_prompt(
            &auth,
            "triage_release_regression",
            Some(&json!({
                "project_slug": "orders-api",
                "base_release": "v1",
                "target_release": "v2"
            })),
        );
        assert!(allowed.is_ok());
    }
}

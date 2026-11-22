use chrono::{DateTime, Utc};
use maxi_loop_platform::access::AccessGuard;
use maxi_loop_platform::config::is_valid_slug;
use maxi_loop_platform::error::AppError;
use maxi_loop_platform::tenant::{TenantContext, begin_tenant_tx};
use sqlx::{FromRow, PgPool, Postgres, QueryBuilder};

use crate::codec::{
    normalize_debug_id, normalize_upload_filename, validate_and_compress_sourcemap,
};
use crate::model::{SourceMapListFilter, SourceMapView, UploadSourceMapRawInput};
use crate::resolver::SourceMapResolver;

#[derive(FromRow)]
struct SourceMapJoinedRow {
    id: i64,
    tenant_id: i64,
    workspace_id: i64,
    workspace_slug: String,
    project_id: i64,
    project_slug: String,
    release_version: String,
    filename: String,
    debug_id: Option<String>,
    sha256: String,
    raw_size_bytes: i32,
    compressed_size_bytes: i32,
    uploaded_by_user_id: Option<i64>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<SourceMapJoinedRow> for SourceMapView {
    fn from(r: SourceMapJoinedRow) -> Self {
        Self {
            id: r.id,
            tenant_id: r.tenant_id,
            workspace_id: r.workspace_id,
            workspace_slug: r.workspace_slug,
            project_id: r.project_id,
            project_slug: r.project_slug,
            release_version: r.release_version,
            filename: r.filename,
            debug_id: r.debug_id,
            sha256: r.sha256,
            raw_size_bytes: r.raw_size_bytes,
            compressed_size_bytes: r.compressed_size_bytes,
            uploaded_by_user_id: r.uploaded_by_user_id,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

/// Upload (or replace) a release-scoped JS/React Source Map v3 artifact (`F-ISS-8`, Design §5.7).
/// Validates Source Map content, compresses with zstd, auto-registers the release row, and
/// invalidates the bounded LRU cache for the target project.
pub async fn upload_source_map(
    pool: &PgPool,
    resolver: &SourceMapResolver,
    tenant_ctx: TenantContext,
    guard: &dyn AccessGuard,
    user_id: i64,
    max_upload_bytes: usize,
    input: UploadSourceMapRawInput,
) -> Result<SourceMapView, AppError> {
    let proj_slug = input.project_slug.trim();
    if !is_valid_slug(proj_slug) {
        return Err(AppError::Validation("invalid project_slug".to_owned()));
    }
    if let Some(ws) = input
        .workspace_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        && !is_valid_slug(ws)
    {
        return Err(AppError::Validation("invalid workspace_slug".to_owned()));
    }

    let release_version = input.release_version.trim();
    if release_version.is_empty()
        || release_version.len() > 128
        || release_version.chars().any(char::is_whitespace)
    {
        return Err(AppError::Validation(
            "release_version must be 1..=128 non-whitespace characters".to_owned(),
        ));
    }

    let normalized_filename = normalize_upload_filename(&input.filename)?;
    let explicit_debug_id = normalize_debug_id(input.debug_id.as_deref())?;

    let validated = validate_and_compress_sourcemap(&input.raw_bytes, max_upload_bytes)?;
    let final_debug_id = explicit_debug_id.or(validated.embedded_debug_id);

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;

    let (project_id, workspace_id): (i64, i64) = match input
        .workspace_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(ws_slug) => sqlx::query_as(
            "SELECT p.id, p.workspace_id
             FROM projects p
             JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
             WHERE p.tenant_id = $1 AND w.slug = $2 AND p.slug = $3",
        )
        .bind(tenant_ctx.tenant_id())
        .bind(ws_slug)
        .bind(proj_slug)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("lookup project for sourcemap: {err}")))?,
        None => sqlx::query_as(
            "SELECT p.id, p.workspace_id
             FROM projects p
             JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
             WHERE p.tenant_id = $1 AND p.slug = $2
             ORDER BY w.is_default DESC, p.id ASC
             LIMIT 1",
        )
        .bind(tenant_ctx.tenant_id())
        .bind(proj_slug)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("lookup project for sourcemap: {err}")))?,
    }
    .ok_or(AppError::NotFound)?;
    guard.check_triage(project_id, workspace_id)?;

    // Ensure the release record exists in `releases` so uploaded source maps link cleanly.
    let inferred_sha = infer_commit_sha(release_version);
    sqlx::query(
        "INSERT INTO releases (tenant_id, project_id, version, commit_sha)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (tenant_id, project_id, version) DO UPDATE SET
             commit_sha = COALESCE(releases.commit_sha, EXCLUDED.commit_sha),
             updated_at = NOW()",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(project_id)
    .bind(release_version)
    .bind(&inferred_sha)
    .execute(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("ensure release for sourcemap: {err}")))?;

    let sm_id: i64 = sqlx::query_scalar(
        "INSERT INTO source_maps (
             tenant_id, project_id, release_version, filename, debug_id, sha256,
             raw_size_bytes, compressed_size_bytes, content_zstd, uploaded_by_user_id
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (tenant_id, project_id, release_version, filename) DO UPDATE SET
             debug_id = COALESCE(EXCLUDED.debug_id, source_maps.debug_id),
             sha256 = EXCLUDED.sha256,
             raw_size_bytes = EXCLUDED.raw_size_bytes,
             compressed_size_bytes = EXCLUDED.compressed_size_bytes,
             content_zstd = EXCLUDED.content_zstd,
             uploaded_by_user_id = EXCLUDED.uploaded_by_user_id,
             updated_at = NOW()
         RETURNING id",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(project_id)
    .bind(release_version)
    .bind(&normalized_filename)
    .bind(&final_debug_id)
    .bind(&validated.sha256)
    .bind(validated.raw_size_bytes)
    .bind(validated.compressed_size_bytes)
    .bind(&validated.content_zstd)
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("upsert source_map: {err}")))?;

    let row: SourceMapJoinedRow = sqlx::query_as(
        "SELECT s.id, s.tenant_id, p.workspace_id, w.slug AS workspace_slug,
                s.project_id, p.slug AS project_slug, s.release_version, s.filename,
                s.debug_id, s.sha256, s.raw_size_bytes, s.compressed_size_bytes,
                s.uploaded_by_user_id, s.created_at, s.updated_at
         FROM source_maps s
         JOIN projects p ON p.tenant_id = s.tenant_id AND p.id = s.project_id
         JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
         WHERE s.tenant_id = $1 AND s.project_id = $2 AND s.id = $3",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(project_id)
    .bind(sm_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("load upserted source_map: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit upload source_map: {err}")))?;

    // Evict stale positive/negative cache entries so newly uploaded artifacts resolve immediately.
    resolver.invalidate_project(tenant_ctx.tenant_id(), project_id);

    Ok(SourceMapView::from(row))
}

/// List uploaded source map metadata for the current tenant (never exposes raw source map blobs).
pub async fn list_source_maps(
    pool: &PgPool,
    tenant_ctx: TenantContext,
    filter: SourceMapListFilter,
) -> Result<Vec<SourceMapView>, AppError> {
    let limit = i64::from(filter.limit.unwrap_or(100).clamp(1, 200));

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT s.id, s.tenant_id, p.workspace_id, w.slug AS workspace_slug,
                s.project_id, p.slug AS project_slug, s.release_version, s.filename,
                s.debug_id, s.sha256, s.raw_size_bytes, s.compressed_size_bytes,
                s.uploaded_by_user_id, s.created_at, s.updated_at
         FROM source_maps s
         JOIN projects p ON p.tenant_id = s.tenant_id AND p.id = s.project_id
         JOIN workspaces w ON w.tenant_id = p.tenant_id AND w.id = p.workspace_id
         WHERE s.tenant_id = ",
    );
    qb.push_bind(tenant_ctx.tenant_id());

    if let Some(ps) = filter
        .project_slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !is_valid_slug(ps) {
            return Err(AppError::Validation("invalid project_slug".to_owned()));
        }
        qb.push(" AND p.slug = ").push_bind(ps.to_owned());
    }
    if let Some(rel) = filter
        .release_version
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if rel.len() > 128 {
            return Err(AppError::Validation("release_version too long".to_owned()));
        }
        qb.push(" AND s.release_version = ")
            .push_bind(rel.to_owned());
    }

    qb.push(" ORDER BY s.updated_at DESC, s.id DESC LIMIT ")
        .push_bind(limit);

    let rows: Vec<SourceMapJoinedRow> = qb
        .build_query_as()
        .fetch_all(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("list source_maps: {err}")))?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit list source_maps: {err}")))?;

    Ok(rows.into_iter().map(SourceMapView::from).collect())
}

/// Delete a source map artifact belonging to the current tenant and invalidate project cache.
pub async fn delete_source_map(
    pool: &PgPool,
    resolver: &SourceMapResolver,
    tenant_ctx: TenantContext,
    guard: &dyn AccessGuard,
    sourcemap_id: i64,
) -> Result<(), AppError> {
    if sourcemap_id <= 0 {
        return Err(AppError::Validation("invalid sourcemap_id".to_owned()));
    }

    let mut tx = begin_tenant_tx(pool, tenant_ctx).await?;
    // Delete and check in one transaction: a forbidden caller's delete is rolled back.
    let deleted: Option<(i64, i64)> = sqlx::query_as(
        "DELETE FROM source_maps s
         USING projects p
         WHERE s.tenant_id = $1 AND s.id = $2
           AND p.tenant_id = s.tenant_id AND p.id = s.project_id
         RETURNING s.project_id, p.workspace_id",
    )
    .bind(tenant_ctx.tenant_id())
    .bind(sourcemap_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|err| AppError::DatabaseQuery(format!("delete source_map: {err}")))?;

    let Some((project_id, workspace_id)) = deleted else {
        return Err(AppError::NotFound);
    };
    guard.check_triage(project_id, workspace_id)?;

    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit delete source_map: {err}")))?;

    resolver.invalidate_project(tenant_ctx.tenant_id(), project_id);
    Ok(())
}

fn infer_commit_sha(version: &str) -> Option<String> {
    let candidate = version.rsplit('@').next().unwrap_or(version).trim();
    if (7..=40).contains(&candidate.len()) && candidate.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(candidate.to_ascii_lowercase())
    } else {
        None
    }
}

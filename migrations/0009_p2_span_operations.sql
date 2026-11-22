-- 0009_p2_span_operations.sql
-- P2 Performance schema addition (`F-PRF-7`):
--   - 1-minute aggregated span operation metrics (`span_minute_metrics`) keyed by normalized
--     operation (`op`) and SHA-256 hash of normalized description (`description_hash`) for slow DB
--     queries, external HTTP calls, cache operations, etc.
--   - Leading `(tenant_id, project_id)` on primary key and indexes
--   - Row-Level Security (`maxi_loop_app` tenant isolation + `maxi_loop_worker` cross-tenant worker role)

CREATE TABLE IF NOT EXISTS span_minute_metrics (
    tenant_id BIGINT NOT NULL,
    project_id BIGINT NOT NULL,
    op TEXT NOT NULL CHECK (char_length(btrim(op)) BETWEEN 1 AND 128),
    description_hash TEXT NOT NULL CHECK (char_length(description_hash) = 64),
    normalized_description TEXT NOT NULL CHECK (char_length(btrim(normalized_description)) BETWEEN 1 AND 512),
    environment TEXT NOT NULL DEFAULT 'production' CHECK (char_length(btrim(environment)) BETWEEN 1 AND 64),
    release TEXT NOT NULL DEFAULT '' CHECK (char_length(release) <= 128),
    minute_bucket TIMESTAMPTZ NOT NULL,
    total_count BIGINT NOT NULL DEFAULT 0 CHECK (total_count >= 0),
    error_count BIGINT NOT NULL DEFAULT 0 CHECK (error_count >= 0),
    duration_sum_ms DOUBLE PRECISION NOT NULL DEFAULT 0 CHECK (duration_sum_ms >= 0),
    duration_min_ms DOUBLE PRECISION NOT NULL DEFAULT 0 CHECK (duration_min_ms >= 0),
    duration_max_ms DOUBLE PRECISION NOT NULL DEFAULT 0 CHECK (duration_max_ms >= 0),
    histogram_buckets BIGINT[] NOT NULL DEFAULT ARRAY[]::BIGINT[],
    sample_trace_id TEXT CHECK (sample_trace_id IS NULL OR char_length(btrim(sample_trace_id)) BETWEEN 1 AND 64),
    PRIMARY KEY (tenant_id, project_id, op, description_hash, environment, release, minute_bucket),
    FOREIGN KEY (tenant_id, project_id) REFERENCES projects(tenant_id, id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS span_metrics_tenant_project_bucket_idx
    ON span_minute_metrics (tenant_id, project_id, minute_bucket DESC);
CREATE INDEX IF NOT EXISTS span_metrics_tenant_project_op_bucket_idx
    ON span_minute_metrics (tenant_id, project_id, op, minute_bucket DESC);
CREATE INDEX IF NOT EXISTS span_metrics_tenant_bucket_idx
    ON span_minute_metrics (tenant_id, minute_bucket DESC);

ALTER TABLE span_minute_metrics ENABLE ROW LEVEL SECURITY;
ALTER TABLE span_minute_metrics FORCE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS tenant_isolation_policy ON span_minute_metrics;
CREATE POLICY tenant_isolation_policy ON span_minute_metrics
    AS PERMISSIVE
    FOR ALL
    TO maxi_loop_app
    USING (tenant_id = app_current_tenant_id())
    WITH CHECK (tenant_id = app_current_tenant_id());

DROP POLICY IF EXISTS worker_cross_tenant_policy ON span_minute_metrics;
CREATE POLICY worker_cross_tenant_policy ON span_minute_metrics
    AS PERMISSIVE
    FOR ALL
    TO maxi_loop_worker
    USING (true)
    WITH CHECK (tenant_id > 0);

GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE span_minute_metrics TO maxi_loop_app, maxi_loop_worker;

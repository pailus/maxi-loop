-- 0005_phase3_performance.sql
-- Phase 3 schema additions:
--   - Immutable log-scale duration histogram merge function & SQL aggregate (`merge_histogram_buckets`, `sum_histogram_buckets`)
--   - 1-minute aggregated transaction metrics (`transaction_minute_metrics`)
--   - Daily-partitioned sampled full traces (`traces`) storing zstd-compressed span blobs (`spans_zstd BYTEA`)
--   - Partition lifecycle helpers (`ensure_traces_partition`, `drop_expired_traces_partitions`)
--   - Row-Level Security (`maxi_loop_app` tenant isolation + `maxi_loop_worker` cross-tenant worker role)

-- 1. Element-wise BIGINT[] histogram bucket merging for upserts and rollups
CREATE OR REPLACE FUNCTION merge_histogram_buckets(a BIGINT[], b BIGINT[])
RETURNS BIGINT[]
LANGUAGE sql
IMMUTABLE
AS $$
    SELECT CASE
        WHEN a IS NULL OR COALESCE(array_length(a, 1), 0) = 0 THEN COALESCE(b, ARRAY[]::BIGINT[])
        WHEN b IS NULL OR COALESCE(array_length(b, 1), 0) = 0 THEN a
        ELSE ARRAY(
            SELECT COALESCE(a[idx], 0) + COALESCE(b[idx], 0)
            FROM generate_series(
                1,
                GREATEST(COALESCE(array_length(a, 1), 0), COALESCE(array_length(b, 1), 0))
            ) AS idx
        )
    END;
$$;

GRANT EXECUTE ON FUNCTION merge_histogram_buckets(BIGINT[], BIGINT[]) TO maxi_loop_app, maxi_loop_worker;

DROP AGGREGATE IF EXISTS sum_histogram_buckets(BIGINT[]);
CREATE AGGREGATE sum_histogram_buckets(BIGINT[]) (
    SFUNC = merge_histogram_buckets,
    STYPE = BIGINT[],
    INITCOND = '{}'
);

GRANT EXECUTE ON FUNCTION sum_histogram_buckets(BIGINT[]) TO maxi_loop_app, maxi_loop_worker;

-- 2. 1-Minute Aggregated Transaction Metrics (leading tenant_id + project_id on PK and indexes)
CREATE TABLE IF NOT EXISTS transaction_minute_metrics (
    tenant_id BIGINT NOT NULL,
    project_id BIGINT NOT NULL,
    transaction_name TEXT NOT NULL CHECK (char_length(btrim(transaction_name)) BETWEEN 1 AND 512),
    op TEXT NOT NULL DEFAULT '' CHECK (char_length(op) <= 128),
    environment TEXT NOT NULL DEFAULT 'production' CHECK (char_length(btrim(environment)) BETWEEN 1 AND 64),
    release TEXT NOT NULL DEFAULT '' CHECK (char_length(release) <= 128),
    minute_bucket TIMESTAMPTZ NOT NULL,
    total_count BIGINT NOT NULL DEFAULT 0 CHECK (total_count >= 0),
    error_count BIGINT NOT NULL DEFAULT 0 CHECK (error_count >= 0),
    duration_sum_ms DOUBLE PRECISION NOT NULL DEFAULT 0 CHECK (duration_sum_ms >= 0),
    duration_min_ms DOUBLE PRECISION NOT NULL DEFAULT 0 CHECK (duration_min_ms >= 0),
    duration_max_ms DOUBLE PRECISION NOT NULL DEFAULT 0 CHECK (duration_max_ms >= 0),
    histogram_buckets BIGINT[] NOT NULL DEFAULT ARRAY[]::BIGINT[],
    PRIMARY KEY (tenant_id, project_id, transaction_name, op, environment, release, minute_bucket),
    FOREIGN KEY (tenant_id, project_id) REFERENCES projects(tenant_id, id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS tx_metrics_tenant_project_bucket_idx
    ON transaction_minute_metrics (tenant_id, project_id, minute_bucket DESC);
CREATE INDEX IF NOT EXISTS tx_metrics_tenant_bucket_idx
    ON transaction_minute_metrics (tenant_id, minute_bucket DESC);

-- 3. Sampled Full Traces (daily-partitioned by received_at; zstd-compressed span blobs)
CREATE TABLE IF NOT EXISTS traces (
    tenant_id BIGINT NOT NULL,
    project_id BIGINT NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    event_id TEXT NOT NULL CHECK (char_length(btrim(event_id)) BETWEEN 1 AND 128),
    trace_id TEXT NOT NULL CHECK (char_length(btrim(trace_id)) BETWEEN 1 AND 64),
    root_span_id TEXT NOT NULL CHECK (char_length(btrim(root_span_id)) BETWEEN 1 AND 64),
    parent_span_id TEXT CHECK (parent_span_id IS NULL OR char_length(btrim(parent_span_id)) BETWEEN 1 AND 64),
    transaction_name TEXT NOT NULL CHECK (char_length(btrim(transaction_name)) BETWEEN 1 AND 512),
    op TEXT NOT NULL DEFAULT '' CHECK (char_length(op) <= 128),
    status TEXT NOT NULL DEFAULT 'ok' CHECK (char_length(status) <= 64),
    is_error BOOLEAN NOT NULL DEFAULT FALSE,
    sample_reason TEXT NOT NULL CHECK (sample_reason IN ('slow', 'error', 'random')),
    environment TEXT NOT NULL DEFAULT 'production' CHECK (char_length(btrim(environment)) BETWEEN 1 AND 64),
    release TEXT CHECK (release IS NULL OR char_length(release) <= 128),
    platform TEXT NOT NULL DEFAULT 'other' CHECK (char_length(platform) <= 64),
    start_timestamp TIMESTAMPTZ NOT NULL,
    end_timestamp TIMESTAMPTZ NOT NULL,
    duration_ms DOUBLE PRECISION NOT NULL DEFAULT 0 CHECK (duration_ms >= 0),
    span_count INTEGER NOT NULL DEFAULT 0 CHECK (span_count >= 0),
    tags JSONB NOT NULL DEFAULT '{}'::jsonb,
    spans_zstd BYTEA NOT NULL,
    PRIMARY KEY (tenant_id, project_id, received_at, event_id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES projects(tenant_id, id) ON DELETE CASCADE
) PARTITION BY RANGE (received_at);

CREATE INDEX IF NOT EXISTS traces_tenant_trace_id_start_idx
    ON traces (tenant_id, trace_id, start_timestamp ASC);
CREATE INDEX IF NOT EXISTS traces_tenant_project_tx_received_idx
    ON traces (tenant_id, project_id, transaction_name, received_at DESC);
CREATE INDEX IF NOT EXISTS traces_tenant_project_received_idx
    ON traces (tenant_id, project_id, received_at DESC);

-- Enable & Force Row-Level Security on both Phase 3 tables
DO $$
DECLARE
    tbl TEXT;
BEGIN
    FOREACH tbl IN ARRAY ARRAY[
        'transaction_minute_metrics',
        'traces'
    ]
    LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', tbl);
        EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', tbl);

        EXECUTE format('DROP POLICY IF EXISTS tenant_isolation_policy ON %I', tbl);
        EXECUTE format(
            'CREATE POLICY tenant_isolation_policy ON %I AS PERMISSIVE FOR ALL TO maxi_loop_app '
            'USING (tenant_id = app_current_tenant_id()) '
            'WITH CHECK (tenant_id = app_current_tenant_id())',
            tbl
        );

        EXECUTE format('DROP POLICY IF EXISTS worker_cross_tenant_policy ON %I', tbl);
        EXECUTE format(
            'CREATE POLICY worker_cross_tenant_policy ON %I AS PERMISSIVE FOR ALL TO maxi_loop_worker '
            'USING (true) '
            'WITH CHECK (tenant_id > 0)',
            tbl
        );

        EXECUTE format('GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE %I TO maxi_loop_app, maxi_loop_worker', tbl);
    END LOOP;
END
$$;

-- Daily partition helper for `traces` (idempotent, applies RLS + policies + grants to child partition)
CREATE OR REPLACE FUNCTION ensure_traces_partition(target_date DATE)
RETURNS TEXT
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public
AS $$
DECLARE
    part_name TEXT;
    start_ts TIMESTAMPTZ;
    end_ts TIMESTAMPTZ;
BEGIN
    IF target_date IS NULL THEN
        RAISE EXCEPTION 'target_date must not be null';
    END IF;

    part_name := format('traces_%s', to_char(target_date, 'YYYY_MM_DD'));
    start_ts := (target_date::timestamp AT TIME ZONE 'UTC');
    end_ts := ((target_date + 1)::timestamp AT TIME ZONE 'UTC');

    EXECUTE format(
        'CREATE TABLE IF NOT EXISTS %I PARTITION OF traces FOR VALUES FROM (%L) TO (%L)',
        part_name,
        start_ts,
        end_ts
    );

    EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', part_name);
    EXECUTE format('ALTER TABLE %I FORCE ROW LEVEL SECURITY', part_name);

    IF NOT EXISTS (
        SELECT 1 FROM pg_policies
        WHERE schemaname = 'public' AND tablename = part_name AND policyname = 'tenant_isolation_policy'
    ) THEN
        EXECUTE format(
            'CREATE POLICY tenant_isolation_policy ON %I AS PERMISSIVE FOR ALL TO maxi_loop_app '
            'USING (tenant_id = app_current_tenant_id()) '
            'WITH CHECK (tenant_id = app_current_tenant_id())',
            part_name
        );
    END IF;

    IF NOT EXISTS (
        SELECT 1 FROM pg_policies
        WHERE schemaname = 'public' AND tablename = part_name AND policyname = 'worker_cross_tenant_policy'
    ) THEN
        EXECUTE format(
            'CREATE POLICY worker_cross_tenant_policy ON %I AS PERMISSIVE FOR ALL TO maxi_loop_worker '
            'USING (true) '
            'WITH CHECK (tenant_id > 0)',
            part_name
        );
    END IF;

    EXECUTE format(
        'GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE %I TO maxi_loop_app, maxi_loop_worker',
        part_name
    );

    RETURN part_name;
END;
$$;

REVOKE ALL ON FUNCTION ensure_traces_partition(DATE) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION ensure_traces_partition(DATE) TO maxi_loop_worker;

-- Drop expired daily child partitions of `public.traces` older than `cutoff_date`
CREATE OR REPLACE FUNCTION drop_expired_traces_partitions(cutoff_date DATE)
RETURNS TEXT[]
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = public
AS $$
DECLARE
    rec RECORD;
    part_date DATE;
    dropped TEXT[] := ARRAY[]::TEXT[];
BEGIN
    IF cutoff_date IS NULL THEN
        RAISE EXCEPTION 'cutoff_date must not be null';
    END IF;

    FOR rec IN
        SELECT c.relname::TEXT AS part_name
        FROM pg_inherits i
        JOIN pg_class c ON c.oid = i.inhrelid
        JOIN pg_class p ON p.oid = i.inhparent
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'public'
          AND p.relname = 'traces'
          AND c.relname ~ '^traces_[0-9]{4}_[0-9]{2}_[0-9]{2}$'
        ORDER BY c.relname ASC
    LOOP
        part_date := to_date(substring(rec.part_name FROM 8 FOR 10), 'YYYY_MM_DD');
        IF part_date < cutoff_date THEN
            EXECUTE format('DROP TABLE IF EXISTS public.%I', rec.part_name);
            dropped := array_append(dropped, rec.part_name);
        END IF;
    END LOOP;

    RETURN dropped;
END;
$$;

REVOKE ALL ON FUNCTION drop_expired_traces_partitions(DATE) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION drop_expired_traces_partitions(DATE) TO maxi_loop_worker;

-- Pre-create trace partitions around current UTC date (-1 day to +7 days)
DO $$
DECLARE
    day_offset INTEGER;
    utc_today DATE := (NOW() AT TIME ZONE 'UTC')::DATE;
BEGIN
    FOR day_offset IN -1..7 LOOP
        PERFORM ensure_traces_partition(utc_today + day_offset);
    END LOOP;
END
$$;

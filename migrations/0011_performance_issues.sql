-- 0011_performance_issues.sql
-- Performance Issues (F017): transaction span patterns (N+1 DB, N+1 API, slow DB query,
-- consecutive DB / HTTP) detected at ingest become regular rows in `issues`, distinguished by
-- `issue_type`. Backward-compatible: existing rows default to 'error'; RLS policies are inherited.

ALTER TABLE issues
    ADD COLUMN IF NOT EXISTS issue_type TEXT NOT NULL DEFAULT 'error';

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'issues_issue_type_chk' AND conrelid = 'issues'::regclass
    ) THEN
        ALTER TABLE issues
            ADD CONSTRAINT issues_issue_type_chk
            CHECK (issue_type IN (
                'error',
                'performance_n_plus_one_db',
                'performance_n_plus_one_api',
                'performance_slow_db_query',
                'performance_consecutive_db',
                'performance_consecutive_http'
            ));
    END IF;
END
$$;

CREATE INDEX IF NOT EXISTS issues_tenant_project_type_last_seen_idx
    ON issues (tenant_id, project_id, issue_type, last_seen DESC);

-- Transactions that trigger a performance issue are always kept as sampled traces so the
-- offending spans can be inspected in the waterfall (`sample_reason = 'issue'`).
-- Dropping the constraint on the partitioned parent also drops it from every partition.
ALTER TABLE traces DROP CONSTRAINT IF EXISTS traces_sample_reason_check;
ALTER TABLE traces
    ADD CONSTRAINT traces_sample_reason_check
    CHECK (sample_reason IN ('slow', 'error', 'random', 'issue'));

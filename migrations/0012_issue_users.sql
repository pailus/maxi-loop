-- 0012_issue_users.sql
-- Users affected per issue (F018):
--   - `instance_secrets`: per-installation random keys that never leave the database/worker. The
--     `user_hash_key` keys an HMAC-SHA256 over the event user identity so `issue_users` only ever
--     stores an opaque, tenant-scoped hash (no raw id / email / username / IP, no dictionary reversal).
--   - `issue_users`: one row per distinct hashed user per issue (exact distinct counts, bounded by the
--     user base; rows cascade away with their issue).
--   - `issues.user_count`: denormalized distinct-user count maintained by the ingest worker so the
--     Issues list can sort by user impact without aggregating.
--   - `events.user_hash`: hashed user of each stored full event so a split can carry its users along.
--   - `alert_rules.spike_metric`: spike rules can fire on distinct users instead of events.

-- 1. Installation secrets (worker read-only; never readable by the tenant API role).
CREATE TABLE IF NOT EXISTS instance_secrets (
    name TEXT PRIMARY KEY,
    secret BYTEA NOT NULL CHECK (octet_length(secret) >= 16),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 32 bytes from two `gen_random_uuid()` values (core PostgreSQL >= 13, CSPRNG-backed, 244 random bits).
INSERT INTO instance_secrets (name, secret)
VALUES (
    'user_hash_key',
    decode(
        replace(gen_random_uuid()::TEXT, '-', '') || replace(gen_random_uuid()::TEXT, '-', ''),
        'hex'
    )
)
ON CONFLICT (name) DO NOTHING;

REVOKE ALL ON TABLE instance_secrets FROM PUBLIC;
REVOKE ALL ON TABLE instance_secrets FROM maxi_loop_app;
REVOKE ALL ON TABLE instance_secrets FROM maxi_loop_worker;
GRANT SELECT ON TABLE instance_secrets TO maxi_loop_worker;

-- 2. Denormalized distinct-user count + impact sort index.
ALTER TABLE issues ADD COLUMN IF NOT EXISTS user_count BIGINT NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS issues_tenant_project_user_count_idx
    ON issues (tenant_id, project_id, user_count DESC, last_seen DESC);

-- 3. Hashed user of stored full events (propagates to every daily partition).
ALTER TABLE events ADD COLUMN IF NOT EXISTS user_hash BYTEA;

-- 4. Distinct hashed users per issue.
CREATE TABLE IF NOT EXISTS issue_users (
    tenant_id BIGINT NOT NULL,
    project_id BIGINT NOT NULL,
    issue_id BIGINT NOT NULL,
    user_hash BYTEA NOT NULL CHECK (octet_length(user_hash) BETWEEN 8 AND 64),
    first_seen TIMESTAMPTZ NOT NULL,
    last_seen TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, project_id, issue_id, user_hash),
    FOREIGN KEY (tenant_id, project_id, issue_id)
        REFERENCES issues(tenant_id, project_id, id) ON DELETE CASCADE
);

-- Spike evaluation counts users whose `last_seen` falls inside the window.
CREATE INDEX IF NOT EXISTS issue_users_tenant_issue_last_seen_idx
    ON issue_users (tenant_id, project_id, issue_id, last_seen DESC);

ALTER TABLE issue_users ENABLE ROW LEVEL SECURITY;
ALTER TABLE issue_users FORCE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS tenant_isolation_policy ON issue_users;
CREATE POLICY tenant_isolation_policy ON issue_users
    AS PERMISSIVE FOR ALL TO maxi_loop_app
    USING (tenant_id = app_current_tenant_id())
    WITH CHECK (tenant_id = app_current_tenant_id());

DROP POLICY IF EXISTS worker_cross_tenant_policy ON issue_users;
CREATE POLICY worker_cross_tenant_policy ON issue_users
    AS PERMISSIVE FOR ALL TO maxi_loop_worker
    USING (true)
    WITH CHECK (tenant_id > 0);

GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE issue_users TO maxi_loop_app, maxi_loop_worker;

-- 5. Spike rules on distinct users.
ALTER TABLE alert_rules
    ADD COLUMN IF NOT EXISTS spike_metric TEXT NOT NULL DEFAULT 'events';

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'alert_rules_spike_metric_chk' AND conrelid = 'alert_rules'::regclass
    ) THEN
        ALTER TABLE alert_rules
            ADD CONSTRAINT alert_rules_spike_metric_chk
            CHECK (spike_metric IN ('events', 'users'));
    END IF;
END
$$;

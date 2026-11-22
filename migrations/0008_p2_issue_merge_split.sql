-- 0008_p2_issue_merge_split.sql
-- P2 Issues (`F-ISS-9` manual merge & split):
--   - Add nullable `fingerprint` column on daily-partitioned `events` so merged events retain their
--     original fingerprint for accurate splitting later (backward compatible across all partitions).
--   - Add `issue_fingerprints` mapping merged/secondary fingerprints to their owning target issue so
--     future ingested envelopes automatically route into the merged target issue, and removing an
--     alias on split restores independent routing.
--   - Enforce Row-Level Security (`maxi_loop_app` tenant isolation + `maxi_loop_worker` ingest worker).

-- 1. Preserve original event fingerprint on stored events (propagates automatically to partitions)
ALTER TABLE events ADD COLUMN IF NOT EXISTS fingerprint TEXT;

-- 2. Secondary / merged fingerprint routing table
CREATE TABLE IF NOT EXISTS issue_fingerprints (
    tenant_id BIGINT NOT NULL,
    project_id BIGINT NOT NULL,
    fingerprint TEXT NOT NULL CHECK (char_length(btrim(fingerprint)) BETWEEN 1 AND 256),
    issue_id BIGINT NOT NULL,
    original_title TEXT NOT NULL DEFAULT '',
    original_culprit TEXT NOT NULL DEFAULT '',
    is_primary BOOLEAN NOT NULL DEFAULT FALSE,
    merged_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, project_id, fingerprint),
    FOREIGN KEY (tenant_id, project_id, issue_id)
        REFERENCES issues(tenant_id, project_id, id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS issue_fingerprints_tenant_project_issue_idx
    ON issue_fingerprints (tenant_id, project_id, issue_id, is_primary DESC, merged_at ASC);

-- Enable & Force Row-Level Security
ALTER TABLE issue_fingerprints ENABLE ROW LEVEL SECURITY;
ALTER TABLE issue_fingerprints FORCE ROW LEVEL SECURITY;

DROP POLICY IF EXISTS tenant_isolation_policy ON issue_fingerprints;
CREATE POLICY tenant_isolation_policy ON issue_fingerprints
    AS PERMISSIVE FOR ALL TO maxi_loop_app
    USING (tenant_id = app_current_tenant_id())
    WITH CHECK (tenant_id = app_current_tenant_id());

DROP POLICY IF EXISTS worker_cross_tenant_policy ON issue_fingerprints;
CREATE POLICY worker_cross_tenant_policy ON issue_fingerprints
    AS PERMISSIVE FOR ALL TO maxi_loop_worker
    USING (true)
    WITH CHECK (tenant_id > 0);

GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE issue_fingerprints TO maxi_loop_app, maxi_loop_worker;

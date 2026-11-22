-- 0016_project_limited_memberships.sql
-- Stage B: a workspace-scoped `member`/`viewer` membership may be limited to specific projects
-- of that workspace. The explicit `project_limited` flag (not "has rows") marks a limited grant,
-- so deleting the last allowed project leaves the member with no projects instead of silently
-- widening access to the whole workspace.

ALTER TABLE memberships
    ADD COLUMN IF NOT EXISTS project_limited BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE invitations
    ADD COLUMN IF NOT EXISTS project_limited BOOLEAN NOT NULL DEFAULT FALSE;

CREATE TABLE IF NOT EXISTS membership_projects (
    tenant_id BIGINT NOT NULL,
    membership_id BIGINT NOT NULL,
    project_id BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, membership_id, project_id),
    FOREIGN KEY (tenant_id, membership_id) REFERENCES memberships(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, project_id) REFERENCES projects(tenant_id, id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS membership_projects_project_idx
    ON membership_projects (tenant_id, project_id);

-- Projects an invitation will grant (copied to membership_projects on accept).
CREATE TABLE IF NOT EXISTS invitation_projects (
    tenant_id BIGINT NOT NULL,
    invitation_id BIGINT NOT NULL,
    project_id BIGINT NOT NULL,
    PRIMARY KEY (tenant_id, invitation_id, project_id),
    FOREIGN KEY (tenant_id, invitation_id) REFERENCES invitations(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, project_id) REFERENCES projects(tenant_id, id) ON DELETE CASCADE
);

DO $$
DECLARE
    tbl TEXT;
BEGIN
    FOREACH tbl IN ARRAY ARRAY['membership_projects', 'invitation_projects']
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
            'USING (true) WITH CHECK (tenant_id > 0)',
            tbl
        );
        EXECUTE format(
            'GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE %I TO maxi_loop_app, maxi_loop_worker',
            tbl
        );
    END LOOP;
END
$$;

-- Eligibility rule from 0015, now honouring project limits on workspace memberships.
CREATE OR REPLACE FUNCTION user_can_triage_project(
    p_tenant_id BIGINT,
    p_user_id BIGINT,
    p_project_id BIGINT
) RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1
        FROM memberships m
        JOIN projects p ON p.tenant_id = m.tenant_id AND p.id = p_project_id
        JOIN users u ON u.id = m.user_id
        WHERE m.tenant_id = p_tenant_id
          AND m.user_id = p_user_id
          AND u.is_active
          AND m.role IN ('tenant_owner', 'tenant_admin', 'workspace_admin', 'member')
          AND (
              m.workspace_id IS NULL
              OR (
                  m.workspace_id = p.workspace_id
                  AND (
                      NOT m.project_limited
                      OR EXISTS (
                          SELECT 1 FROM membership_projects mp
                          WHERE mp.tenant_id = m.tenant_id AND mp.membership_id = m.id
                            AND mp.project_id = p_project_id
                      )
                  )
              )
          )
    )
$$;

-- Re-create the 0015 trigger so turning on `project_limited` also re-checks assignments.
DROP TRIGGER IF EXISTS memberships_unassign_issues ON memberships;
CREATE TRIGGER memberships_unassign_issues
    AFTER DELETE OR UPDATE OF role, workspace_id, project_limited ON memberships
    FOR EACH ROW EXECUTE FUNCTION unassign_issues_after_membership_change();

-- Removing an allowed project narrows access: drop assignments that no longer qualify.
CREATE OR REPLACE FUNCTION unassign_issues_after_project_limit_change()
RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    v_tenant BIGINT := COALESCE(NEW.tenant_id, OLD.tenant_id);
    v_membership BIGINT := COALESCE(NEW.membership_id, OLD.membership_id);
    v_user BIGINT;
BEGIN
    SELECT user_id INTO v_user FROM memberships WHERE tenant_id = v_tenant AND id = v_membership;
    IF v_user IS NULL THEN
        RETURN NULL;
    END IF;
    UPDATE issues i
    SET assignee_user_id = NULL, updated_at = NOW()
    WHERE i.tenant_id = v_tenant
      AND i.assignee_user_id = v_user
      AND NOT user_can_triage_project(i.tenant_id, v_user, i.project_id);
    RETURN NULL;
END
$$;

DROP TRIGGER IF EXISTS membership_projects_unassign_issues ON membership_projects;
CREATE TRIGGER membership_projects_unassign_issues
    AFTER INSERT OR DELETE ON membership_projects
    FOR EACH ROW EXECUTE FUNCTION unassign_issues_after_project_limit_change();

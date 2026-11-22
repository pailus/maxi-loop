-- 0015_issue_assignment.sql
-- F024: one optional assignee per issue.

ALTER TABLE issues
    ADD COLUMN IF NOT EXISTS assignee_user_id BIGINT REFERENCES users(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS issues_tenant_assignee_idx
    ON issues (tenant_id, assignee_user_id, last_seen DESC)
    WHERE assignee_user_id IS NOT NULL;

-- Single source of truth for "who may work on (and be assigned to) issues of this project":
-- an active user with a triage-capable role at tenant level or in the project's workspace.
-- Used by the assign API, the assignable-members list and the unassign trigger below, so a
-- future per-project access rule only has to change this function.
-- SECURITY INVOKER (default): runs under the caller's RLS tenant context.
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
          AND (m.workspace_id IS NULL OR m.workspace_id = p.workspace_id)
    )
$$;

-- When a membership is removed or downgraded, drop assignments the user no longer qualifies for.
CREATE OR REPLACE FUNCTION unassign_issues_after_membership_change()
RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE issues i
    SET assignee_user_id = NULL, updated_at = NOW()
    WHERE i.tenant_id = OLD.tenant_id
      AND i.assignee_user_id = OLD.user_id
      AND NOT user_can_triage_project(i.tenant_id, OLD.user_id, i.project_id);
    RETURN NULL;
END
$$;

DROP TRIGGER IF EXISTS memberships_unassign_issues ON memberships;
CREATE TRIGGER memberships_unassign_issues
    AFTER DELETE OR UPDATE OF role, workspace_id ON memberships
    FOR EACH ROW EXECUTE FUNCTION unassign_issues_after_membership_change();

-- 0010_project_tags.sql
-- Add server-persisted project tags (`TEXT[]`) for project grouping in the sidebar & Projects view.
-- Backward-compatible default empty array; inherits `projects` table Row-Level Security policies.

ALTER TABLE projects
    ADD COLUMN IF NOT EXISTS tags TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[];

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname = 'projects_tags_max_len_chk'
          AND conrelid = 'projects'::regclass
    ) THEN
        ALTER TABLE projects
            ADD CONSTRAINT projects_tags_max_len_chk
            CHECK (COALESCE(array_length(tags, 1), 0) <= 10);
    END IF;
END
$$;

CREATE INDEX IF NOT EXISTS projects_tags_gin_idx
    ON projects USING GIN (tags);

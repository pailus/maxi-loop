-- 0013_performance_alerts.sql
-- Performance alerts (F019): metric rules evaluated every minute from `transaction_minute_metrics`.
--   - `perf_p95`:        p95 duration (ms) over the window  >  perf_threshold
--   - `perf_error_rate`: error_count / total_count (%)      >  perf_threshold
-- Scope = rule workspace/project (+ optional exact `transaction_name`; NULL = whole project).
-- `perf_min_count` suppresses noise from windows with too few transactions.
-- Deliveries reuse `alert_deliveries` (audit log + cooldown): perf deliveries have no issue, so
-- `issue_id` becomes nullable and the evaluated transaction/metric is recorded instead.

-- 1. Rule columns
ALTER TABLE alert_rules ADD COLUMN IF NOT EXISTS transaction_name TEXT;
ALTER TABLE alert_rules ADD COLUMN IF NOT EXISTS perf_threshold DOUBLE PRECISION;
ALTER TABLE alert_rules ADD COLUMN IF NOT EXISTS perf_window_minutes INTEGER;
ALTER TABLE alert_rules ADD COLUMN IF NOT EXISTS perf_min_count INTEGER NOT NULL DEFAULT 10;

ALTER TABLE alert_rules DROP CONSTRAINT IF EXISTS alert_rules_trigger_type_check;
ALTER TABLE alert_rules ADD CONSTRAINT alert_rules_trigger_type_check
    CHECK (trigger_type IN ('new_issue', 'regression', 'spike', 'perf_p95', 'perf_error_rate'));

-- Replace the per-trigger "required fields" check (auto-named in 0003).
ALTER TABLE alert_rules DROP CONSTRAINT IF EXISTS alert_rules_check1;
ALTER TABLE alert_rules DROP CONSTRAINT IF EXISTS alert_rules_trigger_fields_chk;
ALTER TABLE alert_rules ADD CONSTRAINT alert_rules_trigger_fields_chk CHECK (
    (trigger_type = 'spike' AND spike_threshold IS NOT NULL AND spike_window_minutes IS NOT NULL)
    OR (trigger_type IN ('new_issue', 'regression'))
    OR (trigger_type = 'perf_p95'
        AND perf_threshold IS NOT NULL AND perf_threshold >= 1 AND perf_threshold <= 600000
        AND perf_window_minutes IS NOT NULL)
    OR (trigger_type = 'perf_error_rate'
        AND perf_threshold IS NOT NULL AND perf_threshold >= 0 AND perf_threshold < 100
        AND perf_window_minutes IS NOT NULL)
);

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'alert_rules_perf_fields_chk' AND conrelid = 'alert_rules'::regclass
    ) THEN
        ALTER TABLE alert_rules ADD CONSTRAINT alert_rules_perf_fields_chk CHECK (
            (transaction_name IS NULL OR char_length(btrim(transaction_name)) BETWEEN 1 AND 512)
            AND (perf_window_minutes IS NULL OR perf_window_minutes BETWEEN 1 AND 1440)
            AND perf_min_count BETWEEN 1 AND 1000000
        );
    END IF;
END
$$;

-- 2. Deliveries: issue-less performance alerts
ALTER TABLE alert_deliveries ALTER COLUMN issue_id DROP NOT NULL;
ALTER TABLE alert_deliveries ADD COLUMN IF NOT EXISTS transaction_name TEXT;
ALTER TABLE alert_deliveries ADD COLUMN IF NOT EXISTS metric_value DOUBLE PRECISION;
ALTER TABLE alert_deliveries ADD COLUMN IF NOT EXISTS subject TEXT
    CHECK (subject IS NULL OR char_length(subject) <= 512);

ALTER TABLE alert_deliveries DROP CONSTRAINT IF EXISTS alert_deliveries_trigger_type_check;
ALTER TABLE alert_deliveries ADD CONSTRAINT alert_deliveries_trigger_type_check
    CHECK (trigger_type IN ('new_issue', 'regression', 'spike', 'perf_p95', 'perf_error_rate'));

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'alert_deliveries_issue_or_perf_chk'
          AND conrelid = 'alert_deliveries'::regclass
    ) THEN
        ALTER TABLE alert_deliveries ADD CONSTRAINT alert_deliveries_issue_or_perf_chk CHECK (
            issue_id IS NOT NULL OR trigger_type IN ('perf_p95', 'perf_error_rate')
        );
    END IF;
END
$$;

-- Cooldown lookup for performance rules (no issue): rule + project + transaction.
CREATE INDEX IF NOT EXISTS alert_deliveries_perf_cooldown_idx
    ON alert_deliveries (tenant_id, rule_id, project_id, created_at DESC)
    WHERE issue_id IS NULL;

use std::time::Duration;

use maxi_loop_platform::config::Config;

#[derive(Clone, Copy, Debug)]
pub struct RetentionConfig {
    pub global_retention_days: u16,
    pub days_ahead: u16,
    pub batch_delete_limit: i64,
    pub max_batches_per_run: usize,
    pub schedule_interval: Duration,
}

impl RetentionConfig {
    pub fn from_config(config: &Config) -> Self {
        Self {
            global_retention_days: config.retention_default_days,
            days_ahead: 7,
            batch_delete_limit: 1_000,
            max_batches_per_run: 10,
            schedule_interval: Duration::from_secs(3600),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RetentionReport {
    pub ensured_partitions: Vec<String>,
    pub dropped_partitions: Vec<String>,
    pub ensured_trace_partitions: Vec<String>,
    pub dropped_trace_partitions: Vec<String>,
    pub override_deleted_events: u64,
    pub override_deleted_hourly_counters: u64,
    pub override_deleted_issues: u64,
    pub override_deleted_traces: u64,
    pub override_deleted_tx_metrics: u64,
    pub global_deleted_hourly_counters: u64,
    pub global_deleted_issues: u64,
    pub global_deleted_tx_metrics: u64,
    pub deleted_expired_sessions: u64,
}

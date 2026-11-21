use std::time::Duration;

use chrono::{Days, NaiveDate};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use crate::config::Config;
use crate::error::AppError;
use crate::tenant::begin_worker_tx;

/// Build a PostgreSQL connection pool with bounded acquire timeout.
pub async fn connect_db(config: &Config) -> Result<PgPool, AppError> {
    PgPoolOptions::new()
        .max_connections(config.db_max_connections)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&config.database_url)
        .await
        .map_err(|err| AppError::DatabaseConnect(err.to_string()))
}

/// Apply all embedded SQLx migrations in `migrations/`.
pub async fn run_migrations(pool: &PgPool) -> Result<(), AppError> {
    sqlx::migrate!("../../migrations")
        .run(pool)
        .await
        .map_err(|err| AppError::DatabaseMigration(err.to_string()))
}

/// Ensure a single daily partition `events_YYYY_MM_DD` exists with RLS policies attached.
pub async fn ensure_events_partition(
    pool: &PgPool,
    target_date: NaiveDate,
) -> Result<String, AppError> {
    let mut tx = begin_worker_tx(pool).await?;
    let partition_name: String = sqlx::query_scalar("SELECT ensure_events_partition($1)")
        .bind(target_date)
        .fetch_one(&mut *tx)
        .await
        .map_err(|err| {
            AppError::DatabaseQuery(format!("ensure_events_partition({target_date}): {err}"))
        })?;
    tx.commit()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("commit partition creation: {err}")))?;
    Ok(partition_name)
}

/// Ensure daily partitions exist for `[base_date, base_date + days_ahead]`.
pub async fn ensure_upcoming_events_partitions(
    pool: &PgPool,
    base_date: NaiveDate,
    days_ahead: u16,
) -> Result<Vec<String>, AppError> {
    let mut tx = begin_worker_tx(pool).await?;
    let mut created = Vec::with_capacity(usize::from(days_ahead) + 1);
    for offset in 0..=u64::from(days_ahead) {
        let date = base_date
            .checked_add_days(Days::new(offset))
            .ok_or_else(|| AppError::Validation("partition date overflow".to_owned()))?;
        let name: String = sqlx::query_scalar("SELECT ensure_events_partition($1)")
            .bind(date)
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| {
                AppError::DatabaseQuery(format!("ensure_events_partition({date}): {err}"))
            })?;
        created.push(name);
    }
    tx.commit().await.map_err(|err| {
        AppError::DatabaseQuery(format!("commit upcoming partitions creation: {err}"))
    })?;
    Ok(created)
}

/// Ensure daily trace partitions `traces_YYYY_MM_DD` exist for `[base_date, base_date + days_ahead]`.
pub async fn ensure_upcoming_traces_partitions(
    pool: &PgPool,
    base_date: NaiveDate,
    days_ahead: u16,
) -> Result<Vec<String>, AppError> {
    let mut tx = begin_worker_tx(pool).await?;
    let mut created = Vec::with_capacity(usize::from(days_ahead) + 1);
    for offset in 0..=u64::from(days_ahead) {
        let date = base_date
            .checked_add_days(Days::new(offset))
            .ok_or_else(|| AppError::Validation("partition date overflow".to_owned()))?;
        let name: String = sqlx::query_scalar("SELECT ensure_traces_partition($1)")
            .bind(date)
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| {
                AppError::DatabaseQuery(format!("ensure_traces_partition({date}): {err}"))
            })?;
        created.push(name);
    }
    tx.commit().await.map_err(|err| {
        AppError::DatabaseQuery(format!("commit upcoming traces partitions creation: {err}"))
    })?;
    Ok(created)
}

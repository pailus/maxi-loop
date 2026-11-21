use sqlx::{PgPool, Postgres, Transaction};

use crate::error::AppError;

/// Validated tenant scope required by all tenant-isolated service operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TenantContext {
    tenant_id: i64,
}

impl TenantContext {
    pub fn new(tenant_id: i64) -> Result<Self, AppError> {
        if tenant_id <= 0 {
            return Err(AppError::Validation(
                "tenant_id must be a positive integer".to_owned(),
            ));
        }
        Ok(Self { tenant_id })
    }

    pub fn tenant_id(self) -> i64 {
        self.tenant_id
    }
}

/// Begin a PostgreSQL transaction scoped to a single tenant using Row-Level Security.
///
/// Both `SET LOCAL ROLE maxi_loop_app` and `set_config('app.tenant_id', ..., true)`
/// are transaction-local, so the role and GUC automatically reset when the
/// transaction commits or rolls back before returning to the connection pool.
pub async fn begin_tenant_tx(
    pool: &PgPool,
    ctx: TenantContext,
) -> Result<Transaction<'_, Postgres>, AppError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("begin tenant transaction: {err}")))?;

    sqlx::query("SET LOCAL ROLE maxi_loop_app")
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("set local role maxi_loop_app: {err}")))?;

    let tenant_str = ctx.tenant_id().to_string();
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(&tenant_str)
        .execute(&mut *tx)
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("set local app.tenant_id: {err}")))?;

    Ok(tx)
}

/// Begin a PostgreSQL transaction using the dedicated cross-tenant `maxi_loop_worker` role.
///
/// Reserved for background ingest batch writers, partition maintenance, and system bootstrap.
pub async fn begin_worker_tx(pool: &PgPool) -> Result<Transaction<'_, Postgres>, AppError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|err| AppError::DatabaseQuery(format!("begin worker transaction: {err}")))?;

    sqlx::query("SET LOCAL ROLE maxi_loop_worker")
        .execute(&mut *tx)
        .await
        .map_err(|err| {
            AppError::DatabaseQuery(format!("set local role maxi_loop_worker: {err}"))
        })?;

    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_positive_tenant_id() {
        assert!(TenantContext::new(0).is_err());
        assert!(TenantContext::new(-5).is_err());
        let ctx = TenantContext::new(42).expect("positive tenant id");
        assert_eq!(ctx.tenant_id(), 42);
    }
}

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use maxi_loop_platform::bootstrap::{
    AdminUserSpec, BootstrapSpec, bootstrap_defaults, verify_password_argon2id,
};
use maxi_loop_platform::config::Config;
use maxi_loop_platform::db::{
    connect_db, ensure_events_partition, ensure_upcoming_events_partitions, run_migrations,
};
use maxi_loop_platform::tenant::{TenantContext, begin_tenant_tx, begin_worker_tx};
use sqlx::PgPool;

async fn test_pool() -> PgPool {
    let _ = dotenvy::dotenv();
    let config = Config::from_env().expect("DATABASE_URL must be configured for integration tests");
    let pool = connect_db(&config)
        .await
        .expect("should connect to test Postgres 16");
    run_migrations(&pool)
        .await
        .expect("migrations should apply cleanly");
    pool
}

#[tokio::test]
async fn schema_migrations_partitions_bootstrap_and_rls_isolation() {
    let pool = test_pool().await;

    // 1. Verify all 9 required tables exist and have Row-Level Security enabled + forced.
    let expected_tables = [
        "tenants",
        "workspaces",
        "projects",
        "users",
        "memberships",
        "sessions",
        "issues",
        "events",
        "issue_hourly_counters",
    ];
    for table in expected_tables {
        let row: Option<(bool, bool)> = sqlx::query_as(
            "SELECT relrowsecurity, relforcerowsecurity
             FROM pg_class c
             JOIN pg_namespace n ON n.oid = c.relnamespace
             WHERE n.nspname = 'public' AND c.relname = $1",
        )
        .bind(table)
        .fetch_optional(&pool)
        .await
        .expect("query pg_class");

        assert_eq!(
            row,
            Some((true, true)),
            "table '{table}' must exist with relrowsecurity=true and relforcerowsecurity=true"
        );
    }

    // 2. Verify daily partition creation is idempotent and creates child partition with RLS forced.
    let target_date = NaiveDate::from_ymd_opt(2026, 11, 25).expect("valid date");
    let part_name = ensure_events_partition(&pool, target_date)
        .await
        .expect("create daily partition");
    assert_eq!(part_name, "events_2026_11_25");

    let upcoming = ensure_upcoming_events_partitions(&pool, target_date, 2)
        .await
        .expect("create upcoming partitions");
    assert_eq!(
        upcoming,
        vec![
            "events_2026_11_25".to_owned(),
            "events_2026_11_26".to_owned(),
            "events_2026_11_27".to_owned(),
        ]
    );

    let part_rls: Option<(bool, bool)> = sqlx::query_as(
        "SELECT relrowsecurity, relforcerowsecurity
         FROM pg_class c
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE n.nspname = 'public' AND c.relname = $1",
    )
    .bind(&part_name)
    .fetch_optional(&pool)
    .await
    .expect("query partition pg_class");
    assert_eq!(part_rls, Some((true, true)));

    // 3. Verify idempotent bootstrap (default tenant, default workspace, and Argon2id admin user).
    let spec = BootstrapSpec {
        tenant_slug: "default".to_owned(),
        tenant_name: "Default Organization".to_owned(),
        retention_days: 30,
        workspace_slug: "default".to_owned(),
        workspace_name: "Default Workspace".to_owned(),
        admin_user: Some(AdminUserSpec {
            email: "admin-bootstrap-test@maxi-loop.local".to_owned(),
            password: "bootstrap_admin_password_987".to_owned(),
            name: "Bootstrap Admin".to_owned(),
        }),
    };

    let first = bootstrap_defaults(&pool, &spec)
        .await
        .expect("first bootstrap run");
    let second = bootstrap_defaults(&pool, &spec)
        .await
        .expect("second bootstrap run must be idempotent");

    assert_eq!(first.tenant_id, second.tenant_id);
    assert_eq!(first.workspace_id, second.workspace_id);
    assert_eq!(first.admin_user_id, second.admin_user_id);

    let admin_id = first.admin_user_id.expect("admin user created");
    let mut worker_tx = begin_worker_tx(&pool).await.expect("worker tx");
    let stored_hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
        .bind(admin_id)
        .fetch_one(&mut *worker_tx)
        .await
        .expect("fetch admin password hash");
    assert!(
        verify_password_argon2id("bootstrap_admin_password_987", &stored_hash)
            .expect("verify argon2id hash")
    );

    let admin_role: String = sqlx::query_scalar(
        "SELECT role FROM memberships WHERE tenant_id = $1 AND user_id = $2 AND workspace_id IS NULL",
    )
    .bind(first.tenant_id)
    .bind(admin_id)
    .fetch_one(&mut *worker_tx)
    .await
    .expect("fetch admin membership");
    assert_eq!(admin_role, "tenant_owner");

    // 4. Seed two isolated tenants (Tenant A and Tenant B) via worker role to test RLS isolation.
    // Clean up any leftover test tenants from previous runs first.
    sqlx::query("DELETE FROM tenants WHERE slug IN ('rls-tenant-a', 'rls-tenant-b')")
        .execute(&mut *worker_tx)
        .await
        .expect("cleanup previous rls test tenants");

    let tenant_a_id: i64 = sqlx::query_scalar(
        "INSERT INTO tenants (slug, name, retention_days, is_default)
         VALUES ('rls-tenant-a', 'RLS Tenant A', 30, FALSE)
         RETURNING id",
    )
    .fetch_one(&mut *worker_tx)
    .await
    .expect("insert tenant a");

    let tenant_b_id: i64 = sqlx::query_scalar(
        "INSERT INTO tenants (slug, name, retention_days, is_default)
         VALUES ('rls-tenant-b', 'RLS Tenant B', 30, FALSE)
         RETURNING id",
    )
    .fetch_one(&mut *worker_tx)
    .await
    .expect("insert tenant b");

    worker_tx.commit().await.expect("commit seeded tenants");

    let ctx_a = TenantContext::new(tenant_a_id).expect("valid ctx a");
    let ctx_b = TenantContext::new(tenant_b_id).expect("valid ctx b");

    let event_time: DateTime<Utc> = Utc.with_ymd_and_hms(2026, 11, 25, 14, 0, 0).unwrap();
    let hour_bucket: DateTime<Utc> = Utc.with_ymd_and_hms(2026, 11, 25, 14, 0, 0).unwrap();

    // Populate Tenant A data inside a Tenant A RLS transaction.
    let mut tx_a = begin_tenant_tx(&pool, ctx_a).await.expect("begin tx a");
    let ws_a_id: i64 = sqlx::query_scalar(
        "INSERT INTO workspaces (tenant_id, slug, name, is_default)
         VALUES ($1, 'ws-a', 'Workspace A', TRUE)
         RETURNING id",
    )
    .bind(tenant_a_id)
    .fetch_one(&mut *tx_a)
    .await
    .expect("insert ws a");

    let proj_a_id: i64 = sqlx::query_scalar(
        "INSERT INTO projects (tenant_id, workspace_id, slug, name, platform, dsn_public_key)
         VALUES ($1, $2, 'api-a', 'API A', 'python-fastapi', 'dsn_key_tenant_a_000001')
         RETURNING id",
    )
    .bind(tenant_a_id)
    .bind(ws_a_id)
    .fetch_one(&mut *tx_a)
    .await
    .expect("insert proj a");

    let issue_a_id: i64 = sqlx::query_scalar(
        "INSERT INTO issues (tenant_id, project_id, fingerprint, title, exception_type, exception_value)
         VALUES ($1, $2, 'fp-a-1', 'ValueError: invalid tenant a input', 'ValueError', 'invalid tenant a input')
         RETURNING id",
    )
    .bind(tenant_a_id)
    .bind(proj_a_id)
    .fetch_one(&mut *tx_a)
    .await
    .expect("insert issue a");

    sqlx::query(
        "INSERT INTO events (tenant_id, project_id, issue_id, event_id, received_at, environment, level)
         VALUES ($1, $2, $3, 'evt-a-001', $4, 'production', 'error')",
    )
    .bind(tenant_a_id)
    .bind(proj_a_id)
    .bind(issue_a_id)
    .bind(event_time)
    .execute(&mut *tx_a)
    .await
    .expect("insert event a into daily partition");

    sqlx::query(
        "INSERT INTO issue_hourly_counters (tenant_id, project_id, issue_id, hour_bucket, event_count)
         VALUES ($1, $2, $3, $4, 5)",
    )
    .bind(tenant_a_id)
    .bind(proj_a_id)
    .bind(issue_a_id)
    .bind(hour_bucket)
    .execute(&mut *tx_a)
    .await
    .expect("insert counter a");

    tx_a.commit().await.expect("commit tx a");

    // Populate Tenant B data inside a Tenant B RLS transaction.
    let mut tx_b = begin_tenant_tx(&pool, ctx_b).await.expect("begin tx b");
    let ws_b_id: i64 = sqlx::query_scalar(
        "INSERT INTO workspaces (tenant_id, slug, name, is_default)
         VALUES ($1, 'ws-b', 'Workspace B', TRUE)
         RETURNING id",
    )
    .bind(tenant_b_id)
    .fetch_one(&mut *tx_b)
    .await
    .expect("insert ws b");

    let proj_b_id: i64 = sqlx::query_scalar(
        "INSERT INTO projects (tenant_id, workspace_id, slug, name, platform, dsn_public_key)
         VALUES ($1, $2, 'api-b', 'API B', 'go', 'dsn_key_tenant_b_000002')
         RETURNING id",
    )
    .bind(tenant_b_id)
    .bind(ws_b_id)
    .fetch_one(&mut *tx_b)
    .await
    .expect("insert proj b");

    let issue_b_id: i64 = sqlx::query_scalar(
        "INSERT INTO issues (tenant_id, project_id, fingerprint, title, exception_type, exception_value)
         VALUES ($1, $2, 'fp-b-1', 'panic: nil deref in tenant b', 'panic', 'nil deref')
         RETURNING id",
    )
    .bind(tenant_b_id)
    .bind(proj_b_id)
    .fetch_one(&mut *tx_b)
    .await
    .expect("insert issue b");

    sqlx::query(
        "INSERT INTO events (tenant_id, project_id, issue_id, event_id, received_at, environment, level)
         VALUES ($1, $2, $3, 'evt-b-001', $4, 'production', 'fatal')",
    )
    .bind(tenant_b_id)
    .bind(proj_b_id)
    .bind(issue_b_id)
    .bind(event_time)
    .execute(&mut *tx_b)
    .await
    .expect("insert event b into daily partition");

    sqlx::query(
        "INSERT INTO issue_hourly_counters (tenant_id, project_id, issue_id, hour_bucket, event_count)
         VALUES ($1, $2, $3, $4, 9)",
    )
    .bind(tenant_b_id)
    .bind(proj_b_id)
    .bind(issue_b_id)
    .bind(hour_bucket)
    .execute(&mut *tx_b)
    .await
    .expect("insert counter b");

    // 5. Negative RLS test: inside Tenant B's transaction, attempting to insert a row claiming
    // `tenant_id = tenant_a_id` MUST be rejected by PostgreSQL RLS WITH CHECK.
    let cross_tenant_insert = sqlx::query(
        "INSERT INTO workspaces (tenant_id, slug, name, is_default)
         VALUES ($1, 'forged-ws', 'Forged Workspace', FALSE)",
    )
    .bind(tenant_a_id)
    .execute(&mut *tx_b)
    .await;
    assert!(
        cross_tenant_insert.is_err(),
        "RLS must reject inserting rows for another tenant_id"
    );

    tx_b.commit().await.expect("commit tx b");

    // 6. Verify strict read/mutation isolation from Tenant A's perspective (even without WHERE clauses!).
    let mut verify_a = begin_tenant_tx(&pool, ctx_a).await.expect("begin verify a");

    let visible_tenants: Vec<i64> = sqlx::query_scalar("SELECT id FROM tenants")
        .fetch_all(&mut *verify_a)
        .await
        .expect("select tenants as a");
    assert_eq!(visible_tenants, vec![tenant_a_id]);

    let visible_projects: Vec<i64> = sqlx::query_scalar("SELECT id FROM projects")
        .fetch_all(&mut *verify_a)
        .await
        .expect("select projects as a");
    assert_eq!(visible_projects, vec![proj_a_id]);

    let visible_issues: Vec<i64> = sqlx::query_scalar("SELECT id FROM issues")
        .fetch_all(&mut *verify_a)
        .await
        .expect("select issues as a");
    assert_eq!(visible_issues, vec![issue_a_id]);

    let visible_events: Vec<String> = sqlx::query_scalar("SELECT event_id FROM events")
        .fetch_all(&mut *verify_a)
        .await
        .expect("select events parent as a");
    assert_eq!(visible_events, vec!["evt-a-001".to_owned()]);

    // Direct query on the child partition `events_2026_11_25` must ALSO enforce RLS!
    let visible_partition_events: Vec<String> =
        sqlx::query_scalar("SELECT event_id FROM events_2026_11_25")
            .fetch_all(&mut *verify_a)
            .await
            .expect("select child partition directly as a");
    assert_eq!(visible_partition_events, vec!["evt-a-001".to_owned()]);

    let visible_counters: Vec<i64> =
        sqlx::query_scalar("SELECT event_count FROM issue_hourly_counters")
            .fetch_all(&mut *verify_a)
            .await
            .expect("select counters as a");
    assert_eq!(visible_counters, vec![5]);

    // Attempting to UPDATE or DELETE Tenant B's issue from Tenant A's transaction affects 0 rows.
    let updated_b = sqlx::query("UPDATE issues SET status = 'resolved' WHERE id = $1")
        .bind(issue_b_id)
        .execute(&mut *verify_a)
        .await
        .expect("attempt cross-tenant update")
        .rows_affected();
    assert_eq!(updated_b, 0);

    let deleted_b = sqlx::query("DELETE FROM issues WHERE id = $1")
        .bind(issue_b_id)
        .execute(&mut *verify_a)
        .await
        .expect("attempt cross-tenant delete")
        .rows_affected();
    assert_eq!(deleted_b, 0);

    verify_a.commit().await.expect("commit verify a");

    // 7. Verify fail-closed behavior if `maxi_loop_app` runs with unset or invalid `app.tenant_id`.
    let mut unset_tx = pool.begin().await.expect("begin raw tx");
    sqlx::query("SET LOCAL ROLE maxi_loop_app")
        .execute(&mut *unset_tx)
        .await
        .expect("set role maxi_loop_app");
    sqlx::query("SELECT set_config('app.tenant_id', '', true)")
        .execute(&mut *unset_tx)
        .await
        .expect("clear app.tenant_id");

    let leaked_issues: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM issues")
        .fetch_one(&mut *unset_tx)
        .await
        .expect("count issues with unset app.tenant_id");
    assert_eq!(
        leaked_issues, 0,
        "maxi_loop_app with unset app.tenant_id must fail closed (0 rows)"
    );
    unset_tx.rollback().await.expect("rollback unset tx");
}

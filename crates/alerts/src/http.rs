use axum::extract::{FromRef, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch};
use axum::{Json, Router};
use maxi_loop_auth::http::AuthState;
use maxi_loop_auth::model::AuthSession;
use maxi_loop_platform::error::AppError;
use serde::Deserialize;

use crate::model::{
    AlertChannelView, AlertDeliveryView, AlertRuleView, CreateAlertChannelInput,
    CreateAlertRuleInput, UpdateAlertChannelInput, UpdateAlertRuleInput,
};
use crate::service::{
    create_alert_channel, create_alert_rule, delete_alert_channel, delete_alert_rule,
    list_alert_channels, list_alert_deliveries, list_alert_rules, update_alert_channel,
    update_alert_rule,
};

pub fn router<S>() -> Router<S>
where
    AuthState: FromRef<S>,
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/alerts/channels",
            get(list_channels_handler).post(create_channel_handler),
        )
        .route(
            "/alerts/channels/{channel_id}",
            patch(update_channel_handler).delete(delete_channel_handler),
        )
        .route(
            "/alerts/rules",
            get(list_rules_handler).post(create_rule_handler),
        )
        .route(
            "/alerts/rules/{rule_id}",
            patch(update_rule_handler).delete(delete_rule_handler),
        )
        .route("/alerts/deliveries", get(list_deliveries_handler))
}

#[derive(Deserialize, Default)]
struct DeliveriesQuery {
    limit: Option<u32>,
}

async fn list_channels_handler(
    State(state): State<AuthState>,
    session: AuthSession,
) -> Result<Json<Vec<AlertChannelView>>, AppError> {
    let items = list_alert_channels(&state.db, session.tenant_ctx).await?;
    Ok(Json(items))
}

async fn create_channel_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Json(input): Json<CreateAlertChannelInput>,
) -> Result<(StatusCode, Json<AlertChannelView>), AppError> {
    // Production HTTP API ALWAYS enforces strict SSRF protection (allow_private_ips = false).
    let created =
        create_alert_channel(&state.db, session.tenant_ctx, session.role, input, false).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

async fn update_channel_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(channel_id): Path<i64>,
    Json(input): Json<UpdateAlertChannelInput>,
) -> Result<Json<AlertChannelView>, AppError> {
    let updated = update_alert_channel(
        &state.db,
        session.tenant_ctx,
        session.role,
        channel_id,
        input,
    )
    .await?;
    Ok(Json(updated))
}

async fn delete_channel_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(channel_id): Path<i64>,
) -> Result<StatusCode, AppError> {
    delete_alert_channel(&state.db, session.tenant_ctx, session.role, channel_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_rules_handler(
    State(state): State<AuthState>,
    session: AuthSession,
) -> Result<Json<Vec<AlertRuleView>>, AppError> {
    let items = list_alert_rules(&state.db, session.tenant_ctx).await?;
    Ok(Json(items))
}

async fn create_rule_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Json(input): Json<CreateAlertRuleInput>,
) -> Result<(StatusCode, Json<AlertRuleView>), AppError> {
    let created = create_alert_rule(&state.db, session.tenant_ctx, session.role, input).await?;
    Ok((StatusCode::CREATED, Json(created)))
}

async fn update_rule_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(rule_id): Path<i64>,
    Json(input): Json<UpdateAlertRuleInput>,
) -> Result<Json<AlertRuleView>, AppError> {
    let updated =
        update_alert_rule(&state.db, session.tenant_ctx, session.role, rule_id, input).await?;
    Ok(Json(updated))
}

async fn delete_rule_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Path(rule_id): Path<i64>,
) -> Result<StatusCode, AppError> {
    delete_alert_rule(&state.db, session.tenant_ctx, session.role, rule_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_deliveries_handler(
    State(state): State<AuthState>,
    session: AuthSession,
    Query(query): Query<DeliveriesQuery>,
) -> Result<Json<Vec<AlertDeliveryView>>, AppError> {
    let items = list_alert_deliveries(&state.db, session.tenant_ctx, query.limit).await?;
    Ok(Json(items))
}

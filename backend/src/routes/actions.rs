use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::Deserialize;

use crate::error::AppError;
use crate::services::AppState;

#[derive(Debug, Deserialize)]
pub struct ActionQuery {
    pub status: Option<String>,
    pub action_type: Option<String>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/user/:address", get(get_user_actions))
        .route("/", get(list_actions))
}

/// Get actions needed for a user (e.g. sign order, approve USDC, redeem).
async fn get_user_actions(
    State(state): State<AppState>,
    Path(address): Path<String>,
    Query(query): Query<ActionQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let status = query.status.as_deref().unwrap_or("pending");

    let actions: Vec<ActionRow> = sqlx::query_as(
        r#"
        SELECT id, action_type, user_address, market_id, order_id, required_tx, payload, status, created_at
        FROM action_mapper
        WHERE user_address = $1 AND status = $2
        ORDER BY created_at DESC
        LIMIT 50
        "#,
    )
    .bind(&address)
    .bind(status)
    .fetch_all(&state.db)
    .await
    .map_err(AppError::Db)?;

    let actions_json: Vec<_> = actions
        .iter()
        .map(|a| serde_json::json!({
            "id": a.id,
            "actionType": a.action_type,
            "requiredTx": a.required_tx,
            "payload": a.payload,
            "status": a.status,
            "createdAt": a.created_at,
            "marketId": a.market_id,
            "orderId": a.order_id
        }))
        .collect();

    Ok(Json(serde_json::json!({
        "address": address,
        "actions": actions_json
    })))
}

/// List all pending actions (admin/debug).
async fn list_actions(
    State(state): State<AppState>,
    Query(query): Query<ActionQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let status = query.status.as_deref().unwrap_or("pending");

    let actions: Vec<ActionRow> = if let Some(at) = &query.action_type {
        sqlx::query_as(
            r#"
            SELECT id, action_type, user_address, market_id, order_id, required_tx, payload, status, created_at
            FROM action_mapper
            WHERE status = $1 AND action_type = $2
            ORDER BY created_at DESC
            LIMIT 100
            "#,
        )
        .bind(status)
        .bind(at)
        .fetch_all(&state.db)
        .await
        .map_err(AppError::Db)?
    } else {
        sqlx::query_as(
            r#"
            SELECT id, action_type, user_address, market_id, order_id, required_tx, payload, status, created_at
            FROM action_mapper
            WHERE status = $1
            ORDER BY created_at DESC
            LIMIT 100
            "#,
        )
        .bind(status)
        .fetch_all(&state.db)
        .await
        .map_err(AppError::Db)?
    };

    let actions_json: Vec<_> = actions
        .iter()
        .map(|a| serde_json::json!({
            "id": a.id,
            "actionType": a.action_type,
            "userAddress": a.user_address,
            "marketId": a.market_id,
            "orderId": a.order_id,
            "requiredTx": a.required_tx,
            "payload": a.payload,
            "status": a.status,
            "createdAt": a.created_at
        }))
        .collect();

    Ok(Json(serde_json::json!({
        "actions": actions_json
    })))
}

#[derive(sqlx::FromRow)]
struct ActionRow {
    id: i32,
    action_type: String,
    user_address: String,
    market_id: Option<i32>,
    order_id: Option<i32>,
    required_tx: String,
    payload: Option<serde_json::Value>,
    status: String,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
}

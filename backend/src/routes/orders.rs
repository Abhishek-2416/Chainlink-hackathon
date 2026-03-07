use axum::{
    extract::{Path, Query, State},
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::error::AppError;
use crate::services::AppState;
use crate::types::{OrderRequest, SubmitOrderRequest};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/quote", post(quote))
        .route("/", post(submit_order))
        .route("/:market_id", get(get_orderbook))
        .route("/user/:address", get(get_user_orders))
        .route("/:order_id/cancel", delete(cancel_order))
}

#[derive(Debug, Deserialize)]
pub struct UserOrdersQuery {
    pub market_id: Option<i32>,
}

/// POST /api/orders/quote
/// Returns the EIP-712 payload for the user to sign. No DB write yet.
async fn quote(
    State(state): State<AppState>,
    Json(req): Json<OrderRequest>,
) -> Result<Json<crate::types::QuoteResponse>, AppError> {
    let quote = state
        .orders
        .create_quote(
            req.market_id,
            &req.token,
            req.cost,
            &req.user_address,
            req.recipient_address.as_deref(),
            &state.blockchain,
            &state.orderbook,
        )
        .await?;

    // Record that this user is about to sign — useful for frontend UX tracking
    state
        .orders
        .record_action(
            "order_quote",
            &req.user_address,
            Some(req.market_id),
            None,
            "sign_eip712",
            Some(serde_json::json!({
                "marketId": req.market_id,
                "token":    req.token,
                "cost":     req.cost,
                "priceCents": quote.order.price_cents
            })),
        )
        .await
        .ok(); // non-fatal

    Ok(Json(quote))
}

/// POST /api/orders
/// Submit a signed order. Backend stores it then immediately calls fillOrder.
async fn submit_order(
    State(state): State<AppState>,
    Json(req): Json<SubmitOrderRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (order_id, status, tx_hash) = state
        .orders
        .submit_order(
            req.market_id,
            &req.token,
            req.shares,
            req.cost,
            req.price,
            req.nonce,
            req.deadline,
            &req.signature,
            &req.user_address,
            req.recipient_address.as_deref(),
            &state.blockchain,
            &state.orderbook,
        )
        .await?;

    state
        .orders
        .record_action(
            "order_submit",
            &req.user_address,
            Some(req.market_id),
            Some(order_id),
            "fillOrder",
            Some(serde_json::json!({
                "orderId": order_id,
                "token":   req.token,
                "shares":  req.shares,
                "cost":    req.cost,
                "price":   req.price,
                "txHash":  tx_hash
            })),
        )
        .await
        .ok();

    let mut resp = serde_json::json!({
        "orderId": order_id,
        "status":  status,
        "shares":  req.shares,
        "cost":    req.cost,
        "price":   req.price
    });
    if let Some(tx) = tx_hash {
        resp["txHash"] = serde_json::Value::String(tx);
    }
    Ok(Json(resp))
}

/// GET /api/orders/:market_id
/// Live orderbook: aggregated YES/NO bids + implied prices.
async fn get_orderbook(
    State(state): State<AppState>,
    Path(market_id): Path<i32>,
) -> Result<Json<crate::types::OrderbookResponse>, AppError> {
    let ob = state.orderbook.get_orderbook(market_id).await?;
    Ok(Json(ob))
}

/// GET /api/orders/user/:address?market_id=...
/// All orders for a user with their total value (cost sum).
async fn get_user_orders(
    State(state): State<AppState>,
    Path(address): Path<String>,
    Query(query): Query<UserOrdersQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let orders = state
        .orders
        .get_user_orders(&address, query.market_id)
        .await?;

    let total_cost: i64 = orders.iter().map(|o| o.cost).sum();
    let total_shares: i64 = orders.iter().map(|o| o.shares).sum();
    let filled_count = orders.iter().filter(|o| o.status == "filled").count();

    Ok(Json(serde_json::json!({
        "address":      address,
        "orders":       orders,
        "totalCost":    total_cost,
        "totalShares":  total_shares,
        "filledCount":  filled_count,
        "pendingCount": orders.len() - filled_count
    })))
}

/// DELETE /api/orders/:order_id/cancel
/// Soft-cancel a pending order (on-chain nonce is NOT invalidated here;
/// the order simply won't be batch-filled by the backend).
async fn cancel_order(
    State(state): State<AppState>,
    Path(order_id): Path<i32>,
) -> Result<Json<serde_json::Value>, AppError> {
    let rows_affected = sqlx::query(
        "UPDATE orders SET status = 'cancelled' WHERE id = $1 AND status = 'pending'",
    )
    .bind(order_id)
    .execute(&state.db)
    .await
    .map_err(AppError::Db)?
    .rows_affected();

    if rows_affected == 0 {
        return Err(AppError::OrderNotFound(order_id));
    }

    Ok(Json(serde_json::json!({
        "orderId": order_id,
        "status":  "cancelled"
    })))
}

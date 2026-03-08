use axum::{
    extract::{Path, Query, State},
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use tracing::instrument;

use crate::error::AppError;
use crate::services::AppState;
use crate::types::{BatchFillRequest, OrderRequest, SubmitOrderRequest};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/quote", post(quote))
        .route("/", post(submit_order))
        .route("/batch", post(batch_fill))
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
#[instrument(skip(state, req), fields(market_id = req.market_id, token = %req.token))]
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

    tracing::info!(market_id = req.market_id, token = %req.token, price_cents = quote.order.price_cents, "quote created");
    Ok(Json(quote))
}

/// POST /api/orders
/// Submit a signed order. Backend stores it then immediately calls fillOrder.
#[instrument(skip(state, req), fields(market_id = req.market_id, user = %req.user_address))]
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
        resp["txHash"] = serde_json::Value::String(tx.clone());
        tracing::info!(order_id = order_id, market_id = req.market_id, tx_hash = %tx, "order filled");
    } else {
        tracing::info!(order_id = order_id, market_id = req.market_id, "order queued pending");
    }
    Ok(Json(resp))
}

/// POST /api/orders/batch
/// Fill multiple pending orders in a single on-chain transaction for gas savings.
#[instrument(skip(state, req), fields(order_count = req.order_ids.len()))]
async fn batch_fill(
    State(state): State<AppState>,
    Json(req): Json<BatchFillRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if req.order_ids.is_empty() {
        return Err(AppError::BadRequest("order_ids cannot be empty".into()));
    }
    if req.order_ids.len() > 20 {
        return Err(AppError::BadRequest("Maximum 20 orders per batch".into()));
    }

    // Fetch all pending orders by ID
    let placeholders: Vec<String> = (1..=req.order_ids.len())
        .map(|i| format!("${}", i))
        .collect();
    let query_str = format!(
        "SELECT id, market_id, user_address, token, shares, cost, nonce, deadline, signature \
         FROM orders WHERE id IN ({}) AND status = 'pending'",
        placeholders.join(", ")
    );

    let mut query = sqlx::query_as::<_, BatchOrderRow>(&query_str);
    for id in &req.order_ids {
        query = query.bind(id);
    }
    let rows = query.fetch_all(&state.db).await.map_err(AppError::Db)?;

    if rows.is_empty() {
        return Err(AppError::BadRequest("No pending orders found for given IDs".into()));
    }

    // Contract requires nonces to be used sequentially per user. Sort by (user_address, nonce)
    // so that batch fill succeeds on-chain.
    let mut rows = rows;
    rows.sort_by(|a, b| {
        a.user_address
            .cmp(&b.user_address)
            .then_with(|| a.nonce.cmp(&b.nonce))
    });

    let mut contract_orders = Vec::with_capacity(rows.len());
    let mut signatures = Vec::with_capacity(rows.len());

    for row in &rows {
        let outcome = if row.token == "YES" { 1u8 } else { 2u8 };
        let order = state
            .blockchain
            .build_order(
                row.market_id as u64,
                outcome,
                &row.user_address,
                row.shares as u64,
                row.cost as u64,
                row.deadline as u64,
                row.nonce as u64,
            )
            .map_err(|e| AppError::InvalidOrder(format!("build_order: {e}")))?;
        contract_orders.push(order);
        signatures.push(ethers::types::Bytes::from(row.signature.clone()));
    }

    let tx_hash = state
        .blockchain
        .batch_fill_orders(contract_orders, signatures)
        .await
        .map_err(|e| AppError::Blockchain(format!("batchFillOrders tx failed: {e}")))?;

    let tx_hex = format!("{tx_hash:#x}");

    // Update all orders to filled
    for row in &rows {
        sqlx::query(
            "UPDATE orders SET status = 'filled', tx_hash = $1, filled_at = NOW() WHERE id = $2",
        )
        .bind(&tx_hex)
        .bind(row.id)
        .execute(&state.db)
        .await
        .map_err(AppError::Db)?;
    }

    tracing::info!(filled_count = rows.len(), tx_hash = %tx_hex, "batch fill completed");
    Ok(Json(serde_json::json!({
        "txHash":     tx_hex,
        "filledCount": rows.len(),
        "orderIds":   rows.iter().map(|r| r.id).collect::<Vec<_>>()
    })))
}

#[derive(Debug, sqlx::FromRow)]
struct BatchOrderRow {
    id: i32,
    market_id: i32,
    user_address: String,
    token: String,
    shares: i64,
    cost: i64,
    nonce: i64,
    deadline: i64,
    signature: Vec<u8>,
}

/// GET /api/orders/:market_id
/// Live orderbook: aggregated YES/NO bids + implied prices.
#[instrument(skip(state), fields(market_id = %market_id))]
async fn get_orderbook(
    State(state): State<AppState>,
    Path(market_id): Path<i32>,
) -> Result<Json<crate::types::OrderbookResponse>, AppError> {
    let ob = state.orderbook.get_orderbook(market_id).await?;
    Ok(Json(ob))
}

/// GET /api/orders/user/:address?market_id=...
/// All orders for a user with their total value (cost sum), market context, and buy prices.
#[instrument(skip(state), fields(address = %address))]
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

    let orders_json: Vec<_> = orders
        .iter()
        .map(|o| {
            let price_per_share_cents = if o.shares > 0 {
                ((o.cost * 100) / o.shares).clamp(1, 99)
            } else {
                o.price as i64
            };
            serde_json::json!({
                "id":              o.id,
                "marketId":        o.market_id,
                "marketQuestion":  o.market_question,
                "marketStatus":    o.market_status,
                "marketOutcome":   o.market_outcome,
                "token":           o.token,
                "shares":          o.shares,
                "cost":            o.cost,
                "price":           o.price,
                "pricePerShare":   price_per_share_cents,
                "status":          o.status,
                "createdAt":       o.created_at,
                "filledAt":        o.filled_at,
                "txHash":          o.tx_hash,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "address":      address,
        "orders":       orders_json,
        "totalCost":    total_cost,
        "totalShares":  total_shares,
        "filledCount":  filled_count,
        "pendingCount": orders.len() - filled_count
    })))
}

/// DELETE /api/orders/:order_id/cancel
/// Soft-cancel a pending order (on-chain nonce is NOT invalidated here;
/// the order simply won't be batch-filled by the backend).
#[instrument(skip(state), fields(order_id = %order_id))]
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

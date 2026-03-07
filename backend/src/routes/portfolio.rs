use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};

use crate::error::AppError;
use crate::services::orderbook::OrderbookService;
use crate::services::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/:address", get(get_portfolio))
        .route("/:address/history", get(get_trade_history))
        .route("/:address/redemption-status", get(get_redemption_status))
}

/// GET /api/portfolio/:address
/// Returns:
/// - positions (filled orders aggregated by market + token)
/// - open (pending) orders
/// - total cost of all positions
async fn get_portfolio(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Aggregate filled orders by (market_id, token) so we get one row per position
    let positions: Vec<PositionRow> = sqlx::query_as(
        r#"
        SELECT
            o.market_id,
            o.token,
            SUM(o.shares)::bigint                                AS total_shares,
            SUM(o.cost)::bigint                                  AS total_cost,
            (SUM(o.price * o.cost) / NULLIF(SUM(o.cost), 0))::int AS avg_buy_price,
            m.question,
            m.status                                             AS market_status,
            m.outcome                                            AS market_outcome,
            m.yes_token_address,
            m.no_token_address
        FROM orders o
        JOIN markets m ON m.market_id = o.market_id
        WHERE o.user_address = $1 AND o.status = 'filled'
        GROUP BY o.market_id, o.token, m.question, m.status, m.outcome,
                 m.yes_token_address, m.no_token_address
        ORDER BY o.market_id, o.token
        "#,
    )
    .bind(&address)
    .fetch_all(&state.db)
    .await
    .map_err(AppError::Db)?;

    let mut positions_json = Vec::with_capacity(positions.len());
    for p in &positions {
        let ob = state.orderbook.get_orderbook(p.market_id).await;
        let (yes_price, no_price) = ob.map(|o| (o.yes_price, o.no_price)).unwrap_or((50, 50));
        let current_price = if p.token == "YES" { yes_price } else { no_price };

        // PnL = current value − cost  (in USDC 6-decimal units)
        // current value = shares * current_price / 100
        let current_value = (p.total_shares * current_price as i64) / 100;
        let unrealized_pnl = current_value - p.total_cost;

        let can_redeem = p.market_status == "resolved"
            && p.market_outcome.as_deref() == Some(p.token.as_str());

        positions_json.push(serde_json::json!({
            "marketId":      p.market_id,
            "question":      p.question,
            "token":         p.token,
            "shares":        p.total_shares,
            "totalCost":     p.total_cost,
            "avgBuyPrice":   p.avg_buy_price,
            "currentPrice":  current_price,
            "currentValue":  current_value,
            "unrealizedPnl": unrealized_pnl,
            "marketStatus":  p.market_status,
            "marketOutcome": p.market_outcome,
            "canRedeem":     can_redeem,
            "winningToken":  if can_redeem {
                if p.token == "YES" { p.yes_token_address.clone() } else { p.no_token_address.clone() }
            } else { None }
        }));
    }

    // Open / pending orders
    let open_orders: Vec<OpenOrderRow> = sqlx::query_as(
        r#"
        SELECT id, market_id, token, shares, cost, price, created_at
        FROM orders
        WHERE user_address = $1 AND status = 'pending'
        ORDER BY created_at DESC
        "#,
    )
    .bind(&address)
    .fetch_all(&state.db)
    .await
    .map_err(AppError::Db)?;

    let open_orders_json: Vec<_> = open_orders
        .iter()
        .map(|o| serde_json::json!({
            "orderId":   o.id,
            "marketId":  o.market_id,
            "token":     o.token,
            "shares":    o.shares,
            "cost":      o.cost,
            "price":     o.price,
            "createdAt": o.created_at
        }))
        .collect();

    let total_cost: i64 = positions.iter().map(|p| p.total_cost).sum();
    let total_pnl: i64 = positions_json
        .iter()
        .map(|p| p["unrealizedPnl"].as_i64().unwrap_or(0))
        .sum();

    Ok(Json(serde_json::json!({
        "address":    address,
        "positions":  positions_json,
        "openOrders": open_orders_json,
        "totalCost":  total_cost,
        "totalPnl":   total_pnl
    })))
}

/// GET /api/portfolio/:address/history
/// Trade history for a user (from the trades table, watcher-populated).
async fn get_trade_history(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let trades: Vec<TradeRow> = sqlx::query_as(
        r#"
        SELECT t.id, t.market_id, m.question, t.token, t.shares,
               t.cost, t.tx_hash, t.created_at
        FROM trades t
        JOIN markets m ON m.market_id = t.market_id
        WHERE t.buyer_address = $1
        ORDER BY t.created_at DESC
        LIMIT 50
        "#,
    )
    .bind(&address)
    .fetch_all(&state.db)
    .await
    .map_err(AppError::Db)?;

    let trades_json: Vec<_> = trades
        .iter()
        .map(|t| {
            // price_cents = (cost / shares) * 100 — inverse of shares = cost * 100 / price
            let price_cents = OrderbookService::shares_to_price_cents(t.cost, t.shares);
            serde_json::json!({
                "tradeId":    t.id,
                "marketId":   t.market_id,
                "question":   t.question,
                "token":      t.token,
                "shares":     t.shares,
                "cost":       t.cost,
                "priceCents": price_cents,
                "timestamp":  t.created_at,
                "txHash":     t.tx_hash
            })
        })
        .collect();

    Ok(Json(serde_json::json!({ "trades": trades_json })))
}

/// GET /api/portfolio/:address/redemption-status
/// Only shows markets where:
///   1. Market is resolved
///   2. The user actually holds the WINNING token (they filled an order for it)
async fn get_redemption_status(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let contract_address = std::env::var("PREDICTION_MARKET_ADDRESS")
        .unwrap_or_else(|_| "0x45e7911Af8c31bDeDf8A586BeEd8efEcACEb9c37".into());

    // Join orders with markets: user must have a FILLED order on the winning side
    let redeemable: Vec<RedemptionRow> = sqlx::query_as(
        r#"
        SELECT
            m.market_id,
            m.question,
            m.outcome                            AS winning_outcome,
            SUM(o.shares)::bigint                AS redeemable_shares,
            SUM(o.cost)::bigint                  AS original_cost,
            m.yes_token_address,
            m.no_token_address
        FROM markets m
        JOIN orders o ON o.market_id = m.market_id
        WHERE m.status   = 'resolved'
          AND m.outcome  IS NOT NULL
          AND o.user_address = $1
          AND o.status   = 'filled'
          -- Only include the winning side
          AND o.token    = m.outcome
        GROUP BY m.market_id, m.question, m.outcome,
                 m.yes_token_address, m.no_token_address
        "#,
    )
    .bind(&address)
    .fetch_all(&state.db)
    .await
    .map_err(AppError::Db)?;

    let statuses: Vec<_> = redeemable
        .iter()
        .map(|r| {
            let winning_token = if r.winning_outcome.as_deref() == Some("YES") {
                r.yes_token_address.clone()
            } else {
                r.no_token_address.clone()
            };

            // Redemption value: 1 share = 1 USDC (6 decimals) — exactly matches contract
            let redemption_value = r.redeemable_shares;
            let profit = redemption_value - r.original_cost;

            serde_json::json!({
                "marketId":          r.market_id,
                "question":          r.question,
                "winningOutcome":    r.winning_outcome,
                "winningToken":      winning_token,
                "redeemableShares":  r.redeemable_shares,
                "redemptionValue":   redemption_value,
                "originalCost":      r.original_cost,
                "profit":            profit,
                "contractAddress":   contract_address,
                "howToRedeem": format!(
                    "Call redeemWinning({}, {}) on the PredictionMarket contract",
                    r.market_id,
                    r.redeemable_shares
                )
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "address":          address,
        "redeemableMarkets": statuses,
        "totalRedeemable":  redeemable.iter().map(|r| r.redeemable_shares).sum::<i64>(),
        "note": "Call redeemWinning(marketId, shares) on-chain. 1 winning share = 1 USDC."
    })))
}

// ── DB row types ──────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct PositionRow {
    market_id: i32,
    token: String,
    total_shares: i64,
    total_cost: i64,
    avg_buy_price: Option<i32>,
    question: String,
    market_status: String,
    market_outcome: Option<String>,
    yes_token_address: Option<String>,
    no_token_address: Option<String>,
}

#[derive(sqlx::FromRow)]
struct OpenOrderRow {
    id: i32,
    market_id: i32,
    token: String,
    shares: i64,
    cost: i64,
    price: i32,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(sqlx::FromRow)]
struct TradeRow {
    id: i32,
    market_id: i32,
    question: String,
    token: String,
    shares: i64,
    cost: i64,
    tx_hash: String,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(sqlx::FromRow)]
struct RedemptionRow {
    market_id: i32,
    question: String,
    winning_outcome: Option<String>,
    redeemable_shares: i64,
    original_cost: i64,
    yes_token_address: Option<String>,
    no_token_address: Option<String>,
}

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use tracing::instrument;

use crate::error::AppError;
use crate::services::orderbook::OrderbookService;
use crate::services::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/:address", get(get_portfolio))
        .route("/:address/positions", get(get_user_positions))
        .route("/:address/balance", get(get_balance))
        .route("/:address/history", get(get_trade_history))
        .route("/:address/redemption-status", get(get_redemption_status))
        .route("/:address/claim", post(claim_rewards))
}

/// GET /api/portfolio/:address
/// Returns:
/// - positions (filled orders aggregated by market + token)
/// - open (pending) orders
/// - total cost of all positions
#[instrument(skip(state), fields(address = %address))]
async fn get_portfolio(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let address = address.to_lowercase();
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

/// GET /api/portfolio/:address/balance
/// ETH and USDC balances for the address (from chain).
#[instrument(skip(state), fields(address = %address))]
async fn get_balance(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let address = address.to_lowercase();

    let usdc_address = std::env::var("MOCK_USDC_ADDRESS")
        .unwrap_or_else(|_| "0x805593711EdBd2F846035c654e0bF9C7A21dD907".into());

    let (eth_balance, usdc_balance) = tokio::try_join!(
        state.blockchain.get_eth_balance(&address),
        state.blockchain.get_erc20_balance(&usdc_address, &address),
    )
    .map_err(|e| AppError::Blockchain(format!("balance fetch: {e}")))?;

    // ETH: 18 decimals, USDC: 6 decimals
    Ok(Json(serde_json::json!({
        "address": address,
        "eth": eth_balance.to_string(),
        "usdc": usdc_balance.to_string(),
        "ethFormatted": (eth_balance.as_u128() as f64) / 1e18,
        "usdcFormatted": (usdc_balance.as_u128() as f64) / 1e6
    })))
}

/// GET /api/portfolio/:address/history
/// Trade history for a user (from the trades table, watcher-populated).
#[instrument(skip(state), fields(address = %address))]
async fn get_trade_history(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let address = address.to_lowercase();
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
#[instrument(skip(state), fields(address = %address))]
async fn get_redemption_status(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let address = address.to_lowercase();
    let contract_address = std::env::var("PREDICTION_MARKET_ADDRESS")
        .unwrap_or_else(|_| "0x45e7911Af8c31bDeDf8A586BeEd8efEcACEb9c37".into());

    // Use user_positions for redeemable_shares (decremented on redeem by watcher).
    // Fallback to orders sum if user_positions not populated.
    let redeemable: Vec<RedemptionRow> = sqlx::query_as(
        r#"
        SELECT
            m.market_id,
            m.question,
            m.outcome                                    AS winning_outcome,
            COALESCE(up.shares, SUM(o.shares))::bigint   AS redeemable_shares,
            SUM(o.cost)::bigint                          AS original_cost,
            m.yes_token_address,
            m.no_token_address
        FROM markets m
        JOIN orders o ON o.market_id = m.market_id
          AND o.user_address = $1 AND o.status = 'filled' AND o.token = m.outcome
        LEFT JOIN user_positions up ON up.market_id = m.market_id
          AND up.user_address = $1 AND up.token = m.outcome
        WHERE m.status = 'resolved' AND m.outcome IS NOT NULL
        GROUP BY m.market_id, m.question, m.outcome,
                 m.yes_token_address, m.no_token_address, up.shares
        HAVING COALESCE(up.shares, SUM(o.shares)) > 0
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

/// GET /api/portfolio/:address/positions
/// Detailed user positions across all markets with current prices and PnL.
#[instrument(skip(state), fields(address = %address))]
async fn get_user_positions(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let address = address.to_lowercase();

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
    let mut total_invested: i64 = 0;
    let mut total_current_value: i64 = 0;

    for p in &positions {
        let ob = state.orderbook.get_orderbook(p.market_id).await;
        let (yes_price, no_price) = ob.map(|o| (o.yes_price, o.no_price)).unwrap_or((50, 50));
        let current_price = if p.token == "YES" { yes_price } else { no_price };

        let current_value = (p.total_shares * current_price as i64) / 100;
        let unrealized_pnl = current_value - p.total_cost;

        let can_redeem = p.market_status == "resolved"
            && p.market_outcome.as_deref() == Some(p.token.as_str());

        let is_loser = p.market_status == "resolved"
            && p.market_outcome.is_some()
            && p.market_outcome.as_deref() != Some(p.token.as_str());

        let realized_pnl = if can_redeem {
            p.total_shares - p.total_cost
        } else if is_loser {
            -p.total_cost
        } else {
            0
        };

        total_invested += p.total_cost;
        total_current_value += if p.market_status == "resolved" {
            if can_redeem { p.total_shares } else { 0 }
        } else {
            current_value
        };

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
            "realizedPnl":   realized_pnl,
            "marketStatus":  p.market_status,
            "marketOutcome": p.market_outcome,
            "canRedeem":     can_redeem,
            "isLoser":       is_loser,
        }));
    }

    Ok(Json(serde_json::json!({
        "address":           address,
        "positions":         positions_json,
        "totalInvested":     total_invested,
        "totalCurrentValue": total_current_value,
        "totalPnl":          total_current_value - total_invested,
        "positionCount":     positions_json.len()
    })))
}

#[derive(Debug, Deserialize)]
pub struct ClaimRequest {
    pub market_id: i32,
}

/// POST /api/portfolio/:address/claim
/// Claim rewards for a resolved market. The user must hold the winning token.
/// Body: { "market_id": 3 }
#[instrument(skip(state, req), fields(address = %address, market_id = req.market_id))]
async fn claim_rewards(
    State(state): State<AppState>,
    Path(address): Path<String>,
    Json(req): Json<ClaimRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let address = address.to_lowercase();

    // Verify the market is resolved
    #[derive(sqlx::FromRow)]
    struct MarketInfo {
        status: String,
        outcome: Option<String>,
        yes_token_address: Option<String>,
        no_token_address: Option<String>,
    }

    let market: MarketInfo = sqlx::query_as(
        "SELECT status, outcome, yes_token_address, no_token_address FROM markets WHERE market_id = $1",
    )
    .bind(req.market_id)
    .fetch_optional(&state.db)
    .await
    .map_err(AppError::Db)?
    .ok_or(AppError::MarketNotFound(req.market_id))?;

    if market.status != "resolved" {
        return Err(AppError::BadRequest(format!(
            "Market {} is not resolved (status: {})",
            req.market_id, market.status
        )));
    }

    let winning_outcome = market.outcome.as_deref().ok_or_else(|| {
        AppError::BadRequest(format!("Market {} resolved but outcome is null", req.market_id))
    })?;

    // Use user_positions (decremented on-chain redeem by watcher) for accurate remaining balance.
    // Fallback to orders sum if user_positions not yet populated.
    let redeemable_shares: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT COALESCE(
            (SELECT shares FROM user_positions
             WHERE market_id = $1 AND user_address = $2 AND token = $3 AND shares > 0),
            (SELECT SUM(shares)::bigint FROM orders
             WHERE market_id = $1 AND user_address = $2 AND status = 'filled' AND token = $3)
        )
        "#,
    )
    .bind(req.market_id)
    .bind(&address)
    .bind(winning_outcome)
    .fetch_one(&state.db)
    .await
    .map_err(AppError::Db)?;

    let shares = redeemable_shares.unwrap_or(0);
    if shares <= 0 {
        return Err(AppError::BadRequest(format!(
            "No redeemable shares for market {}. You need {} tokens to redeem.",
            req.market_id, winning_outcome
        )));
    }

    let winning_token = if winning_outcome == "YES" {
        &market.yes_token_address
    } else {
        &market.no_token_address
    };

    let contract_address = std::env::var("PREDICTION_MARKET_ADDRESS")
        .unwrap_or_else(|_| "0x45e7911Af8c31bDeDf8A586BeEd8efEcACEb9c37".into());

    // 1 share = 1 USDC — redemption value is exactly shares
    let redemption_value = shares;
    let original_cost: i64 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(SUM(cost), 0)::bigint
        FROM orders
        WHERE market_id = $1 AND user_address = $2 AND status = 'filled' AND token = $3
        "#,
    )
    .bind(req.market_id)
    .bind(&address)
    .bind(winning_outcome)
    .fetch_one(&state.db)
    .await
    .map_err(AppError::Db)?;

    Ok(Json(serde_json::json!({
        "address":          address,
        "marketId":         req.market_id,
        "winningOutcome":   winning_outcome,
        "winningToken":     winning_token,
        "redeemableShares": shares,
        "redemptionValue":  redemption_value,
        "originalCost":     original_cost,
        "profit":           redemption_value - original_cost,
        "contractAddress":  contract_address,
        "action": {
            "method":   "redeemWinning",
            "args":     [req.market_id, shares],
            "to":       contract_address,
            "note":     "Call redeemWinning(marketId, amount) on-chain from your wallet"
        }
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

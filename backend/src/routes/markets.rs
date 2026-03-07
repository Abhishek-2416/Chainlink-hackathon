use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::Deserialize;

use crate::error::AppError;
use crate::services::AppState;

#[derive(Debug, Deserialize)]
pub struct MarketsQuery {
    /// "open" | "resolved" | "cancelled" | "all"  (default: "open")
    pub status: Option<String>,
    pub category: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    20
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_markets))
        .route("/:market_id", get(get_market))
        .route("/:market_id/price", get(get_market_price))
}

/// GET /api/markets
/// List markets with yes/no prices derived from orderbook.
async fn list_markets(
    State(state): State<AppState>,
    Query(q): Query<MarketsQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let limit = q.limit.min(100);
    let offset = q.offset;

    // "all" bypasses the status filter
    let markets: Vec<MarketRow> = if q.status.as_deref() == Some("all") {
        if let Some(cat) = &q.category {
            sqlx::query_as(
                r#"
                SELECT market_id, question, category, creator_address,
                       yes_token_address, no_token_address, resolution_date,
                       status, outcome, created_at
                FROM markets
                WHERE category = $1
                ORDER BY created_at DESC
                LIMIT $2 OFFSET $3
                "#,
            )
            .bind(cat)
            .bind(limit)
            .bind(offset)
        } else {
            sqlx::query_as(
                r#"
                SELECT market_id, question, category, creator_address,
                       yes_token_address, no_token_address, resolution_date,
                       status, outcome, created_at
                FROM markets
                ORDER BY created_at DESC
                LIMIT $1 OFFSET $2
                "#,
            )
            .bind(limit)
            .bind(offset)
        }
        .fetch_all(&state.db)
        .await
        .map_err(AppError::Db)?
    } else {
        let status = q.status.as_deref().unwrap_or("open");
        if let Some(cat) = &q.category {
            sqlx::query_as(
                r#"
                SELECT market_id, question, category, creator_address,
                       yes_token_address, no_token_address, resolution_date,
                       status, outcome, created_at
                FROM markets
                WHERE status = $1 AND category = $2
                ORDER BY created_at DESC
                LIMIT $3 OFFSET $4
                "#,
            )
            .bind(status)
            .bind(cat)
            .bind(limit)
            .bind(offset)
        } else {
            sqlx::query_as(
                r#"
                SELECT market_id, question, category, creator_address,
                       yes_token_address, no_token_address, resolution_date,
                       status, outcome, created_at
                FROM markets
                WHERE status = $1
                ORDER BY created_at DESC
                LIMIT $2 OFFSET $3
                "#,
            )
            .bind(status)
            .bind(limit)
            .bind(offset)
        }
        .fetch_all(&state.db)
        .await
        .map_err(AppError::Db)?
    };

    let total: i64 = {
        #[derive(sqlx::FromRow)]
        struct C { count: i64 }
        let row: C = sqlx::query_as("SELECT COUNT(*) as count FROM markets")
            .fetch_one(&state.db)
            .await
            .map_err(AppError::Db)?;
        row.count
    };

    let mut out = Vec::with_capacity(markets.len());
    for m in markets {
        let ob = state.orderbook.get_orderbook(m.market_id).await;
        let (yes_price, no_price) = ob.map(|o| (o.yes_price, o.no_price)).unwrap_or((50, 50));

        out.push(serde_json::json!({
            "marketId":        m.market_id,
            "question":        m.question,
            "category":        m.category,
            "creatorAddress":  m.creator_address,
            "yesTokenAddress": m.yes_token_address,
            "noTokenAddress":  m.no_token_address,
            "yesPrice":        yes_price,
            "noPrice":         no_price,
            "resolutionDate":  m.resolution_date,
            "status":          m.status,
            "outcome":         m.outcome,
            "createdAt":       m.created_at
        }));
    }

    Ok(Json(serde_json::json!({ "markets": out, "total": total })))
}

/// GET /api/markets/:market_id
/// Full market detail: metadata + orderbook + price history + recent trades.
async fn get_market(
    State(state): State<AppState>,
    Path(market_id): Path<i32>,
) -> Result<Json<serde_json::Value>, AppError> {
    let m: MarketRow = sqlx::query_as(
        r#"
        SELECT market_id, question, category, creator_address,
               yes_token_address, no_token_address, resolution_date,
               status, outcome, created_at
        FROM markets
        WHERE market_id = $1
        "#,
    )
    .bind(market_id)
    .fetch_optional(&state.db)
    .await
    .map_err(AppError::Db)?
    .ok_or(AppError::MarketNotFound(market_id))?;

    let ob = state.orderbook.get_orderbook(market_id).await?;

    // Price history for charting (up to 100 snapshots)
    let price_history: Vec<PriceRow> = sqlx::query_as(
        r#"
        SELECT yes_price, no_price, volume_24h, timestamp
        FROM price_snapshots
        WHERE market_id = $1
        ORDER BY timestamp ASC
        LIMIT 100
        "#,
    )
    .bind(market_id)
    .fetch_all(&state.db)
    .await
    .map_err(AppError::Db)?;

    let recent_trades: Vec<TradeRow> = sqlx::query_as(
        r#"
        SELECT buyer_address, token, shares, cost, created_at, tx_hash
        FROM trades
        WHERE market_id = $1
        ORDER BY created_at DESC
        LIMIT 10
        "#,
    )
    .bind(market_id)
    .fetch_all(&state.db)
    .await
    .map_err(AppError::Db)?;

    // Total volume = sum of all fills for this market
    let total_volume: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(cost), 0)::bigint FROM trades WHERE market_id = $1",
    )
    .bind(market_id)
    .fetch_one(&state.db)
    .await
    .map_err(AppError::Db)?;

    Ok(Json(serde_json::json!({
        "marketId":        m.market_id,
        "question":        m.question,
        "category":        m.category,
        "creator":         m.creator_address,
        "yesTokenAddress": m.yes_token_address,
        "noTokenAddress":  m.no_token_address,
        "yesPrice":        ob.yes_price,
        "noPrice":         ob.no_price,
        "lastPrice":       ob.last_price,
        "totalVolume":     total_volume,
        "resolutionDate":  m.resolution_date,
        "status":          m.status,
        "outcome":         m.outcome,
        "createdAt":       m.created_at,
        "orderbook":       ob,
        "priceHistory":    price_history,
        "recentTrades":    recent_trades
    })))
}

/// GET /api/markets/:market_id/price
/// Lightweight price endpoint — just the current YES/NO price for this market.
/// Suitable for polling from the frontend without loading full market data.
async fn get_market_price(
    State(state): State<AppState>,
    Path(market_id): Path<i32>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Make sure the market exists first
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM markets WHERE market_id = $1)",
    )
    .bind(market_id)
    .fetch_one(&state.db)
    .await
    .map_err(AppError::Db)?;

    if !exists {
        return Err(AppError::MarketNotFound(market_id));
    }

    let ob = state.orderbook.get_orderbook(market_id).await?;

    Ok(Json(serde_json::json!({
        "marketId":  market_id,
        // Prices in cents (1–99). YES + NO = 100.
        "yesPrice":  ob.yes_price,
        "noPrice":   ob.no_price,
        // Last fill price for YES (None = no fills yet, price from orderbook only)
        "lastPrice": ob.last_price,
        // Implied probability as a decimal (0.0–1.0) for frontend convenience
        "yesProbability": ob.yes_price as f64 / 100.0,
        "noProbability":  ob.no_price  as f64 / 100.0,
        // Depth at top of book
        "yesBestBid": ob.bids.first().map(|b| b.price),
        "noBestBid":  ob.no_bids.first().map(|b| b.price)
    })))
}

// ── DB row types ──────────────────────────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct MarketRow {
    market_id: i32,
    question: String,
    category: String,
    creator_address: String,
    yes_token_address: Option<String>,
    no_token_address: Option<String>,
    resolution_date: chrono::DateTime<chrono::Utc>,
    status: String,
    outcome: Option<String>,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(sqlx::FromRow, serde::Serialize)]
struct TradeRow {
    buyer_address: String,
    token: String,
    shares: i64,
    cost: i64,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
    tx_hash: String,
}

#[derive(sqlx::FromRow, serde::Serialize)]
struct PriceRow {
    yes_price: i32,
    no_price: i32,
    volume_24h: i64,
    timestamp: Option<chrono::DateTime<chrono::Utc>>,
}

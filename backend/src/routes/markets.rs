use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use ethers::utils::keccak256;
use serde::Deserialize;
use tracing::instrument;

use crate::error::AppError;
use crate::services::AppState;
use crate::types::CreateMarketRequest;

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
        .route("/", get(list_markets).post(create_market))
        .route("/trending", get(get_trending_markets))
        .route("/categories", get(get_categories))
        .route("/search", get(search_markets))
        .route("/ready-to-resolve", get(get_ready_to_resolve))
        .route("/ready-to-resolve-multi", get(get_ready_to_resolve_multi))
        .route("/resolved", get(get_resolved_markets))
        .route("/:market_id", get(get_market))
        .route("/:market_id/price", get(get_market_price))
        .route("/:market_id/activity", get(get_market_activity))
        .route("/:market_id/positions", get(get_market_positions))
        .route("/:market_id/cancel", post(cancel_market))
        .route("/:market_id/claim", post(claim_market_rewards))
}

/// POST /api/markets
/// Anyone can create a market. The backend hashes the question, calls createMarket
/// on-chain, and stores the human-readable question so the UI shows it instead of the hash.
#[instrument(skip(state, req))]
async fn create_market(
    State(state): State<AppState>,
    Json(req): Json<CreateMarketRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    tracing::info!(category = %req.category, "create_market request");
    let question = req.question.trim().to_string();
    if question.is_empty() {
        return Err(AppError::BadRequest("Question cannot be empty".into()));
    }

    let question_hash: [u8; 32] = keccak256(question.as_bytes());
    let question_hash_hex = format!("0x{}", hex::encode(question_hash));

    // Parse resolution date (supports ISO-8601 or unix timestamp)
    let resolution_ts: i64 = if let Ok(ts) = req.resolution_date.parse::<i64>() {
        ts
    } else {
        chrono::DateTime::parse_from_rfc3339(&req.resolution_date)
            .map_err(|_| AppError::BadRequest(
                "resolution_date must be ISO-8601 (e.g. 2026-12-31T23:59:59Z) or unix timestamp".into(),
            ))?
            .timestamp()
    };

    if resolution_ts <= chrono::Utc::now().timestamp() {
        return Err(AppError::BadRequest("Resolution date must be in the future".into()));
    }

    // Check if a market with this question hash already exists
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM markets WHERE question_hash = $1)",
    )
    .bind(&question_hash[..])
    .fetch_one(&state.db)
    .await
    .map_err(AppError::Db)?;

    if exists {
        return Err(AppError::SimilarMarketExists(question.clone()));
    }

    // Read on-chain marketCount — this is the exact ID the contract will assign next.
    // Using the contract as source of truth avoids desync if markets are created externally.
    let next_id: i32 = state
        .blockchain
        .get_market_count()
        .await
        .map_err(|e| AppError::Blockchain(format!("Failed to read marketCount: {e}")))?
        as i32;

    let creator = req.creator_address.to_lowercase();

    // Insert into DB first (watcher will backfill token addresses and tx_hash).
    // ON CONFLICT handles the race where watcher already inserted this market_id.
    sqlx::query(
        r#"
        INSERT INTO markets
            (market_id, question, question_hash, category, creator_address, resolution_date, status)
        VALUES ($1, $2, $3, $4, $5, to_timestamp($6), 'pending')
        ON CONFLICT (market_id) DO UPDATE
        SET question = EXCLUDED.question,
            question_hash = EXCLUDED.question_hash,
            category = EXCLUDED.category,
            creator_address = EXCLUDED.creator_address,
            resolution_date = EXCLUDED.resolution_date
        "#,
    )
    .bind(next_id)
    .bind(&question)
    .bind(&question_hash[..])
    .bind(&req.category)
    .bind(&creator)
    .bind(resolution_ts as f64)
    .execute(&state.db)
    .await
    .map_err(AppError::Db)?;

    // Call createMarket on-chain
    let tx_hash = state
        .blockchain
        .create_market(question_hash, resolution_ts as u64)
        .await
        .map_err(|e| AppError::Blockchain(format!("createMarket tx failed: {e}")))?;

    let tx_hex = format!("{tx_hash:#x}");

    // Update DB with tx hash and set status to open
    sqlx::query(
        "UPDATE markets SET tx_hash = $1, status = 'open' WHERE market_id = $2",
    )
    .bind(&tx_hex)
    .bind(next_id)
    .execute(&state.db)
    .await
    .map_err(AppError::Db)?;

    tracing::info!(market_id = next_id, tx_hash = %tx_hex, "market created");
    Ok(Json(serde_json::json!({
        "marketId":       next_id,
        "question":       question,
        "questionHash":   question_hash_hex,
        "category":       req.category,
        "creatorAddress": creator,
        "resolutionDate": resolution_ts,
        "txHash":         tx_hex,
        "status":         "open"
    })))
}

/// GET /api/markets/ready-to-resolve
/// Returns the oldest open market whose resolution_date has passed.
/// Used by the CRE workflow to discover which market to resolve next.
#[instrument(skip(state))]
async fn get_ready_to_resolve(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let market: Option<MarketRow> = sqlx::query_as(
        r#"
        SELECT market_id, question, category, creator_address,
               yes_token_address, no_token_address, resolution_date,
               status, outcome, created_at
        FROM markets
        WHERE status = 'open' AND resolution_date <= NOW()
        ORDER BY resolution_date ASC
        LIMIT 1
        "#,
    )
    .fetch_optional(&state.db)
    .await
    .map_err(AppError::Db)?;

    match market {
        Some(m) => {
            tracing::info!(market_id = m.market_id, question = %m.question, "ready_to_resolve found");
            Ok(Json(serde_json::json!({
            "marketId":        m.market_id,
            "question":        m.question,
            "category":        m.category,
            "creatorAddress":  m.creator_address,
            "resolutionDate":  m.resolution_date,
            "status":          m.status,
        })))
        }
        None => {
            tracing::debug!("ready_to_resolve: no markets pending");
            Ok(Json(serde_json::json!({
                "market": null
            })))
        }
    }
}

/// GET /api/markets/ready-to-resolve-multi
/// Returns ALL open markets whose resolution_date has passed.
#[instrument(skip(state))]
async fn get_ready_to_resolve_multi(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let markets: Vec<MarketRow> = sqlx::query_as(
        r#"
        SELECT market_id, question, category, creator_address,
               yes_token_address, no_token_address, resolution_date,
               status, outcome, created_at
        FROM markets
        WHERE status = 'open' AND resolution_date <= NOW()
        ORDER BY resolution_date ASC
        "#,
    )
    .fetch_all(&state.db)
    .await
    .map_err(AppError::Db)?;

    let out: Vec<_> = markets
        .iter()
        .map(|m| serde_json::json!({
            "marketId":        m.market_id,
            "question":        m.question,
            "category":        m.category,
            "creatorAddress":  m.creator_address,
            "resolutionDate":  m.resolution_date,
            "status":          m.status,
        }))
        .collect();

    tracing::info!(count = out.len(), "ready_to_resolve_multi");
    Ok(Json(serde_json::json!({
        "markets": out,
        "count":   out.len()
    })))
}

/// GET /api/markets/resolved
/// Returns all resolved markets with their outcomes.
#[instrument(skip(state))]
async fn get_resolved_markets(
    State(state): State<AppState>,
    Query(q): Query<MarketsQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let limit = q.limit.min(100);
    let offset = q.offset;

    let markets: Vec<MarketRow> = sqlx::query_as(
        r#"
        SELECT market_id, question, category, creator_address,
               yes_token_address, no_token_address, resolution_date,
               status, outcome, created_at
        FROM markets
        WHERE status = 'resolved'
        ORDER BY created_at DESC
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(AppError::Db)?;

    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM markets WHERE status = 'resolved'",
    )
    .fetch_one(&state.db)
    .await
    .map_err(AppError::Db)?;

    let mut out = Vec::with_capacity(markets.len());
    for m in &markets {
        let ob = state.orderbook.get_orderbook(m.market_id).await;
        let (yes_price, no_price) = ob.map(|o| (o.yes_price, o.no_price)).unwrap_or((50, 50));

        let total_volume: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(cost), 0)::bigint FROM trades WHERE market_id = $1",
        )
        .bind(m.market_id)
        .fetch_one(&state.db)
        .await
        .map_err(AppError::Db)?;

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
            "totalVolume":     total_volume,
            "createdAt":       m.created_at
        }));
    }

    Ok(Json(serde_json::json!({
        "markets": out,
        "total":   total
    })))
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
        let row: C = if q.status.as_deref() == Some("all") {
            if let Some(cat) = &q.category {
                sqlx::query_as("SELECT COUNT(*) as count FROM markets WHERE category = $1")
                    .bind(cat)
                    .fetch_one(&state.db)
                    .await
            } else {
                sqlx::query_as("SELECT COUNT(*) as count FROM markets")
                    .fetch_one(&state.db)
                    .await
            }
        } else {
            let status = q.status.as_deref().unwrap_or("open");
            if let Some(cat) = &q.category {
                sqlx::query_as("SELECT COUNT(*) as count FROM markets WHERE status = $1 AND category = $2")
                    .bind(status)
                    .bind(cat)
                    .fetch_one(&state.db)
                    .await
            } else {
                sqlx::query_as("SELECT COUNT(*) as count FROM markets WHERE status = $1")
                    .bind(status)
                    .fetch_one(&state.db)
                    .await
            }
        }.map_err(AppError::Db)?;
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
#[instrument(skip(state), fields(market_id = %market_id))]
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

/// GET /api/markets/trending
/// Markets ranked by 24h volume — the homepage hero section.
async fn get_trending_markets(
    State(state): State<AppState>,
    Query(q): Query<MarketsQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let limit = q.limit.min(50);

    let rows: Vec<TrendingRow> = sqlx::query_as(
        r#"
        SELECT m.market_id, m.question, m.category,
               m.resolution_date, m.status, m.created_at,
               COALESCE(SUM(t.cost), 0)::bigint AS total_volume,
               COUNT(t.id)::int                 AS trade_count
        FROM markets m
        LEFT JOIN trades t ON t.market_id = m.market_id AND t.token IN ('YES','NO')
        WHERE m.status = 'open'
        GROUP BY m.market_id, m.question, m.category,
                 m.resolution_date, m.status, m.created_at
        ORDER BY total_volume DESC, trade_count DESC
        LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(&state.db)
    .await
    .map_err(AppError::Db)?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let ob = state.orderbook.get_orderbook(r.market_id).await;
        let (yes_price, no_price) = ob.map(|o| (o.yes_price, o.no_price)).unwrap_or((50, 50));

        out.push(serde_json::json!({
            "marketId":        r.market_id,
            "question":        r.question,
            "category":        r.category,
            "yesPrice":        yes_price,
            "noPrice":         no_price,
            "totalVolume":     r.total_volume,
            "tradeCount":      r.trade_count,
            "resolutionDate":  r.resolution_date,
            "status":          r.status,
            "createdAt":       r.created_at
        }));
    }

    Ok(Json(serde_json::json!({ "markets": out })))
}

/// GET /api/markets/categories
/// Distinct categories with market counts — for the category filter bar.
async fn get_categories(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let categories: Vec<CategoryRow> = sqlx::query_as(
        r#"
        SELECT category,
               COUNT(*)::int AS market_count,
               COUNT(*) FILTER (WHERE status = 'open')::int AS open_count
        FROM markets
        GROUP BY category
        ORDER BY market_count DESC
        "#,
    )
    .fetch_all(&state.db)
    .await
    .map_err(AppError::Db)?;

    let cats: Vec<_> = categories
        .iter()
        .map(|c| serde_json::json!({
            "category":    c.category,
            "marketCount": c.market_count,
            "openCount":   c.open_count
        }))
        .collect();

    Ok(Json(serde_json::json!({ "categories": cats })))
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

/// GET /api/markets/search?q=bitcoin&limit=10
/// Full-text search on market questions.
async fn search_markets(
    State(state): State<AppState>,
    Query(sq): Query<SearchQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let query = sq.q.trim().to_string();
    if query.is_empty() {
        return Err(AppError::BadRequest("Search query cannot be empty".into()));
    }
    let limit = sq.limit.min(50);
    let pattern = format!("%{}%", query.to_lowercase());

    let markets: Vec<MarketRow> = sqlx::query_as(
        r#"
        SELECT market_id, question, category, creator_address,
               yes_token_address, no_token_address, resolution_date,
               status, outcome, created_at
        FROM markets
        WHERE LOWER(question) LIKE $1
        ORDER BY created_at DESC
        LIMIT $2
        "#,
    )
    .bind(&pattern)
    .bind(limit)
    .fetch_all(&state.db)
    .await
    .map_err(AppError::Db)?;

    let mut out = Vec::with_capacity(markets.len());
    for m in markets {
        let ob = state.orderbook.get_orderbook(m.market_id).await;
        let (yes_price, no_price) = ob.map(|o| (o.yes_price, o.no_price)).unwrap_or((50, 50));

        out.push(serde_json::json!({
            "marketId":       m.market_id,
            "question":       m.question,
            "category":       m.category,
            "yesPrice":       yes_price,
            "noPrice":        no_price,
            "resolutionDate": m.resolution_date,
            "status":         m.status,
            "outcome":        m.outcome,
            "createdAt":      m.created_at
        }));
    }

    Ok(Json(serde_json::json!({ "markets": out, "query": query })))
}

/// GET /api/markets/:market_id/activity
/// Recent activity feed for a single market (trades + key events).
async fn get_market_activity(
    State(state): State<AppState>,
    Path(market_id): Path<i32>,
) -> Result<Json<serde_json::Value>, AppError> {
    let trades: Vec<ActivityRow> = sqlx::query_as(
        r#"
        SELECT buyer_address, token, shares, cost, tx_hash, created_at
        FROM trades
        WHERE market_id = $1 AND token IN ('YES', 'NO')
        ORDER BY created_at DESC
        LIMIT 50
        "#,
    )
    .bind(market_id)
    .fetch_all(&state.db)
    .await
    .map_err(AppError::Db)?;

    let feed: Vec<_> = trades
        .iter()
        .map(|t| {
            let price_cents = crate::services::orderbook::OrderbookService::shares_to_price_cents(t.cost, t.shares);
            serde_json::json!({
                "type":        "trade",
                "address":     t.buyer_address,
                "token":       t.token,
                "shares":      t.shares,
                "cost":        t.cost,
                "priceCents":  price_cents,
                "txHash":      t.tx_hash,
                "timestamp":   t.created_at
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "marketId": market_id,
        "activity": feed
    })))
}

/// GET /api/markets/:market_id/positions
/// All user positions for a specific market (how many people hold YES vs NO).
async fn get_market_positions(
    State(state): State<AppState>,
    Path(market_id): Path<i32>,
) -> Result<Json<serde_json::Value>, AppError> {
    #[derive(sqlx::FromRow, serde::Serialize)]
    struct MarketPositionRow {
        user_address: String,
        token: String,
        shares: i64,
        cost: i64,
        avg_price: i32,
    }

    let positions: Vec<MarketPositionRow> = sqlx::query_as(
        r#"
        SELECT user_address, token, shares, cost, avg_price
        FROM user_positions
        WHERE market_id = $1 AND shares > 0
        ORDER BY shares DESC
        "#,
    )
    .bind(market_id)
    .fetch_all(&state.db)
    .await
    .map_err(AppError::Db)?;

    let yes_holders = positions.iter().filter(|p| p.token == "YES").count();
    let no_holders = positions.iter().filter(|p| p.token == "NO").count();
    let yes_shares: i64 = positions.iter().filter(|p| p.token == "YES").map(|p| p.shares).sum();
    let no_shares: i64 = positions.iter().filter(|p| p.token == "NO").map(|p| p.shares).sum();

    Ok(Json(serde_json::json!({
        "marketId":    market_id,
        "positions":   positions,
        "yesHolders":  yes_holders,
        "noHolders":   no_holders,
        "yesShares":   yes_shares,
        "noShares":    no_shares,
        "totalHolders": yes_holders + no_holders
    })))
}

/// POST /api/markets/:market_id/cancel
/// Cancel an open market. Calls cancelMarket on-chain and updates DB.
#[instrument(skip(state), fields(market_id = %market_id))]
async fn cancel_market(
    State(state): State<AppState>,
    Path(market_id): Path<i32>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Verify market exists and is open
    let status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM markets WHERE market_id = $1",
    )
    .bind(market_id)
    .fetch_optional(&state.db)
    .await
    .map_err(AppError::Db)?;

    match status.as_deref() {
        None => return Err(AppError::MarketNotFound(market_id)),
        Some("open") => {}
        Some(s) => {
            return Err(AppError::BadRequest(format!(
                "Cannot cancel market with status '{s}'"
            )))
        }
    }

    let tx_hash = state
        .blockchain
        .cancel_market(market_id as u64)
        .await
        .map_err(|e| AppError::Blockchain(format!("cancelMarket tx failed: {e}")))?;

    let tx_hex = format!("{tx_hash:#x}");

    // Update DB optimistically (watcher will confirm)
    sqlx::query("UPDATE markets SET status = 'cancelled' WHERE market_id = $1")
        .bind(market_id)
        .execute(&state.db)
        .await
        .map_err(AppError::Db)?;

    // Cancel all pending orders for this market
    sqlx::query("UPDATE orders SET status = 'cancelled' WHERE market_id = $1 AND status = 'pending'")
        .bind(market_id)
        .execute(&state.db)
        .await
        .map_err(AppError::Db)?;

    tracing::info!(market_id = market_id, tx_hash = %tx_hex, "market cancelled");
    Ok(Json(serde_json::json!({
        "marketId": market_id,
        "status":   "cancelled",
        "txHash":   tx_hex
    })))
}

#[derive(Debug, Deserialize)]
pub struct ClaimMarketRequest {
    pub user_address: String,
}

/// POST /api/markets/:market_id/claim
/// Claim rewards for a resolved market.
/// Body: { "user_address": "0x..." }
#[instrument(skip(state, req), fields(market_id = %market_id))]
async fn claim_market_rewards(
    State(state): State<AppState>,
    Path(market_id): Path<i32>,
    Json(req): Json<ClaimMarketRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let user = req.user_address.to_lowercase();

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
    .bind(market_id)
    .fetch_optional(&state.db)
    .await
    .map_err(AppError::Db)?
    .ok_or(AppError::MarketNotFound(market_id))?;

    if market.status != "resolved" {
        return Err(AppError::BadRequest(format!(
            "Market {} is not resolved (status: {})",
            market_id, market.status
        )));
    }

    let winning_outcome = market.outcome.as_deref().ok_or_else(|| {
        AppError::BadRequest(format!("Market {} resolved but outcome is null", market_id))
    })?;

    // Use user_positions (decremented on redeem by watcher) for accurate remaining balance.
    // Fallback to orders sum if user_positions not populated (e.g. watcher lag).
    let shares: i64 = sqlx::query_scalar::<_, Option<i64>>(
        r#"
        SELECT COALESCE(
            (SELECT shares FROM user_positions
             WHERE market_id = $1 AND user_address = $2 AND token = $3 AND shares > 0),
            (SELECT SUM(shares)::bigint FROM orders
             WHERE market_id = $1 AND user_address = $2 AND status = 'filled' AND token = $3)
        )
        "#,
    )
    .bind(market_id)
    .bind(&user)
    .bind(winning_outcome)
    .fetch_one(&state.db)
    .await
    .map_err(AppError::Db)?
    .unwrap_or(0);
    if shares <= 0 {
        return Err(AppError::BadRequest(format!(
            "No redeemable shares for market {}. You need {} tokens to redeem.",
            market_id, winning_outcome
        )));
    }

    let winning_token = if winning_outcome == "YES" {
        &market.yes_token_address
    } else {
        &market.no_token_address
    };

    let contract_address = std::env::var("PREDICTION_MARKET_ADDRESS")
        .unwrap_or_else(|_| "0x45e7911Af8c31bDeDf8A586BeEd8efEcACEb9c37".into());

    let original_cost: i64 = sqlx::query_scalar(
        r#"
        SELECT COALESCE(SUM(cost), 0)::bigint
        FROM orders
        WHERE market_id = $1 AND user_address = $2 AND status = 'filled' AND token = $3
        "#,
    )
    .bind(market_id)
    .bind(&user)
    .bind(winning_outcome)
    .fetch_one(&state.db)
    .await
    .map_err(AppError::Db)?;

    tracing::info!(market_id = market_id, user = %user, shares = shares, "claim data prepared");
    Ok(Json(serde_json::json!({
        "address":          user,
        "marketId":         market_id,
        "winningOutcome":   winning_outcome,
        "winningToken":     winning_token,
        "redeemableShares": shares,
        "redemptionValue":  shares,
        "originalCost":     original_cost,
        "profit":           shares - original_cost,
        "contractAddress":  contract_address,
        "action": {
            "method":   "redeemWinning",
            "args":     [market_id, shares],
            "to":       contract_address,
            "note":     "Call redeemWinning(marketId, amount) on-chain from your wallet"
        }
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

#[derive(sqlx::FromRow)]
struct TrendingRow {
    market_id: i32,
    question: String,
    category: String,
    resolution_date: chrono::DateTime<chrono::Utc>,
    status: String,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
    total_volume: i64,
    trade_count: i32,
}

#[derive(sqlx::FromRow)]
struct CategoryRow {
    category: String,
    market_count: i32,
    open_count: i32,
}

#[derive(sqlx::FromRow)]
struct ActivityRow {
    buyer_address: String,
    token: String,
    shares: i64,
    cost: i64,
    tx_hash: String,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
}

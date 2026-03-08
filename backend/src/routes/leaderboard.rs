use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use tracing::instrument;

use crate::error::AppError;
use crate::services::AppState;

#[derive(Debug, Deserialize)]
pub struct LeaderboardQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    /// "volume" | "profit" | "trades"  (default: "volume")
    pub sort: Option<String>,
}

fn default_limit() -> i64 {
    20
}

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(get_leaderboard))
}

/// GET /api/leaderboard?sort=volume&limit=20
/// Top traders ranked by total volume, profit, or trade count.
#[instrument(skip(state))]
async fn get_leaderboard(
    State(state): State<AppState>,
    Query(q): Query<LeaderboardQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let limit = q.limit.min(100);
    let sort_by = q.sort.as_deref().unwrap_or("volume");

    let order_clause = match sort_by {
        "profit" => "total_profit DESC",
        "trades" => "trade_count DESC",
        _ => "total_volume DESC",
    };

    let query_str = format!(
        r#"
        SELECT
            t.buyer_address                       AS address,
            COUNT(*)::int                         AS trade_count,
            COALESCE(SUM(t.cost), 0)::bigint      AS total_volume,
            COUNT(DISTINCT t.market_id)::int       AS markets_traded,
            COALESCE(SUM(
                CASE
                    WHEN m.status = 'resolved' AND m.outcome = t.token
                    THEN t.shares - t.cost
                    WHEN m.status = 'resolved' AND m.outcome != t.token
                    THEN -t.cost
                    ELSE 0
                END
            ), 0)::bigint                          AS total_profit
        FROM trades t
        JOIN markets m ON m.market_id = t.market_id
        WHERE t.token IN ('YES', 'NO')
        GROUP BY t.buyer_address
        ORDER BY {order_clause}
        LIMIT $1
        "#
    );

    let rows: Vec<LeaderboardRow> = sqlx::query_as(&query_str)
        .bind(limit)
        .fetch_all(&state.db)
        .await
        .map_err(AppError::Db)?;

    let entries: Vec<_> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            serde_json::json!({
                "rank":           i + 1,
                "address":        r.address,
                "tradeCount":     r.trade_count,
                "totalVolume":    r.total_volume,
                "marketsTraded":  r.markets_traded,
                "totalProfit":    r.total_profit
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "leaderboard": entries,
        "sortedBy":    sort_by
    })))
}

#[derive(sqlx::FromRow)]
struct LeaderboardRow {
    address: String,
    trade_count: i32,
    total_volume: i64,
    markets_traded: i32,
    total_profit: i64,
}

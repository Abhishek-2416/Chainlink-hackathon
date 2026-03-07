use crate::error::AppError;
use crate::types::{OrderbookLevel, OrderbookResponse};
use sqlx::PgPool;

/// Orderbook service: aggregates pending orders by price level and derives the
/// implied YES/NO probability from the order flow.
///
/// # How pricing works (matches the contract)
///
/// The contract uses probability pricing:
///   shares = floor(cost / (price / 100))
///   e.g. $5 USDC at 72¢ YES → 6_944_444 shares
///   e.g. 28¢ NO → you receive 1 USDC per share if NO wins (28% probability)
///
/// Price derivation priority (highest wins):
///   1. Last fill price (from `price_snapshots`, populated after each OrderFilled)
///   2. Volume-weighted mid from live pending orderbook
///   3. Best YES bid / Best NO bid (one-sided)
///   4. 50/50 default for brand-new markets
pub struct OrderbookService {
    db: PgPool,
}

impl OrderbookService {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    /// Full orderbook for a market: aggregated YES/NO bids + implied prices.
    pub async fn get_orderbook(&self, market_id: i32) -> Result<OrderbookResponse, AppError> {
        let yes_bids: Vec<OrderbookRow> = sqlx::query_as(
            r#"
            SELECT price,
                   COALESCE(SUM(shares), 0)::bigint AS total_shares,
                   COUNT(*)::int                    AS order_count
            FROM orders
            WHERE market_id = $1 AND token = 'YES' AND status = 'pending'
            GROUP BY price
            ORDER BY price DESC
            LIMIT 10
            "#,
        )
        .bind(market_id)
        .fetch_all(&self.db)
        .await
        .map_err(AppError::Db)?;

        let no_bids: Vec<OrderbookRow> = sqlx::query_as(
            r#"
            SELECT price,
                   COALESCE(SUM(shares), 0)::bigint AS total_shares,
                   COUNT(*)::int                    AS order_count
            FROM orders
            WHERE market_id = $1 AND token = 'NO' AND status = 'pending'
            GROUP BY price
            ORDER BY price DESC
            LIMIT 10
            "#,
        )
        .bind(market_id)
        .fetch_all(&self.db)
        .await
        .map_err(AppError::Db)?;

        // Most recent fill price (snapshotted by backend after each fillOrder)
        let last_trade_price: Option<(i32, i32)> = sqlx::query_as(
            r#"
            SELECT yes_price, no_price
            FROM price_snapshots
            WHERE market_id = $1
            ORDER BY timestamp DESC
            LIMIT 1
            "#,
        )
        .bind(market_id)
        .fetch_optional(&self.db)
        .await
        .map_err(AppError::Db)?;

        let last_price = last_trade_price.map(|(y, _)| y);

        let (yes_price, no_price) =
            Self::compute_implied_prices(&yes_bids, &no_bids, last_trade_price);

        Ok(OrderbookResponse {
            market_id,
            bids: yes_bids
                .into_iter()
                .map(|r| OrderbookLevel {
                    price: r.price,
                    shares: r.total_shares,
                    orders: r.order_count,
                })
                .collect(),
            no_bids: no_bids
                .into_iter()
                .map(|r| OrderbookLevel {
                    price: r.price,
                    shares: r.total_shares,
                    orders: r.order_count,
                })
                .collect(),
            last_price,
            yes_price,
            no_price,
        })
    }

    /// Returns (yes_price_cents, no_price_cents).
    ///
    /// Algorithm:
    /// 1. Last trade snapshot (most reliable — actual fill prices)
    /// 2. Volume-weighted mid across both sides of the book
    /// 3. One-sided best bid fallback
    /// 4. 50 / 50 for new markets
    fn compute_implied_prices(
        yes_bids: &[OrderbookRow],
        no_bids: &[OrderbookRow],
        last_trade: Option<(i32, i32)>,
    ) -> (i32, i32) {
        if let Some((y, n)) = last_trade {
            return (y, n);
        }

        // Volume-weighted average price for YES side
        let vwap_yes = vwap(yes_bids);
        // For NO bids: a NO bid at price p implies YES is at (100 - p).
        // So we convert each NO bid into its implied YES price, weight by shares.
        let vwap_no_implied = vwap_implied_yes(no_bids);

        let (yes_price, no_price) = match (vwap_yes, vwap_no_implied) {
            (Some(vy), Some(ny)) => {
                // Mid-point between YES VWAP and NO-implied YES VWAP
                let yes_total_shares: i64 = yes_bids.iter().map(|r| r.total_shares).sum();
                let no_total_shares: i64 = no_bids.iter().map(|r| r.total_shares).sum();
                // Volume-weight the two sides
                let total = yes_total_shares + no_total_shares;
                let mid = if total > 0 {
                    (vy * yes_total_shares + ny * no_total_shares) / total
                } else {
                    (vy + ny) / 2
                };
                let mid = mid.clamp(1, 99) as i32;
                (mid, 100 - mid)
            }
            (Some(vy), None) => {
                let y = vy.clamp(1, 99) as i32;
                (y, 100 - y)
            }
            (None, Some(ny)) => {
                let y = ny.clamp(1, 99) as i32;
                (y, 100 - y)
            }
            (None, None) => (50, 50),
        };

        (yes_price, no_price)
    }

    /// Get the current implied price for a specific token side.
    pub async fn get_token_price(&self, market_id: i32, token: &str) -> Result<i32, AppError> {
        let ob = self.get_orderbook(market_id).await?;
        Ok(if token.to_uppercase() == "YES" {
            ob.yes_price
        } else {
            ob.no_price
        })
    }

    /// Record a price snapshot after a successful on-chain fill.
    /// Called by the order service immediately after fillOrder succeeds.
    pub async fn record_price_snapshot(
        &self,
        market_id: i32,
        fill_price_cents: i32,
        token: &str,
        volume: i64,
    ) -> Result<(), AppError> {
        let (yes_price, no_price) = if token.to_uppercase() == "YES" {
            (fill_price_cents, 100 - fill_price_cents)
        } else {
            (100 - fill_price_cents, fill_price_cents)
        };

        sqlx::query(
            r#"
            INSERT INTO price_snapshots (market_id, yes_price, no_price, volume_24h)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(market_id)
        .bind(yes_price)
        .bind(no_price)
        .bind(volume)
        .execute(&self.db)
        .await
        .map_err(AppError::Db)?;

        Ok(())
    }

    /// Calculate shares from cost and price.
    ///   shares = floor(cost / (price / 100))
    ///   e.g. cost=5_000_000 ($5), price=72 → 6_944_444 shares
    pub fn cost_to_shares(cost: i64, price_cents: i32) -> i64 {
        if price_cents <= 0 || price_cents >= 100 {
            return 0;
        }
        (cost * 100) / (price_cents as i64)
    }

    /// Reverse: infer price from cost/shares as stored in the trades table.
    pub fn shares_to_price_cents(cost: i64, shares: i64) -> i32 {
        if shares == 0 {
            return 50;
        }
        ((cost * 100) / shares).clamp(1, 99) as i32
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Volume-weighted average price (YES side). Returns None if no bids.
fn vwap(bids: &[OrderbookRow]) -> Option<i64> {
    let total_shares: i64 = bids.iter().map(|r| r.total_shares).sum();
    if total_shares == 0 {
        return None;
    }
    let weighted: i64 = bids
        .iter()
        .map(|r| r.price as i64 * r.total_shares)
        .sum();
    Some(weighted / total_shares)
}

/// Volume-weighted implied YES price derived from NO bids.
/// A NO bid at price `n` implies YES is at `100 - n`.
fn vwap_implied_yes(no_bids: &[OrderbookRow]) -> Option<i64> {
    let total_shares: i64 = no_bids.iter().map(|r| r.total_shares).sum();
    if total_shares == 0 {
        return None;
    }
    let weighted: i64 = no_bids
        .iter()
        .map(|r| (100 - r.price as i64) * r.total_shares)
        .sum();
    Some(weighted / total_shares)
}

#[derive(sqlx::FromRow)]
struct OrderbookRow {
    price: i32,
    total_shares: i64,
    order_count: i32,
}

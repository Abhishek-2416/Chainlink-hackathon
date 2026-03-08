//! Event parsing and DB handlers for PredictionMarket events.
//! Events sourced from contracts/src/PredictionMarket.sol.

use anyhow::Result;
use ethers::types::{Address, Log, H256, U256};
use sqlx::PgPool;
use tracing::instrument;

/// Event topic0 constants (keccak256 of event signature).
/// Using const byte arrays to avoid runtime unwrap/parse.
const MARKET_CREATED_TOPIC: H256 = H256([
    0x52, 0x5a, 0xd0, 0x05, 0x86, 0xa1, 0x61, 0xc1, 0x10, 0x70, 0xe5, 0xc5, 0xde, 0x95, 0x32,
    0x31, 0x65, 0xa7, 0x8a, 0xe0, 0x13, 0x76, 0x15, 0xb2, 0xf6, 0x98, 0xba, 0x5a, 0x52, 0x7d,
    0x45, 0x8b,
]);
const ORDER_FILLED_TOPIC: H256 = H256([
    0xca, 0x33, 0x9e, 0xbc, 0xfc, 0xcd, 0x32, 0x20, 0x48, 0x47, 0xdd, 0xc1, 0xf4, 0xb1, 0x6f,
    0x9a, 0xbf, 0xf9, 0x07, 0x4a, 0xc2, 0x68, 0x02, 0x4a, 0x6f, 0x72, 0x44, 0x95, 0x45, 0xec,
    0xc2, 0x18,
]);
const MARKET_RESOLVED_TOPIC: H256 = H256([
    0x73, 0x9f, 0x28, 0x35, 0x63, 0xfb, 0x51, 0xab, 0x6b, 0x89, 0xee, 0x95, 0xd9, 0x37, 0xb2,
    0xe6, 0x3a, 0x6c, 0xfc, 0xb8, 0x3c, 0x38, 0x5d, 0xbe, 0xbb, 0x62, 0x9f, 0x9d, 0x97, 0xbd,
    0x43, 0xe6,
]);
const MARKET_CANCELLED_TOPIC: H256 = H256([
    0x2c, 0xa4, 0x40, 0xfb, 0x7f, 0xca, 0x85, 0xd7, 0xf5, 0x5d, 0x39, 0x5a, 0x4a, 0xbd, 0x94,
    0x81, 0x73, 0x30, 0xb8, 0x3a, 0x62, 0xf3, 0x50, 0x2e, 0xfb, 0xb4, 0x77, 0x01, 0x44, 0xe4,
    0xca, 0x97,
]);
const WINNINGS_REDEEMED_TOPIC: H256 = H256([
    0xa5, 0xb5, 0xf9, 0x99, 0xd3, 0x56, 0xbb, 0x14, 0xd5, 0x11, 0x14, 0x32, 0x7e, 0x0f, 0x0e,
    0xef, 0xee, 0xd3, 0xa4, 0x08, 0x31, 0x69, 0xd0, 0x23, 0xdd, 0x7f, 0xe6, 0x74, 0x4c, 0xa1,
    0x51, 0x74,
]);

fn format_address(a: Address) -> String {
    format!("{:?}", a).to_lowercase()
}

fn format_tx_hash(h: Option<H256>) -> String {
    h.map(|h| format!("{:?}", h).to_lowercase())
        .unwrap_or_default()
}

#[instrument(skip(pool, log), fields(tx = ?log.transaction_hash, block = ?log.block_number))]
pub async fn process_log(pool: &PgPool, log: &Log) -> Result<()> {
    let topic0 = log.topics.get(0).copied().unwrap_or_default();

    if topic0 == MARKET_CREATED_TOPIC {
        tracing::debug!(tx = ?log.transaction_hash, "processing MarketCreated");
        process_market_created(pool, log).await?;
    } else if topic0 == ORDER_FILLED_TOPIC {
        tracing::debug!(tx = ?log.transaction_hash, "processing OrderFilled");
        process_order_filled(pool, log).await?;
    } else if topic0 == MARKET_RESOLVED_TOPIC {
        tracing::debug!(tx = ?log.transaction_hash, "processing MarketResolved");
        process_market_resolved(pool, log).await?;
    } else if topic0 == MARKET_CANCELLED_TOPIC {
        tracing::debug!(tx = ?log.transaction_hash, "processing MarketCancelled");
        process_market_cancelled(pool, log).await?;
    } else if topic0 == WINNINGS_REDEEMED_TOPIC {
        tracing::debug!(tx = ?log.transaction_hash, "processing WinningsRedeemed");
        process_winnings_redeemed(pool, log).await?;
    }

    Ok(())
}

async fn process_market_created(pool: &PgPool, log: &Log) -> Result<()> {
    let market_id = log
        .topics
        .get(1)
        .map(|t| U256::from(t.as_bytes()))
        .unwrap_or_default();

    // data layout: questionHash(32) | creator(32) | yesToken(32) | noToken(32) | resolutionTimestamp(32)
    let data = &log.data.0;
    if data.len() < 160 {
        tracing::warn!("MarketCreated: insufficient data len {}", data.len());
        return Ok(());
    }

    let question_hash = &data[0..32];
    let creator = Address::from_slice(&data[44..64]);
    let yes_token = Address::from_slice(&data[76..96]);
    let no_token = Address::from_slice(&data[108..128]);
    let resolution_ts = U256::from_big_endian(&data[128..160]);

    let tx_hash = format_tx_hash(log.transaction_hash);
    let market_id_num = market_id.as_u64() as i32;

    // Try to update an existing row by market_id first (exact match)
    let result = sqlx::query(
        r#"
        UPDATE markets
        SET tx_hash = $1,
            yes_token_address = $2,
            no_token_address = $3,
            market_id = $4,
            status = 'open'
        WHERE market_id = $4
        "#,
    )
    .bind(&tx_hash)
    .bind(format_address(yes_token))
    .bind(format_address(no_token))
    .bind(market_id_num)
    .execute(pool)
    .await?;

    // Fallback: match by question_hash (backend pre-inserted with question_hash but
    // the market_id might differ if there was a desync)
    let result = if result.rows_affected() == 0 {
        sqlx::query(
            r#"
            UPDATE markets
            SET tx_hash = $1,
                yes_token_address = $2,
                no_token_address = $3,
                market_id = $4,
                status = 'open'
            WHERE question_hash = $5 AND (status = 'pending' OR yes_token_address IS NULL)
            "#,
        )
        .bind(&tx_hash)
        .bind(format_address(yes_token))
        .bind(format_address(no_token))
        .bind(market_id_num)
        .bind(question_hash)
        .execute(pool)
        .await?
    } else {
        result
    };

    // If still no match (market created externally / script), insert it
    if result.rows_affected() == 0 {
        let resolution_secs = resolution_ts.as_u64() as i64;
        sqlx::query(
            r#"
            INSERT INTO markets
                (market_id, question, question_hash, category, creator_address,
                 yes_token_address, no_token_address, resolution_date, status, tx_hash)
            VALUES ($1, $2, $3, 'general', $4, $5, $6, to_timestamp($7), 'open', $8)
            ON CONFLICT (market_id) DO UPDATE
            SET yes_token_address = $5, no_token_address = $6, tx_hash = $8, status = 'open'
            "#,
        )
        .bind(market_id_num)
        .bind(format!("Market #{}", market_id_num))
        .bind(question_hash)
        .bind(format_address(creator))
        .bind(format_address(yes_token))
        .bind(format_address(no_token))
        .bind(resolution_secs as f64)
        .bind(&tx_hash)
        .execute(pool)
        .await?;

        tracing::info!(
            "MarketCreated: inserted new market_id={} (created externally)",
            market_id_num
        );
    }

    tracing::info!(market_id = market_id_num, tx_hash = %tx_hash, "MarketCreated processed");
    Ok(())
}

async fn process_order_filled(pool: &PgPool, log: &Log) -> Result<()> {
    let market_id = log
        .topics
        .get(1)
        .map(|t| U256::from(t.as_bytes()))
        .unwrap_or_default();
    let buyer = log
        .topics
        .get(2)
        .map(|t| Address::from_slice(&t.as_bytes()[12..]))
        .unwrap_or_default();

    let data = &log.data.0;
    if data.len() < 96 {
        tracing::warn!("OrderFilled: insufficient data len {}", data.len());
        return Ok(());
    }
    // ABI layout follows non-indexed param order: outcome(uint8), shares(uint256), cost(uint256)
    let outcome = U256::from_big_endian(&data[0..32]).as_u64();
    let shares = U256::from_big_endian(&data[32..64]);
    let cost = U256::from_big_endian(&data[64..96]);

    let token = if outcome == 1 { "YES" } else { "NO" };

    let tx_hash = format_tx_hash(log.transaction_hash);
    let block_number = log.block_number.map(|b| b.as_u64() as i64);

    let _ = sqlx::query(
        r#"
        INSERT INTO trades (market_id, buyer_address, token, shares, cost, tx_hash, block_number)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        ON CONFLICT (tx_hash, market_id, buyer_address, token) DO NOTHING
        "#,
    )
    .bind(market_id.as_u64() as i32)
    .bind(format_address(buyer))
    .bind(token)
    .bind(shares.as_u64() as i64)
    .bind(cost.as_u64() as i64)
    .bind(&tx_hash)
    .bind(block_number)
    .execute(pool)
    .await?;

    // Update matching pending order
    let _ = sqlx::query(
        r#"
        UPDATE orders
        SET status = 'filled', tx_hash = $1, filled_at = NOW()
        WHERE market_id = $2 AND user_address = $3 AND shares = $4 AND cost = $5 AND status = 'pending'
        "#,
    )
    .bind(&tx_hash)
    .bind(market_id.as_u64() as i32)
    .bind(format_address(buyer))
    .bind(shares.as_u64() as i64)
    .bind(cost.as_u64() as i64)
    .execute(pool)
    .await?;

    // Upsert user position: accumulate shares and cost for this (user, market, token)
    let shares_i64 = shares.as_u64() as i64;
    let cost_i64 = cost.as_u64() as i64;
    let price_for_pos = if shares_i64 > 0 {
        ((cost_i64 * 100) / shares_i64).clamp(1, 99) as i32
    } else {
        50
    };

    let _ = sqlx::query(
        r#"
        INSERT INTO user_positions (user_address, market_id, token, shares, cost, avg_price)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (user_address, market_id, token) DO UPDATE
        SET shares = user_positions.shares + $4,
            cost   = user_positions.cost + $5,
            avg_price = CASE
                WHEN (user_positions.shares + $4) > 0
                THEN (((user_positions.cost + $5) * 100) / (user_positions.shares + $4))::int
                ELSE 50
            END,
            updated_at = NOW()
        "#,
    )
    .bind(format_address(buyer))
    .bind(market_id.as_u64() as i32)
    .bind(token)
    .bind(shares_i64)
    .bind(cost_i64)
    .bind(price_for_pos)
    .execute(pool)
    .await;

    // Record a price snapshot so orderbook pricing reflects this fill
    if shares_i64 > 0 {
        let price_cents = ((cost_i64 * 100) / shares_i64).clamp(1, 99) as i32;
        let (yes_price, no_price) = if token == "YES" {
            (price_cents, 100 - price_cents)
        } else {
            (100 - price_cents, price_cents)
        };

        let _ = sqlx::query(
            r#"
            INSERT INTO price_snapshots (market_id, yes_price, no_price, volume_24h)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(market_id.as_u64() as i32)
        .bind(yes_price)
        .bind(no_price)
        .bind(cost_i64)
        .execute(pool)
        .await;
    }

    tracing::info!(
        market_id = %market_id,
        buyer = %format_address(buyer),
        token = %token,
        shares = %shares,
        cost = %cost,
        "OrderFilled processed"
    );
    Ok(())
}

async fn process_market_resolved(pool: &PgPool, log: &Log) -> Result<()> {
    let market_id = log
        .topics
        .get(1)
        .map(|t| U256::from(t.as_bytes()))
        .unwrap_or_default();

    let outcome = if log.data.0.len() >= 32 {
        U256::from_big_endian(&log.data.0[0..32]).as_u64()
    } else {
        1
    };
    let outcome_str = if outcome == 1 { "YES" } else { "NO" };

    sqlx::query(
        r#"
        UPDATE markets SET status = 'resolved', outcome = $1, resolved_at = NOW()
        WHERE market_id = $2
        "#,
    )
    .bind(outcome_str)
    .bind(market_id.as_u64() as i32)
    .execute(pool)
    .await?;

    tracing::info!(market_id = %market_id, outcome = %outcome_str, "MarketResolved processed");
    Ok(())
}

async fn process_market_cancelled(pool: &PgPool, log: &Log) -> Result<()> {
    let market_id = log
        .topics
        .get(1)
        .map(|t| U256::from(t.as_bytes()))
        .unwrap_or_default();

    sqlx::query("UPDATE markets SET status = 'cancelled' WHERE market_id = $1")
        .bind(market_id.as_u64() as i32)
        .execute(pool)
        .await?;

    tracing::info!(market_id = %market_id, "MarketCancelled processed");
    Ok(())
}

async fn process_winnings_redeemed(pool: &PgPool, log: &Log) -> Result<()> {
    let market_id = log
        .topics
        .get(1)
        .map(|t| U256::from(t.as_bytes()))
        .unwrap_or_default();
    let user = log
        .topics
        .get(2)
        .map(|t| Address::from_slice(&t.as_bytes()[12..]))
        .unwrap_or_default();

    let amount = if log.data.0.len() >= 32 {
        U256::from_big_endian(&log.data.0[0..32]).as_u64()
    } else {
        0
    };

    let tx_hash = format_tx_hash(log.transaction_hash);
    let market_id_i32 = market_id.as_u64() as i32;
    let user_addr = format_address(user);
    let amount_i64 = amount as i64;

    // Track redemption in the trades table as a "REDEEM" entry
    let _ = sqlx::query(
        r#"
        INSERT INTO trades (market_id, buyer_address, token, shares, cost, tx_hash, block_number)
        VALUES ($1, $2, 'REDEEM', $3, $3, $4, $5)
        ON CONFLICT (tx_hash, market_id, buyer_address, token) DO NOTHING
        "#,
    )
    .bind(market_id_i32)
    .bind(&user_addr)
    .bind(amount_i64)
    .bind(&tx_hash)
    .bind(log.block_number.map(|b| b.as_u64() as i64))
    .execute(pool)
    .await;

    // Deduct redeemed shares from user_positions.
    // Determine winning token from market outcome.
    let winning_token: Option<String> = sqlx::query_scalar(
        "SELECT outcome FROM markets WHERE market_id = $1",
    )
    .bind(market_id_i32)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    if let Some(token) = winning_token {
        let _ = sqlx::query(
            r#"
            UPDATE user_positions
            SET shares = GREATEST(shares - $1, 0),
                cost   = GREATEST(cost - $1, 0),
                updated_at = NOW()
            WHERE user_address = $2 AND market_id = $3 AND token = $4
            "#,
        )
        .bind(amount_i64)
        .bind(&user_addr)
        .bind(market_id_i32)
        .bind(&token)
        .execute(pool)
        .await;
    }

    tracing::info!(market_id = %market_id, user = %user_addr, amount = %amount, "WinningsRedeemed processed");
    Ok(())
}

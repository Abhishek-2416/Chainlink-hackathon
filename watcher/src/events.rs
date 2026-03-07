//! Event parsing and DB handlers for PredictionMarket events.
//! Events sourced from contracts/src/PredictionMarket.sol.

use anyhow::Result;
use ethers::types::{Address, Log, H256, U256};
use sqlx::PgPool;
use std::str::FromStr;
use tracing::instrument;

/// Event topic0 (keccak256 of event signature)
fn market_created_topic() -> H256 {
    H256::from_str("0x525ad00586a161c11070e5c5de95323165a78ae0137615b2f698ba5a527d458b").unwrap()
}
fn order_filled_topic() -> H256 {
    H256::from_str("0xca339ebcfccd32204847ddc1f4b16f9abff9074ac268024a6f72449545ecc218").unwrap()
}
fn market_resolved_topic() -> H256 {
    H256::from_str("0x739f283563fb51ab6b89ee95d937b2e63a6cfcb83c385dbebb629f9d97bd43e6").unwrap()
}
fn market_cancelled_topic() -> H256 {
    H256::from_str("0x2ca440fb7fca85d7f55d395a4abd94817330b83a62f3502efbb4770144e4ca97").unwrap()
}
fn winnings_redeemed_topic() -> H256 {
    H256::from_str("0xa5b5f999d356bb14d51114327e0f0eefeed3a4083169d023dd7fe6744ca15174").unwrap()
}

fn format_address(a: Address) -> String {
    format!("{:?}", a).to_lowercase()
}

#[instrument(skip(pool, log), fields(tx = ?log.transaction_hash,
    block = ?log.block_number))]
pub async fn process_log(pool: &PgPool, log: &Log) -> Result<()> {
    let topic0 = log.topics.get(0).copied().unwrap_or_default();

    if topic0 == market_created_topic() {
        process_market_created(pool, log).await?;
    } else if topic0 == order_filled_topic() {
        process_order_filled(pool, log).await?;
    } else if topic0 == market_resolved_topic() {
        process_market_resolved(pool, log).await?;
    } else if topic0 == market_cancelled_topic() {
        process_market_cancelled(pool, log).await?;
    } else if topic0 == winnings_redeemed_topic() {
        process_winnings_redeemed(pool, log).await?;
    }

    Ok(())
}

async fn process_market_created(pool: &PgPool, log: &Log) -> Result<()> {
    // topics[1] = marketId (indexed)
    let market_id = log
        .topics
        .get(1)
        .map(|t| U256::from(t.as_bytes()))
        .unwrap_or_default();

    // data = questionHash, creator, yesToken, noToken, resolutionTimestamp (32 bytes each, 5 * 32 = 160)
    let data = &log.data.0;
    if data.len() < 160 {
        tracing::warn!("MarketCreated: insufficient data len {}", data.len());
        return Ok(());
    }

    let yes_token = Address::from_slice(&data[76..96]);
    let no_token = Address::from_slice(&data[108..128]);

    let tx_hash = format!("{:?}", log.transaction_hash).to_lowercase();
    let market_id_num = market_id.as_u64() as i32;

    // Update existing market (created by API) or insert if from external source
    let result = sqlx::query(
        r#"
        UPDATE markets
        SET tx_hash = $1, yes_token_address = $2, no_token_address = $3
        WHERE market_id = $4
        "#,
    )
    .bind(&tx_hash)
    .bind(format_address(yes_token))
    .bind(format_address(no_token))
    .bind(market_id_num)
    .execute(pool)
    .await?;

    if result.rows_affected() == 0 {
        tracing::debug!(
            "MarketCreated: market_id={} not in DB (backend creates first), tx_hash={}",
            market_id_num,
            tx_hash
        );
    }

    tracing::info!("MarketCreated: market_id={} tx={}", market_id_num, tx_hash);
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
    if data.len() < 64 {
        return Ok(());
    }
    let shares = U256::from_big_endian(&data[0..32]);
    let cost = U256::from_big_endian(&data[32..64]);

    let outcome = if data.len() >= 96 {
        U256::from_big_endian(&data[64..96]).as_u64()
    } else {
        1
    };
    let token = if outcome == 1 { "YES" } else { "NO" };

    let tx_hash = format!("{:?}", log.transaction_hash).to_lowercase();
    let block_number = log.block_number.map(|b| b.as_u64() as i64);

    // Insert trade (ignore duplicate on same tx+market+buyer+shares)
    let _ = sqlx::query(
        r#"
        INSERT INTO trades (market_id, buyer_address, token, shares, cost, tx_hash, block_number)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
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

    tracing::info!(
        "OrderFilled: market_id={} buyer={} token={} shares={}",
        market_id,
        format_address(buyer),
        token,
        shares
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

    tracing::info!(
        "MarketResolved: market_id={} outcome={}",
        market_id,
        outcome_str
    );
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

    tracing::info!("MarketCancelled: market_id={}", market_id);
    Ok(())
}

async fn process_winnings_redeemed(_pool: &PgPool, log: &Log) -> Result<()> {
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

    tracing::info!(
        "WinningsRedeemed: market_id={} user={} amount={}",
        market_id,
        format_address(user),
        amount
    );
    // Optional: could insert into a redemptions table for analytics
    Ok(())
}

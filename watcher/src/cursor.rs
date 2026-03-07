use anyhow::Result;
use sqlx::PgPool;
use std::sync::atomic::{AtomicU64, Ordering};

/// Tracks last processed block per chain. Uses in-memory + DB sync.
pub struct Cursor {
    pool: PgPool,
    chain_id: i32,
    contract_address: String,
    last_block: AtomicU64,
}

impl Cursor {
    pub fn new(pool: PgPool, chain_id: u64, contract_address: String) -> Self {
        Self {
            pool,
            chain_id: chain_id as i32,
            contract_address,
            last_block: AtomicU64::new(0),
        }
    }

    pub async fn load(&self) -> Result<u64> {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT last_block FROM watcher_cursor WHERE chain_id = $1")
                .bind(self.chain_id)
                .fetch_optional(&self.pool)
                .await?;

        let block = row.map(|(b,)| b as u64).unwrap_or(0);
        self.last_block.store(block, Ordering::SeqCst);
        Ok(block)
    }

    pub fn get(&self) -> u64 {
        self.last_block.load(Ordering::SeqCst)
    }

    pub async fn set(&self, block: u64) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO watcher_cursor (chain_id, contract_address, last_block, updated_at)
            VALUES ($1, $2, $3, NOW())
            ON CONFLICT (chain_id) DO UPDATE SET last_block = $3, updated_at = NOW()
            "#,
        )
        .bind(self.chain_id)
        .bind(&self.contract_address)
        .bind(block as i64)
        .execute(&self.pool)
        .await?;

        self.last_block.store(block, Ordering::SeqCst);
        Ok(())
    }
}

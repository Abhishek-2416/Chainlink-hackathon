use crate::error::AppError;
use crate::services::blockchain::BlockchainService;
use crate::services::orderbook::OrderbookService;
use crate::types::{Eip712Domain, Eip712TypedData, OrderPayload, QuoteResponse};
use ethers::types::Bytes;
use sqlx::PgPool;

pub struct OrderService {
    db: PgPool,
}

impl OrderService {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    /// Build an EIP-712 quote payload for the frontend to sign.
    ///
    /// Pricing: current implied YES/NO price from the orderbook (VWAP / last
    /// trade) → shares = floor(cost / (price / 100)).
    pub async fn create_quote(
        &self,
        market_id: i32,
        token: &str,
        cost: i64,
        user_address: &str,
        recipient: Option<&str>,
        blockchain: &BlockchainService,
        orderbook: &OrderbookService,
    ) -> Result<QuoteResponse, AppError> {
        // outcome: 1 = Yes, 2 = No (matches contract Outcome enum)
        let outcome = if token.to_uppercase() == "YES" { 1u8 } else { 2u8 };

        let price_cents = orderbook.get_token_price(market_id, token).await?;
        let shares = OrderbookService::cost_to_shares(cost, price_cents);
        if shares <= 0 {
            return Err(AppError::InvalidOrder(format!(
                "Cannot compute shares from cost={cost} at price={price_cents}¢"
            )));
        }

        let to = recipient.unwrap_or(user_address);
        let nonce = blockchain
            .get_nonce(user_address)
            .await
            .map_err(|e| AppError::Blockchain(format!("get_nonce: {e}")))?;
        let deadline = chrono::Utc::now().timestamp() + 3600; // 1-hour window

        let contract_order = blockchain.build_order(
            market_id as u64,
            outcome,
            to,
            shares as u64,
            cost as u64,
            deadline as u64,
            nonce,
        );

        // Computed locally from the pre-built domain separator — no RPC call needed.
        let digest = blockchain.get_order_digest(&contract_order);

        let contract_address = std::env::var("PREDICTION_MARKET_ADDRESS")
            .unwrap_or_else(|_| "0x45e7911Af8c31bDeDf8A586BeEd8efEcACEb9c37".into());

        // Full EIP-712 typed data — pass verbatim to eth_signTypedData_v4
        let types = serde_json::json!({
            "EIP712Domain": [
                { "name": "name",              "type": "string"  },
                { "name": "version",           "type": "string"  },
                { "name": "chainId",           "type": "uint256" },
                { "name": "verifyingContract", "type": "address" }
            ],
            "Order": [
                { "name": "marketId",  "type": "uint256" },
                { "name": "outcome",   "type": "uint8"   },
                { "name": "to",        "type": "address" },
                { "name": "shares",    "type": "uint256" },
                { "name": "cost",      "type": "uint256" },
                { "name": "deadline",  "type": "uint256" },
                { "name": "nonce",     "type": "uint256" }
            ]
        });

        // Stringify large numbers to prevent JS precision loss
        let message = serde_json::json!({
            "marketId":  market_id.to_string(),
            "outcome":   outcome.to_string(),
            "to":        to,
            "shares":    shares.to_string(),
            "cost":      cost.to_string(),
            "deadline":  deadline.to_string(),
            "nonce":     nonce.to_string()
        });

        Ok(QuoteResponse {
            order: OrderPayload {
                market_id: market_id as u64,
                outcome,
                to: to.to_string(),
                shares: shares as u64,
                cost: cost as u64,
                deadline: deadline as u64,
                nonce,
                price_cents,
            },
            order_digest: format!("0x{}", hex::encode(digest)),
            signing_payload: Eip712TypedData {
                types,
                primary_type: "Order".into(),
                domain: Eip712Domain {
                    name: "PredictionMarket".into(),
                    version: "1".into(),
                    chain_id: blockchain.chain_id(),
                    verifying_contract: contract_address,
                },
                message,
            },
        })
    }

    /// Verify + persist a signed order, then immediately attempt on-chain fill.
    ///
    /// Returns `(order_db_id, status_string, optional_tx_hash)`.
    pub async fn submit_order(
        &self,
        market_id: i32,
        token: &str,
        shares: i64,
        cost: i64,
        price: i32,
        nonce: i64,
        deadline: i64,
        signature_hex: &str,
        user_address: &str,
        recipient: Option<&str>,
        blockchain: &BlockchainService,
        orderbook: &OrderbookService,
    ) -> Result<(i32, String, Option<String>), AppError> {
        if chrono::Utc::now().timestamp() > deadline {
            return Err(AppError::OrderExpired);
        }

        let outcome = if token.to_uppercase() == "YES" { 1u8 } else { 2u8 };
        let to = recipient.unwrap_or(user_address);

        let sig_bytes = hex::decode(signature_hex.trim_start_matches("0x"))
            .map_err(|_| AppError::InvalidSignature)?;
        if sig_bytes.len() != 65 && sig_bytes.len() != 64 {
            return Err(AppError::InvalidSignature);
        }

        // Persist with status=pending
        let row: (i32,) = sqlx::query_as(
            r#"
            INSERT INTO orders
                (market_id, user_address, token, shares, cost, price, nonce, signature, status)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'pending')
            RETURNING id
            "#,
        )
        .bind(market_id)
        .bind(user_address)
        .bind(token.to_uppercase())
        .bind(shares)
        .bind(cost)
        .bind(price)
        .bind(nonce)
        .bind(&sig_bytes)
        .fetch_one(&self.db)
        .await
        .map_err(AppError::Db)?;

        let order_id = row.0;

        let contract_order = blockchain.build_order(
            market_id as u64,
            outcome,
            to,
            shares as u64,
            cost as u64,
            deadline as u64,
            nonce as u64,
        );
        let signature = Bytes::from(sig_bytes);

        let tx_hash = match blockchain.fill_order(contract_order, signature).await {
            Ok(h) => {
                let tx_hex = format!("{h:#x}");
                sqlx::query(
                    r#"
                    UPDATE orders
                    SET status = 'filled', tx_hash = $1, filled_at = NOW()
                    WHERE id = $2
                    "#,
                )
                .bind(&tx_hex)
                .bind(order_id)
                .execute(&self.db)
                .await
                .map_err(AppError::Db)?;

                // Snapshot the fill price so future quotes reflect this trade
                orderbook
                    .record_price_snapshot(market_id, price, token, cost)
                    .await
                    .ok(); // non-fatal if snapshot fails

                Some(tx_hex)
            }
            Err(e) => {
                tracing::warn!("fillOrder on-chain failed (order queued pending): {e}");
                None
            }
        };

        let status = if tx_hash.is_some() { "filled" } else { "pending" };
        Ok((order_id, status.to_string(), tx_hash))
    }

    /// All orders for a user, optionally filtered by market.
    pub async fn get_user_orders(
        &self,
        user_address: &str,
        market_id: Option<i32>,
    ) -> Result<Vec<UserOrderRow>, AppError> {
        let rows = if let Some(mid) = market_id {
            sqlx::query_as::<_, UserOrderRow>(
                r#"
                SELECT id, market_id, token, shares, cost, price,
                       status, created_at, filled_at, tx_hash
                FROM orders
                WHERE user_address = $1 AND market_id = $2
                ORDER BY created_at DESC
                "#,
            )
            .bind(user_address)
            .bind(mid)
            .fetch_all(&self.db)
            .await
        } else {
            sqlx::query_as::<_, UserOrderRow>(
                r#"
                SELECT id, market_id, token, shares, cost, price,
                       status, created_at, filled_at, tx_hash
                FROM orders
                WHERE user_address = $1
                ORDER BY created_at DESC
                "#,
            )
            .bind(user_address)
            .fetch_all(&self.db)
            .await
        }
        .map_err(AppError::Db)?;

        Ok(rows)
    }

    /// Record an action in the action_mapper table for tracking.
    pub async fn record_action(
        &self,
        action_type: &str,
        user_address: &str,
        market_id: Option<i32>,
        order_id: Option<i32>,
        required_tx: &str,
        payload: Option<serde_json::Value>,
    ) -> Result<i32, AppError> {
        let row: (i32,) = sqlx::query_as(
            r#"
            INSERT INTO action_mapper
                (action_type, user_address, market_id, order_id, required_tx, payload, status)
            VALUES ($1, $2, $3, $4, $5, $6, 'pending')
            RETURNING id
            "#,
        )
        .bind(action_type)
        .bind(user_address)
        .bind(market_id)
        .bind(order_id)
        .bind(required_tx)
        .bind(payload)
        .fetch_one(&self.db)
        .await
        .map_err(AppError::Db)?;

        Ok(row.0)
    }
}

#[derive(Debug, serde::Serialize, sqlx::FromRow)]
pub struct UserOrderRow {
    pub id: i32,
    pub market_id: i32,
    pub token: String,
    pub shares: i64,
    pub cost: i64,
    pub price: i32,
    pub status: String,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub filled_at: Option<chrono::DateTime<chrono::Utc>>,
    pub tx_hash: Option<String>,
}

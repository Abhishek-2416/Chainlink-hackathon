use serde::{Deserialize, Serialize};

// ── Request bodies ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRequest {
    pub market_id: i32,
    /// "YES" or "NO"
    pub token: String,
    /// USDC amount in smallest unit (6 decimals), e.g. $5 = 5_000_000
    pub cost: i64,
    pub user_address: String,
    #[serde(default)]
    pub recipient_address: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitOrderRequest {
    pub market_id: i32,
    pub token: String,
    /// Shares to receive (6 decimals)
    pub shares: i64,
    /// USDC cost (6 decimals)
    pub cost: i64,
    /// Price in cents (1–99); the effective probability
    pub price: i32,
    pub nonce: i64,
    /// Unix timestamp
    pub deadline: i64,
    /// EIP-712 signature hex, 65 bytes: 0x{r}{s}{v}
    pub signature: String,
    pub user_address: String,
    #[serde(default)]
    pub recipient_address: Option<String>,
}

// ── Response bodies ───────────────────────────────────────────────────────────

/// The order struct the user must sign (returned by /quote).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderPayload {
    pub market_id: u64,
    /// 1 = Yes, 2 = No
    pub outcome: u8,
    /// Token recipient
    pub to: String,
    pub shares: u64,
    pub cost: u64,
    pub deadline: u64,
    pub nonce: u64,
    /// The implied price in cents that was used to compute shares (informational)
    pub price_cents: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuoteResponse {
    pub order: OrderPayload,
    /// 0x-prefixed keccak256 EIP-712 digest (for display / verification)
    pub order_digest: String,
    /// Pass verbatim to eth_signTypedData_v4 / MetaMask signTypedData
    pub signing_payload: Eip712TypedData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Eip712TypedData {
    pub types: serde_json::Value,
    pub primary_type: String,
    pub domain: Eip712Domain,
    pub message: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Eip712Domain {
    pub name: String,
    pub version: String,
    pub chain_id: u64,
    pub verifying_contract: String,
}

// ── Orderbook ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderbookLevel {
    /// Price in cents (1–99)
    pub price: i32,
    /// Total pending shares at this price level
    pub shares: i64,
    /// Number of individual orders at this level
    pub orders: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderbookResponse {
    pub market_id: i32,
    /// YES buy orders (bid side), sorted price DESC
    pub bids: Vec<OrderbookLevel>,
    /// NO buy orders, sorted price DESC
    pub no_bids: Vec<OrderbookLevel>,
    /// Last fill price for YES in cents (None if no trades yet)
    pub last_price: Option<i32>,
    /// Current YES price in cents — this is the cost per share in %
    pub yes_price: i32,
    /// Current NO price in cents (= 100 - yes_price)
    pub no_price: i32,
}

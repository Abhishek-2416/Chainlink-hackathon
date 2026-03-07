use anyhow::Result;
use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub rpc_url: String,
    pub prediction_market_address: String,
    pub chain_id: u64,
    pub batch_size: u64,
    pub poll_interval_ms: u64,
    pub port: u16,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();

        let database_url = env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgresql://localhost:5432/prediction_market".to_string());
        let rpc_url = env::var("RPC_URL")
            .unwrap_or_else(|_| "https://ethereum-sepolia-rpc.publicnode.com".to_string());
        let prediction_market_address = env::var("PREDICTION_MARKET_ADDRESS")
            .unwrap_or_else(|_| "0x45e7911Af8c31bDeDf8A586BeEd8efEcACEb9c37".to_string());
        let chain_id: u64 = env::var("CHAIN_ID")
            .unwrap_or_else(|_| "11155111".to_string())
            .parse()?;
        let batch_size: u64 = env::var("BATCH_SIZE")
            .unwrap_or_else(|_| "2000".to_string())
            .parse()?;
        let poll_interval_ms: u64 = env::var("POLL_INTERVAL_MS")
            .unwrap_or_else(|_| "1000".to_string())
            .parse()?;
        let port: u16 = env::var("PORT")
            .unwrap_or_else(|_| "3002".to_string())
            .parse()?;

        Ok(Self {
            database_url,
            rpc_url,
            prediction_market_address,
            chain_id,
            batch_size,
            poll_interval_ms,
            port,
        })
    }
}

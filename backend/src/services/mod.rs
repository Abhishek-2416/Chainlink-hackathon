pub mod blockchain;
pub mod orderbook;
pub mod orders;

pub use blockchain::BlockchainService;
pub use orderbook::OrderbookService;
pub use orders::OrderService;

use crate::db::DbPool;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub db: DbPool,
    pub blockchain: Arc<BlockchainService>,
    pub orders: Arc<OrderService>,
    pub orderbook: Arc<OrderbookService>,
}

impl AppState {
    pub async fn new(db: DbPool) -> anyhow::Result<Self> {
        let blockchain = Arc::new(BlockchainService::new().await?);
        let orders = Arc::new(OrderService::new(db.clone()));
        let orderbook = Arc::new(OrderbookService::new(db.clone()));

        Ok(Self {
            db,
            blockchain,
            orders,
            orderbook,
        })
    }
}

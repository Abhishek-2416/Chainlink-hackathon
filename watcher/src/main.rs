mod config;
mod cursor;
mod db;
mod events;

use anyhow::Result;
use ethers::middleware::Middleware;
use ethers::providers::{Http, Provider};
use ethers::types::{Address, BlockNumber, Filter, Log, U64};
use std::sync::Arc;
use tokio::time::{interval, Duration};
use tracing::{info, instrument};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("watcher=info".parse()?)
                .add_directive("info".parse()?),
        )
        .with_target(true)
        .with_thread_ids(false)
        .init();

    let config = config::Config::from_env()?;
    let pool = db::create_pool(&config.database_url).await?;

    let provider = Provider::<Http>::try_from(&config.rpc_url)?;
    let provider = Arc::new(provider);

    let contract_addr: Address = config.prediction_market_address.parse()?;
    let cursor = cursor::Cursor::new(
        pool.clone(),
        config.chain_id,
        config.prediction_market_address.clone(),
    );
    cursor.load().await?;
    let cursor = Arc::new(cursor);

    // Start HTTP server for health/status
    let app_state = AppState {
        cursor: cursor.clone(),
        chain_id: config.chain_id,
    };
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", config.port)).await?;
    let server = axum::serve(
        listener,
        axum::Router::new()
            .route("/health", axum::routing::get(health))
            .route("/status", axum::routing::get(status))
            .with_state(app_state),
    );

    let watcher_handle = tokio::spawn(async move {
        run_watcher(
            provider,
            pool,
            cursor,
            contract_addr,
            config.chain_id,
            config.batch_size,
            config.poll_interval_ms,
        )
        .await
    });

    tokio::select! {
        r = server => {
            if let Err(e) = r {
                tracing::error!("HTTP server error: {}", e);
            }
        }
        r = watcher_handle => {
            if let Err(e) = r {
                tracing::error!("Watcher error: {:?}", e);
            }
        }
    }

    Ok(())
}

#[derive(Clone)]
struct AppState {
    cursor: Arc<cursor::Cursor>,
    chain_id: u64,
}

async fn health() -> &'static str {
    "ok"
}

async fn status(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> impl axum::response::IntoResponse {
    axum::Json(serde_json::json!({
        "chain_id": state.chain_id,
        "last_block": state.cursor.get(),
    }))
}

#[instrument(skip(provider, pool, cursor))]
async fn run_watcher(
    provider: Arc<Provider<Http>>,
    pool: sqlx::PgPool,
    cursor: Arc<cursor::Cursor>,
    contract_addr: Address,
    _chain_id: u64,
    batch_size: u64,
    poll_interval_ms: u64,
) -> Result<()> {
    let mut from_block = cursor.get();
    if from_block == 0 {
        let current = provider.get_block_number().await?.as_u64();
        from_block = current.saturating_sub(1000);
        info!("Bootstrap: starting from block {}", from_block);
    }

    let mut ticker = interval(Duration::from_millis(poll_interval_ms));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;

        let current = provider.get_block_number().await?.as_u64();
        let to_block = (from_block + batch_size).min(current);

        if from_block > to_block {
            continue;
        }

        let filter = Filter::new()
            .address(contract_addr)
            .from_block(BlockNumber::Number(U64::from(from_block)))
            .to_block(BlockNumber::Number(U64::from(to_block)));

        match provider.get_logs(&filter).await {
            Ok(logs) => {
                let logs: Vec<Log> = logs;
                let count = logs.len();
                for log in &logs {
                    if let Err(e) = events::process_log(&pool, log).await {
                        tracing::error!(
                            tx_hash = ?log.transaction_hash,
                            block = ?log.block_number,
                            "Event processing failed: {:?}",
                            e
                        );
                    }
                }
                cursor.set(to_block).await?;
                if count > 0 {
                    info!(from_block, to_block, count, "Processed events");
                }
                from_block = to_block + 1;
            }
            Err(e) => {
                tracing::error!(from_block, to_block, "get_logs failed: {}", e);
            }
        }
    }
}

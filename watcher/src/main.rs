mod config;
mod cursor;
mod db;
mod events;

use anyhow::Result;
use ethers::middleware::Middleware;
use ethers::providers::{Http, Provider};
use ethers::types::{Address, BlockNumber, Filter, Log, U64};
use std::sync::Arc;
use tokio::time::{interval, sleep, Duration};
use tracing::{info, warn, instrument};

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
        .with_file(false)
        .with_line_number(false)
        .init();

    tracing::info!("Watcher starting");
    let config = config::Config::from_env()?;
    tracing::info!(rpc_url = %config.rpc_url, chain_id = config.chain_id, "config loaded");

    tracing::info!("Connecting to database");
    let pool = db::create_pool(&config.database_url).await?;
    tracing::info!("Database connected");

    tracing::info!("Connecting to RPC");
    let provider = Provider::<Http>::try_from(&config.rpc_url)?;
    let provider = Arc::new(provider);

    let contract_addr: Address = config.prediction_market_address.parse()?;
    let cursor = cursor::Cursor::new(
        pool.clone(),
        config.chain_id,
        config.prediction_market_address.clone(),
    );
    let last_block = cursor.load().await?;
    tracing::info!(last_block = last_block, "cursor loaded");
    let cursor = Arc::new(cursor);

    // Start HTTP server for health/status
    let app_state = AppState {
        cursor: cursor.clone(),
        chain_id: config.chain_id,
        pool: pool.clone(),
        provider: provider.clone(),
    };
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", config.port)).await?;
    tracing::info!(port = config.port, "HTTP server listening");
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
    pool: sqlx::PgPool,
    provider: Arc<Provider<Http>>,
}

async fn health(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> impl axum::response::IntoResponse {
    match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.pool)
        .await
    {
        Ok(_) => {
            tracing::debug!("health check ok");
            (axum::http::StatusCode::OK, "ok")
        }
        Err(e) => {
            tracing::warn!(error = %e, "health check failed: db unreachable");
            (axum::http::StatusCode::SERVICE_UNAVAILABLE, "db unreachable")
        }
    }
}

async fn status(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> impl axum::response::IntoResponse {
    tracing::debug!("status request");
    let last_block = state.cursor.get();
    let current_block = state
        .provider
        .get_block_number()
        .await
        .map(|b| b.as_u64())
        .unwrap_or(0);
    let blocks_behind = current_block.saturating_sub(last_block);

    axum::Json(serde_json::json!({
        "chain_id": state.chain_id,
        "last_block": last_block,
        "current_block": current_block,
        "blocks_behind": blocks_behind,
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
        // Bootstrap: retry getting current block with backoff
        let current = retry_get_block_number(&provider).await?;
        from_block = current.saturating_sub(1000);
        // Persist bootstrap block so restarts don't re-bootstrap
        cursor.set(from_block).await?;
        info!("Bootstrap: starting from block {}", from_block);
    }

    let mut ticker = interval(Duration::from_millis(poll_interval_ms));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Consecutive RPC failure counter for exponential backoff
    let mut consecutive_failures: u32 = 0;
    const MAX_BACKOFF_SECS: u64 = 60;

    loop {
        ticker.tick().await;

        // If we had recent failures, apply exponential backoff
        if consecutive_failures > 0 {
            let backoff_secs = (2u64.pow(consecutive_failures.min(6))).min(MAX_BACKOFF_SECS);
            warn!(
                consecutive_failures,
                backoff_secs, "Backing off before retry"
            );
            sleep(Duration::from_secs(backoff_secs)).await;
        }

        // Handle get_block_number failure gracefully instead of crashing
        let current = match provider.get_block_number().await {
            Ok(n) => n.as_u64(),
            Err(e) => {
                consecutive_failures += 1;
                tracing::error!("get_block_number failed (attempt {}): {}", consecutive_failures, e);
                continue;
            }
        };
        let to_block = (from_block + batch_size).min(current);

        if from_block > to_block {
            consecutive_failures = 0; // RPC is healthy, reset counter
            continue;
        }

        tracing::debug!(from_block, to_block, "fetching logs");
        let filter = Filter::new()
            .address(contract_addr)
            .from_block(BlockNumber::Number(U64::from(from_block)))
            .to_block(BlockNumber::Number(U64::from(to_block)));

        match provider.get_logs(&filter).await {
            Ok(logs) => {
                consecutive_failures = 0; // Reset on success
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
                    info!(from_block, to_block, count, "processed events");
                } else {
                    tracing::trace!(from_block, to_block, "no events in range");
                }
                from_block = to_block + 1;
            }
            Err(e) => {
                consecutive_failures += 1;
                tracing::error!(
                    from_block,
                    to_block,
                    attempt = consecutive_failures,
                    "get_logs failed: {}",
                    e
                );
                // Do NOT advance from_block — retry the same range next iteration
            }
        }
    }
}

/// Retry get_block_number with exponential backoff (for bootstrap).
async fn retry_get_block_number(provider: &Provider<Http>) -> Result<u64> {
    let mut attempts = 0u32;
    loop {
        match provider.get_block_number().await {
            Ok(n) => return Ok(n.as_u64()),
            Err(e) => {
                attempts += 1;
                if attempts >= 10 {
                    return Err(anyhow::anyhow!(
                        "Failed to get block number after {} attempts: {}",
                        attempts,
                        e
                    ));
                }
                let backoff = Duration::from_secs(2u64.pow(attempts.min(5)));
                warn!("get_block_number failed (attempt {}), retrying in {:?}: {}", attempts, backoff, e);
                sleep(backoff).await;
            }
        }
    }
}

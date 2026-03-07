mod db;
mod error;
mod routes;
mod services;
mod types;

use axum::{routing::get, Router};
use std::net::SocketAddr;
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use db::DbPool;
use routes::{actions, markets, orders, portfolio};
use services::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let database_url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set");
    let pool = DbPool::connect(&database_url).await?;

    sqlx::migrate!("./migrations").run(&pool).await?;

    let state = AppState::new(pool).await?;

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health))
        .nest("/api/markets", markets::router())
        .nest("/api/orders", orders::router())
        .nest("/api/portfolio", portfolio::router())
        .nest("/api/actions", actions::router())
        .layer(cors)
        .with_state(state);

    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "3001".into())
        .parse()?;
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    tracing::info!("Listening on {}", addr);
    axum::serve(
        tokio::net::TcpListener::bind(addr).await?,
        app,
    )
    .await?;

    Ok(())
}

async fn health() -> &'static str {
    "OK"
}

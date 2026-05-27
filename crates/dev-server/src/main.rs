//! Local dev server — `vercel dev` doesn't support the official Rust runtime
//! (plan §12.1), so we mount the production Axum router on :3000 ourselves.
//!
//! Boot:
//! 1. Connect to Postgres (env `DATABASE_URL`, defaults to local docker).
//! 2. Run all pending migrations.
//! 3. If `ETH_TOOLS_AUTO_SEED=1` (default-on in dev), apply `fixtures/seed.sql`
//!    so `/api/v1/agents` returns rows on first hit.
//! 4. Build `AppState` + mount `eth_tools_api::router`.
//!
//! Try it:
//!   `docker compose up -d postgres && cargo run -p eth-tools-dev-server`
//!   then `curl http://127.0.0.1:3000/api/v1/agents | jq`.

use std::net::SocketAddr;

const SEED_SQL: &str = include_str!("../../../fixtures/seed.sql");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_target(false).init();

    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://dev:dev@localhost:5432/eth_tools?sslmode=disable".into());
    tracing::info!(database_url = %url.split('@').next_back().unwrap_or("…"), "connecting");

    let pool = eth_tools_db::connect(&url).await?;
    eth_tools_db::migrate(&pool).await?;

    let auto_seed = std::env::var("ETH_TOOLS_AUTO_SEED")
        .map(|s| s == "1" || s == "true")
        .unwrap_or(true);
    if auto_seed {
        sqlx::raw_sql(SEED_SQL).execute(&pool).await?;
        tracing::info!("applied fixtures/seed.sql (disable with ETH_TOOLS_AUTO_SEED=0)");
    }

    let app = eth_tools_api::router(eth_tools_api::AppState::new(pool));
    let addr: SocketAddr = "127.0.0.1:3000".parse()?;
    tracing::info!("dev-server listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

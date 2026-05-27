//! Local dev server — `vercel dev` doesn't support the official Rust runtime
//! (plan §12.1), so we mount the production Axum handlers on :3000 ourselves.
//!
//! Usage: `cargo run -p eth-tools-dev-server` then visit http://localhost:3000/api/v1/health

use std::net::SocketAddr;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    let app = eth_tools_api::router();
    let addr: SocketAddr = "127.0.0.1:3000".parse()?;
    tracing::info!("dev-server listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

//! Cron worker W4 — aggregate per-agent reputation summaries.
//! Cadence: every 5 minutes (vercel.json).

use vercel_runtime::{run, Body, Error, Request, Response};

const WORKER_NAME: &str = "reputation_aggregator";
const BASE_MAINNET_CHAIN_ID: u64 = 8453;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_ansi(false)
        .json()
        .init();
    run(handler).await
}

async fn handler(req: Request) -> Result<Response<Body>, Error> {
    Ok(eth_tools_workers::cron::serve_with_context(WORKER_NAME, req, |ctx| async move {
        eth_tools_workers::reputation_aggregator::run(&ctx, BASE_MAINNET_CHAIN_ID).await
    })
    .await)
}

//! Cron worker W8 — daily wallet balance + worker watchdog.
//! Cadence: midnight UTC (`0 0 * * *`).
//!
//! Wraps `eth_tools_workers::cron::serve_with_context` for the auth/env
//! guards and `eth_tools_workers::wallet_balance_keeper::run` for the
//! balance check + watchdog + cursor-lag scan.

use vercel_runtime::{run, Body, Error, Request, Response};

const WORKER_NAME: &str = "wallet_balance_keeper";

/// The chain we monitor balances on. W8 is a single-chain worker today
/// (Base mainnet). Multi-chain expansion lands when we ship signers
/// per chain.
const TARGET_CHAIN_ID: u64 = 8453;

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
        eth_tools_workers::wallet_balance_keeper::run(&ctx, TARGET_CHAIN_ID).await
    })
    .await)
}

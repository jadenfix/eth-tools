//! Cron worker W6 — invalidate cached agentWallet on Transfer / MetadataSet.
//! Cadence: every minute (the wallet-cache-poisoning fix per spec gotcha #2).
//!
//! Wires the Upstash REST client (or no-op fallback) and hands off to
//! `eth_tools_workers::wallet_rotation_watcher::run`. The worker body
//! does its own cursor management; we just supply pool + rpc + kv.

use vercel_runtime::{run, Body, Error, Request, Response};

const WORKER_NAME: &str = "wallet_rotation_watcher";

/// Single-chain (Base mainnet) today. Multi-chain rotation lands when
/// we expand the indexed chain set.
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
    let kv = eth_tools_workers::kv::from_env_or_noop();
    Ok(eth_tools_workers::cron::serve_with_context(WORKER_NAME, req, move |ctx| {
        let kv = kv.clone();
        async move {
            eth_tools_workers::wallet_rotation_watcher::run(&ctx, TARGET_CHAIN_ID, kv.as_ref()).await
        }
    })
    .await)
}

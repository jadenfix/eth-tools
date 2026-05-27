//! Cron worker W6 — invalidate cached agentWallet on Transfer / MetadataSet.
//! Cadence: every minute (the wallet-cache-poisoning fix per spec gotcha #2).

use vercel_runtime::{run, Body, Error, Request, Response};

const WORKER_NAME: &str = "wallet_rotation_watcher";

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
    eth_tools_workers::cron::serve(WORKER_NAME, req, |_dryrun| async move {
        Ok(eth_tools_workers::WorkerSummary::ok(WORKER_NAME))
    })
    .await
}

//! Cron worker W2 — fetch agentURI manifests, validate, store snapshots.
//! Cadence: every 15 minutes.

use vercel_runtime::{run, Body, Error, Request, Response};

const WORKER_NAME: &str = "manifest_fetcher";

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

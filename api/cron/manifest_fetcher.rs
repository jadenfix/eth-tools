//! Cron worker W2 — fetch agentURI manifests, validate, store snapshots.
//! Cadence: every 15 minutes (vercel.json).
//!
//! The handler is a thin shim over `eth_tools_workers::cron::
//! serve_with_context`. The real worker lives in
//! `eth_tools_workers::manifest_fetcher::run` so it's unit-testable
//! without the Vercel runtime boilerplate.

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
    Ok(eth_tools_workers::cron::serve_with_context(
        WORKER_NAME,
        req,
        |ctx| async move { eth_tools_workers::manifest_fetcher::run(ctx).await },
    )
    .await)
}

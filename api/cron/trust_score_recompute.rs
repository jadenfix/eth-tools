//! Cron worker W7 — materialize trust_scores from reputation + endpoint + manifest.
//! Cadence: hourly (`0 * * * *`).
//!
//! Thin wrapper: defers to `eth_tools_workers::cron::serve_with_context`
//! for env-skip / cron-secret / telemetry, and to
//! `eth_tools_workers::trust_score_recompute::run` for the actual
//! recompute statement.

use vercel_runtime::{run, Body, Error, Request, Response};

const WORKER_NAME: &str = "trust_score_recompute";

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
        eth_tools_workers::trust_score_recompute::run(&ctx).await
    })
    .await)
}

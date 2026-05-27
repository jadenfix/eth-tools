//! Cron worker W3 — probe agent service endpoints for liveness.
//! Cadence: every 30 minutes (vercel.json).
//!
//! Wire-only: the real worker body lives in
//! `eth_tools_workers::endpoint_prober::run`. This entry point exists
//! purely to bind the binary to the `serve_with_context` substrate so the
//! per-worker invariants (cron secret, env-skip, dryrun, telemetry envelope)
//! evaluate uniformly across all 8 workers.

use vercel_runtime::{run, Body, Error, Request, Response};

const WORKER_NAME: &str = "endpoint_prober";

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
        eth_tools_workers::endpoint_prober::run(ctx).await
    })
    .await)
}

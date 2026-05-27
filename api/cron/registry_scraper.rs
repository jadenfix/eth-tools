//! Cron worker W1 — scrape Identity / Reputation / Validation registries.
//! Cadence: every 2 minutes (vercel.json).
//!
//! Bootstrap stub. Per-worker invariants in plan §3:
//!
//! 1. Concurrency lock (pg_try_advisory_lock) — added when DB lands.
//! 2. Env-aware: no-op in non-production.
//! 3. Cron secret header required.
//! 4. Dry-run mode via `?dryrun=1`.
//! 5. Cursor + idempotency + telemetry + lag — added with real impl.

use vercel_runtime::{run, service_fn, Error, Request, Response, ResponseBody};

const WORKER_NAME: &str = "registry_scraper";

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_ansi(false)
        .json()
        .init();
    run(service_fn(handler)).await
}

async fn handler(req: Request) -> Result<Response<ResponseBody>, Error> {
    eth_tools_workers::cron::serve(WORKER_NAME, req, |_dryrun| async move {
        Ok(eth_tools_workers::WorkerSummary::ok(WORKER_NAME))
    })
    .await
}

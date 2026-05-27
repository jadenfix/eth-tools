//! Cron worker W1 — scrape ERC-8004 Identity registry events.
//! Cadence: every 2 minutes (`vercel.json`).
//!
//! Per-worker invariants in plan §3 (all enforced by
//! `eth_tools_workers::cron::serve_with_context`):
//!
//! 1. Concurrency lock (advisory; PR3 follow-up).
//! 2. Env-aware: no-op in non-production unless `?force=1`.
//! 3. Cron secret header required (fail-closed in production).
//! 4. Dry-run mode via `?dryrun=1` (runs reads but skips upserts/cursor).
//! 5. Cursor + idempotency + telemetry + lag — implemented in
//!    [`eth_tools_workers::registry_scraper::run`].
//!
//! The real scraping logic lives in `crates/workers/src/registry_scraper.rs`;
//! this entrypoint only wires Vercel's `lambda_http` glue to the worker body.

use eth_tools_workers::{cron::serve_with_context, registry_scraper};
use vercel_runtime::{run, Body, Error, Request, Response};

const WORKER_NAME: &str = "registry_scraper";

/// Base mainnet is the launch chain (plan §5). W1 is wired to a single chain
/// today; the multi-chain dispatcher lands in v1.x.
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
    // `serve_with_context` returns a `Response<Body>` directly (it never
    // bubbles errors out — any failure is mapped to a 500 with a JSON body).
    // We log inside `map_err` so the original error reaches both the
    // tracing layer (for ops dashboards) AND `worker_runs::finish_err`
    // (for the audit table).
    Ok(serve_with_context(WORKER_NAME, req, |ctx| async move {
        registry_scraper::run(&ctx, BASE_MAINNET_CHAIN_ID)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "registry_scraper run failed");
                e
            })
    })
    .await)
}

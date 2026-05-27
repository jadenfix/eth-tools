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

use vercel_runtime::{run, Body, Error, Request, Response, StatusCode};

const WORKER_NAME: &str = "registry_scraper";

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
    let env = std::env::var("VERCEL_ENV").unwrap_or_else(|_| "development".into());
    let cron_secret_ok = match (
        std::env::var("CRON_SECRET").ok(),
        req.headers().get("x-vercel-cron-secret"),
    ) {
        (Some(expected), Some(got)) => got.as_bytes() == expected.as_bytes(),
        (None, _) => true, // local / preview without the secret configured
        _ => false,
    };
    if !cron_secret_ok {
        return Ok(Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(Body::Text(
                r#"{"error":"missing or invalid x-vercel-cron-secret"}"#.into(),
            ))?);
    }
    let summary = if env != "production" {
        eth_tools_workers::WorkerSummary::skipped(WORKER_NAME, "non-production")
    } else {
        eth_tools_workers::WorkerSummary::ok(WORKER_NAME)
    };
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::Text(serde_json::to_string(&summary)?))?)
}

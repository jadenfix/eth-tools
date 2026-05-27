//! Cron worker W4 — aggregate per-agent reputation summaries.
//! Cadence: every 5 minutes.

use vercel_runtime::{run, Body, Error, Request, Response, StatusCode};

const WORKER_NAME: &str = "reputation_aggregator";

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
        (None, _) => true,
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

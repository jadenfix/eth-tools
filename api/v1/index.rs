//! Vercel function entrypoint for /api/v1/*.
//!
//! **PR #1 scope: bootstrap stub only.** Returns a JSON envelope echoing the
//! request. Real Axum integration with `eth_tools_api::router()` lands in
//! Phase 1 (next PR) along with the `vercel_runtime` 2.x migration (2.x's
//! Service-based API replaces 1.x's closure-based `run(handler)`).
//!
//! Vercel routes `/api/v1/*` to this single binary via `vercel.json` rewrites
//! (`api/v1/(.*)` → `/api/v1/index`). Internal path dispatch happens inside
//! the Axum router once wired.

use vercel_runtime::{run, Body, Error, Request, Response, StatusCode};

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
    tracing::info!(method = %req.method(), uri = %req.uri(), "api_v1 request");
    let body = serde_json::json!({
        "service": "eth-tools",
        "function": "api_v1",
        "status": "bootstrap",
        "method": req.method().as_str(),
        "uri": req.uri().to_string(),
    });
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::Text(body.to_string()))?)
}

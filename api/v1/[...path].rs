//! Vercel function entrypoint — catch-all for /api/v1/*
//!
//! Hands the request to the Axum router defined in `eth-tools-api`.
//! Per plan §11.1: one binary serves every REST route to keep build time and
//! cold-start budget bounded.

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
    // Bootstrap stub: real Axum integration lands in the next PR.
    // We log and return a JSON envelope describing the request the function received.
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

//! Vercel function entrypoint — MCP server, catch-all for /api/mcp/*
//!
//! Per plan §11.1: single function, rmcp + Axum router lands in next PR.

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
    tracing::info!(method = %req.method(), uri = %req.uri(), "api_mcp request");
    let body = serde_json::json!({
        "service": "eth-tools",
        "function": "api_mcp",
        "status": "bootstrap",
        "tools_planned": eth_tools_mcp::TOOL_NAMES,
    });
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::Text(body.to_string()))?)
}

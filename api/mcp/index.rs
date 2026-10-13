//! Vercel function entrypoint for /api/mcp/* — MCP server.
//!
//! **PR #1 scope: bootstrap stub.** Real rmcp + Axum integration lands in
//! Phase 5 alongside the `vercel_runtime` 2.x migration. Vercel routes
//! `/api/mcp/*` here via `vercel.json` rewrites.

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
    if req.method() != http::Method::POST {
        return Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(Body::Empty)?);
    }
    let bytes = match req.into_body() {
        Body::Text(s) => s.into_bytes(),
        Body::Binary(b) => b,
        Body::Empty => Vec::new(),
    };
    let request: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => {
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::Empty)?)
        }
    };
    let base =
        std::env::var("ETH_TOOLS_API_URL").map_err(|_| Error::from("ETH_TOOLS_API_URL required for MCP"))?;
    let result = eth_tools_mcp::handle(request, &base).await;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::Text(result.to_string()))?)
}

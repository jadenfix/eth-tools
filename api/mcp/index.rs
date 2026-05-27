//! Vercel function entrypoint for /api/mcp/* — MCP server.
//!
//! Bridges `vercel_runtime::Request` (1.x) → the shared MCP `axum::Router`
//! built by `eth_tools_mcp::router(state)` → `vercel_runtime::Response<Body>`.
//! Identical lifecycle pattern to `api/v1/index.rs`: at cold-start, lazily
//! build `AppState` once into a `OnceCell` and reuse the router across warm
//! invocations.
//!
//! NOTE: `vercel_runtime` is still pinned at 1.1 on this branch; the
//! plan's Phase-5 "vercel_runtime 2.x" constraint can't be satisfied until
//! the M4 migration lands. Track in the workspace Cargo.toml comment.
//!
//! Vercel routes `/api/mcp/*` here via the `vercel.json` rewrite
//! (`api/mcp/(.*)` → `/api/mcp/index`).

use axum::body::Body as AxumBody;
use http::Request as HttpRequest;
use tokio::sync::OnceCell;
use tower::ServiceExt;
use vercel_runtime::{run, Body, Error, Request, Response};

static APP: OnceCell<axum::Router> = OnceCell::const_new();

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
    let app = APP
        .get_or_try_init(|| async {
            let url = std::env::var("DATABASE_URL")
                .map_err(|_| Error::from("DATABASE_URL must be set in production"))?;
            let pool = eth_tools_db::connect(&url)
                .await
                .map_err(|e| Error::from(format!("db connect: {e}")))?;
            let state = eth_tools_mcp::AppState::new(pool);
            Ok::<_, Error>(eth_tools_mcp::router(state))
        })
        .await?;
    bridge(app, req).await
}

/// Response body cap. Matches the v1 bridge — Vercel itself enforces ~4.5 MB
/// on Function bodies, but rmcp tool output is JSON envelopes well under
/// 100 KB so 10 MB is the safe abuse-prevention ceiling.
const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

/// Pure conversion function — extracted so a unit test can drive it against
/// any `axum::Router` without env-var plumbing. Mirrors `api/v1/index.rs`
/// line-for-line on purpose; when M4 ships and we migrate to
/// `vercel_runtime` 2.x, both files change in lockstep.
async fn bridge(app: &axum::Router, req: Request) -> Result<Response<Body>, Error> {
    let (parts, body) = req.into_parts();
    let body_bytes: Vec<u8> = match body {
        Body::Text(s) => s.into_bytes(),
        Body::Binary(b) => b,
        Body::Empty => Vec::new(),
    };
    let axum_req = HttpRequest::from_parts(parts, AxumBody::from(body_bytes));

    let axum_resp = app
        .clone()
        .oneshot(axum_req)
        .await
        .unwrap_or_else(|e: std::convert::Infallible| match e {});

    let (parts, body) = axum_resp.into_parts();
    let body_bytes = axum::body::to_bytes(body, MAX_BODY_BYTES)
        .await
        .map_err(|e| Error::from(format!("response body collect: {e}")))?;
    let vercel_body = if body_bytes.is_empty() {
        Body::Empty
    } else {
        match std::str::from_utf8(&body_bytes) {
            Ok(s) => Body::Text(s.to_string()),
            Err(_) => Body::Binary(body_bytes.to_vec()),
        }
    };
    Ok(Response::from_parts(parts, vercel_body))
}

#[cfg(test)]
mod tests {
    //! Bridge round-trip against a stub Router (no DB, no rmcp). Proves the
    //! Vercel↔Axum conversion contract. The real MCP path is covered by
    //! `crates/mcp/tests/integration.rs`.

    use super::*;
    use axum::routing::get;

    fn stub_router() -> axum::Router {
        axum::Router::new()
            .route("/", get(|| async { "mcp-stub" }))
            .fallback(|| async {
                (
                    http::StatusCode::NOT_FOUND,
                    axum::Json(serde_json::json!({"error": {"code": "ROUTE_NOT_FOUND"}})),
                )
            })
    }

    #[tokio::test]
    async fn bridge_round_trip_text() {
        let app = stub_router();
        let req = http::Request::builder()
            .method("GET")
            .uri("https://example.com/")
            .body(Body::Empty)
            .unwrap();
        let resp = bridge(&app, req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = match resp.into_body() {
            Body::Text(s) => s,
            _ => panic!("expected text body"),
        };
        assert_eq!(body, "mcp-stub");
    }

    #[tokio::test]
    async fn bridge_passes_through_404() {
        let app = stub_router();
        let req = http::Request::builder()
            .method("GET")
            .uri("https://example.com/no-such-route")
            .body(Body::Empty)
            .unwrap();
        let resp = bridge(&app, req).await.unwrap();
        assert_eq!(resp.status(), 404);
    }
}

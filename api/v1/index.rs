//! Vercel function entrypoint for /api/v1/*.
//!
//! Bridges `vercel_runtime::Request` (1.x — `http::Request<vercel_runtime::Body>`)
//! to `axum::Router::oneshot` and back. The router itself is defined in
//! `crates/api` and shared with `crates/dev-server`, so prod and local dev
//! exercise the same handler code.
//!
//! Vercel routes `/api/v1/*` to this single binary via `vercel.json` rewrites
//! (`api/v1/(.*)` → `/api/v1/index`). Internal path dispatch happens inside
//! the Axum router.
//!
//! The router is constructed once at cold-start time and stored in a `OnceLock`
//! so subsequent warm invocations skip rebuild. (Future Phase 1 work will use
//! `tokio::sync::OnceCell` once the router needs an async DB-pool init.)

use std::sync::OnceLock;

use axum::body::Body as AxumBody;
use http::Request as HttpRequest;
use tower::ServiceExt;
use vercel_runtime::{run, Body, Error, Request, Response};

static APP: OnceLock<axum::Router> = OnceLock::new();

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_ansi(false)
        .json()
        .init();
    // Build the router once at cold-start.
    let _ = APP.set(eth_tools_api::router());
    run(handler).await
}

async fn handler(req: Request) -> Result<Response<Body>, Error> {
    let app = APP.get().expect("router initialized in main()");

    // vercel_runtime::Request → axum http::Request<axum::body::Body>
    let (parts, body) = req.into_parts();
    let body_bytes: Vec<u8> = match body {
        Body::Text(s) => s.into_bytes(),
        Body::Binary(b) => b,
        Body::Empty => Vec::new(),
    };
    let axum_req = HttpRequest::from_parts(parts, AxumBody::from(body_bytes));

    // Route through Axum. Router's `Service::Error` is `Infallible`, so the
    // only failure mode is a handler returning a non-2xx Response — never Err.
    // `Infallible` can't be constructed, so `unwrap` is type-safe here.
    let axum_resp = app.clone().oneshot(axum_req).await.unwrap();

    // axum::Response → vercel_runtime::Response<Body>
    let (parts, body) = axum_resp.into_parts();
    let body_bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|e| Error::from(format!("response body collect: {e}")))?;

    // Prefer Body::Text when the bytes are valid UTF-8 so logs/streaming stay
    // human-readable; fall back to Binary otherwise.
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
    //! Bridge round-trip test: synthesize a `vercel_runtime::Request`, drive
    //! it through `handler`, assert we get back the JSON body the Axum router
    //! produced. Pins the body/parts conversion so a future refactor can't
    //! silently regress the contract.
    use super::*;

    #[tokio::test]
    async fn bridge_round_trip_health() {
        let _ = APP.set(eth_tools_api::router());

        let req = http::Request::builder()
            .method("GET")
            .uri("https://example.com/api/v1/health")
            .body(Body::Empty)
            .unwrap();

        let resp = handler(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = match resp.into_body() {
            Body::Text(s) => s,
            Body::Binary(b) => String::from_utf8(b).unwrap(),
            Body::Empty => String::new(),
        };
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["status"], "ok");
    }

    #[tokio::test]
    async fn bridge_returns_typed_404_for_unknown_route() {
        let _ = APP.set(eth_tools_api::router());

        let req = http::Request::builder()
            .method("GET")
            .uri("https://example.com/api/v1/no-such-route")
            .body(Body::Empty)
            .unwrap();

        let resp = handler(req).await.unwrap();
        assert_eq!(resp.status(), 404);
        let body = match resp.into_body() {
            Body::Text(s) => s,
            Body::Binary(b) => String::from_utf8(b).unwrap(),
            Body::Empty => String::new(),
        };
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], "ROUTE_NOT_FOUND");
    }
}

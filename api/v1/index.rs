//! Vercel function entrypoint for /api/v1/*.
//!
//! Bridges `vercel_runtime::Request` (1.x — `http::Request<vercel_runtime::Body>`)
//! to the shared `eth_tools_api::router` via `tower::Service::oneshot`, and
//! converts the response back to `vercel_runtime::Response<Body>`.
//!
//! Lifecycle: at cold-start we lazily build `AppState { pool }` once and
//! stash it (plus the router that holds it) in a `tokio::sync::OnceCell`.
//! Subsequent warm invocations skip the rebuild. Initialization is async
//! because `eth_tools_db::connect` is.
//!
//! Vercel routes `/api/v1/*` here via `vercel.json` rewrites
//! (`api/v1/(.*)` → `/api/v1/index`).

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
            Ok::<_, Error>(eth_tools_api::router(eth_tools_api::AppState::new(pool)))
        })
        .await?;

    bridge(app, req).await
}

/// Pure conversion function — Vercel Request → Axum oneshot → Vercel Response.
/// Extracted so the test suite can drive it against any Router, no env-var
/// or DATABASE_URL plumbing required.
async fn bridge(app: &axum::Router, req: Request) -> Result<Response<Body>, Error> {
    let (parts, body) = req.into_parts();
    let body_bytes: Vec<u8> = match body {
        Body::Text(s) => s.into_bytes(),
        Body::Binary(b) => b,
        Body::Empty => Vec::new(),
    };
    let axum_req = HttpRequest::from_parts(parts, AxumBody::from(body_bytes));

    // Router::Service::Error is Infallible — `unwrap` is type-safe.
    let axum_resp = app.clone().oneshot(axum_req).await.unwrap();

    let (parts, body) = axum_resp.into_parts();
    let body_bytes = axum::body::to_bytes(body, usize::MAX)
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
    //! Bridge round-trip against a stub Router (no DB needed) — proves the
    //! conversion contract. The full handler + AppState path is covered by
    //! crates/api's testcontainers tests.

    use super::*;
    use axum::routing::get;

    fn stub_router() -> axum::Router {
        axum::Router::new()
            .route("/echo", get(|| async { "echoed" }))
            .route(
                "/api/v1/health",
                get(|| async { axum::Json(serde_json::json!({"status": "ok"})) }),
            )
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
            .uri("https://example.com/echo")
            .body(Body::Empty)
            .unwrap();
        let resp = bridge(&app, req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = match resp.into_body() {
            Body::Text(s) => s,
            _ => panic!("expected text body"),
        };
        assert_eq!(body, "echoed");
    }

    #[tokio::test]
    async fn bridge_round_trip_json() {
        let app = stub_router();
        let req = http::Request::builder()
            .method("GET")
            .uri("https://example.com/api/v1/health")
            .body(Body::Empty)
            .unwrap();
        let resp = bridge(&app, req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = match resp.into_body() {
            Body::Text(s) => s,
            _ => panic!("expected text body"),
        };
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["status"], "ok");
    }

    #[tokio::test]
    async fn bridge_passes_through_404_envelope() {
        let app = stub_router();
        let req = http::Request::builder()
            .method("GET")
            .uri("https://example.com/no-such-route")
            .body(Body::Empty)
            .unwrap();
        let resp = bridge(&app, req).await.unwrap();
        assert_eq!(resp.status(), 404);
        let body = match resp.into_body() {
            Body::Text(s) => s,
            _ => panic!("expected text body"),
        };
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"]["code"], "ROUTE_NOT_FOUND");
    }
}

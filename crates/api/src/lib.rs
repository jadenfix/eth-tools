//! HTTP API router for eth-tools.
//!
//! Mounted by `api/v1/[...path].rs` (the Vercel function entrypoint) and by
//! `crates/dev-server` (local development).

use axum::{http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use serde_json::{json, Value};

pub fn router() -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .fallback(not_found)
}

async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "chains": [],
        "rpc": { "primary": "unknown", "fallback": "unknown" }
    }))
}

/// Fallback returns a typed envelope matching the `DeniedReason` shape per
/// plan §9.1, so 404s are machine-parseable instead of opaque.
async fn not_found() -> impl IntoResponse {
    let body = json!({
        "error": {
            "code": "ROUTE_NOT_FOUND",
            "policy_version": "v1",
            "evaluator": "api.router",
            "override_hint": "see /api/v1/health for valid routes; full list at /openapi.json"
        }
    });
    (StatusCode::NOT_FOUND, Json(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_returns_ok() {
        let app = router();
        let req = http::Request::builder()
            .uri("/api/v1/health")
            .body(axum::body::Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), 200);
        let body = to_bytes(res.into_body(), 1024).await.unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["status"], "ok");
    }

    #[tokio::test]
    async fn unknown_route_returns_typed_404() {
        let app = router();
        let req = http::Request::builder()
            .uri("/api/v1/does-not-exist")
            .body(axum::body::Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), 404);
        let body = to_bytes(res.into_body(), 1024).await.unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "ROUTE_NOT_FOUND");
    }
}

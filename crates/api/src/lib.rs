//! HTTP API router for eth-tools.
//!
//! Mounted by `api/v1/[...path].rs` (the Vercel function entrypoint) and by
//! `crates/dev-server` (local development).

use axum::{routing::get, Json, Router};
use serde_json::{json, Value};

pub fn router() -> Router {
    Router::new()
        .route("/", get(root))
        .route("/api/v1/health", get(health))
}

async fn root() -> Json<Value> {
    Json(json!({ "service": "eth-tools", "version": env!("CARGO_PKG_VERSION") }))
}

async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "chains": [],
        "rpc": { "primary": "unknown", "fallback": "unknown" }
    }))
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
}

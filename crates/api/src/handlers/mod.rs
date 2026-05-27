//! HTTP handlers grouped by surface area.
//!
//! Each handler is a small `async fn(State<AppState>, ...) -> Result<Json, ApiError>`.
//! Heavy logic (db queries, cursor encoding) lives in helpers under the
//! relevant submodule.

pub mod agents;
pub mod health;
pub mod invoke;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;

/// Router fallback. Returns the same `error.{code,policy_version,evaluator}`
/// envelope as typed errors so clients have a single shape to parse.
pub async fn not_found() -> impl IntoResponse {
    let body = json!({
        "error": {
            "code": "ROUTE_NOT_FOUND",
            "policy_version": "v1",
            "evaluator": "api.router",
            "override_hint": "see /api/v1/health for liveness; /openapi.json for the full route list"
        }
    });
    (StatusCode::NOT_FOUND, Json(body))
}

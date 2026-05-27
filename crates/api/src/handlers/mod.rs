//! HTTP handlers grouped by surface area.
//!
//! Each handler is a small `async fn(State<AppState>, ...) -> Result<Json, ApiError>`.
//! Heavy logic (db queries, cursor encoding) lives in helpers under the
//! relevant submodule.

pub mod agents;
pub mod health;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;

use crate::dto::{ApiErrorBody, ApiErrorPayload};

/// Router fallback. Returns the same `error.{code,policy_version,evaluator}`
/// envelope as typed errors so clients have a single shape to parse.
pub async fn not_found() -> impl IntoResponse {
    let body = ApiErrorBody {
        error: ApiErrorPayload {
            code: "ROUTE_NOT_FOUND".to_string(),
            policy_version: "v1".to_string(),
            evaluator: "api.router".to_string(),
            override_hint: Some(
                "see /api/v1/health for liveness; /openapi.json for the full route list"
                    .to_string(),
            ),
        },
    };
    (StatusCode::NOT_FOUND, Json(body))
}

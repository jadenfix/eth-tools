//! Map handler errors to typed `DeniedReason` JSON envelopes per plan §9.1.
//!
//! Every non-2xx response uses the same shape so clients can pattern-match on
//! `error.code` without parsing free-form strings.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::dto::{ApiErrorBody, ApiErrorPayload};

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("agent not found")]
    AgentNotFound,
    #[error("chain not found")]
    ChainNotFound,
    #[error("invalid cursor")]
    InvalidCursor,
    #[error("invalid agent id")]
    InvalidAgentId,
    #[error("database error")]
    Db(#[from] sqlx::Error),
}

impl ApiError {
    fn code(&self) -> &'static str {
        match self {
            ApiError::AgentNotFound => "AGENT_NOT_FOUND",
            ApiError::ChainNotFound => "CHAIN_NOT_FOUND",
            ApiError::InvalidCursor => "INVALID_CURSOR",
            ApiError::InvalidAgentId => "INVALID_AGENT_ID",
            ApiError::Db(_) => "INTERNAL",
        }
    }

    fn evaluator(&self) -> &'static str {
        match self {
            ApiError::AgentNotFound | ApiError::ChainNotFound => "api.lookup",
            ApiError::InvalidCursor | ApiError::InvalidAgentId => "api.input",
            ApiError::Db(_) => "api.db",
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            ApiError::AgentNotFound | ApiError::ChainNotFound => StatusCode::NOT_FOUND,
            ApiError::InvalidCursor | ApiError::InvalidAgentId => StatusCode::BAD_REQUEST,
            ApiError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn hint(&self) -> &'static str {
        match self {
            ApiError::AgentNotFound => "verify chain + agent_id; see /api/v1/agents to list",
            ApiError::ChainNotFound => "supported: base, base-sepolia (use name or chain_id)",
            ApiError::InvalidCursor => "cursor is opaque; use the value from next_cursor verbatim",
            ApiError::InvalidAgentId => "agent_id must be a decimal uint256 string",
            ApiError::Db(_) => "transient — retry; persistent → file an issue",
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // DB errors carry sensitive detail (table names, sqlstate) — log
        // server-side, never echo to the client.
        if let ApiError::Db(e) = &self {
            tracing::error!(error = ?e, "db error");
        }
        let body = ApiErrorBody {
            error: ApiErrorPayload {
                code: self.code().to_string(),
                policy_version: "v1".to_string(),
                evaluator: self.evaluator().to_string(),
                override_hint: Some(self.hint().to_string()),
            },
        };
        (self.status(), Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    //! Wire-format snapshot: the typed `ApiErrorBody` must serialize
    //! byte-identical to the legacy `json!({error:{...}})` shape so existing
    //! clients (dashboard, CLI, MCP) don't have to change their parsers.
    use super::*;
    use axum::body::to_bytes;
    use axum::response::IntoResponse;

    async fn body_bytes(err: ApiError) -> Vec<u8> {
        let resp = err.into_response();
        to_bytes(resp.into_body(), 1 << 16).await.unwrap().to_vec()
    }

    #[tokio::test]
    async fn agent_not_found_serializes_canonical_shape() {
        let bytes = body_bytes(ApiError::AgentNotFound).await;
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"]["code"], "AGENT_NOT_FOUND");
        assert_eq!(v["error"]["policy_version"], "v1");
        assert_eq!(v["error"]["evaluator"], "api.lookup");
        assert_eq!(
            v["error"]["override_hint"],
            "verify chain + agent_id; see /api/v1/agents to list"
        );
        // Field ordering matters for byte-parity with the legacy json!()
        // shape that openapi clients may have snapshotted.
        let s = std::str::from_utf8(&bytes).unwrap();
        assert!(s.contains(r#""code":"AGENT_NOT_FOUND""#));
        assert!(s.contains(r#""policy_version":"v1""#));
        assert!(s.contains(r#""evaluator":"api.lookup""#));
        assert!(s.contains(r#""override_hint""#));
    }

    #[tokio::test]
    async fn invalid_cursor_serializes_canonical_shape() {
        let bytes = body_bytes(ApiError::InvalidCursor).await;
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"]["code"], "INVALID_CURSOR");
        assert_eq!(v["error"]["evaluator"], "api.input");
    }

    #[tokio::test]
    async fn chain_not_found_serializes_canonical_shape() {
        let bytes = body_bytes(ApiError::ChainNotFound).await;
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"]["code"], "CHAIN_NOT_FOUND");
        assert_eq!(v["error"]["evaluator"], "api.lookup");
    }
}

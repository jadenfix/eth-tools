//! Map handler errors to typed `DeniedReason` JSON envelopes per plan §9.1.
//!
//! Every non-2xx response uses the same shape so clients can pattern-match on
//! `error.code` without parsing free-form strings.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

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
        let body = json!({
            "error": {
                "code": self.code(),
                "policy_version": "v1",
                "evaluator": self.evaluator(),
                "override_hint": self.hint()
            }
        });
        (self.status(), Json(body)).into_response()
    }
}

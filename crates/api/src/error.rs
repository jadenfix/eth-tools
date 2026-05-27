//! Map handler errors to typed `DeniedReason` JSON envelopes per plan §9.1.
//!
//! Every non-2xx response uses the same shape (`ApiErrorBody`) so clients can
//! pattern-match on `error.code` without parsing free-form strings.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::dto::{ApiErrorBody, ApiErrorPayload};

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    // --- read-side lookups ---------------------------------------------------
    #[error("agent not found")]
    AgentNotFound,
    #[error("chain not found")]
    ChainNotFound,
    #[error("validation not found")]
    ValidationNotFound,

    // --- input validation ----------------------------------------------------
    #[error("invalid cursor")]
    InvalidCursor,
    #[error("invalid agent id")]
    InvalidAgentId,
    #[error("invalid request hash")]
    InvalidRequestHash,
    #[error("invalid address")]
    InvalidAddress,
    /// Free-form input rejection. `0` = client-facing message (safe to echo).
    #[error("invalid input: {0}")]
    InvalidInput(String),

    // --- write-side gating ---------------------------------------------------
    /// Missing/malformed `Authorization: Bearer …` on a write endpoint.
    #[error("unauthorized")]
    Unauthorized,
    /// x402 payment required (handler is gated by the x402 middleware).
    #[error("payment required")]
    PaymentRequired,
    /// Caller sent `Idempotency-Key: X` twice with different request body
    /// shapes pointing at the same path. We can't return the cached response
    /// because the new call would observably differ.
    #[error("idempotency key reused with different request")]
    IdempotencyConflict,

    // --- not-yet-implemented (depend on a sibling branch) --------------------
    /// Handler exists for OpenAPI generation but the substrate (wallet rails,
    /// x402, schema validator, etc.) lives on a branch that has not landed in
    /// `main` yet. `0` = sibling branch name for the operator.
    #[error("feature not yet wired: {0}")]
    NotImplemented(&'static str),

    // --- infrastructure ------------------------------------------------------
    #[error("database error")]
    Db(#[from] sqlx::Error),
}

impl ApiError {
    fn code(&self) -> &'static str {
        match self {
            ApiError::AgentNotFound => "AGENT_NOT_FOUND",
            ApiError::ChainNotFound => "CHAIN_NOT_FOUND",
            ApiError::ValidationNotFound => "VALIDATION_NOT_FOUND",
            ApiError::InvalidCursor => "INVALID_CURSOR",
            ApiError::InvalidAgentId => "INVALID_AGENT_ID",
            ApiError::InvalidRequestHash => "INVALID_REQUEST_HASH",
            ApiError::InvalidAddress => "INVALID_ADDRESS",
            ApiError::InvalidInput(_) => "INVALID_INPUT",
            ApiError::Unauthorized => "UNAUTHORIZED",
            ApiError::PaymentRequired => "PAYMENT_REQUIRED",
            ApiError::IdempotencyConflict => "IDEMPOTENCY_CONFLICT",
            ApiError::NotImplemented(_) => "NOT_IMPLEMENTED",
            ApiError::Db(_) => "INTERNAL",
        }
    }

    fn evaluator(&self) -> &'static str {
        match self {
            ApiError::AgentNotFound
            | ApiError::ChainNotFound
            | ApiError::ValidationNotFound => "api.lookup",
            ApiError::InvalidCursor
            | ApiError::InvalidAgentId
            | ApiError::InvalidRequestHash
            | ApiError::InvalidAddress
            | ApiError::InvalidInput(_) => "api.input",
            ApiError::Unauthorized => "api.auth",
            ApiError::PaymentRequired => "api.x402",
            ApiError::IdempotencyConflict => "api.idempotency",
            ApiError::NotImplemented(_) => "api.feature_flag",
            ApiError::Db(_) => "api.db",
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            ApiError::AgentNotFound
            | ApiError::ChainNotFound
            | ApiError::ValidationNotFound => StatusCode::NOT_FOUND,
            ApiError::InvalidCursor
            | ApiError::InvalidAgentId
            | ApiError::InvalidRequestHash
            | ApiError::InvalidAddress
            | ApiError::InvalidInput(_) => StatusCode::BAD_REQUEST,
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::PaymentRequired => StatusCode::PAYMENT_REQUIRED,
            ApiError::IdempotencyConflict => StatusCode::CONFLICT,
            ApiError::NotImplemented(_) => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::Db(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn hint(&self) -> String {
        match self {
            ApiError::AgentNotFound => {
                "verify chain + agent_id; see /api/v1/agents to list".into()
            }
            ApiError::ChainNotFound => {
                "supported: base, base-sepolia (use name or chain_id)".into()
            }
            ApiError::ValidationNotFound => {
                "verify the request_hash; see /api/v1/agents for related rows".into()
            }
            ApiError::InvalidCursor => {
                "cursor is opaque; use the value from next_cursor verbatim".into()
            }
            ApiError::InvalidAgentId => "agent_id must be a decimal uint256 string".into(),
            ApiError::InvalidRequestHash => {
                "request_hash must be 0x-prefixed 32-byte hex (64 hex chars)".into()
            }
            ApiError::InvalidAddress => {
                "address must be 0x-prefixed 20-byte hex (40 hex chars)".into()
            }
            ApiError::InvalidInput(m) => m.clone(),
            ApiError::Unauthorized => {
                "send `Authorization: Bearer <api-key>`; see /api/v1/keys to mint one".into()
            }
            ApiError::PaymentRequired => {
                "supply an x402 micropayment; see the WWW-Authenticate header".into()
            }
            ApiError::IdempotencyConflict => {
                "reuse the same Idempotency-Key with the same body, or pick a new key".into()
            }
            ApiError::NotImplemented(b) => {
                format!("depends on branch {b}; will land before GA")
            }
            ApiError::Db(_) => "transient — retry; persistent → file an issue".into(),
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
                override_hint: Some(self.hint()),
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

    #[tokio::test]
    async fn unauthorized_returns_401() {
        let resp = ApiError::Unauthorized.into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let bytes = to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"]["code"], "UNAUTHORIZED");
    }

    #[tokio::test]
    async fn payment_required_returns_402() {
        let resp = ApiError::PaymentRequired.into_response();
        assert_eq!(resp.status(), StatusCode::PAYMENT_REQUIRED);
    }

    #[tokio::test]
    async fn not_implemented_returns_503() {
        let resp = ApiError::NotImplemented("feat/phase-6-wallet-rails").into_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"]["code"], "NOT_IMPLEMENTED");
        assert!(
            v["error"]["override_hint"]
                .as_str()
                .unwrap()
                .contains("feat/phase-6-wallet-rails")
        );
    }

    #[tokio::test]
    async fn idempotency_conflict_returns_409() {
        let resp = ApiError::IdempotencyConflict.into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }
}

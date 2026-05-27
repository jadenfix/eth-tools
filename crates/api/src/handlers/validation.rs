//! `/api/v1/validation/*`.
//!
//! - `GET /api/v1/validation/:chain/:hash` — single row by `request_hash`.
//! - `POST /api/v1/validation/request` — STUBBED (wallet rails dependency).
//! - `POST /api/v1/validation/respond`  — STUBBED (wallet rails dependency).

use axum::extract::{Path, State};
use axum::Json;
use eth_tools_db::validations;

use crate::dto::{OneEnvelope, ValidationDto, ValidationRequest, ValidationRespond};
use crate::error::ApiError;
use crate::handlers::agents::resolve_chain;
use crate::AppState;

/// Parse a 0x-prefixed 32-byte hex string (64 hex chars). Rejects any other
/// length so we never query the DB with a truncated key (which could match
/// the wrong row if the index ever loses its full-key uniqueness).
fn parse_hash32(s: &str) -> Result<Vec<u8>, ApiError> {
    let s = s.strip_prefix("0x").ok_or(ApiError::InvalidRequestHash)?;
    if s.len() != 64 {
        return Err(ApiError::InvalidRequestHash);
    }
    let mut out = Vec::with_capacity(32);
    let bytes = s.as_bytes();
    for chunk in bytes.chunks(2) {
        let hi = (chunk[0] as char)
            .to_digit(16)
            .ok_or(ApiError::InvalidRequestHash)?;
        let lo = (chunk[1] as char)
            .to_digit(16)
            .ok_or(ApiError::InvalidRequestHash)?;
        out.push(((hi as u8) << 4) | (lo as u8));
    }
    Ok(out)
}

#[utoipa::path(
    get,
    path = "/api/v1/validation/{chain}/{request_hash}",
    tag = "validation",
    operation_id = "validation_read",
    params(
        ("chain" = String, Path, description = "Chain name or chain_id"),
        ("request_hash" = String, Path, description = "0x-prefixed 32-byte hex"),
    ),
    responses(
        (status = 200, description = "Single validation row", body = crate::dto::ValidationDetail),
        (status = 400, description = "bad input", body = crate::dto::ApiErrorBody),
        (status = 404, description = "chain or validation not found", body = crate::dto::ApiErrorBody),
        (status = 429, description = "rate limited", body = crate::dto::ApiErrorBody),
        (status = 500, description = "internal error", body = crate::dto::ApiErrorBody),
    )
)]
pub async fn read(
    State(state): State<AppState>,
    Path((chain, request_hash)): Path<(String, String)>,
) -> Result<Json<OneEnvelope<ValidationDto>>, ApiError> {
    let chain = resolve_chain(&chain)?;
    let hash = parse_hash32(&request_hash)?;
    let row = validations::get_one(&state.pool, chain.chain_id as i64, &hash)
        .await?
        .ok_or(ApiError::ValidationNotFound)?;
    Ok(Json(OneEnvelope {
        data: ValidationDto::from_row(row),
        staleness_ms: 0,
        source: "db",
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/validation/request",
    tag = "validation",
    operation_id = "validation_request",
    request_body = crate::dto::ValidationRequest,
    responses(
        (status = 200, description = "Validation request submitted on-chain", body = crate::dto::ApiErrorBody),
        (status = 400, description = "bad input", body = crate::dto::ApiErrorBody),
        (status = 401, description = "missing bearer token", body = crate::dto::ApiErrorBody),
        (status = 402, description = "x402 payment required", body = crate::dto::ApiErrorBody),
        (status = 429, description = "rate limited", body = crate::dto::ApiErrorBody),
        (status = 503, description = "wallet rails not yet wired", body = crate::dto::ApiErrorBody),
    )
)]
pub async fn request_validation(
    State(_state): State<AppState>,
    Json(_req): Json<ValidationRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // TODO(feat/phase-6-wallet-rails): ValidationRegistry.validationRequest(...)
    Err(ApiError::NotImplemented("feat/phase-6-wallet-rails"))
}

#[utoipa::path(
    post,
    path = "/api/v1/validation/respond",
    tag = "validation",
    operation_id = "validation_respond",
    request_body = crate::dto::ValidationRespond,
    responses(
        (status = 200, description = "Validation response submitted on-chain", body = crate::dto::ApiErrorBody),
        (status = 400, description = "bad input", body = crate::dto::ApiErrorBody),
        (status = 401, description = "missing bearer token", body = crate::dto::ApiErrorBody),
        (status = 402, description = "x402 payment required", body = crate::dto::ApiErrorBody),
        (status = 429, description = "rate limited", body = crate::dto::ApiErrorBody),
        (status = 503, description = "wallet rails not yet wired", body = crate::dto::ApiErrorBody),
    )
)]
pub async fn respond_validation(
    State(_state): State<AppState>,
    Json(_req): Json<ValidationRespond>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // TODO(feat/phase-6-wallet-rails): ValidationRegistry.validationResponse(...)
    Err(ApiError::NotImplemented("feat/phase-6-wallet-rails"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hash32_accepts_64_hex() {
        let bytes = parse_hash32("0x0000000000000000000000000000000000000000000000000000000000000001").unwrap();
        assert_eq!(bytes.len(), 32);
        assert_eq!(bytes[31], 1);
    }
    #[test]
    fn parse_hash32_rejects_wrong_length() {
        assert!(parse_hash32("0xabc").is_err());
        assert!(parse_hash32("0x").is_err());
    }
    #[test]
    fn parse_hash32_rejects_missing_prefix() {
        assert!(parse_hash32(
            "0000000000000000000000000000000000000000000000000000000000000001"
        )
        .is_err());
    }
    #[test]
    fn parse_hash32_rejects_non_hex() {
        assert!(parse_hash32(
            "0xzz00000000000000000000000000000000000000000000000000000000000001"
        )
        .is_err());
    }
}

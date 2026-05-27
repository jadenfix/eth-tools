//! `/api/v1/invoke/*`.
//!
//! - `POST /api/v1/invoke/prepare` — pure-compute. Builds the calldata that
//!   *would* be sent. Useful for clients that hold their own keys (Frame /
//!   MetaMask / Safe). REAL today.
//! - `POST /api/v1/invoke` — execute. STUBBED until `feat/phase-6-x402` and
//!   the wallet rails branch land; the OpenAPI shape is committed so MCP +
//!   CLI can integrate against it now.

use axum::extract::State;
use axum::Json;
use bigdecimal::BigDecimal;
use std::str::FromStr;

use crate::dto::{InvokeExecute, InvokePrepare, InvokePrepareResponse};
use crate::error::ApiError;
use crate::handlers::agents::{parse_address, resolve_chain};
use crate::AppState;

fn parse_selector(s: &str) -> Result<[u8; 4], ApiError> {
    let s = s
        .strip_prefix("0x")
        .ok_or_else(|| ApiError::InvalidInput("selector must be 0x-prefixed".into()))?;
    if s.len() != 8 {
        return Err(ApiError::InvalidInput(
            "selector must be 4 bytes (8 hex chars)".into(),
        ));
    }
    let bytes = hex_bytes(s)?;
    let mut out = [0u8; 4];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn hex_bytes(s: &str) -> Result<Vec<u8>, ApiError> {
    if s.len() % 2 != 0 {
        return Err(ApiError::InvalidInput("odd-length hex".into()));
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    for chunk in bytes.chunks(2) {
        let hi = (chunk[0] as char)
            .to_digit(16)
            .ok_or_else(|| ApiError::InvalidInput("non-hex char".into()))?;
        let lo = (chunk[1] as char)
            .to_digit(16)
            .ok_or_else(|| ApiError::InvalidInput("non-hex char".into()))?;
        out.push(((hi as u8) << 4) | (lo as u8));
    }
    Ok(out)
}

#[utoipa::path(
    post,
    path = "/api/v1/invoke/prepare",
    tag = "invoke",
    operation_id = "invoke_prepare",
    request_body = crate::dto::InvokePrepare,
    responses(
        (status = 200, description = "Calldata + tx envelope, ready to sign", body = crate::dto::InvokePrepareResponse),
        (status = 400, description = "bad input", body = crate::dto::ApiErrorBody),
        (status = 404, description = "chain not found", body = crate::dto::ApiErrorBody),
        (status = 401, description = "missing bearer token", body = crate::dto::ApiErrorBody),
        (status = 429, description = "rate limited", body = crate::dto::ApiErrorBody),
        (status = 500, description = "internal error", body = crate::dto::ApiErrorBody),
    )
)]
pub async fn prepare(
    State(_state): State<AppState>,
    Json(req): Json<InvokePrepare>,
) -> Result<Json<InvokePrepareResponse>, ApiError> {
    let chain = resolve_chain(&req.chain)?;
    let _agent_id = BigDecimal::from_str(&req.agent_id).map_err(|_| ApiError::InvalidAgentId)?;
    let to_bytes = parse_address(&req.to)?;
    let selector = parse_selector(&req.selector)?;
    let args = if req.args_hex.is_empty() {
        Vec::new()
    } else {
        let trimmed = req.args_hex.strip_prefix("0x").unwrap_or(&req.args_hex);
        hex_bytes(trimmed)?
    };

    // Build the calldata. We DON'T pull in alloy::sol! for this because the
    // selector + args are already pre-computed by the caller — concatenation
    // is all the wire format requires for a raw eth_call / eth_sendTransaction.
    let mut calldata = String::with_capacity(2 + 8 + args.len() * 2);
    calldata.push_str("0x");
    for b in selector.iter() {
        calldata.push_str(&format!("{b:02x}"));
    }
    for b in args.iter() {
        calldata.push_str(&format!("{b:02x}"));
    }
    let to_hex_s = {
        let mut s = String::with_capacity(42);
        s.push_str("0x");
        for b in &to_bytes {
            s.push_str(&format!("{b:02x}"));
        }
        s
    };
    let value_wei = if req.value_wei.is_empty() {
        "0".to_string()
    } else {
        // Validate it parses as a decimal — we don't carry the value through
        // any arithmetic here, just echo it back.
        BigDecimal::from_str(&req.value_wei)
            .map_err(|_| ApiError::InvalidInput("value_wei must be a decimal integer".into()))?;
        req.value_wei.clone()
    };
    Ok(Json(InvokePrepareResponse {
        calldata,
        to: to_hex_s,
        value_wei,
        chain_id: chain.chain_id as i64,
        agent_id: req.agent_id,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/invoke",
    tag = "invoke",
    operation_id = "invoke_execute",
    request_body = crate::dto::InvokeExecute,
    responses(
        (status = 200, description = "Submitted (broadcast=true) or signed-only (broadcast=false)", body = crate::dto::ApiErrorBody),
        (status = 400, description = "bad input", body = crate::dto::ApiErrorBody),
        (status = 401, description = "missing bearer token", body = crate::dto::ApiErrorBody),
        (status = 402, description = "x402 payment required", body = crate::dto::ApiErrorBody),
        (status = 429, description = "rate limited", body = crate::dto::ApiErrorBody),
        (status = 503, description = "x402 + wallet rails not yet wired", body = crate::dto::ApiErrorBody),
    )
)]
pub async fn execute(
    State(_state): State<AppState>,
    Json(_req): Json<InvokeExecute>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // TODO(feat/phase-6-x402 + feat/phase-6-wallet-rails): once the x402
    // middleware is registered and the wallet rails expose a signer, gate
    // this on payment + push the tx.
    Err(ApiError::NotImplemented(
        "feat/phase-6-x402 + feat/phase-6-wallet-rails",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_selector_ok() {
        let s = parse_selector("0xdeadbeef").unwrap();
        assert_eq!(s, [0xde, 0xad, 0xbe, 0xef]);
    }
    #[test]
    fn parse_selector_bad_len() {
        assert!(parse_selector("0xdead").is_err());
        assert!(parse_selector("0xdeadbeef00").is_err());
    }
    #[test]
    fn parse_selector_missing_prefix() {
        assert!(parse_selector("deadbeef").is_err());
    }
}

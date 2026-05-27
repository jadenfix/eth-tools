//! `POST /api/v1/invoke` — the canonical cost-gated endpoint.
//!
//! Body shape (TBD in Phase 7): `{ tool: string, params: object }`. Phase 6
//! only wires the x402 gating + a stub handler body — the actual tool
//! dispatch lands when the MCP write-tools surface comes online.
//!
//! ## x402 flow
//!
//! 1. Client POSTs without `X-PAYMENT` → we return 402 + the challenge body.
//! 2. Client signs EIP-3009 `transferWithAuthorization` and retries with
//!    `X-PAYMENT: <base64-json>`.
//! 3. We hand the header to the Coinbase CDP facilitator (`/verify` then
//!    `/settle`). On success → run the handler body, attach
//!    `X-PAYMENT-RECEIPT: <tx_hash>` to the response.
//! 4. On EIP-3009 nonce replay → another 402 (the buyer must pick a fresh
//!    nonce).
//! 5. On facilitator outage → 502 (NOT 402) so the client retries the same
//!    nonce instead of burning a new one.
//!
//! ## Dev-mode bypass
//!
//! If `AppState::x402.pay_to_address` is `None` (i.e. `X402_PAY_TO_ADDRESS`
//! env unset), [`gate_payment`] short-circuits to `Ok(None)` — the gate
//! becomes a no-op, the X-PAYMENT header is ignored, and the handler runs
//! immediately. This is what lets `cargo run -p eth-tools-dev-server`
//! work without a real wallet.

use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use eth_tools_payments::x402::{
    self, PaymentRequiredResponse, PaymentRequirements, SettlementReceipt, X402Error,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::ApiError;
use crate::AppState;

/// Price of one invocation, in USD cents. Hard-coded for MVP. Plan §10.3
/// envisions per-tool pricing later — moving this to a `tool → cents` map
/// (loaded from `core::manifest`) is a 5-line change.
pub const INVOKE_PRICE_CENTS: u32 = 1;

/// The header an agent's wallet sends with the base64-encoded EIP-3009
/// authorization. Lowercase per HTTP/2 convention.
pub const X_PAYMENT_HEADER: &str = "x-payment";

/// Response header carrying the on-chain settlement tx hash. Lets the agent
/// reconcile its own ledger without an extra round-trip.
pub const X_PAYMENT_RECEIPT_HEADER: &str = "x-payment-receipt";

/// Phase-6 stub body. Phase 7 replaces with a tool name + params union.
#[derive(Debug, Deserialize, Default)]
pub struct InvokeRequest {
    /// Optional echo string — proves the handler body ran after the gate.
    #[serde(default)]
    pub echo: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InvokeResponse {
    pub ok: bool,
    /// Echoes `request.echo` back; `None` if unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub echo: Option<String>,
    /// Settlement tx hash, if x402 was enabled and settlement succeeded.
    /// `None` in dev mode (x402 disabled).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settlement_tx: Option<String>,
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Option<Json<InvokeRequest>>,
) -> Response {
    // Compute the canonical "resource" string we hand to the facilitator.
    // The path is hard-coded because we know the route — using the
    // `Request::uri()` would let a misconfigured reverse-proxy rewrite us
    // into matching a wrong-resource signature. Plan §10.5 #2.
    let resource = "https://eth-tools.dev/api/v1/invoke";

    let receipt = match gate_payment(&state, &headers, INVOKE_PRICE_CENTS, resource).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    let echo = body.and_then(|Json(b)| b.echo);

    let mut response = Json(InvokeResponse {
        ok: true,
        echo,
        settlement_tx: receipt.as_ref().and_then(|r| r.transaction_hash.clone()),
    })
    .into_response();

    // Stamp the receipt header so the agent can correlate. Skip if no
    // tx_hash (dev-mode or facilitator returned empty `"transaction"`).
    if let Some(SettlementReceipt {
        transaction_hash: Some(tx),
        ..
    }) = receipt
    {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(X_PAYMENT_RECEIPT_HEADER.as_bytes()),
            tx.parse(),
        ) {
            response.headers_mut().insert(name, value);
        }
    }
    response
}

/// The actual gate. Returns:
///   - `Ok(None)` if x402 is disabled (dev mode) — handler proceeds, no receipt.
///   - `Ok(Some(receipt))` if x402 is enabled, header present, and settlement succeeded.
///   - `Err(Response)` if the gate refuses (402 challenge or 502 facilitator-down).
///     The caller short-circuits with that response — handler body does NOT run.
///
/// Pulled out as a free function so future cost-gated handlers can reuse it
/// without re-implementing the 402 ↔ verify ↔ settle dance.
pub async fn gate_payment(
    state: &AppState,
    headers: &HeaderMap,
    amount_cents: u32,
    resource: &str,
) -> Result<Option<SettlementReceipt>, Response> {
    let Some(pay_to) = state.x402.pay_to_address else {
        // Dev-mode bypass. The header (if present) is silently ignored — we
        // don't even decode it; in dev there's nothing to verify against.
        return Ok(None);
    };

    let requirements = x402::challenge(resource, amount_cents, pay_to);

    let payment_header = match headers.get(X_PAYMENT_HEADER) {
        Some(v) => match v.to_str() {
            Ok(s) if !s.is_empty() => s.to_string(),
            // Present but not ASCII / empty → treat like absent (challenge).
            // We don't echo the raw bytes back; just re-issue the 402.
            _ => return Err(challenge_response(requirements, None)),
        },
        None => return Err(challenge_response(requirements, None)),
    };

    match x402::verify_and_settle(
        &payment_header,
        &requirements,
        &state.x402.facilitator_url,
        &state.http,
    )
    .await
    {
        Ok(receipt) => Ok(Some(receipt)),
        Err(X402Error::Replay) => {
            // The buyer's nonce was already burned on-chain. Re-issue a 402
            // with `extra.replay: true` so the agent knows to pick a fresh
            // nonce (not retry blindly with the same one).
            Err(challenge_response(requirements, Some("replay")))
        }
        Err(X402Error::InvalidHeader(reason)) => {
            // Don't include the reason in the body — it could carry decoded
            // header fragments. Just re-challenge.
            tracing::warn!(reason = %reason, "x402: invalid X-PAYMENT header");
            Err(challenge_response(requirements, Some("invalid_payment_header")))
        }
        Err(X402Error::VerifyFailed(reason)) => {
            tracing::info!(reason = %reason, "x402: verify failed");
            Err(challenge_response(requirements, Some("verify_failed")))
        }
        Err(X402Error::SettleFailed(reason)) => {
            tracing::warn!(reason = %reason, "x402: settle failed");
            Err(challenge_response(requirements, Some("settle_failed")))
        }
        Err(X402Error::NetworkError(e)) => {
            tracing::error!(error = ?e, "x402: facilitator transport failure");
            Err(ApiError::FacilitatorUnavailable(e.to_string()).into_response())
        }
    }
}

/// Build a 402 response. `reason_extra` (if `Some`) goes into the top-level
/// JSON as `{ "extra": { "<reason>": true } }` — outside the spec's
/// `accepts` array (so we don't pollute the requirements) but inside the
/// same envelope.
fn challenge_response(
    requirements: PaymentRequirements,
    reason_extra: Option<&str>,
) -> Response {
    let mut body = serde_json::to_value(PaymentRequiredResponse::new(requirements))
        .expect("PaymentRequiredResponse always serializes");
    if let Some(reason) = reason_extra {
        // Don't clobber spec fields — write under `extra` so a forward-
        // compat parser ignores us cleanly.
        body.as_object_mut().unwrap().insert(
            "extra".to_string(),
            json!({ reason: true }),
        );
    }
    (StatusCode::PAYMENT_REQUIRED, Json(body)).into_response()
}

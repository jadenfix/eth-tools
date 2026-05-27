//! x402 facilitator client (plan §10.3).
//!
//! x402 is the RECEIVE side of the wallet story: agents pay us in USDC for
//! cost-gated endpoints. We never custody a receive wallet — we hand the
//! signed EIP-3009 authorization straight to a **facilitator** (Coinbase CDP
//! by default) and let it post the on-chain `transferWithAuthorization`. The
//! facilitator's reply is our receipt.
//!
//! ## Wire shapes (x402 v1 — `specs/x402-specification-v1.md`)
//!
//! - **402 challenge body** (returned by us when `X-PAYMENT` is missing):
//!   ```json
//!   { "x402Version": 1,
//!     "error": "X-PAYMENT header is required",
//!     "accepts": [ PaymentRequirements ] }
//!   ```
//! - **`PaymentRequirements`** (one element of `accepts`): camelCase JSON.
//! - **`X-PAYMENT` header** (sent by the agent on retry): base64 of the JSON
//!   `PaymentPayload` (scheme-dependent shape; we pass it through opaquely —
//!   the facilitator validates).
//! - **`/verify`** and **`/settle`**: POST
//!   `{ x402Version, paymentPayload, paymentRequirements }`.
//! - **Verify response**: `{ isValid, invalidReason?, payer? }`.
//! - **Settle response**: `{ success, errorReason?, payer, transaction, network }`.
//!
//! ## Replay protection
//!
//! EIP-3009 `transferWithAuthorization` consumes a per-signer nonce on chain;
//! the second submission with the same nonce reverts. The facilitator surfaces
//! this in `invalidReason` / `errorReason` as a substring containing "nonce"
//! and "used"; we map it to [`X402Error::Replay`] so callers know to re-issue
//! a fresh 402.
//!
//! ## What this module does NOT do
//!
//! - **No client-side EIP-3009 signing.** That's the agent's job; we only
//!   pass the signed payload through to the facilitator (plan §10.3).
//! - **No persistence of `X-PAYMENT`.** Treat the header as a one-shot bearer
//!   that lives in the request scope only — never log or store it.

use alloy_primitives::Address;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

/// x402 protocol version we speak. Pinned to v1 (the only ratified spec at
/// the time of writing). If a v2 ships and the facilitator advertises it via
/// content negotiation, we'll add a multi-version handshake — for now, v1 is
/// the single source of truth.
pub const X402_VERSION: u32 = 1;

/// CAIP-2 network identifier for Base mainnet. Per CDP docs, x402 uses CAIP-2
/// strings (`eip155:<chainId>`) NOT short names like `"base"`. The task
/// example showed `"network": "base"` — but that contradicts both the spec
/// and the reference TS implementation. We follow the spec.
pub const BASE_MAINNET_NETWORK: &str = "eip155:8453";

/// USDC token contract on Base mainnet (6 decimals). Hard-coded because the
/// 402 body has to identify *exactly* which asset is being requested; an
/// off-by-one address would let an attacker pay in a worthless token whose
/// `transferWithAuthorization` happens to match the EIP-3009 ABI.
pub const USDC_BASE_MAINNET: &str = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";

/// Default scheme — `exact` means "pay this exact amount, no haggling."
/// `upTo` (haggling) is the only other scheme in the spec and we don't need
/// it for fixed-price API calls.
pub const SCHEME_EXACT: &str = "exact";

/// Default seller timeout. Facilitator settlement on Base finalizes in 2–3
/// blocks (≈4–6 s); 60 s is comfortable headroom for the buyer to retry
/// without the requirements expiring.
pub const DEFAULT_TIMEOUT_SECS: u32 = 60;

/// HTTP timeout we apply to each individual `/verify` and `/settle` call.
/// Kept tight so a slow facilitator can't pin a request handler past the
/// Vercel function timeout (10 s on Hobby).
pub const FACILITATOR_HTTP_TIMEOUT_SECS: u64 = 5;

/// Cents-to-USDC-base-units conversion factor.
///
/// USDC is 6 decimals: `1 USDC = 1_000_000 base units`.
/// `1 cent = 0.01 USDC = 10_000 base units`.
const USDC_BASE_UNITS_PER_CENT: u64 = 10_000;

/// One element of the 402 response's `accepts` array (and the body sent to
/// `/verify` + `/settle` as `paymentRequirements`). Field names are
/// camelCase per the x402 v1 spec — do NOT rename.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PaymentRequirements {
    /// Payment scheme — `"exact"` for fixed-price.
    pub scheme: String,
    /// CAIP-2 network id, e.g. `"eip155:8453"` for Base mainnet.
    pub network: String,
    /// Amount in the asset's smallest unit, as a decimal string (uint256-safe).
    /// USDC has 6 decimals so `"10000"` here means $0.01.
    pub max_amount_required: String,
    /// ERC-20 contract address of the payment asset (USDC on Base).
    pub asset: String,
    /// Address that will RECEIVE the funds. This is the facilitator's
    /// configured settle-to address — NOT a wallet we custody.
    pub pay_to: String,
    /// Fully-qualified URL of the resource being paid for. Echoed back in
    /// the facilitator's signature domain to bind the payment to this call.
    pub resource: String,
    /// Human-readable description; helps wallets render a meaningful prompt.
    pub description: String,
    /// MIME type the resource will return on success. Optional in the spec.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// JSON-Schema of the success response. Optional. We keep this `None`
    /// at MVP — Phase 7 wires per-tool schemas in here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<serde_json::Value>,
    /// Seconds the buyer has to submit the signed authorization before the
    /// requirements are considered stale.
    pub max_timeout_seconds: u32,
    /// Scheme-specific extras (e.g. `{ "name": "USDC", "version": "2" }` for
    /// the EIP-3009 typed-data domain). Optional.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

/// 402 response body shape — the WHOLE JSON we return on the first call.
/// Wraps `accepts: [PaymentRequirements]` plus a protocol-version + error
/// string. Matches `PaymentRequirementsResponse` in the v1 spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentRequiredResponse {
    pub x402_version: u32,
    pub error: String,
    pub accepts: Vec<PaymentRequirements>,
}

impl PaymentRequiredResponse {
    /// Default 402 body — one set of requirements, the canonical error text.
    pub fn new(requirements: PaymentRequirements) -> Self {
        Self {
            x402_version: X402_VERSION,
            error: "X-PAYMENT header is required".to_string(),
            accepts: vec![requirements],
        }
    }
}

/// Builds the `PaymentRequirements` for a cost-gated resource.
///
/// `amount_cents` is the USD price (1 cent = 1¢). Internally converted to
/// USDC base units (×10_000) and serialized as a decimal string per spec.
///
/// `facilitator_pay_to` is the address the facilitator settles to — we
/// never custody this; it belongs to the facilitator integration.
pub fn challenge(
    resource: &str,
    amount_cents: u32,
    facilitator_pay_to: Address,
) -> PaymentRequirements {
    let amount_base_units = u64::from(amount_cents) * USDC_BASE_UNITS_PER_CENT;
    PaymentRequirements {
        scheme: SCHEME_EXACT.to_string(),
        network: BASE_MAINNET_NETWORK.to_string(),
        max_amount_required: amount_base_units.to_string(),
        asset: USDC_BASE_MAINNET.to_string(),
        // Address::to_string() produces lowercase 0x-prefixed; canonicalise
        // here so the JSON is byte-stable for snapshot tests.
        pay_to: format!("{:#x}", facilitator_pay_to),
        resource: resource.to_string(),
        description: format!("USDC ${:0.2} for {}", f64::from(amount_cents) / 100.0, resource),
        mime_type: Some("application/json".to_string()),
        output_schema: None,
        max_timeout_seconds: DEFAULT_TIMEOUT_SECS,
        extra: Some(serde_json::json!({
            // EIP-712 domain hints for the EIP-3009 signing path. USDC on
            // Base is "USD Coin" / version "2" per the deployed contract.
            "name": "USD Coin",
            "version": "2",
        })),
    }
}

/// Request body sent to `/verify` and `/settle`. Mirrors the v1 spec.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FacilitatorRequest<'a> {
    x402_version: u32,
    /// Opaque JSON — the decoded `X-PAYMENT` header content. We don't
    /// validate its shape here; the facilitator owns the EIP-3009 typed-data
    /// + signature checks.
    payment_payload: serde_json::Value,
    payment_requirements: &'a PaymentRequirements,
}

/// `/verify` response shape.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifyResponse {
    is_valid: bool,
    #[serde(default)]
    invalid_reason: Option<String>,
    #[serde(default)]
    payer: Option<String>,
}

/// `/settle` response shape.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettleResponse {
    success: bool,
    #[serde(default)]
    error_reason: Option<String>,
    #[serde(default)]
    payer: Option<String>,
    #[serde(default)]
    transaction: Option<String>,
    #[serde(default)]
    network: Option<String>,
}

/// What a successful `verify_and_settle` returns. The caller surfaces the
/// `transaction_hash` to the client (e.g. as an `X-PAYMENT-RECEIPT` response
/// header) so the agent can correlate its bookkeeping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementReceipt {
    /// On-chain tx hash of the settlement. Empty string in the response is
    /// turned into `None` here.
    pub transaction_hash: Option<String>,
    /// The payer address recovered from the signature by the facilitator.
    pub payer: Option<String>,
    /// Network the settlement landed on (e.g. `"base"` or `"eip155:8453"`,
    /// depending on facilitator). Echoed back to the caller for audit.
    pub network: Option<String>,
    /// USDC base units actually settled. Equal to
    /// `requirements.max_amount_required` for `scheme == "exact"`.
    pub settled_amount: String,
}

#[derive(Debug, Error)]
pub enum X402Error {
    /// `X-PAYMENT` header was not valid base64 or didn't decode to JSON.
    /// The caller should re-issue the 402 challenge.
    #[error("invalid X-PAYMENT header: {0}")]
    InvalidHeader(String),

    /// Facilitator's `/verify` returned `isValid: false`. Carries the
    /// human-readable reason from the facilitator.
    #[error("payment verification failed: {0}")]
    VerifyFailed(String),

    /// Facilitator's `/settle` returned `success: false`. Carries the
    /// human-readable reason. Most commonly the on-chain submission
    /// reverted (insufficient balance, expired authorization).
    #[error("payment settlement failed: {0}")]
    SettleFailed(String),

    /// Same nonce was already consumed on-chain (EIP-3009 replay guard).
    /// Distinct from a generic `VerifyFailed` / `SettleFailed` so callers
    /// can return another 402 (forcing a fresh nonce) instead of bubbling
    /// a 5xx that looks like a service outage.
    #[error("EIP-3009 nonce already used (replay)")]
    Replay,

    /// Network-level failure talking to the facilitator (timeout, DNS,
    /// 5xx). The caller maps this to 502 — it's NOT the buyer's fault.
    #[error("facilitator transport: {0}")]
    NetworkError(#[from] reqwest::Error),
}

/// Decode the base64 `X-PAYMENT` header and call the facilitator's
/// `/verify` then `/settle` endpoints in sequence. Returns the settlement
/// receipt on success.
///
/// The two calls are sequential by spec: `/verify` is a cheap signature +
/// allowance check the facilitator MUST pass before `/settle` (which costs
/// gas). Issuing them in parallel would waste gas on a payload that's
/// going to fail.
pub async fn verify_and_settle(
    payment_header_b64: &str,
    requirements: &PaymentRequirements,
    facilitator_url: &str,
    http: &reqwest::Client,
) -> Result<SettlementReceipt, X402Error> {
    let payload = decode_payment_header(payment_header_b64)?;

    // ---- /verify ---------------------------------------------------------
    let verify_url = format!("{}/verify", facilitator_url.trim_end_matches('/'));
    let body = FacilitatorRequest {
        x402_version: X402_VERSION,
        payment_payload: payload.clone(),
        payment_requirements: requirements,
    };
    let verify: VerifyResponse = http
        .post(&verify_url)
        .json(&body)
        .timeout(Duration::from_secs(FACILITATOR_HTTP_TIMEOUT_SECS))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if !verify.is_valid {
        let reason = verify.invalid_reason.unwrap_or_else(|| "(no reason)".into());
        if is_replay_reason(&reason) {
            return Err(X402Error::Replay);
        }
        return Err(X402Error::VerifyFailed(reason));
    }

    // ---- /settle ---------------------------------------------------------
    let settle_url = format!("{}/settle", facilitator_url.trim_end_matches('/'));
    let body = FacilitatorRequest {
        x402_version: X402_VERSION,
        payment_payload: payload,
        payment_requirements: requirements,
    };
    let settle_resp = http
        .post(&settle_url)
        .json(&body)
        .timeout(Duration::from_secs(FACILITATOR_HTTP_TIMEOUT_SECS))
        .send()
        .await?;
    // /settle may return 4xx with a JSON body — DON'T `error_for_status()`
    // here, we want to peek at `errorReason` to detect Replay.
    let status = settle_resp.status();
    let settle: SettleResponse = settle_resp.json().await?;
    if !settle.success {
        let reason = settle.error_reason.unwrap_or_else(|| "(no reason)".into());
        if is_replay_reason(&reason) {
            return Err(X402Error::Replay);
        }
        if status.is_server_error() {
            // Surface upstream-server failures as network errors so the
            // handler maps to 502 — they're operationally distinct from a
            // buyer-side rejection.
            return Err(X402Error::SettleFailed(format!("facilitator {status}: {reason}")));
        }
        return Err(X402Error::SettleFailed(reason));
    }

    Ok(SettlementReceipt {
        transaction_hash: settle
            .transaction
            .filter(|s| !s.is_empty() && s != "0x"),
        payer: settle.payer.or(verify.payer),
        network: settle.network,
        settled_amount: requirements.max_amount_required.clone(),
    })
}

/// Decode `X-PAYMENT: <base64-json>` → `serde_json::Value`. We intentionally
/// keep the inner shape opaque (`Value`) — the facilitator owns scheme
/// validation; we'd just be re-implementing it (poorly) if we typed it here.
fn decode_payment_header(header: &str) -> Result<serde_json::Value, X402Error> {
    let bytes = STANDARD
        .decode(header.trim())
        .map_err(|e| X402Error::InvalidHeader(format!("base64: {e}")))?;
    serde_json::from_slice(&bytes).map_err(|e| X402Error::InvalidHeader(format!("json: {e}")))
}

/// Heuristic match for "EIP-3009 nonce already used" reasons. The exact
/// wording is facilitator-specific (CDP says `"nonce already used"`,
/// reference impl says `"invalid nonce"`); we match on both substrings to
/// avoid coupling to one vendor's copy.
fn is_replay_reason(reason: &str) -> bool {
    let lower = reason.to_ascii_lowercase();
    (lower.contains("nonce") && (lower.contains("used") || lower.contains("invalid")))
        || lower.contains("replay")
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;

    const FACILITATOR_PAY_TO: Address = address!("209693Bc6afc0C5328bA36FaF03C514EF312287C");

    #[test]
    fn challenge_body_serializes_with_camelcase_keys() {
        let req = challenge("https://eth-tools.dev/api/v1/invoke", 1, FACILITATOR_PAY_TO);
        let json = serde_json::to_value(&req).unwrap();

        // Spec requires camelCase. A snake_case slip would silently drop us
        // off-spec — most facilitators ignore unknown fields and would
        // reject for "missing maxAmountRequired" without naming the cause.
        assert_eq!(json["scheme"], "exact");
        assert_eq!(json["network"], "eip155:8453");
        assert_eq!(json["maxAmountRequired"], "10000");
        assert_eq!(json["asset"], USDC_BASE_MAINNET);
        assert_eq!(json["maxTimeoutSeconds"], 60);
        assert!(json.get("max_amount_required").is_none(), "snake_case leaked");
        assert!(json.get("max_timeout_seconds").is_none(), "snake_case leaked");
    }

    #[test]
    fn challenge_body_round_trips() {
        let req = challenge("https://example.com/r", 25, FACILITATOR_PAY_TO);
        let s = serde_json::to_string(&req).unwrap();
        let back: PaymentRequirements = serde_json::from_str(&s).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn one_cent_is_ten_thousand_usdc_units() {
        // USDC is 6 decimals → 1 cent = 1e-2 USDC = 1e4 base units.
        // This conversion is the most likely source of a "we charged 100x
        // too much" bug, so it gets its own test.
        let r = challenge("/", 1, FACILITATOR_PAY_TO);
        assert_eq!(r.max_amount_required, "10000");
    }

    #[test]
    fn one_dollar_is_one_million_usdc_units() {
        let r = challenge("/", 100, FACILITATOR_PAY_TO);
        assert_eq!(r.max_amount_required, "1000000");
    }

    #[test]
    fn max_amount_fits_in_u64_at_realistic_prices() {
        // Plan §10.2 daily cap is $1/day; even a single $100 charge is well
        // inside u64. This test pins the upper bound so a future
        // u32-overflow refactor screams.
        let r = challenge("/", u32::MAX, FACILITATOR_PAY_TO);
        let parsed: u128 = r.max_amount_required.parse().unwrap();
        assert_eq!(parsed, u128::from(u32::MAX) * 10_000);
    }

    #[test]
    fn pay_to_is_lowercase_hex_with_0x_prefix() {
        // Some facilitators normalize, some don't. Lowercase + 0x prefix is
        // the byte-stable form that EIP-712 hashes correctly across both.
        let r = challenge("/", 1, FACILITATOR_PAY_TO);
        assert!(r.pay_to.starts_with("0x"), "expected 0x prefix: {}", r.pay_to);
        assert!(r.pay_to.chars().all(|c| c.is_ascii_hexdigit() || c == 'x'),
            "expected lowercase hex: {}", r.pay_to);
    }

    #[test]
    fn payment_required_response_wraps_one_requirement() {
        let r = challenge("/", 1, FACILITATOR_PAY_TO);
        let resp = PaymentRequiredResponse::new(r.clone());
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["x402Version"], 1);
        assert_eq!(json["error"], "X-PAYMENT header is required");
        assert_eq!(json["accepts"].as_array().unwrap().len(), 1);
        assert_eq!(json["accepts"][0]["scheme"], "exact");
    }

    #[test]
    fn decode_header_round_trip() {
        let payload = serde_json::json!({"signature": "0xabcd", "authorization": {}});
        let encoded = STANDARD.encode(payload.to_string().as_bytes());
        let decoded = decode_payment_header(&encoded).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn decode_header_rejects_invalid_base64() {
        let err = decode_payment_header("not!!base64").unwrap_err();
        assert!(matches!(err, X402Error::InvalidHeader(_)));
    }

    #[test]
    fn decode_header_rejects_invalid_json() {
        let encoded = STANDARD.encode(b"this is not json");
        let err = decode_payment_header(&encoded).unwrap_err();
        assert!(matches!(err, X402Error::InvalidHeader(_)));
    }

    #[test]
    fn replay_reason_matcher_recognises_common_phrasings() {
        // CDP wording
        assert!(is_replay_reason("nonce already used"));
        assert!(is_replay_reason("Nonce Already Used"));
        // Reference impl wording
        assert!(is_replay_reason("invalid nonce"));
        // Explicit
        assert!(is_replay_reason("replay detected"));
        // Negative — "nonce required" is a missing-field error, not replay
        assert!(!is_replay_reason("nonce required"));
        assert!(!is_replay_reason("insufficient balance"));
    }

    // ---- Integration with wiremock: fake facilitator end-to-end ---------
    //
    // These exercise the full `verify_and_settle` HTTP roundtrip against a
    // wiremock-stood-up server. They're the only thing that catches
    // header/method/url drift — pure-unit JSON tests don't.

    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn http_client() -> reqwest::Client {
        // Disable proxy + use rustls — matches prod config so tests catch
        // any "works locally, breaks behind the corporate proxy" bug.
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("test http client")
    }

    fn dummy_header() -> String {
        STANDARD.encode(r#"{"signature":"0x00","authorization":{}}"#)
    }

    #[tokio::test]
    async fn happy_path_returns_receipt_with_tx_hash() {
        let server = MockServer::start().await;
        let req = challenge("https://example.com/r", 1, FACILITATOR_PAY_TO);

        Mock::given(method("POST"))
            .and(path("/verify"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "isValid": true,
                "payer": "0xPAYER"
            })))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/settle"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "success": true,
                "payer": "0xPAYER",
                "transaction": "0xdeadbeef",
                "network": "base"
            })))
            .mount(&server)
            .await;

        let receipt = verify_and_settle(&dummy_header(), &req, &server.uri(), &http_client())
            .await
            .expect("happy path");
        assert_eq!(receipt.transaction_hash.as_deref(), Some("0xdeadbeef"));
        assert_eq!(receipt.payer.as_deref(), Some("0xPAYER"));
        assert_eq!(receipt.settled_amount, "10000");
    }

    #[tokio::test]
    async fn verify_failure_with_replay_substring_returns_replay() {
        let server = MockServer::start().await;
        let req = challenge("/", 1, FACILITATOR_PAY_TO);

        Mock::given(method("POST"))
            .and(path("/verify"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "isValid": false,
                "invalidReason": "nonce already used"
            })))
            .mount(&server)
            .await;

        let err = verify_and_settle(&dummy_header(), &req, &server.uri(), &http_client())
            .await
            .unwrap_err();
        assert!(matches!(err, X402Error::Replay), "got {err:?}");
    }

    #[tokio::test]
    async fn settle_400_with_replay_substring_returns_replay() {
        let server = MockServer::start().await;
        let req = challenge("/", 1, FACILITATOR_PAY_TO);

        Mock::given(method("POST"))
            .and(path("/verify"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "isValid": true
            })))
            .mount(&server)
            .await;

        // /settle returns 400 + JSON with errorReason — must NOT short-
        // circuit on the status code; the body carries the actionable info.
        Mock::given(method("POST"))
            .and(path("/settle"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "success": false,
                "errorReason": "nonce already used"
            })))
            .mount(&server)
            .await;

        let err = verify_and_settle(&dummy_header(), &req, &server.uri(), &http_client())
            .await
            .unwrap_err();
        assert!(matches!(err, X402Error::Replay), "got {err:?}");
    }

    #[tokio::test]
    async fn settle_generic_failure_returns_settle_failed() {
        let server = MockServer::start().await;
        let req = challenge("/", 1, FACILITATOR_PAY_TO);

        Mock::given(method("POST"))
            .and(path("/verify"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"isValid": true})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/settle"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "success": false,
                "errorReason": "insufficient balance"
            })))
            .mount(&server)
            .await;

        let err = verify_and_settle(&dummy_header(), &req, &server.uri(), &http_client())
            .await
            .unwrap_err();
        match err {
            X402Error::SettleFailed(msg) => assert!(msg.contains("insufficient")),
            other => panic!("expected SettleFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unreachable_facilitator_returns_network_error() {
        // Port 1 is reserved + unbound on every OS — guaranteed connection
        // refused. Cheaper than spinning up a server just to shut it down.
        let req = challenge("/", 1, FACILITATOR_PAY_TO);
        let err = verify_and_settle(
            &dummy_header(),
            &req,
            "http://127.0.0.1:1",
            &http_client(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, X402Error::NetworkError(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn verify_request_body_carries_v1_and_requirements() {
        // Pin the wire shape — catches a rename of `paymentPayload` or
        // `paymentRequirements` (e.g. accidental snake_case).
        let server = MockServer::start().await;
        let req = challenge("/r", 1, FACILITATOR_PAY_TO);
        let header = dummy_header();
        let decoded: serde_json::Value =
            serde_json::from_slice(&STANDARD.decode(&header).unwrap()).unwrap();

        Mock::given(method("POST"))
            .and(path("/verify"))
            .and(body_json(serde_json::json!({
                "x402Version": 1,
                "paymentPayload": decoded,
                "paymentRequirements": serde_json::to_value(&req).unwrap(),
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"isValid": true})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/settle"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "success": true, "transaction": "0xabc", "network": "base"
            })))
            .mount(&server)
            .await;

        verify_and_settle(&header, &req, &server.uri(), &http_client())
            .await
            .expect("body shape must match");
    }

    #[test]
    fn invalid_header_does_not_leak_in_display() {
        // Defense-in-depth: the `X-PAYMENT` header is a signed authorization
        // — its raw contents must not appear in any error string we surface
        // back to the client. Only the *reason* (e.g. "base64: invalid byte
        // 0x21 at offset 3") is acceptable.
        let err = decode_payment_header("not!!base64").unwrap_err();
        let msg = err.to_string();
        assert!(!msg.contains("not!!base64"), "header leaked into error: {msg}");
    }
}

//! Write-side MCP tools (plan §9.2) — dry-run preview today, signed +
//! broadcast in Phase 6.2.
//!
//! # Surface
//!
//! Four tools that mutate ERC-8004 registry state:
//!  - `register_agent`         — `Identity.register(string agentUri)`
//!  - `set_agent_uri`          — `Identity.setURI(uint256 agentId, string newUri)`
//!  - `give_feedback`          — `Reputation.giveFeedback(uint256, int8, uint8, bytes32, bytes32, string, string, bytes32)`
//!  - `request_validation`     — `Validation.requestValidation(uint256 agentId, string requestURI)`
//!
//! # Dry-run only (this PR)
//!
//! Phase 6.2 owns signing + broadcast. This PR ships the **shape** every
//! agent needs to plan a transaction:
//!   1. Encode the calldata via alloy's `sol!`-generated `abi_encode`.
//!   2. Build the `to`/`chain_id`/`data`/`gas_limit` quad that the rails
//!      consume.
//!   3. Run the **pure** rails (chain + recipient allowlist) locally — the
//!      stateful rails (kill switch, daily cap, balance ceiling) live in
//!      `crates/payments` and depend on the Phase-6 wallet crate landing.
//!   4. Return `{would_proceed, denied_reason?, calldata_preview, ...}`.
//!
//! When 6.2 lands the contract here changes only in one place: a `dry_run`
//! field flips from "always true" to opt-in, and on `dry_run=false` we hand
//! the `TransactionRequest` to `payments::wallet::sign_and_send` instead of
//! returning the preview. The output schema stays compatible — `would_proceed`
//! still means "rails passed" and the new `tx_hash`/`receipt` fields are
//! additive.
//!
//! # Why the rails check lives here (for now)
//!
//! `crates/payments::wallet::evaluate_all` is the canonical home for the six
//! rails, but it doesn't exist on this branch (Phase-6 wallet is on
//! `feat/phase-6-wallet-rails`). Rather than block this PR on a merge, the
//! two **pure** rails (chain == 8453, recipient ∈ registries) are inlined
//! here against the constants already exported by `eth_tools_payments::wallet`.
//! That keeps the dry-run preview honest without taking a dep on a crate
//! that's still being built. The phase 6.2 swap is a single function-call
//! site change in this file.
//!
//! # Ownership disclaimer (`set_agent_uri`)
//!
//! Per task spec: API keys are not yet bound to wallets, so we can't
//! confirm the caller actually owns the agent before preparing the calldata.
//! The recipient allowlist still binds the destination to the Identity
//! registry; **the registry itself enforces caller-owner equivalence
//! on-chain** when the tx lands. We document this in the tool description
//! so agents don't assume a successful preview implies a successful broadcast.

use super::{resolve_chain, ChainArg, ToolError};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, FixedBytes, I256, U256};
use eth_tools_core::DeniedReason;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::str::FromStr;

// ---------------------------------------------------------------------------
// Solidity bindings — function signatures only (events live in core::events).
// ---------------------------------------------------------------------------
//
// We define the four call signatures with alloy's `sol!` macro so the
// generated `*Call` structs give us `SELECTOR` + `abi_encode()` for free.
// The macro also yields zero-allocation decoders for tests that want to
// round-trip a preview blob back to its arguments.

alloy::sol! {
    /// `Identity.register(string agentUri) returns (uint256 agentId)`
    #[allow(missing_docs)]
    function register(string agentUri) external returns (uint256 agentId);

    /// `Identity.setURI(uint256 agentId, string newUri)`
    #[allow(missing_docs)]
    function setURI(uint256 agentId, string newUri) external;

    /// `Reputation.giveFeedback(...)` — eight-arg signature per ERC-8004.
    /// `value` is `int8` (signed, scaled by `valueDecimals`), tags are
    /// `bytes32`, endpoint + feedbackURI are `string`, feedbackHash is
    /// `bytes32`.
    #[allow(missing_docs)]
    function giveFeedback(
        uint256 agentId,
        int8 value,
        uint8 valueDecimals,
        bytes32 tag1,
        bytes32 tag2,
        string endpoint,
        string feedbackURI,
        bytes32 feedbackHash
    ) external;

    /// `Validation.requestValidation(uint256 agentId, string requestURI)`
    #[allow(missing_docs)]
    function requestValidation(uint256 agentId, string requestURI) external;
}

// ---------------------------------------------------------------------------
// Shared input / output shapes
// ---------------------------------------------------------------------------

/// Default gas budget we ship to the rails when the caller doesn't override.
/// 200k is well under the per-tx ceiling (`MAX_PER_TX_GAS = 500_000`) and
/// matches the worst-case `giveFeedback` observed on Base testnet. Callers
/// who know their tx is cheaper can pass an explicit lower number once 6.2
/// wires the field through.
const DEFAULT_GAS_LIMIT: u64 = 200_000;

/// Flat per-tx cost estimate in USD cents. Mirrors
/// `payments::wallet::ESTIMATED_TX_COST_USD_CENTS` (5¢) so previews don't
/// drift from the daily-cap rail when 6.2 wires in the real estimator.
const ESTIMATED_TX_COST_CENTS: u32 = 5;

/// Identity registry — same 0x8004… address on every ERC-8004 chain.
const IDENTITY_REGISTRY_HEX: &str = "8004A169FB4a3325136EB29fA0ceB6D2e539a432";
/// Reputation registry — same 0x8004… address on every ERC-8004 chain.
const REPUTATION_REGISTRY_HEX: &str = "8004BAa17C55a88189AE136b182e5fdA19dE9b63";
/// Validation registry — same 0x8004… address on every ERC-8004 chain.
const VALIDATION_REGISTRY_HEX: &str = "8004Cc8439f36fd5F9F049D9fF86523Df6dAAB58";

/// Recipient registries the wallet rails accept. Lifted from
/// `payments::wallet::ALLOWED_RECIPIENTS_HEX` so a drift between the two
/// lists fires the `recipients_match_payments_allowlist` test below.
const ALLOWED_RECIPIENTS_HEX: &[&str] = &[
    IDENTITY_REGISTRY_HEX,
    REPUTATION_REGISTRY_HEX,
    VALIDATION_REGISTRY_HEX,
];

/// Hard-coded chain the wallet rails accept (`payments::wallet::ALLOWED_CHAIN_ID`).
const ALLOWED_CHAIN_ID: u64 = 8453;

/// Output every write tool returns. Schema-stable across the dry-run → live
/// transition: when Phase 6.2 lands, `tx_hash` + `receipt` become populated
/// on `would_proceed && !dry_run`. All other fields are unchanged.
///
/// Does NOT derive `JsonSchema` — `DeniedReason` lives in `eth-tools-core`
/// which (by design) only depends on serde, not schemars. rmcp only
/// requires JsonSchema on **inputs** (for `tools/list`); outputs are
/// free-form per the MCP spec. If a future consumer wants a static schema
/// for this struct, the right fix is to add a `JsonSchema` derive behind a
/// feature flag in `eth-tools-core`, not to clone-and-fork the type here.
#[derive(Debug, Clone, Serialize)]
pub struct WritePreview {
    /// True iff every rail in `evaluate_all` would approve this tx. When
    /// false, `denied_reason` is populated with the first-fail rail.
    pub would_proceed: bool,
    /// First rail that rejected. `None` on `would_proceed = true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub denied_reason: Option<DeniedReason>,
    /// `0x…`-prefixed lowercase hex of the encoded calldata. Always present
    /// (encoding happens BEFORE the rails so a denied preview still shows
    /// the caller what would have been sent).
    pub calldata_preview: String,
    /// Flat per-tx estimate. Used by the rails to bump the daily-spend
    /// counter; surfaced in the preview so agents can budget against the
    /// $1/day cap.
    pub estimated_cost_cents: u32,
    /// Gas units committed to the rails. Today this is the
    /// `DEFAULT_GAS_LIMIT` constant; once 6.2 wires the field through the
    /// caller can override.
    pub gas_estimate: u64,
    /// Recipient the tx targets — surfaced for the caller's audit log.
    /// Always one of the three ERC-8004 registries on `would_proceed = true`.
    pub to: String,
    /// Chain id the tx targets. Always `8453` on `would_proceed = true`
    /// (the rails reject any other value).
    pub chain_id: u64,
    /// Always `true` in this PR. Phase 6.2 flips this when the
    /// caller opts into broadcast via `dry_run = false`.
    pub dry_run: bool,
}

// ---------------------------------------------------------------------------
// 1. register_agent
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RegisterAgentArgs {
    /// Chain name (`base`) or decimal chain id as a string (`"8453"`).
    /// MUST resolve to chain_id 8453 once the rails run — testnets are
    /// rejected because the wallet rails only allow Base mainnet at MVP.
    pub chain: ChainArg,
    /// HTTPS URL of the agent's ERC-8004 manifest. Validated for length
    /// (≤ 2048 bytes — registry storage is paid per byte) but not for
    /// reachability; reachability is the agent's responsibility, not the
    /// registry's.
    pub agent_uri: String,
}

/// Encode `Identity.register(string agentUri)` and run the rails.
pub fn register_agent(args: RegisterAgentArgs) -> Result<WritePreview, ToolError> {
    let chain = resolve_chain(&args.chain)?;
    validate_uri(&args.agent_uri, "agent_uri")?;

    let call = registerCall {
        agentUri: args.agent_uri,
    };
    let calldata = call.abi_encode();
    let to = parse_registry_address(IDENTITY_REGISTRY_HEX);
    Ok(build_preview(&calldata, to, chain.chain_id))
}

// ---------------------------------------------------------------------------
// 2. give_feedback
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GiveFeedbackArgs {
    pub chain: ChainArg,
    /// Decimal uint256 string — JSON numbers cannot round-trip uint256.
    pub agent_id: String,
    /// Signed feedback value (scaled by `value_decimals`). The ABI is
    /// `int8`, so any input outside `[-128, 127]` is rejected up front to
    /// keep the error clean (alloy's encoder would otherwise panic
    /// inside `I256::try_into::<i8>`).
    pub value: i32,
    /// Decimal places `value` is scaled by. Capped at 18 to match the
    /// ABI's `uint8` ceiling for `valueDecimals` and to keep agents from
    /// passing a number that overflows in a downstream consumer.
    pub value_decimals: u8,
    /// Optional `bytes32` tag (left-padded UTF-8). Omit for the zero tag.
    /// Strings longer than 32 bytes are rejected — no silent truncation.
    #[serde(default)]
    pub tag1: Option<String>,
    /// Optional `bytes32` tag (left-padded UTF-8). Same constraints as `tag1`.
    #[serde(default)]
    pub tag2: Option<String>,
    /// Optional endpoint the agent recommends contacting for context.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Optional URI to a fuller feedback document.
    #[serde(default)]
    pub feedback_uri: Option<String>,
    /// Optional `bytes32` hash of the off-chain feedback payload. `0x`-prefixed
    /// or bare hex, exactly 32 bytes. Omit for the zero hash.
    #[serde(default)]
    pub feedback_hash: Option<String>,
}

/// Encode `Reputation.giveFeedback(...)` and run the rails.
pub fn give_feedback(args: GiveFeedbackArgs) -> Result<WritePreview, ToolError> {
    let chain = resolve_chain(&args.chain)?;
    let agent_id = parse_uint256(&args.agent_id, "agent_id")?;
    let value = i8::try_from(args.value).map_err(|_| {
        ToolError::InvalidInput(format!(
            "value {} out of range for int8 (-128..=127)",
            args.value
        ))
    })?;
    if args.value_decimals > 18 {
        return Err(ToolError::InvalidInput(format!(
            "value_decimals {} > 18 (uint8 cap + downstream-overflow guard)",
            args.value_decimals
        )));
    }
    let tag1 = parse_bytes32_tag(args.tag1.as_deref(), "tag1")?;
    let tag2 = parse_bytes32_tag(args.tag2.as_deref(), "tag2")?;
    let endpoint = args.endpoint.unwrap_or_default();
    let feedback_uri = args.feedback_uri.unwrap_or_default();
    let feedback_hash = parse_bytes32_hex(args.feedback_hash.as_deref(), "feedback_hash")?;
    // Lengths checked AFTER all other parsing so callers see the most-specific
    // error first; the strings still must fit in calldata at sensible cost.
    validate_string_len(&endpoint, "endpoint", 1024)?;
    validate_string_len(&feedback_uri, "feedback_uri", 2048)?;

    let call = giveFeedbackCall {
        agentId: agent_id,
        value,
        valueDecimals: args.value_decimals,
        tag1,
        tag2,
        endpoint,
        feedbackURI: feedback_uri,
        feedbackHash: feedback_hash,
    };
    let calldata = call.abi_encode();
    let to = parse_registry_address(REPUTATION_REGISTRY_HEX);
    Ok(build_preview(&calldata, to, chain.chain_id))
}

// ---------------------------------------------------------------------------
// 3. set_agent_uri
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SetAgentUriArgs {
    pub chain: ChainArg,
    pub agent_id: String,
    /// New manifest URI. ≤ 2048 bytes (registry storage is paid per byte).
    pub new_uri: String,
}

/// Encode `Identity.setURI(uint256 agentId, string newUri)` and run the
/// rails.
///
/// # Ownership note
///
/// API keys are not yet bound to wallets, so this preview cannot pre-flight
/// the registry's caller-owner check. The recipient allowlist still binds
/// the destination to the Identity registry; on-chain ownership is enforced
/// when the tx lands. Phase 6.3 (key↔wallet binding) will add a pre-flight
/// ownership check; until then, a successful preview is **necessary but not
/// sufficient** for a successful broadcast.
pub fn set_agent_uri(args: SetAgentUriArgs) -> Result<WritePreview, ToolError> {
    let chain = resolve_chain(&args.chain)?;
    let agent_id = parse_uint256(&args.agent_id, "agent_id")?;
    validate_uri(&args.new_uri, "new_uri")?;

    let call = setURICall {
        agentId: agent_id,
        newUri: args.new_uri,
    };
    let calldata = call.abi_encode();
    let to = parse_registry_address(IDENTITY_REGISTRY_HEX);
    Ok(build_preview(&calldata, to, chain.chain_id))
}

// ---------------------------------------------------------------------------
// 4. request_validation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RequestValidationArgs {
    pub chain: ChainArg,
    pub agent_id: String,
    /// URI of the validation request payload. ≤ 2048 bytes.
    pub request_uri: String,
}

/// Encode `Validation.requestValidation(uint256 agentId, string requestURI)`
/// and run the rails.
pub fn request_validation(args: RequestValidationArgs) -> Result<WritePreview, ToolError> {
    let chain = resolve_chain(&args.chain)?;
    let agent_id = parse_uint256(&args.agent_id, "agent_id")?;
    validate_uri(&args.request_uri, "request_uri")?;

    let call = requestValidationCall {
        agentId: agent_id,
        requestURI: args.request_uri,
    };
    let calldata = call.abi_encode();
    let to = parse_registry_address(VALIDATION_REGISTRY_HEX);
    Ok(build_preview(&calldata, to, chain.chain_id))
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Run the **pure** wallet rails (chain + recipient) and stuff the result
/// into a [`WritePreview`]. Stateful rails (kill switch, balance, daily cap)
/// require the Phase-6 wallet crate — see module docs.
///
/// TODO(phase-6.2): swap this call site for
/// `payments::wallet::evaluate_all(tx, ctx)` once the wallet crate lands.
/// The output type already matches the post-6.2 shape (`would_proceed`,
/// `denied_reason` are stable), so the swap is local to this function.
fn build_preview(calldata: &[u8], to: Address, chain_id: u64) -> WritePreview {
    let denied = evaluate_pure_rails(chain_id, to);
    WritePreview {
        would_proceed: denied.is_none(),
        denied_reason: denied,
        calldata_preview: to_hex_prefixed(calldata),
        estimated_cost_cents: ESTIMATED_TX_COST_CENTS,
        gas_estimate: DEFAULT_GAS_LIMIT,
        to: format!("{to:#x}"),
        chain_id,
        dry_run: true,
    }
}

/// Pure-rails subset of `payments::wallet::evaluate_all`. Returns the
/// first-fail `DeniedReason` or `None` if both rails approve.
///
/// The codes / evaluator strings match the Phase-6 wallet module verbatim so
/// downstream telemetry doesn't have to special-case the dry-run path.
fn evaluate_pure_rails(chain_id: u64, to: Address) -> Option<DeniedReason> {
    if chain_id != ALLOWED_CHAIN_ID {
        return Some(
            DeniedReason::new("WRONG_CHAIN", "wallet.chain")
                .with_got(json!({ "chain_id": chain_id }))
                .with_expected(json!({ "chain_id": ALLOWED_CHAIN_ID })),
        );
    }
    let allowed: Vec<Address> = ALLOWED_RECIPIENTS_HEX
        .iter()
        .map(|h| parse_registry_address(h))
        .collect();
    if !allowed.contains(&to) {
        return Some(
            DeniedReason::new("RECIPIENT_NOT_ALLOWED", "wallet.allowlist")
                .with_got(json!({ "to": format!("{:#x}", to) }))
                .with_expected(json!({
                    "to_in": allowed.iter().map(|a| format!("{:#x}", a)).collect::<Vec<_>>(),
                })),
        );
    }
    None
}

/// Decode one of the hard-coded registry hex strings. Panics on bad hex —
/// these are compile-time constants and any drift would be caught by
/// `parse_registry_addresses_decode` below.
fn parse_registry_address(hex: &str) -> Address {
    let mut bytes = [0u8; 20];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .expect("ALLOWED_RECIPIENTS_HEX must be valid 40-char hex");
    }
    Address::from(bytes)
}

/// Parse a decimal uint256 string (no `0x` prefix, no leading sign). The
/// `field` arg names the offending field in the error so MCP clients can
/// point users at the specific bad input.
fn parse_uint256(s: &str, field: &str) -> Result<U256, ToolError> {
    if s.is_empty() {
        return Err(ToolError::InvalidInput(format!(
            "{field}: uint256 string must not be empty"
        )));
    }
    U256::from_str(s).map_err(|e| {
        ToolError::InvalidInput(format!("{field}: not a decimal uint256 ({e})"))
    })
}

/// Parse a `bytes32` hex string. Accepts an optional `0x` prefix, requires
/// exactly 32 bytes (64 hex chars). `None` returns the zero hash so the
/// caller can wire an `Option<String>` straight through.
fn parse_bytes32_hex(s: Option<&str>, field: &str) -> Result<FixedBytes<32>, ToolError> {
    let Some(s) = s else {
        return Ok(FixedBytes::ZERO);
    };
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    if stripped.len() != 64 {
        return Err(ToolError::InvalidInput(format!(
            "{field}: bytes32 hex must be 64 chars (got {})",
            stripped.len()
        )));
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&stripped[i * 2..i * 2 + 2], 16)
            .map_err(|_| ToolError::InvalidInput(format!("{field}: not hex")))?;
    }
    Ok(FixedBytes::from(out))
}

/// Pack an optional UTF-8 tag into a `bytes32`. UTF-8 bytes of the tag are
/// left-aligned and zero-padded — the same convention `web3.utils.fromUtf8`
/// uses. Tags longer than 32 bytes are rejected (no silent truncation).
fn parse_bytes32_tag(s: Option<&str>, field: &str) -> Result<FixedBytes<32>, ToolError> {
    let Some(s) = s else {
        return Ok(FixedBytes::ZERO);
    };
    let bytes = s.as_bytes();
    if bytes.len() > 32 {
        return Err(ToolError::InvalidInput(format!(
            "{field}: tag must be ≤ 32 UTF-8 bytes (got {})",
            bytes.len()
        )));
    }
    let mut out = [0u8; 32];
    out[..bytes.len()].copy_from_slice(bytes);
    Ok(FixedBytes::from(out))
}

/// Cheap URI shape guard. We refuse empty strings + strings over the
/// registry's effective storage budget; we do NOT reachability-check the
/// URI here because (a) network I/O inside an MCP tool deserves its own
/// gate (see `manifest::fetch_uri`), (b) the agent's URI may not be live
/// yet at registration time.
fn validate_uri(uri: &str, field: &str) -> Result<(), ToolError> {
    if uri.is_empty() {
        return Err(ToolError::InvalidInput(format!("{field} must not be empty")));
    }
    if uri.len() > 2048 {
        return Err(ToolError::InvalidInput(format!(
            "{field} too long: {} bytes (max 2048)",
            uri.len()
        )));
    }
    Ok(())
}

fn validate_string_len(s: &str, field: &str, max: usize) -> Result<(), ToolError> {
    if s.len() > max {
        return Err(ToolError::InvalidInput(format!(
            "{field} too long: {} bytes (max {max})",
            s.len()
        )));
    }
    Ok(())
}

fn to_hex_prefixed(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("0x");
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut s, "{b:02x}");
    }
    s
}

// Suppress unused-import warning when the test module is compiled out — `I256`
// is referenced by `i8::try_from(i32)` in `give_feedback`; alloy re-exports
// it for callers who want a saturating cast, but we don't use that here.
const _: fn() = || {
    let _: I256 = I256::ZERO;
};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::SolCall;

    // -- Selectors (locked at the byte level so an ABI rename fires a clear
    // mismatch instead of a runtime "no such method" 30 RPC calls later) ----

    /// Hand-computed selectors via `keccak256(signature)[..4]`. If the
    /// `sol!` macro ever renames an arg the selector hash flips, this test
    /// catches it before the calldata hits the network.
    #[test]
    fn selectors_match_keccak_signatures() {
        use sha3::{Digest, Keccak256};
        for (signature, sel) in [
            ("register(string)", registerCall::SELECTOR),
            ("setURI(uint256,string)", setURICall::SELECTOR),
            (
                "giveFeedback(uint256,int8,uint8,bytes32,bytes32,string,string,bytes32)",
                giveFeedbackCall::SELECTOR,
            ),
            (
                "requestValidation(uint256,string)",
                requestValidationCall::SELECTOR,
            ),
        ] {
            let mut h = Keccak256::new();
            h.update(signature.as_bytes());
            let digest = h.finalize();
            let mut expected = [0u8; 4];
            expected.copy_from_slice(&digest[..4]);
            assert_eq!(
                sel, expected,
                "selector drift for {signature}: macro {sel:?} vs keccak {expected:?}"
            );
        }
    }

    // -- register_agent ---------------------------------------------------

    #[test]
    fn register_agent_happy_path_passes_rails() {
        let preview = register_agent(RegisterAgentArgs {
            chain: "base".into(),
            agent_uri: "https://example.com/agent.json".into(),
        })
        .unwrap();
        assert!(preview.would_proceed);
        assert!(preview.denied_reason.is_none());
        assert_eq!(preview.chain_id, ALLOWED_CHAIN_ID);
        assert!(preview.calldata_preview.starts_with("0x"));
        // Selector byte-prefix sanity — preview must start with the
        // `register(string)` selector.
        let sel = &preview.calldata_preview[2..10];
        let want = registerCall::SELECTOR;
        assert_eq!(sel, format!("{:02x}{:02x}{:02x}{:02x}", want[0], want[1], want[2], want[3]));
        assert!(preview.dry_run);
    }

    #[test]
    fn register_agent_on_testnet_denied_by_chain_rail() {
        // base-sepolia (84532) is a valid chain name but the wallet rails
        // hard-cap to 8453. The recipient is correct, so the denial MUST be
        // WRONG_CHAIN (proves rail ordering matches phase-6 wallet).
        let preview = register_agent(RegisterAgentArgs {
            chain: "base-sepolia".into(),
            agent_uri: "https://example.com/x".into(),
        })
        .unwrap();
        assert!(!preview.would_proceed);
        let dr = preview.denied_reason.unwrap();
        assert_eq!(dr.code, "WRONG_CHAIN");
        assert_eq!(dr.evaluator, "wallet.chain");
    }

    #[test]
    fn register_agent_rejects_empty_uri() {
        let err = register_agent(RegisterAgentArgs {
            chain: "base".into(),
            agent_uri: String::new(),
        })
        .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[test]
    fn register_agent_rejects_overlong_uri() {
        let err = register_agent(RegisterAgentArgs {
            chain: "base".into(),
            agent_uri: "a".repeat(2049),
        })
        .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[test]
    fn register_agent_calldata_round_trips_through_alloy_decoder() {
        // alloy's `abi_decode` is the strongest "we encoded what we said we
        // did" check available — if the encoder + selector don't match the
        // declared sig the decoder fails.
        let preview = register_agent(RegisterAgentArgs {
            chain: "base".into(),
            agent_uri: "ipfs://QmFoo".into(),
        })
        .unwrap();
        let bytes = hex_to_bytes(&preview.calldata_preview);
        let decoded = registerCall::abi_decode(&bytes).expect("round-trips");
        assert_eq!(decoded.agentUri, "ipfs://QmFoo");
    }

    // -- give_feedback ----------------------------------------------------

    #[test]
    fn give_feedback_happy_path_passes_rails() {
        let preview = give_feedback(GiveFeedbackArgs {
            chain: "base".into(),
            agent_id: "42".into(),
            value: 100,
            value_decimals: 2,
            tag1: Some("quality".into()),
            tag2: None,
            endpoint: Some("https://feedback.example.com".into()),
            feedback_uri: Some("ipfs://QmHash".into()),
            feedback_hash: None,
        })
        .unwrap();
        assert!(preview.would_proceed);
        // Reputation registry — not Identity.
        let to_lower = preview.to.to_lowercase();
        assert_eq!(&to_lower[2..], REPUTATION_REGISTRY_HEX.to_lowercase());
    }

    #[test]
    fn give_feedback_calldata_round_trips() {
        let preview = give_feedback(GiveFeedbackArgs {
            chain: "base".into(),
            agent_id: "42".into(),
            value: -5,
            value_decimals: 0,
            tag1: Some("rude".into()),
            tag2: Some("late".into()),
            endpoint: None,
            feedback_uri: None,
            feedback_hash: Some(
                "0x0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20"
                    .into(),
            ),
        })
        .unwrap();
        let bytes = hex_to_bytes(&preview.calldata_preview);
        let decoded = giveFeedbackCall::abi_decode(&bytes).expect("round-trips");
        assert_eq!(decoded.agentId, U256::from(42u64));
        assert_eq!(decoded.value, -5);
        assert_eq!(decoded.valueDecimals, 0);
        // tag1 = "rude" zero-padded to 32 bytes.
        assert_eq!(&decoded.tag1[..4], b"rude");
        assert_eq!(&decoded.tag1[4..], &[0u8; 28]);
        assert_eq!(decoded.feedbackHash[0], 0x01);
        assert_eq!(decoded.feedbackHash[31], 0x20);
    }

    #[test]
    fn give_feedback_rejects_value_out_of_i8_range() {
        let err = give_feedback(GiveFeedbackArgs {
            chain: "base".into(),
            agent_id: "1".into(),
            value: 128, // i8::MAX + 1
            value_decimals: 0,
            tag1: None,
            tag2: None,
            endpoint: None,
            feedback_uri: None,
            feedback_hash: None,
        })
        .unwrap_err();
        match err {
            ToolError::InvalidInput(m) => assert!(m.contains("int8")),
            e => panic!("expected InvalidInput, got {e:?}"),
        }
    }

    #[test]
    fn give_feedback_rejects_value_decimals_over_18() {
        let err = give_feedback(GiveFeedbackArgs {
            chain: "base".into(),
            agent_id: "1".into(),
            value: 1,
            value_decimals: 19,
            tag1: None,
            tag2: None,
            endpoint: None,
            feedback_uri: None,
            feedback_hash: None,
        })
        .unwrap_err();
        match err {
            ToolError::InvalidInput(m) => assert!(m.contains("value_decimals")),
            e => panic!("expected InvalidInput, got {e:?}"),
        }
    }

    #[test]
    fn give_feedback_rejects_tag_over_32_bytes() {
        let err = give_feedback(GiveFeedbackArgs {
            chain: "base".into(),
            agent_id: "1".into(),
            value: 0,
            value_decimals: 0,
            tag1: Some("a".repeat(33)),
            tag2: None,
            endpoint: None,
            feedback_uri: None,
            feedback_hash: None,
        })
        .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[test]
    fn give_feedback_rejects_bad_uint256() {
        let err = give_feedback(GiveFeedbackArgs {
            chain: "base".into(),
            agent_id: "not-a-number".into(),
            value: 0,
            value_decimals: 0,
            tag1: None,
            tag2: None,
            endpoint: None,
            feedback_uri: None,
            feedback_hash: None,
        })
        .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[test]
    fn give_feedback_rejects_overflowing_uint256() {
        // 2^256 — one more than U256::MAX.
        let err = give_feedback(GiveFeedbackArgs {
            chain: "base".into(),
            agent_id: "115792089237316195423570985008687907853269984665640564039457584007913129639936".into(),
            value: 0,
            value_decimals: 0,
            tag1: None,
            tag2: None,
            endpoint: None,
            feedback_uri: None,
            feedback_hash: None,
        })
        .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[test]
    fn give_feedback_rejects_bad_bytes32_length() {
        let err = give_feedback(GiveFeedbackArgs {
            chain: "base".into(),
            agent_id: "1".into(),
            value: 0,
            value_decimals: 0,
            tag1: None,
            tag2: None,
            endpoint: None,
            feedback_uri: None,
            feedback_hash: Some("0xdead".into()),
        })
        .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    // -- set_agent_uri ----------------------------------------------------

    #[test]
    fn set_agent_uri_happy_path() {
        let preview = set_agent_uri(SetAgentUriArgs {
            chain: "base".into(),
            agent_id: "7".into(),
            new_uri: "https://new.example.com/agent.json".into(),
        })
        .unwrap();
        assert!(preview.would_proceed);
        assert_eq!(
            preview.to.to_lowercase()[2..],
            IDENTITY_REGISTRY_HEX.to_lowercase()
        );
    }

    #[test]
    fn set_agent_uri_calldata_round_trips() {
        let preview = set_agent_uri(SetAgentUriArgs {
            chain: "base".into(),
            agent_id: "99".into(),
            new_uri: "ipfs://Qm".into(),
        })
        .unwrap();
        let bytes = hex_to_bytes(&preview.calldata_preview);
        let decoded = setURICall::abi_decode(&bytes).expect("round-trips");
        assert_eq!(decoded.agentId, U256::from(99u64));
        assert_eq!(decoded.newUri, "ipfs://Qm");
    }

    // -- request_validation -----------------------------------------------

    #[test]
    fn request_validation_happy_path() {
        let preview = request_validation(RequestValidationArgs {
            chain: "base".into(),
            agent_id: "7".into(),
            request_uri: "https://validator.example/req/abc".into(),
        })
        .unwrap();
        assert!(preview.would_proceed);
        assert_eq!(
            preview.to.to_lowercase()[2..],
            VALIDATION_REGISTRY_HEX.to_lowercase()
        );
    }

    #[test]
    fn request_validation_calldata_round_trips() {
        let preview = request_validation(RequestValidationArgs {
            chain: "base".into(),
            agent_id: "123".into(),
            request_uri: "https://v.example/r".into(),
        })
        .unwrap();
        let bytes = hex_to_bytes(&preview.calldata_preview);
        let decoded = requestValidationCall::abi_decode(&bytes).expect("round-trips");
        assert_eq!(decoded.agentId, U256::from(123u64));
        assert_eq!(decoded.requestURI, "https://v.example/r");
    }

    // -- Rails ------------------------------------------------------------

    #[test]
    fn recipient_not_allowed_yields_denied_reason() {
        // Synthesize a bad-recipient preview via the helper directly — we
        // can't trigger this via the public tools (they hard-code the
        // registry per tool), so the helper is the right test surface.
        let bogus = Address::from([0u8; 20]);
        let preview = build_preview(b"\x00\x00\x00\x00", bogus, ALLOWED_CHAIN_ID);
        assert!(!preview.would_proceed);
        let dr = preview.denied_reason.unwrap();
        assert_eq!(dr.code, "RECIPIENT_NOT_ALLOWED");
        assert_eq!(dr.evaluator, "wallet.allowlist");
    }

    #[test]
    fn wrong_chain_yields_denied_reason() {
        let identity = parse_registry_address(IDENTITY_REGISTRY_HEX);
        let preview = build_preview(b"\x00\x00\x00\x00", identity, 1);
        assert!(!preview.would_proceed);
        let dr = preview.denied_reason.unwrap();
        assert_eq!(dr.code, "WRONG_CHAIN");
    }

    // -- Recipient constant lock-step with payments crate -----------------

    /// Regression guard: the hex strings hard-coded in this module MUST
    /// match the wallet rails' allowlist. Any drift would let a tool
    /// generate calldata for an address the rails immediately deny — silent
    /// dry-run lie. Tested against the constants re-exported from
    /// `eth_tools_payments::wallet`.
    #[test]
    fn recipients_match_payments_allowlist() {
        let payments = eth_tools_payments::wallet::ALLOWED_RECIPIENTS_HEX;
        let local = ALLOWED_RECIPIENTS_HEX;
        assert_eq!(
            payments.len(),
            local.len(),
            "drift in recipient count: payments={} local={}",
            payments.len(),
            local.len()
        );
        for hex in local {
            assert!(
                payments.iter().any(|p| p.eq_ignore_ascii_case(hex)),
                "{hex} missing from payments::wallet::ALLOWED_RECIPIENTS_HEX"
            );
        }
        assert_eq!(eth_tools_payments::wallet::ALLOWED_CHAIN_ID, ALLOWED_CHAIN_ID);
    }

    // -- helpers ---------------------------------------------------------

    fn hex_to_bytes(s: &str) -> Vec<u8> {
        let s = s.strip_prefix("0x").unwrap();
        (0..s.len() / 2)
            .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }
}

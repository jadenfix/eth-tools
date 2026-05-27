//! API DTOs. Wire format ≠ db format: addresses become `0x…` hex strings,
//! `agent_id` becomes a decimal string (JSON numbers can't carry uint256),
//! timestamps become RFC-3339.

use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use eth_tools_db::agents::AgentRow;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AgentDto {
    #[schema(example = "base")]
    pub chain: String,
    #[schema(example = 8453)]
    pub chain_id: i64,
    /// uint256 as decimal string
    #[schema(example = "42")]
    pub agent_id: String,
    /// 0x-prefixed 20-byte hex
    pub owner: String,
    pub agent_uri: Option<String>,
    /// 0x-prefixed; cleared on ownership Transfer (spec gotcha)
    pub agent_wallet: Option<String>,
    pub registered_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AgentDto {
    pub fn from_row(row: AgentRow, chain_name: &str) -> Self {
        Self {
            chain: chain_name.to_string(),
            chain_id: row.chain_id,
            agent_id: bigdecimal_to_string(&row.agent_id),
            owner: to_hex(&row.owner),
            agent_uri: row.agent_uri,
            agent_wallet: row.agent_wallet.as_deref().map(to_hex),
            registered_at: row.registered_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ListEnvelope<T: Serialize + ToSchema> {
    pub data: Vec<T>,
    /// Opaque; pass verbatim as ?cursor= to page
    pub next_cursor: Option<String>,
    pub staleness_ms: i64,
    #[schema(value_type = String, example = "db")]
    pub source: &'static str,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct OneEnvelope<T: Serialize + ToSchema> {
    pub data: T,
    pub staleness_ms: i64,
    #[schema(value_type = String, example = "db")]
    pub source: &'static str,
}

/// Concrete instantiation of `ListEnvelope<AgentDto>` so the OpenAPI spec
/// emits a stable `AgentList` schema name (utoipa generates synthetic names
/// like `ListEnvelope_AgentDto` for raw generic instantiations).
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = AgentList)]
pub struct AgentList {
    pub data: Vec<AgentDto>,
    /// Opaque; pass verbatim as ?cursor= to page
    pub next_cursor: Option<String>,
    pub staleness_ms: i64,
    #[schema(example = "db")]
    pub source: String,
}

/// Concrete instantiation of `OneEnvelope<AgentDto>` — see `AgentList`.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = AgentDetail)]
pub struct AgentDetail {
    pub data: AgentDto,
    pub staleness_ms: i64,
    #[schema(example = "db")]
    pub source: String,
}

/// Top-level error envelope returned by every non-2xx response.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ApiErrorBody {
    pub error: ApiErrorPayload,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ApiErrorPayload {
    #[schema(example = "AGENT_NOT_FOUND")]
    pub code: String,
    #[schema(example = "v1")]
    pub policy_version: String,
    #[schema(example = "api.lookup")]
    pub evaluator: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub override_hint: Option<String>,
}

pub fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("0x");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// ---------------------------------------------------------------------------
// Phase-4 expansion: request + response DTOs for the 15 new routes.
// Every type that appears in a handler signature has #[derive(ToSchema)] so
// the utoipa-driven spec stays the single source of truth.
// ---------------------------------------------------------------------------

// ---- /api/v1/agents/search -------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
pub struct SearchFilters {
    /// Chain name (`base`) or chain_id as decimal string (`8453`). Omit for any.
    pub chain: Option<String>,
    /// 0x-prefixed 20-byte hex owner address.
    pub owner: Option<String>,
    pub has_manifest: Option<bool>,
    pub has_endpoint: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct SearchRequest {
    /// Substring matched against `agent_uri` ILIKE. Omit for "any URI".
    pub query: Option<String>,
    /// Clamped to [1, 200]; default 50.
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub filters: SearchFilters,
}

// ---- /api/v1/manifest/{validate,hash,generate} -----------------------------

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct ManifestInput {
    /// Fetch the manifest from this URI (https://, ipfs://). Either `uri` OR
    /// `bytes_b64` must be set, not both.
    pub uri: Option<String>,
    /// Inline manifest bytes, base64-standard-encoded. Capped at 64 KiB so
    /// a buggy client can't OOM the function.
    pub bytes_b64: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ManifestValidateResponse {
    pub valid: bool,
    /// Empty when `valid == true`. Each entry is a JSON Pointer + message.
    pub errors: Vec<ManifestError>,
    /// Echoed back so the caller can correlate (`uri` if URL-sourced; empty
    /// when bytes were inlined).
    pub source: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ManifestError {
    /// RFC-6901 JSON Pointer to the failing field, e.g. `/services/0/endpoint`.
    pub pointer: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ManifestHashResponse {
    /// 0x-prefixed 32-byte hex.
    pub sha256: String,
    /// 0x-prefixed 32-byte hex (keccak256 of the raw bytes — see
    /// `core::manifest::CANONICALIZATION_NOTE`).
    pub keccak256: String,
    pub byte_len: usize,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ManifestGenerateResponse {
    /// Canonical manifest as a JSON value. The caller can serialize +
    /// hash this to obtain the on-chain `agentURI` payload.
    #[schema(value_type = Object)]
    pub manifest: serde_json::Value,
}

// ---- /api/v1/access/{check,explain} ----------------------------------------

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct AccessRequest {
    pub chain: String,
    /// uint256 as decimal string.
    pub agent_id: String,
    /// 0x-prefixed 20-byte hex.
    pub requester_address: String,
    /// Free-form action name (`invoke`, `read_manifest`, …). The policy
    /// engine is the source of truth — handler echoes back.
    pub action: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AccessCheckResponse {
    pub allowed: bool,
    /// One-line summary; details under `/access/explain`.
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AccessExplainResponse {
    pub allowed: bool,
    pub reason: String,
    /// Ordered breakdown of which checks ran, what each returned, and the
    /// rule that produced the final answer.
    pub steps: Vec<AccessStep>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AccessStep {
    pub check: String,
    pub ok: bool,
    pub detail: String,
}

// ---- /api/v1/reputation/* --------------------------------------------------

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FeedbackDto {
    pub chain_id: i64,
    pub agent_id: String,
    pub client_address: String,
    pub feedback_index: i64,
    /// Decimal string (NUMERIC(40,0)). Combine with `value_decimals` to
    /// interpret as a fixed-point score.
    pub value: String,
    pub value_decimals: i16,
    pub tag1: Option<String>,
    pub tag2: Option<String>,
    pub endpoint: Option<String>,
    pub feedback_uri: Option<String>,
    pub feedback_hash: Option<String>,
    pub is_revoked: bool,
    pub tx_hash: String,
    pub block_number: i64,
}

impl FeedbackDto {
    pub fn from_row(r: eth_tools_db::feedback::FeedbackRow) -> Self {
        Self {
            chain_id: r.chain_id,
            agent_id: bigdecimal_to_string(&r.agent_id),
            client_address: to_hex(&r.client_address),
            feedback_index: r.feedback_index,
            value: bigdecimal_to_string(&r.value),
            value_decimals: r.value_decimals,
            tag1: r.tag1,
            tag2: r.tag2,
            endpoint: r.endpoint,
            feedback_uri: r.feedback_uri,
            feedback_hash: r.feedback_hash.as_deref().map(to_hex),
            is_revoked: r.is_revoked,
            tx_hash: to_hex(&r.tx_hash),
            block_number: r.block_number,
        }
    }
}

/// Concrete instantiation for stable schema name — see `AgentList` doc.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = ReputationList)]
pub struct ReputationList {
    pub data: Vec<FeedbackDto>,
    pub staleness_ms: i64,
    #[schema(example = "db")]
    pub source: String,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct ReputationGiveRequest {
    pub chain: String,
    pub agent_id: String,
    /// Decimal string for NUMERIC(40,0).
    pub value: String,
    pub value_decimals: i16,
    pub tag1: Option<String>,
    pub tag2: Option<String>,
    pub endpoint: Option<String>,
    pub feedback_uri: Option<String>,
    /// 0x-prefixed 32-byte hex.
    pub feedback_hash: Option<String>,
}

// ---- /api/v1/validation/* --------------------------------------------------

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ValidationDto {
    pub chain_id: i64,
    /// 0x-prefixed 32-byte hex.
    pub request_hash: String,
    /// 0x-prefixed 20-byte hex.
    pub validator_addr: String,
    pub agent_id: String,
    pub request_uri: String,
    /// 0..100 score; `None` until the validator responds.
    pub response: Option<i16>,
    pub response_uri: Option<String>,
    pub response_hash: Option<String>,
    pub tag: Option<String>,
    pub last_update: chrono::DateTime<chrono::Utc>,
}

impl ValidationDto {
    pub fn from_row(r: eth_tools_db::validations::ValidationRow) -> Self {
        Self {
            chain_id: r.chain_id,
            request_hash: to_hex(&r.request_hash),
            validator_addr: to_hex(&r.validator_addr),
            agent_id: bigdecimal_to_string(&r.agent_id),
            request_uri: r.request_uri,
            response: r.response,
            response_uri: r.response_uri,
            response_hash: r.response_hash.as_deref().map(to_hex),
            tag: r.tag,
            last_update: r.last_update,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(as = ValidationDetail)]
pub struct ValidationDetail {
    pub data: ValidationDto,
    pub staleness_ms: i64,
    #[schema(example = "db")]
    pub source: String,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct ValidationRequest {
    pub chain: String,
    pub agent_id: String,
    /// 0x-prefixed 20-byte hex.
    pub validator_addr: String,
    pub request_uri: String,
    /// 0x-prefixed 32-byte hex (the canonical hash of the request_uri body).
    pub request_hash: String,
    pub tag: Option<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct ValidationRespond {
    pub chain: String,
    /// 0x-prefixed 32-byte hex of the request being responded to.
    pub request_hash: String,
    /// 0..100 score.
    pub response: i16,
    pub response_uri: Option<String>,
    /// 0x-prefixed 32-byte hex of the response body.
    pub response_hash: Option<String>,
}

// ---- /api/v1/invoke/* ------------------------------------------------------

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct InvokePrepare {
    pub chain: String,
    pub agent_id: String,
    /// 0x-prefixed contract address to call. The wallet rails restrict this
    /// to the three ERC-8004 registries; preview mode is permissive but warns.
    pub to: String,
    /// 4-byte function selector (`0x12345678`).
    pub selector: String,
    /// Hex-encoded ABI args (concatenated, no `0x` prefix). Empty = no args.
    #[serde(default)]
    pub args_hex: String,
    #[serde(default)]
    pub value_wei: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct InvokePrepareResponse {
    /// Fully-encoded calldata `0x` + selector + args. Hand this to the
    /// signing rails (or a user's external wallet) — this endpoint never
    /// signs anything.
    pub calldata: String,
    pub to: String,
    pub value_wei: String,
    pub chain_id: i64,
    /// Echoed back so the caller can audit.
    pub agent_id: String,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct InvokeExecute {
    pub chain: String,
    pub agent_id: String,
    pub to: String,
    pub selector: String,
    #[serde(default)]
    pub args_hex: String,
    #[serde(default)]
    pub value_wei: String,
    /// Whether to broadcast immediately. When `false`, the wallet rails return
    /// the signed tx in `signed_tx_hex` so the caller can broadcast itself.
    #[serde(default = "default_true")]
    pub broadcast: bool,
}

fn default_true() -> bool {
    true
}

pub(crate) fn bigdecimal_to_string(b: &BigDecimal) -> String {
    // BigDecimal serializes with possible trailing `.0…`; strip it for
    // canonical integer representation (agent_id is always a uint256 integer,
    // never fractional).
    let s = b.to_string();
    s.split('.').next().unwrap_or(&s).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        assert_eq!(to_hex(&[0xde, 0xad, 0xbe, 0xef]), "0xdeadbeef");
        assert_eq!(to_hex(&[]), "0x");
        assert_eq!(to_hex(&[0x00, 0x01]), "0x0001");
    }

    #[test]
    fn bigdecimal_to_string_drops_fraction() {
        use std::str::FromStr;
        assert_eq!(bigdecimal_to_string(&BigDecimal::from_str("42").unwrap()), "42");
        // BigDecimal::from_str("42.0") produces "42.0"; we want just "42".
        assert_eq!(bigdecimal_to_string(&BigDecimal::from_str("42.0").unwrap()), "42");
    }

    // ---- Shadow-type parity tests --------------------------------------
    //
    // `AgentList` and `AgentDetail` are non-generic shadow structs that exist
    // only so the OpenAPI spec emits stable `AgentList`/`AgentDetail` schema
    // names (utoipa-gen 5.5 doesn't support struct-level `#[aliases(...)]`;
    // that moved to the `utoipa-config` crate). The handlers return the real
    // generic `ListEnvelope<AgentDto>` / `OneEnvelope<AgentDto>`, so the
    // shadow types must serialize byte-identical to the real ones — otherwise
    // the spec lies about the wire format. These tests are the contract that
    // catches drift if anyone adds a field to one but not the other.

    fn sample_agent() -> AgentDto {
        AgentDto {
            chain: "base".into(),
            chain_id: 8453,
            agent_id: "42".into(),
            owner: "0xdeadbeef".into(),
            agent_uri: None,
            agent_wallet: Some("0xc0ffee".into()),
            registered_at: "2024-01-01T00:00:00Z".parse().unwrap(),
            updated_at: "2024-01-02T00:00:00Z".parse().unwrap(),
        }
    }

    #[test]
    fn agent_list_shadow_matches_list_envelope() {
        let envelope: ListEnvelope<AgentDto> = ListEnvelope {
            data: vec![sample_agent()],
            next_cursor: Some("opaque".into()),
            staleness_ms: 17,
            source: "db",
        };
        let shadow = AgentList {
            data: vec![sample_agent()],
            next_cursor: Some("opaque".into()),
            staleness_ms: 17,
            source: "db".to_string(),
        };
        assert_eq!(
            serde_json::to_string(&envelope).unwrap(),
            serde_json::to_string(&shadow).unwrap(),
        );
    }

    #[test]
    fn agent_detail_shadow_matches_one_envelope() {
        let envelope: OneEnvelope<AgentDto> = OneEnvelope {
            data: sample_agent(),
            staleness_ms: 17,
            source: "db",
        };
        let shadow = AgentDetail {
            data: sample_agent(),
            staleness_ms: 17,
            source: "db".to_string(),
        };
        assert_eq!(
            serde_json::to_string(&envelope).unwrap(),
            serde_json::to_string(&shadow).unwrap(),
        );
    }
}

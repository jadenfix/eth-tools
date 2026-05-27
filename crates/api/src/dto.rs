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

fn bigdecimal_to_string(b: &BigDecimal) -> String {
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

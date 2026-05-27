//! `find_agent`, `inspect_agent`, `search_agents`.
//!
//! Wire-format conversions live here too — chain names ↔ ids, `0x…` hex
//! ↔ `Vec<u8>`, `BigDecimal` ↔ decimal string. Mirrors `crates/api/src/dto.rs`
//! intentionally so REST and MCP return byte-identical agent payloads.

use super::{resolve_chain, ChainArg, DataEnvelope, ToolError};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use eth_tools_db::{
    agents::{self, SearchParams},
    Pool,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

// -----------------------------------------------------------------------------
// Shared output DTO.

/// Identical wire shape to `eth_tools_api::dto::AgentDto`. Duplicated here
/// (instead of re-exported) so the MCP crate doesn't pull a transitive HTTP
/// router dep and so the schemars derive isn't forced onto the REST DTO.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct AgentDto {
    pub chain: String,
    pub chain_id: i64,
    pub agent_id: String,
    pub owner: String,
    pub agent_uri: Option<String>,
    pub agent_wallet: Option<String>,
    pub registered_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AgentDto {
    fn from_row(row: agents::AgentRow, chain_name: &str) -> Self {
        Self {
            chain: chain_name.to_string(),
            chain_id: row.chain_id,
            agent_id: bigdecimal_to_str(&row.agent_id),
            owner: to_hex(&row.owner),
            agent_uri: row.agent_uri,
            agent_wallet: row.agent_wallet.as_deref().map(to_hex),
            registered_at: row.registered_at,
            updated_at: row.updated_at,
        }
    }
}

// -----------------------------------------------------------------------------
// 1. find_agent

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct FindAgentArgs {
    /// Substring match against `agent_uri` (case-insensitive). At least one
    /// match is returned per chain that has a hit; use `search_agents` to
    /// page through full results.
    pub query: String,
    /// Maximum rows to return. Clamped to [1, 50]. Default 5.
    #[serde(default = "default_find_limit")]
    pub limit: i64,
}

fn default_find_limit() -> i64 {
    5
}

pub async fn find_agent(pool: &Pool, args: FindAgentArgs) -> Result<DataEnvelope<Vec<AgentDto>>, ToolError> {
    if args.query.is_empty() {
        return Err(ToolError::InvalidInput("query must be non-empty".into()));
    }
    let rows = agents::search(
        pool,
        SearchParams {
            query: Some(args.query),
            limit: args.limit.clamp(1, 50),
            ..Default::default()
        },
    )
    .await?;
    Ok(DataEnvelope::db(rows_to_dtos(rows)))
}

// -----------------------------------------------------------------------------
// 2. inspect_agent

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct InspectAgentArgs {
    /// Chain name (`base`, `base-sepolia`) or decimal chain id as a string
    /// (`"8453"`). Accepting both spellings matches the REST contract.
    pub chain: ChainArg,
    /// Decimal uint256 string. JSON numbers can't carry uint256 losslessly.
    pub agent_id: String,
}

pub async fn inspect_agent(
    pool: &Pool,
    args: InspectAgentArgs,
) -> Result<DataEnvelope<Option<AgentDto>>, ToolError> {
    let chain = resolve_chain(&args.chain)?;
    let id = BigDecimal::from_str(&args.agent_id)
        .map_err(|_| ToolError::InvalidInput("agent_id must be a decimal uint256 string".into()))?;
    let row = agents::get_one(pool, chain.chain_id as i64, &id).await?;
    Ok(DataEnvelope::db(row.map(|r| AgentDto::from_row(r, chain.name))))
}

// -----------------------------------------------------------------------------
// 3. search_agents

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SearchAgentsArgs {
    /// Free-text substring (ILIKE on `agent_uri`). Empty / omitted = no
    /// text filter.
    #[serde(default)]
    pub query: Option<String>,
    /// Chain name or decimal id; omit to search across all chains.
    #[serde(default)]
    pub chain: Option<String>,
    /// Owner address (`0x…`, 20 bytes). Omit to ignore.
    #[serde(default)]
    pub owner: Option<String>,
    /// `true` = only agents that have an `agent_uri` set; `false` = only
    /// agents that don't; omit = both.
    #[serde(default)]
    pub has_manifest: Option<bool>,
    /// `true` = only agents whose wallet/endpoint has been verified; omit =
    /// both. (Backed by `agent_wallet IS NOT NULL` until the endpoint
    /// prober populates a real table in Phase-7 — see DB module comment.)
    #[serde(default)]
    pub has_endpoint: Option<bool>,
    /// Max rows. Clamped to [1, 200]. Default 50.
    #[serde(default = "default_search_limit")]
    pub limit: i64,
}

fn default_search_limit() -> i64 {
    50
}

pub async fn search_agents(
    pool: &Pool,
    args: SearchAgentsArgs,
) -> Result<DataEnvelope<Vec<AgentDto>>, ToolError> {
    let chain_id = match args.chain.as_deref() {
        None => None,
        Some(s) => Some(resolve_chain(s)?.chain_id as i64),
    };
    let owner = match args.owner.as_deref() {
        None => None,
        Some(s) => Some(parse_hex_address(s)?),
    };
    let rows = agents::search(
        pool,
        SearchParams {
            query: args.query,
            chain_id,
            owner,
            has_manifest: args.has_manifest,
            has_endpoint: args.has_endpoint,
            limit: args.limit.clamp(1, 200),
        },
    )
    .await?;
    Ok(DataEnvelope::db(rows_to_dtos(rows)))
}

// -----------------------------------------------------------------------------
// helpers

fn rows_to_dtos(rows: Vec<agents::AgentRow>) -> Vec<AgentDto> {
    rows.into_iter()
        .map(|r| {
            let chain_name = eth_tools_core::chains::by_id(r.chain_id as u64)
                .map(|c| c.name)
                .unwrap_or("unknown");
            AgentDto::from_row(r, chain_name)
        })
        .collect()
}

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("0x");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn bigdecimal_to_str(b: &BigDecimal) -> String {
    let s = b.to_string();
    s.split('.').next().unwrap_or(&s).to_string()
}

fn parse_hex_address(s: &str) -> Result<Vec<u8>, ToolError> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.len() != 40 {
        return Err(ToolError::InvalidInput(
            "owner must be a 20-byte 0x-hex address".into(),
        ));
    }
    let mut out = Vec::with_capacity(20);
    for i in 0..20 {
        let byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)
            .map_err(|_| ToolError::InvalidInput("owner must be hex".into()))?;
        out.push(byte);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn find_agent_args_deser() {
        let v: FindAgentArgs = serde_json::from_value(json!({"query": "foo"})).unwrap();
        assert_eq!(v.query, "foo");
        assert_eq!(v.limit, 5);

        let v: FindAgentArgs = serde_json::from_value(json!({"query": "foo", "limit": 10})).unwrap();
        assert_eq!(v.limit, 10);
    }

    #[test]
    fn inspect_agent_args_require_both_fields() {
        let r: Result<InspectAgentArgs, _> = serde_json::from_value(json!({"chain": "base"}));
        assert!(r.is_err());
        let v: InspectAgentArgs = serde_json::from_value(json!({"chain": "base", "agent_id": "42"})).unwrap();
        assert_eq!(v.chain, "base");
        assert_eq!(v.agent_id, "42");
    }

    #[test]
    fn search_agents_args_all_optional_except_limit_default() {
        let v: SearchAgentsArgs = serde_json::from_value(json!({})).unwrap();
        assert_eq!(v.limit, 50);
        assert!(v.query.is_none());
        assert!(v.owner.is_none());
        assert!(v.has_manifest.is_none());
    }

    #[test]
    fn parse_hex_address_accepts_with_and_without_prefix() {
        let a = parse_hex_address("0x0000000000000000000000000000000000000001").unwrap();
        assert_eq!(a.len(), 20);
        assert_eq!(a[19], 1);
        let b = parse_hex_address("0000000000000000000000000000000000000001").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn parse_hex_address_rejects_bad_length() {
        assert!(parse_hex_address("0xdead").is_err());
        assert!(parse_hex_address("not-hex-at-all-just-padding-padding-padding").is_err());
    }

    #[test]
    fn to_hex_canonical() {
        assert_eq!(to_hex(&[0xde, 0xad, 0xbe, 0xef]), "0xdeadbeef");
        assert_eq!(to_hex(&[]), "0x");
    }
}

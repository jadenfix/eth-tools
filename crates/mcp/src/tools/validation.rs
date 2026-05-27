//! `read_validation` — single-row lookup by `(chain, request_hash)`.

use super::{resolve_chain, ChainArg, DataEnvelope, ToolError};
use crate::tools::agents::to_hex;
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use eth_tools_db::{validations, Pool};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ReadValidationArgs {
    /// Chain name or decimal id.
    pub chain: ChainArg,
    /// `0x…` hex of the keccak256(request payload). 32 bytes / 66 chars.
    pub request_hash: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ValidationDto {
    pub chain: String,
    pub chain_id: i64,
    pub request_hash: String,
    pub validator_addr: String,
    pub agent_id: String,
    pub request_uri: String,
    pub response: Option<i16>,
    pub response_uri: Option<String>,
    pub response_hash: Option<String>,
    pub tag: Option<String>,
    pub last_update: DateTime<Utc>,
}

impl ValidationDto {
    fn from_row(row: validations::ValidationRow, chain_name: &str) -> Self {
        Self {
            chain: chain_name.to_string(),
            chain_id: row.chain_id,
            request_hash: to_hex(&row.request_hash),
            validator_addr: to_hex(&row.validator_addr),
            agent_id: bd_to_str(&row.agent_id),
            request_uri: row.request_uri,
            response: row.response,
            response_uri: row.response_uri,
            response_hash: row.response_hash.as_deref().map(to_hex),
            tag: row.tag,
            last_update: row.last_update,
        }
    }
}

pub async fn read_validation(
    pool: &Pool,
    args: ReadValidationArgs,
) -> Result<DataEnvelope<Option<ValidationDto>>, ToolError> {
    let chain = resolve_chain(&args.chain)?;
    let hash = parse_hash(&args.request_hash)?;
    let row = validations::get_one(pool, chain.chain_id as i64, &hash).await?;
    Ok(DataEnvelope::db(
        row.map(|r| ValidationDto::from_row(r, chain.name)),
    ))
}

fn parse_hash(s: &str) -> Result<Vec<u8>, ToolError> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.len() != 64 {
        return Err(ToolError::InvalidInput(
            "request_hash must be 32 bytes / 64 hex chars".into(),
        ));
    }
    let mut out = Vec::with_capacity(32);
    for i in 0..32 {
        let byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)
            .map_err(|_| ToolError::InvalidInput("request_hash must be hex".into()))?;
        out.push(byte);
    }
    Ok(out)
}

fn bd_to_str(b: &BigDecimal) -> String {
    let s = b.to_string();
    s.split('.').next().unwrap_or(&s).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn args_deser_with_0x_prefix() {
        let v: ReadValidationArgs = serde_json::from_value(json!({
            "chain": "base",
            "request_hash": "0x0000000000000000000000000000000000000000000000000000000000000001"
        }))
        .unwrap();
        assert_eq!(v.chain, "base");
    }

    #[test]
    fn parse_hash_accepts_both_prefixes() {
        let with = parse_hash("0x0000000000000000000000000000000000000000000000000000000000000001").unwrap();
        let without = parse_hash("0000000000000000000000000000000000000000000000000000000000000001").unwrap();
        assert_eq!(with, without);
        assert_eq!(with.len(), 32);
        assert_eq!(with[31], 1);
    }

    #[test]
    fn parse_hash_rejects_wrong_length() {
        assert!(parse_hash("0xdead").is_err());
    }
}

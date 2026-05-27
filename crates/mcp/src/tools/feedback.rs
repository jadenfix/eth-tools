//! `read_feedback` — surface on-chain reputation events for one agent.

use super::{resolve_chain, ChainArg, DataEnvelope, ToolError};
use crate::tools::agents::to_hex;
use bigdecimal::BigDecimal;
use eth_tools_db::{feedback, Pool};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ReadFeedbackArgs {
    /// Chain name or decimal id.
    pub chain: ChainArg,
    /// Decimal uint256 agent id.
    pub agent_id: String,
    /// Include revoked rows in the result. Default: false (revoked rows
    /// don't count toward reputation per ERC-8004 §4.2).
    #[serde(default)]
    pub include_revoked: bool,
}

/// Wire shape — mirrors the DB row but with hex strings and decimal-string
/// numerics so JSON round-trips losslessly.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FeedbackDto {
    pub chain: String,
    pub chain_id: i64,
    pub agent_id: String,
    pub client_address: String,
    pub feedback_index: i64,
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
    fn from_row(row: feedback::FeedbackRow, chain_name: &str) -> Self {
        Self {
            chain: chain_name.to_string(),
            chain_id: row.chain_id,
            agent_id: bd_to_str(&row.agent_id),
            client_address: to_hex(&row.client_address),
            feedback_index: row.feedback_index,
            value: bd_to_str(&row.value),
            value_decimals: row.value_decimals,
            tag1: row.tag1,
            tag2: row.tag2,
            endpoint: row.endpoint,
            feedback_uri: row.feedback_uri,
            feedback_hash: row.feedback_hash.as_deref().map(to_hex),
            is_revoked: row.is_revoked,
            tx_hash: to_hex(&row.tx_hash),
            block_number: row.block_number,
        }
    }
}

pub async fn read_feedback(
    pool: &Pool,
    args: ReadFeedbackArgs,
) -> Result<DataEnvelope<Vec<FeedbackDto>>, ToolError> {
    let chain = resolve_chain(&args.chain)?;
    let id = BigDecimal::from_str(&args.agent_id)
        .map_err(|_| ToolError::InvalidInput("agent_id must be a decimal uint256 string".into()))?;
    let rows = feedback::list_for_agent(pool, chain.chain_id as i64, &id, args.include_revoked).await?;
    let dtos = rows
        .into_iter()
        .map(|r| FeedbackDto::from_row(r, chain.name))
        .collect();
    Ok(DataEnvelope::db(dtos))
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
    fn args_deser_minimum_fields() {
        let v: ReadFeedbackArgs = serde_json::from_value(json!({"chain": "base", "agent_id": "42"})).unwrap();
        assert_eq!(v.chain, "base");
        assert!(!v.include_revoked);
    }

    #[test]
    fn args_deser_with_include_revoked() {
        let v: ReadFeedbackArgs =
            serde_json::from_value(json!({"chain": "8453", "agent_id": "1", "include_revoked": true}))
                .unwrap();
        assert!(v.include_revoked);
    }
}

//! API DTOs. Wire format ≠ db format: addresses become `0x…` hex strings,
//! `agent_id` becomes a decimal string (JSON numbers can't carry uint256),
//! timestamps become RFC-3339.

use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use eth_tools_db::agents::AgentRow;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize)]
pub struct ListEnvelope<T: Serialize> {
    pub data: Vec<T>,
    pub next_cursor: Option<String>,
    pub staleness_ms: i64,
    pub source: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct OneEnvelope<T: Serialize> {
    pub data: T,
    pub staleness_ms: i64,
    pub source: &'static str,
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
}

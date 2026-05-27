//! ERC-8004 registration manifest types + JSON Schema + canonicalization.
//!
//! ## CANONICALIZATION_NOTE
//!
//! The ERC-8004 spec **does not define a canonical JSON form** for the manifest.
//! Per plan §15, eth-tools commits to hashing the **raw bytes** returned by the
//! `agentURI` (no whitespace normalization, no key sorting). This matches the
//! in-repo spec wording and avoids the "two hashes for the same manifest" trap.
//!
//! Real schema + validator lands in the next PR; this is the bootstrap stub.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCard {
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub services: Vec<Service>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Service {
    /// "web" | "A2A" | "MCP" | "OASF" | "ENS" | "DID" | "email"
    #[serde(rename = "type")]
    pub kind: String,
    pub endpoint: String,
}

pub fn keccak256_raw_bytes(_bytes: &[u8]) -> [u8; 32] {
    // Real keccak lands with the alloy dep in the next PR.
    [0u8; 32]
}

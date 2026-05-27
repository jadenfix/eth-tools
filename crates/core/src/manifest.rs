//! ERC-8004 registration manifest types + JSON Schema + canonical hash.
//!
//! ## CANONICALIZATION_NOTE
//!
//! The ERC-8004 spec **does not define a canonical JSON form** for the
//! manifest. Per plan §15, eth-tools commits to hashing the **raw bytes**
//! returned by the `agentURI` (no whitespace normalization, no key sorting).
//! This matches the in-repo spec wording and avoids the "two hashes for the
//! same manifest" trap.
//!
//! ## Shared schema
//!
//! [`manifest_schema`] returns the ERC-8004 manifest v1 JSON Schema used by
//! both the MCP `validate_manifest` tool and the W2 `manifest_fetcher`
//! worker. Single source of truth — change here, both call sites pick it up.

use alloy_primitives::keccak256;
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

/// keccak256 of the raw manifest bytes. Used as the on-chain
/// tamper-evidence digest per ERC-8004; the spec hashes raw bytes, not a
/// canonical JSON form (see module note above).
pub fn keccak256_raw_bytes(bytes: &[u8]) -> [u8; 32] {
    keccak256(bytes).0
}

/// ERC-8004 manifest v1 JSON Schema. Kept inline so callers don't need to
/// ship a `.json` file with the binary. Cross-checks `name`, `description`,
/// the `endpoints[]` shape, and the optional `signature` field. Real spec
/// reference: https://eips.ethereum.org/EIPS/eip-8004 §Manifest.
pub fn manifest_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["name", "endpoints"],
        "properties": {
            "name": {"type": "string", "minLength": 1, "maxLength": 256},
            "description": {"type": "string", "maxLength": 4096},
            "endpoints": {
                "type": "array",
                "minItems": 1,
                "items": {
                    "type": "object",
                    "required": ["protocol", "url"],
                    "properties": {
                        "protocol": {"type": "string"},
                        "url": {"type": "string", "format": "uri"}
                    }
                }
            },
            "signature": {"type": "string", "pattern": "^0x[0-9a-fA-F]+$"}
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keccak_matches_known_vector() {
        // keccak256("") = c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470
        let h = keccak256_raw_bytes(b"");
        assert_eq!(
            format!("{}", alloy_primitives::hex::encode(h)),
            "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        );
    }

    #[test]
    fn schema_includes_required_fields() {
        let s = manifest_schema();
        let req = s["required"].as_array().expect("required is an array");
        let names: Vec<&str> = req.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"name"));
        assert!(names.contains(&"endpoints"));
    }
}

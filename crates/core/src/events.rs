//! Solidity event bindings for the ERC-8004 Identity registry + ERC-721
//! `Transfer` event (used by Worker W1, the registry scraper).
//!
//! The four events scraped by W1 (see plan §3 row W1):
//! 1. `Registered(uint256 indexed agentId, address indexed owner, string agentUri)`
//! 2. `URIUpdated(uint256 indexed agentId, string newUri)`
//! 3. `MetadataSet(uint256 indexed agentId, string key, bytes value)`
//! 4. ERC-721 `Transfer(address indexed from, address indexed to, uint256 indexed tokenId)`
//!
//! ## Topic-0 (event signature hash) constants
//!
//! Every alloy `sol!` event type exposes its keccak256 signature hash via
//! `Event::SIGNATURE_HASH` (a `B256`). We re-export those as `pub const`s so
//! downstream filter construction is one symbol lookup with no string churn.

use alloy::sol;
use alloy::sol_types::SolEvent;
use alloy_primitives::B256;

sol! {
    /// Emitted by the ERC-8004 Identity registry when a new agent is registered.
    #[derive(Debug)]
    event Registered(uint256 indexed agentId, address indexed owner, string agentUri);

    /// Emitted when an agent's manifest URI is rotated.
    #[derive(Debug)]
    event URIUpdated(uint256 indexed agentId, string newUri);

    /// Emitted when arbitrary key/value metadata is set on an agent
    /// (e.g. `agentWallet` rotations — see W6).
    #[derive(Debug)]
    event MetadataSet(uint256 indexed agentId, string key, bytes value);

    /// Standard ERC-721 transfer — emitted on agent ownership rotation.
    #[derive(Debug)]
    event Transfer(address indexed from, address indexed to, uint256 indexed tokenId);
}

/// `keccak256("Registered(uint256,address,string)")`
pub const REGISTERED_TOPIC: B256 = Registered::SIGNATURE_HASH;

/// `keccak256("URIUpdated(uint256,string)")`
pub const URI_UPDATED_TOPIC: B256 = URIUpdated::SIGNATURE_HASH;

/// `keccak256("MetadataSet(uint256,string,bytes)")`
pub const METADATA_SET_TOPIC: B256 = MetadataSet::SIGNATURE_HASH;

/// `keccak256("Transfer(address,address,uint256)")` — standard ERC-721 topic0.
pub const TRANSFER_TOPIC: B256 = Transfer::SIGNATURE_HASH;

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{b256, hex};

    /// The canonical ERC-721 `Transfer` topic0 is well known and rfc-grade —
    /// guards against any future macro regression.
    #[test]
    fn erc721_transfer_topic_matches_canonical() {
        assert_eq!(
            TRANSFER_TOPIC,
            b256!("0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef")
        );
    }

    /// Sanity check: every topic is a distinct 32-byte hash.
    #[test]
    fn all_topics_are_distinct() {
        let topics = [
            REGISTERED_TOPIC,
            URI_UPDATED_TOPIC,
            METADATA_SET_TOPIC,
            TRANSFER_TOPIC,
        ];
        for (i, a) in topics.iter().enumerate() {
            for (j, b) in topics.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "topic {i} == topic {j}");
                }
            }
            // Topic-0 hashes are never the zero hash.
            assert_ne!(*a, B256::ZERO);
        }
    }

    /// `Registered.SIGNATURE` is the canonical "Registered(uint256,address,string)"
    /// string emitted by the alloy macro — guards against parameter-rename drift.
    #[test]
    fn registered_signature_matches_spec() {
        assert_eq!(Registered::SIGNATURE, "Registered(uint256,address,string)");
        // hex/B256 imports kept used so clippy doesn't complain.
        let _ = hex::encode([0u8; 4]);
    }
}

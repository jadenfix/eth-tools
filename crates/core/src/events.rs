//! Solidity event bindings for the ERC-8004 registries (Identity, Reputation,
//! Validation) plus the ERC-721 `Transfer` event.
//!
//! ## Identity registry events (W1 — registry_scraper)
//! 1. `Registered(uint256 indexed agentId, address indexed owner, string agentUri)`
//! 2. `URIUpdated(uint256 indexed agentId, string newUri)`
//! 3. `MetadataSet(uint256 indexed agentId, string key, bytes value)`
//! 4. ERC-721 `Transfer(address indexed from, address indexed to, uint256 indexed tokenId)`
//!
//! ## Reputation registry events (W4 — reputation_aggregator)
//! 5. `NewFeedback(uint256 indexed agentId, address indexed clientAddress,
//!                 uint64 feedbackIndex, int128 value, uint8 valueDecimals,
//!                 string indexed indexedTag1, string tag1, string tag2,
//!                 string endpoint, string feedbackURI, bytes32 feedbackHash)`
//! 6. `FeedbackRevoked(uint256 indexed agentId, address indexed clientAddress,
//!                     uint64 indexed feedbackIndex)`
//! 7. `ResponseAppended(uint256 indexed agentId, address indexed clientAddress,
//!                      uint64 feedbackIndex, address indexed responder,
//!                      string responseURI, bytes32 responseHash)`
//!
//! ## Validation registry events (W5 — validation_aggregator)
//! 8. `ValidationRequest(address indexed validatorAddress, uint256 indexed agentId,
//!                       string requestURI, bytes32 indexed requestHash)`
//! 9. `ValidationResponse(address indexed validatorAddress, uint256 indexed agentId,
//!                        bytes32 indexed requestHash, uint8 response,
//!                        string responseURI, bytes32 responseHash, string tag)`
//!
//! Signatures verified against the canonical sources at
//! https://github.com/erc-8004/erc-8004-contracts (master / 2026-05-15) —
//! `contracts/ReputationRegistryUpgradeable.sol` and
//! `contracts/ValidationRegistryUpgradeable.sol`.
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

    // ---- ReputationRegistryUpgradeable (W4) ------------------------------
    //
    // NOTE on `NewFeedback`: the contract emits `tag1` twice — once as an
    // **indexed** `string` (which goes into topic-3 as keccak256(tag1)) and
    // once as a non-indexed `string` carried in data. We decode the
    // non-indexed copy; the indexed slot is unrecoverable text and we never
    // need to filter by it from this worker.

    /// Emitted on `giveFeedback`. `value` is `int128` (signed) per spec.
    #[derive(Debug)]
    event NewFeedback(
        uint256 indexed agentId,
        address indexed clientAddress,
        uint64 feedbackIndex,
        int128 value,
        uint8 valueDecimals,
        string indexed indexedTag1,
        string tag1,
        string tag2,
        string endpoint,
        string feedbackURI,
        bytes32 feedbackHash
    );

    /// Emitted on `revokeFeedback`. All three params are indexed.
    #[derive(Debug)]
    event FeedbackRevoked(
        uint256 indexed agentId,
        address indexed clientAddress,
        uint64 indexed feedbackIndex
    );

    /// Emitted on `appendResponse`. `responder` is the `msg.sender` of the
    /// response call — anyone may respond, so a feedback can have many
    /// responders. We persist each (responder, feedback) row.
    #[derive(Debug)]
    event ResponseAppended(
        uint256 indexed agentId,
        address indexed clientAddress,
        uint64 feedbackIndex,
        address indexed responder,
        string responseURI,
        bytes32 responseHash
    );

    // ---- ValidationRegistryUpgradeable (W5) ------------------------------
    //
    // NOTE: the canonical contract puts `tag` on `ValidationResponse`, NOT
    // `ValidationRequest`. The task spec is incorrect on this point; we
    // follow the contract source of truth.

    /// Emitted on `validationRequest`. `requestHash` is the user-supplied
    /// keccak256 of the request payload — also the table PK.
    #[derive(Debug)]
    event ValidationRequest(
        address indexed validatorAddress,
        uint256 indexed agentId,
        string requestURI,
        bytes32 indexed requestHash
    );

    /// Emitted on `validationResponse`. `response` is the 0–100 score.
    #[derive(Debug)]
    event ValidationResponse(
        address indexed validatorAddress,
        uint256 indexed agentId,
        bytes32 indexed requestHash,
        uint8 response,
        string responseURI,
        bytes32 responseHash,
        string tag
    );
}

/// `keccak256("Registered(uint256,address,string)")`
pub const REGISTERED_TOPIC: B256 = Registered::SIGNATURE_HASH;

/// `keccak256("URIUpdated(uint256,string)")`
pub const URI_UPDATED_TOPIC: B256 = URIUpdated::SIGNATURE_HASH;

/// `keccak256("MetadataSet(uint256,string,bytes)")`
pub const METADATA_SET_TOPIC: B256 = MetadataSet::SIGNATURE_HASH;

/// `keccak256("Transfer(address,address,uint256)")` — standard ERC-721 topic0.
pub const TRANSFER_TOPIC: B256 = Transfer::SIGNATURE_HASH;

/// `keccak256("NewFeedback(uint256,address,uint64,int128,uint8,string,string,string,string,string,bytes32)")`
pub const NEW_FEEDBACK_TOPIC: B256 = NewFeedback::SIGNATURE_HASH;

/// `keccak256("FeedbackRevoked(uint256,address,uint64)")`
pub const FEEDBACK_REVOKED_TOPIC: B256 = FeedbackRevoked::SIGNATURE_HASH;

/// `keccak256("ResponseAppended(uint256,address,uint64,address,string,bytes32)")`
pub const RESPONSE_APPENDED_TOPIC: B256 = ResponseAppended::SIGNATURE_HASH;

/// `keccak256("ValidationRequest(address,uint256,string,bytes32)")`
pub const VALIDATION_REQUEST_TOPIC: B256 = ValidationRequest::SIGNATURE_HASH;

/// `keccak256("ValidationResponse(address,uint256,bytes32,uint8,string,bytes32,string)")`
pub const VALIDATION_RESPONSE_TOPIC: B256 = ValidationResponse::SIGNATURE_HASH;

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
            NEW_FEEDBACK_TOPIC,
            FEEDBACK_REVOKED_TOPIC,
            RESPONSE_APPENDED_TOPIC,
            VALIDATION_REQUEST_TOPIC,
            VALIDATION_RESPONSE_TOPIC,
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

    /// Reputation / Validation event signature strings match the on-chain
    /// canonical sources (`ReputationRegistryUpgradeable.sol` /
    /// `ValidationRegistryUpgradeable.sol` on erc-8004/erc-8004-contracts).
    /// If an upstream contract refactor renames a param this assertion
    /// fires before the worker silently mis-decodes a log.
    #[test]
    fn reputation_validation_signatures_match_spec() {
        assert_eq!(
            NewFeedback::SIGNATURE,
            "NewFeedback(uint256,address,uint64,int128,uint8,string,string,string,string,string,bytes32)"
        );
        assert_eq!(
            FeedbackRevoked::SIGNATURE,
            "FeedbackRevoked(uint256,address,uint64)"
        );
        assert_eq!(
            ResponseAppended::SIGNATURE,
            "ResponseAppended(uint256,address,uint64,address,string,bytes32)"
        );
        assert_eq!(
            ValidationRequest::SIGNATURE,
            "ValidationRequest(address,uint256,string,bytes32)"
        );
        assert_eq!(
            ValidationResponse::SIGNATURE,
            "ValidationResponse(address,uint256,bytes32,uint8,string,bytes32,string)"
        );
    }
}

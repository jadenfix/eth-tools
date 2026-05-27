//! Golden-file decode of a hand-crafted ERC-8004 `Registered` log.
//!
//! NO network calls — we encode the event ourselves with the alloy `sol!`
//! bindings, then decode the result and assert every indexed/non-indexed
//! field round-trips. This catches:
//!   1. topic-0 (event signature hash) regressions,
//!   2. ABI parameter-order drift between the `sol!` definition and the spec,
//!   3. `B256` constants in `events.rs` getting out of sync with the macro.

use alloy::sol_types::SolEvent;
use alloy_primitives::{address, Address, LogData, U256};
use eth_tools_core::events::{Registered, REGISTERED_TOPIC};

/// Realistic Base mainnet `Registered` payload — agentId 42, owner
/// 0x8004...A432 (Identity registry creator addr, used as a stand-in for an
/// agent owner since we don't want network calls), agentUri pointing at an
/// IPFS gateway.
#[test]
fn decode_registered_roundtrip() {
    let agent_id = U256::from(42u64);
    let owner: Address = address!("0x8004A169FB4a3325136EB29fA0ceB6D2e539a432");
    let agent_uri = "ipfs://bafybeibtfaqzh4i3pkbz4nfbmlwq6f3fxgxhzgkyxzukacqgzwthypzdli/agent.json";

    // 1) Construct the event and encode it to a LogData (topics + ABI-encoded data).
    let event = Registered {
        agentId: agent_id,
        owner,
        agentUri: agent_uri.to_string(),
    };
    let log_data: LogData = event.encode_log_data();

    // Topic-0 must equal the constant exported by events.rs.
    assert_eq!(log_data.topics()[0], REGISTERED_TOPIC);
    // Two indexed params -> three total topics.
    assert_eq!(log_data.topics().len(), 3);

    // 2) Decode the LogData back through the SolEvent trait and assert fields.
    let decoded = Registered::decode_log_data(&log_data).expect("decode_log_data");
    assert_eq!(decoded.agentId, agent_id);
    assert_eq!(decoded.owner, owner);
    assert_eq!(decoded.agentUri, agent_uri);

    // 3) `decode_raw_log` is the API the worker scraper will use — exercise it too.
    let topics: Vec<_> = log_data.topics().to_vec();
    let decoded_raw = Registered::decode_raw_log(topics, &log_data.data).expect("decode_raw_log");
    assert_eq!(decoded_raw.agentId, agent_id);
    assert_eq!(decoded_raw.owner, owner);
}

/// A log whose topic-0 is wrong (e.g. a `URIUpdated` log fed to the
/// `Registered` decoder) must fail validation — protects the scraper from
/// silently mis-routing events when multiple subscriptions share a contract.
#[test]
fn decode_rejects_wrong_topic0() {
    // Build a valid `Registered` log, then mutate topic-0 to garbage.
    let event = Registered {
        agentId: U256::from(1u64),
        owner: Address::ZERO,
        agentUri: String::new(),
    };
    let mut log_data = event.encode_log_data();
    // Mutate topic[0] to a non-matching hash.
    let mut topics = log_data.topics().to_vec();
    topics[0] = alloy_primitives::B256::repeat_byte(0xAA);
    log_data = LogData::new(topics, log_data.data).expect("LogData::new");

    let result = Registered::decode_log_data_validate(&log_data);
    assert!(result.is_err(), "expected wrong-topic decode to fail");
}

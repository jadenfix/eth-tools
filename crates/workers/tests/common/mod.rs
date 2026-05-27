//! Shared mock `RpcProvider` and Log fixtures used by W4/W5 integration tests.
//!
//! Each integration test file is a separate binary, so the `OnceCell<DEPS>`
//! in `context.rs` doesn't conflict across files — but within a file we run
//! one sequential `#[tokio::test]` to keep env-var mutation deterministic.
#![allow(dead_code)] // each test binary uses a different subset

use std::collections::VecDeque;
use std::sync::Mutex;

use alloy::primitives::{Address, B256, U256};
use alloy::rpc::types::{Filter, Log};
use alloy::sol_types::SolEvent;
use alloy_primitives::LogData;
use async_trait::async_trait;
use eth_tools_rpc::{RpcError, RpcProvider};

pub struct MockProvider {
    name: String,
    pub block_number_q: Mutex<VecDeque<Result<u64, RpcError>>>,
    pub get_logs_q: Mutex<VecDeque<Result<Vec<Log>, RpcError>>>,
    pub get_logs_calls: Mutex<Vec<Filter>>,
}

impl MockProvider {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            block_number_q: Mutex::new(VecDeque::new()),
            get_logs_q: Mutex::new(VecDeque::new()),
            get_logs_calls: Mutex::new(Vec::new()),
        }
    }
    pub fn push_block_number(&self, r: Result<u64, RpcError>) {
        self.block_number_q.lock().unwrap().push_back(r);
    }
    pub fn push_logs(&self, r: Result<Vec<Log>, RpcError>) {
        self.get_logs_q.lock().unwrap().push_back(r);
    }
    pub fn get_logs_call_count(&self) -> usize {
        self.get_logs_calls.lock().unwrap().len()
    }
    pub fn get_logs_calls_snapshot(&self) -> Vec<Filter> {
        self.get_logs_calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl RpcProvider for MockProvider {
    fn name(&self) -> &str {
        &self.name
    }
    async fn get_block_number(&self) -> Result<u64, RpcError> {
        self.block_number_q
            .lock()
            .unwrap()
            .pop_front()
            .expect("MockProvider::get_block_number queue empty")
    }
    async fn get_logs(&self, filter: &Filter) -> Result<Vec<Log>, RpcError> {
        self.get_logs_calls.lock().unwrap().push(filter.clone());
        self.get_logs_q
            .lock()
            .unwrap()
            .pop_front()
            .expect("MockProvider::get_logs queue empty")
    }
}

/// Wrap a `LogData` with the per-log metadata workers consume
/// (block_number, log_index, tx_hash). One factory keeps every test's log
/// the same shape — only the event-specific encoding varies.
pub fn make_log(
    address: Address,
    log_data: LogData,
    block_number: u64,
    log_index: u64,
    tx_hash: B256,
) -> Log {
    let inner = alloy_primitives::Log {
        address,
        data: log_data,
    };
    Log {
        inner,
        block_hash: None,
        block_number: Some(block_number),
        block_timestamp: Some(1_700_000_000 + block_number),
        transaction_hash: Some(tx_hash),
        transaction_index: Some(0),
        log_index: Some(log_index),
        removed: false,
    }
}

// ---- Reputation registry event fixtures --------------------------------

use eth_tools_core::events::{
    FeedbackRevoked, NewFeedback, ResponseAppended, ValidationRequest, ValidationResponse,
};

#[allow(clippy::too_many_arguments)]
pub fn new_feedback_log(
    address: Address,
    agent_id: U256,
    client: Address,
    feedback_index: u64,
    value: i128,
    value_decimals: u8,
    tag1: &str,
    tag2: &str,
    endpoint: &str,
    feedback_uri: &str,
    feedback_hash: B256,
    block_number: u64,
    log_index: u64,
    tx_hash: B256,
) -> Log {
    // For an indexed `string` param, alloy's generated struct stores the
    // keccak256 hash as `FixedBytes<32>` (you can't reconstruct the string
    // from a hash on the wire). Workers always read the non-indexed copy
    // — we just need *some* B256 here for round-trip encoding.
    let indexed_tag1_hash = alloy_primitives::keccak256(tag1.as_bytes());
    let event = NewFeedback {
        agentId: agent_id,
        clientAddress: client,
        feedbackIndex: feedback_index,
        value,
        valueDecimals: value_decimals,
        indexedTag1: indexed_tag1_hash,
        tag1: tag1.to_string(),
        tag2: tag2.to_string(),
        endpoint: endpoint.to_string(),
        feedbackURI: feedback_uri.to_string(),
        feedbackHash: feedback_hash,
    };
    make_log(address, event.encode_log_data(), block_number, log_index, tx_hash)
}

pub fn feedback_revoked_log(
    address: Address,
    agent_id: U256,
    client: Address,
    feedback_index: u64,
    block_number: u64,
    log_index: u64,
    tx_hash: B256,
) -> Log {
    let event = FeedbackRevoked {
        agentId: agent_id,
        clientAddress: client,
        feedbackIndex: feedback_index,
    };
    make_log(address, event.encode_log_data(), block_number, log_index, tx_hash)
}

#[allow(clippy::too_many_arguments)]
pub fn response_appended_log(
    address: Address,
    agent_id: U256,
    client: Address,
    feedback_index: u64,
    responder: Address,
    response_uri: &str,
    response_hash: B256,
    block_number: u64,
    log_index: u64,
    tx_hash: B256,
) -> Log {
    let event = ResponseAppended {
        agentId: agent_id,
        clientAddress: client,
        feedbackIndex: feedback_index,
        responder,
        responseURI: response_uri.to_string(),
        responseHash: response_hash,
    };
    make_log(address, event.encode_log_data(), block_number, log_index, tx_hash)
}

// ---- Validation registry event fixtures --------------------------------

#[allow(clippy::too_many_arguments)]
pub fn validation_request_log(
    address: Address,
    validator: Address,
    agent_id: U256,
    request_uri: &str,
    request_hash: B256,
    block_number: u64,
    log_index: u64,
    tx_hash: B256,
) -> Log {
    let event = ValidationRequest {
        validatorAddress: validator,
        agentId: agent_id,
        requestURI: request_uri.to_string(),
        requestHash: request_hash,
    };
    make_log(address, event.encode_log_data(), block_number, log_index, tx_hash)
}

#[allow(clippy::too_many_arguments)]
pub fn validation_response_log(
    address: Address,
    validator: Address,
    agent_id: U256,
    request_hash: B256,
    response: u8,
    response_uri: &str,
    response_hash: B256,
    tag: &str,
    block_number: u64,
    log_index: u64,
    tx_hash: B256,
) -> Log {
    let event = ValidationResponse {
        validatorAddress: validator,
        agentId: agent_id,
        requestHash: request_hash,
        response,
        responseURI: response_uri.to_string(),
        responseHash: response_hash,
        tag: tag.to_string(),
    };
    make_log(address, event.encode_log_data(), block_number, log_index, tx_hash)
}

// ---- Postgres setup ----------------------------------------------------

use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

/// Boot a fresh Postgres container, run migrations, return the pool. None
/// on Docker-unavailable so CI can skip the suite cleanly.
pub async fn boot_pg() -> Option<(Box<dyn std::any::Any + Send + Sync>, eth_tools_db::Pool)> {
    let container = match Postgres::default().with_tag("16-alpine").start().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[skip] Docker unavailable: {e}");
            return None;
        }
    };
    let host = container.get_host().await.ok()?;
    let port = container.get_host_port_ipv4(5432).await.ok()?;
    let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
    let pool = eth_tools_db::connect(&url).await.expect("connect");
    eth_tools_db::migrate(&pool).await.expect("migrate");
    Some((Box::new(container), pool))
}

use std::sync::Arc;

use eth_tools_rpc::RotatingProvider;
use eth_tools_workers::WorkerContext;

/// Build a `WorkerContext` wired to the given pool + mock provider.
pub fn ctx_for(pool: eth_tools_db::Pool, mock: Arc<MockProvider>) -> WorkerContext {
    WorkerContext {
        pool,
        rpc: Arc::new(RotatingProvider::new(vec![mock])),
        vercel_env: "production".into(),
        dryrun: false,
        force: false,
    }
}

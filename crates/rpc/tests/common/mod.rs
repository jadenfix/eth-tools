//! Shared mock `RpcProvider` used by both integration tests.
#![allow(dead_code)] // Each integration test file uses a different subset.

use std::collections::VecDeque;
use std::sync::Mutex;

use alloy::rpc::types::{Filter, Log};
use async_trait::async_trait;
use eth_tools_rpc::{RpcError, RpcProvider};

/// A scripted RpcProvider that returns queued responses in FIFO order.
///
/// Each method has its own queue; pushing onto the queue determines the
/// next response. If a queue is empty, the method panics — every test
/// should know exactly how many calls it expects.
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

//! Placeholder RPC provider so the worker substrate compiles and
//! `RotatingProvider::new(vec![…])` (which asserts non-empty) can be
//! constructed by `context::deps()`.
//!
//! **PR3 replaces this with the real alloy-HTTP `RpcProvider`** that actually
//! talks to Alchemy / QuickNode / public Base. PR2 stops at the substrate
//! because no worker body exists yet to exercise live RPC, and dragging
//! `alloy-transport-http` + a `reqwest` Client into the cold-start path is
//! weight the architectural-refactor PR shouldn't bear.
//!
//! Calls return `RpcError::Transient` so any accidental invocation from a
//! test that *forgot* to install fake deps trips a recognisable rotator-
//! failover error rather than silently returning empty data.

use async_trait::async_trait;
use eth_tools_rpc::{RpcError, RpcProvider};

/// Sentinel provider — every method returns transient errors.
///
/// PR3 TODO: replace with `AlloyHttpProvider { client: reqwest::Client, url }`
/// implementing `RpcProvider` via `alloy::providers::Provider`.
#[derive(Debug, Default)]
pub(crate) struct StubProvider {
    name: &'static str,
}

impl StubProvider {
    pub(crate) const fn new(name: &'static str) -> Self {
        Self { name }
    }
}

#[async_trait]
impl RpcProvider for StubProvider {
    fn name(&self) -> &str {
        self.name
    }

    async fn get_block_number(&self) -> Result<u64, RpcError> {
        Err(RpcError::Transient(
            "rpc backend not wired — PR3 TODO (StubProvider)".into(),
        ))
    }

    async fn get_logs(
        &self,
        _filter: &alloy::rpc::types::Filter,
    ) -> Result<Vec<alloy::rpc::types::Log>, RpcError> {
        Err(RpcError::Transient(
            "rpc backend not wired — PR3 TODO (StubProvider)".into(),
        ))
    }

    async fn get_balance(
        &self,
        _addr: alloy_primitives::Address,
    ) -> Result<alloy_primitives::U256, RpcError> {
        Err(RpcError::Transient(
            "rpc backend not wired — PR3 TODO (StubProvider)".into(),
        ))
    }
}

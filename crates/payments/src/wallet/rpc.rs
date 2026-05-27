//! Wallet-side RPC abstraction (phase 6.2).
//!
//! We deliberately do NOT depend on the full `alloy::providers::Provider`
//! trait — its surface (signing, subscriptions, fillers, network generic) is
//! enormous, and our signing path only needs five calls:
//!
//!   1. `chain_id()`              — sanity-check signer chain vs allowed chain
//!   2. `pending_tx_count()`      — one-shot seed for `wallet_nonces`
//!   3. `balance_wei()`           — balance rail (converted to USD cents
//!      inside the balance provider)
//!   4. `eip1559_fees()`          — gas pricing for the tx envelope
//!   5. `send_raw_transaction()`  — submit + wait for receipt
//!
//! Two impls:
//!   * [`InMemoryWalletRpc`] — drives every unit test (happy, revert,
//!     timeout, gas-cap, nonce-conflict) without touching a real RPC.
//!   * [`AlloyWalletRpc`]    — production: builds an `alloy` HTTP provider
//!     for a single URL, plus a thin `RotatingProvider`-aware retry layer
//!     deferred to a follow-up (see `RotatingProvider` unification note in
//!     `wallet::sign`).
//!
//! The mock impl tracks every call in a ledger so concurrency tests can
//! assert exact tx-hash + nonce ordering.

use alloy_primitives::{Address, B256, U256};
use async_trait::async_trait;
use std::sync::{Mutex, RwLock};
use thiserror::Error;

use crate::wallet::nonce::{NonceError, NonceSeed};

#[derive(Debug, Error)]
pub enum WalletRpcError {
    /// Network / 5xx / timeout. The signing path treats this the same as
    /// `RotatingProvider::AllProvidersOpen` — bubble up, do NOT write a
    /// `wallet_txs` row (we don't know if anything landed).
    #[error("rpc transport: {0}")]
    Transport(String),
    /// `eth_sendRawTransaction` rejected the tx (e.g. nonce too low,
    /// insufficient funds). Caller logs and surfaces a denial.
    #[error("rpc rejected: {0}")]
    Rejected(String),
    /// Receipt didn't arrive within the timeout window. The tx hash MAY
    /// still confirm later — caller leaves `wallet_txs` row in
    /// `status='submitted'` and does NOT release the nonce.
    #[error("receipt timeout after {0:?}")]
    Timeout(std::time::Duration),
}

/// Per-tx gas pricing from `eth_feeHistory` (last 5 blocks, percentile 50).
#[derive(Debug, Clone, Copy)]
pub struct GasPrice {
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: u128,
}

/// Confirmed transaction receipt — only the fields the signing path needs
/// for reconciliation. Mirrors the shape of `alloy_rpc_types_eth::TransactionReceipt`
/// without forcing every test impl to construct the full receipt.
#[derive(Debug, Clone, Copy)]
pub struct WalletReceipt {
    pub tx_hash: B256,
    pub gas_used: u64,
    /// Effective gas price in wei — actual price paid per gas after EIP-1559
    /// base-fee adjustments. `gas_used * effective_gas_price` is total wei spent.
    pub effective_gas_price: u128,
    /// True iff the EVM `status` field is `0x1` (success). Reverts come back
    /// with `success = false`; the receipt is still returned because we paid
    /// the gas.
    pub success: bool,
}

/// The five-method RPC surface the signing path needs. Implementations may
/// hit the network (alloy) or stub everything in memory (tests).
#[async_trait]
pub trait WalletRpc: Send + Sync {
    /// `eth_chainId`. Compared with the rail's [`ALLOWED_CHAIN_ID`].
    ///
    /// [`ALLOWED_CHAIN_ID`]: crate::wallet::ALLOWED_CHAIN_ID
    async fn chain_id(&self) -> Result<u64, WalletRpcError>;

    /// `eth_getBalance(addr, "latest")` in wei. Native ETH only — the wallet
    /// does not hold ERC-20 collateral at MVP.
    async fn balance_wei(&self, addr: Address) -> Result<U256, WalletRpcError>;

    /// `eth_feeHistory(5, "latest", [50])` distilled into a single tx-ready
    /// `(maxFeePerGas, maxPriorityFeePerGas)` pair. The impl is free to use
    /// alloy's `estimate_eip1559_fees` or roll its own — the signing path
    /// only sees the final pair.
    async fn eip1559_fees(&self) -> Result<GasPrice, WalletRpcError>;

    /// Submit a signed-and-RLP-encoded transaction. Returns the transaction
    /// hash immediately; receipt fetch is separate (`wait_for_receipt`).
    async fn send_raw_transaction(&self, raw: &[u8]) -> Result<B256, WalletRpcError>;

    /// Block until the receipt is available, or return `Timeout`. The
    /// `confirmations` parameter is honoured by impls that can — the mock
    /// ignores it (the test sets up the receipt before calling).
    async fn wait_for_receipt(
        &self,
        tx_hash: B256,
        confirmations: u64,
        timeout: std::time::Duration,
    ) -> Result<WalletReceipt, WalletRpcError>;
}

/// Bridge so `WalletRpc` can be used as a [`NonceSeed`] without an extra
/// trait param on the caller. Forwards `pending_tx_count` to a
/// hypothetical `eth_getTransactionCount(signer, "pending")` call.
#[async_trait]
pub trait WalletRpcSeed: WalletRpc {
    async fn pending_tx_count(&self, signer: Address) -> Result<u64, WalletRpcError>;
}

/// Newtype that adapts any `WalletRpcSeed` into a [`NonceSeed`] (the trait
/// the nonce store accepts).
pub struct SeedAdapter<'a, T: WalletRpcSeed + ?Sized>(pub &'a T);

#[async_trait]
impl<T: WalletRpcSeed + ?Sized> NonceSeed for SeedAdapter<'_, T> {
    async fn pending_tx_count(&self, _chain_id: u64, signer: Address) -> Result<u64, NonceError> {
        self.0
            .pending_tx_count(signer)
            .await
            .map_err(|e| NonceError::Seed(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// In-memory mock
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockOutcome {
    /// Receipt with success=true, configurable gas_used and effective_gas_price.
    Success { gas_used: u64, effective_gas_price: u128 },
    /// Receipt with success=false (on-chain revert). Gas is still consumed.
    Revert { gas_used: u64, effective_gas_price: u128 },
    /// `wait_for_receipt` returns `Timeout` after the configured duration.
    /// (Caller observes the timeout immediately — we don't sleep.)
    Timeout,
    /// `send_raw_transaction` rejects with the given message (e.g. "nonce too low").
    SendRejected(&'static str),
    /// `send_raw_transaction` raises a transport error (5xx, network down).
    SendTransport(&'static str),
}

/// Records every interaction with the mock so tests can assert call order.
#[derive(Debug, Clone)]
pub enum MockOp {
    ChainId,
    Balance(Address),
    Fees,
    SendRaw { tx_hash: B256, raw_len: usize },
    WaitReceipt(B256),
    PendingTxCount(Address),
}

#[derive(Debug)]
pub struct InMemoryWalletRpc {
    state: RwLock<MockState>,
    ops: Mutex<Vec<MockOp>>,
}

#[derive(Debug)]
struct MockState {
    chain_id: u64,
    balance: U256,
    fees: GasPrice,
    pending_count: u64,
    /// Outcome that `send_raw_transaction` + the subsequent `wait_for_receipt`
    /// will produce. Each `send_raw_transaction` consumes the first entry;
    /// if empty, defaults to a 21_000-gas success.
    outcomes: std::collections::VecDeque<MockOutcome>,
    /// Counter so each `send_raw_transaction` produces a distinct tx_hash —
    /// concurrency tests depend on this for ordering assertions.
    tx_hash_seed: u64,
}

impl Default for InMemoryWalletRpc {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryWalletRpc {
    pub fn new() -> Self {
        Self {
            state: RwLock::new(MockState {
                chain_id: crate::wallet::ALLOWED_CHAIN_ID,
                balance: U256::from(1_000_000_000_000_000u128), // 0.001 ETH
                fees: GasPrice {
                    max_fee_per_gas: 1_000_000_000,         // 1 gwei
                    max_priority_fee_per_gas: 100_000_000,  // 0.1 gwei
                },
                pending_count: 0,
                outcomes: std::collections::VecDeque::new(),
                tx_hash_seed: 0,
            }),
            ops: Mutex::new(Vec::new()),
        }
    }

    pub fn set_chain_id(&self, id: u64) {
        self.state.write().unwrap().chain_id = id;
    }
    pub fn set_balance_wei(&self, w: U256) {
        self.state.write().unwrap().balance = w;
    }
    pub fn set_fees(&self, f: GasPrice) {
        self.state.write().unwrap().fees = f;
    }
    pub fn set_pending_count(&self, n: u64) {
        self.state.write().unwrap().pending_count = n;
    }
    /// Push an outcome that the next `send_raw_transaction` will produce.
    pub fn push_outcome(&self, o: MockOutcome) {
        self.state.write().unwrap().outcomes.push_back(o);
    }
    /// Snapshot the op ledger for assertions.
    pub fn ops(&self) -> Vec<MockOp> {
        self.ops.lock().unwrap().clone()
    }

    fn record(&self, op: MockOp) {
        self.ops.lock().unwrap().push(op);
    }
}

#[async_trait]
impl WalletRpc for InMemoryWalletRpc {
    async fn chain_id(&self) -> Result<u64, WalletRpcError> {
        self.record(MockOp::ChainId);
        Ok(self.state.read().unwrap().chain_id)
    }

    async fn balance_wei(&self, addr: Address) -> Result<U256, WalletRpcError> {
        self.record(MockOp::Balance(addr));
        Ok(self.state.read().unwrap().balance)
    }

    async fn eip1559_fees(&self) -> Result<GasPrice, WalletRpcError> {
        self.record(MockOp::Fees);
        Ok(self.state.read().unwrap().fees)
    }

    async fn send_raw_transaction(&self, raw: &[u8]) -> Result<B256, WalletRpcError> {
        let mut g = self.state.write().unwrap();
        // Honor pre-set outcomes that affect send.
        if let Some(o) = g.outcomes.front().copied() {
            match o {
                MockOutcome::SendRejected(m) => {
                    g.outcomes.pop_front();
                    return Err(WalletRpcError::Rejected(m.into()));
                }
                MockOutcome::SendTransport(m) => {
                    g.outcomes.pop_front();
                    return Err(WalletRpcError::Transport(m.into()));
                }
                _ => {}
            }
        }
        g.tx_hash_seed = g.tx_hash_seed.saturating_add(1);
        let mut bytes = [0u8; 32];
        bytes[24..32].copy_from_slice(&g.tx_hash_seed.to_be_bytes());
        let hash = B256::from(bytes);
        drop(g);
        self.record(MockOp::SendRaw {
            tx_hash: hash,
            raw_len: raw.len(),
        });
        Ok(hash)
    }

    async fn wait_for_receipt(
        &self,
        tx_hash: B256,
        _confirmations: u64,
        _timeout: std::time::Duration,
    ) -> Result<WalletReceipt, WalletRpcError> {
        self.record(MockOp::WaitReceipt(tx_hash));
        let outcome = self
            .state
            .write()
            .unwrap()
            .outcomes
            .pop_front()
            .unwrap_or(MockOutcome::Success {
                gas_used: 21_000,
                effective_gas_price: 1_000_000_000,
            });
        match outcome {
            MockOutcome::Success {
                gas_used,
                effective_gas_price,
            } => Ok(WalletReceipt {
                tx_hash,
                gas_used,
                effective_gas_price,
                success: true,
            }),
            MockOutcome::Revert {
                gas_used,
                effective_gas_price,
            } => Ok(WalletReceipt {
                tx_hash,
                gas_used,
                effective_gas_price,
                success: false,
            }),
            MockOutcome::Timeout => Err(WalletRpcError::Timeout(std::time::Duration::from_secs(60))),
            // Send outcomes shouldn't sit at the head when wait_for_receipt
            // is called — caller would've returned earlier. Tolerate them
            // so out-of-order test setup doesn't panic the suite.
            MockOutcome::SendRejected(m) | MockOutcome::SendTransport(m) => {
                Err(WalletRpcError::Transport(m.into()))
            }
        }
    }
}

#[async_trait]
impl WalletRpcSeed for InMemoryWalletRpc {
    async fn pending_tx_count(&self, addr: Address) -> Result<u64, WalletRpcError> {
        self.record(MockOp::PendingTxCount(addr));
        Ok(self.state.read().unwrap().pending_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;

    #[tokio::test]
    async fn mock_send_returns_distinct_hashes() {
        let rpc = InMemoryWalletRpc::new();
        let h1 = rpc.send_raw_transaction(&[1, 2, 3]).await.unwrap();
        let h2 = rpc.send_raw_transaction(&[1, 2, 3]).await.unwrap();
        assert_ne!(h1, h2, "each send must yield a distinct tx_hash");
    }

    #[tokio::test]
    async fn mock_default_receipt_is_success() {
        let rpc = InMemoryWalletRpc::new();
        let h = rpc.send_raw_transaction(&[]).await.unwrap();
        let r = rpc
            .wait_for_receipt(h, 1, std::time::Duration::from_secs(1))
            .await
            .unwrap();
        assert!(r.success);
        assert_eq!(r.gas_used, 21_000);
    }

    #[tokio::test]
    async fn mock_revert_outcome_propagates() {
        let rpc = InMemoryWalletRpc::new();
        rpc.push_outcome(MockOutcome::Revert {
            gas_used: 25_000,
            effective_gas_price: 2_000_000_000,
        });
        let h = rpc.send_raw_transaction(&[]).await.unwrap();
        let r = rpc
            .wait_for_receipt(h, 1, std::time::Duration::from_secs(1))
            .await
            .unwrap();
        assert!(!r.success);
        assert_eq!(r.gas_used, 25_000);
    }

    #[tokio::test]
    async fn mock_timeout_outcome_propagates() {
        let rpc = InMemoryWalletRpc::new();
        rpc.push_outcome(MockOutcome::Timeout);
        let h = rpc.send_raw_transaction(&[]).await.unwrap();
        let err = rpc
            .wait_for_receipt(h, 1, std::time::Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(matches!(err, WalletRpcError::Timeout(_)));
    }

    #[tokio::test]
    async fn mock_send_rejected_outcome_propagates() {
        let rpc = InMemoryWalletRpc::new();
        rpc.push_outcome(MockOutcome::SendRejected("nonce too low"));
        let err = rpc.send_raw_transaction(&[]).await.unwrap_err();
        assert!(matches!(err, WalletRpcError::Rejected(m) if m.contains("nonce")));
    }

    #[tokio::test]
    async fn mock_send_transport_outcome_propagates() {
        let rpc = InMemoryWalletRpc::new();
        rpc.push_outcome(MockOutcome::SendTransport("503 upstream"));
        let err = rpc.send_raw_transaction(&[]).await.unwrap_err();
        assert!(matches!(err, WalletRpcError::Transport(m) if m.contains("503")));
    }

    #[tokio::test]
    async fn seed_adapter_threads_nonce_seed_calls() {
        let rpc = InMemoryWalletRpc::new();
        rpc.set_pending_count(7);
        let adapter = SeedAdapter(&rpc);
        let n = NonceSeed::pending_tx_count(&adapter, 8453, address!("00000000000000000000000000000000000000aa"))
            .await
            .unwrap();
        assert_eq!(n, 7);
    }
}

//! Postgres-backed monotonic nonce allocator (plan §10 + phase 6.2).
//!
//! Vercel isolates have no shared in-process state. Calling
//! `eth_getTransactionCount(signer, "pending")` per tx is racy: two isolates
//! that dispatch concurrently can both see the same `pending` count and try
//! to use the same nonce, leading to one tx being rejected by the mempool
//! with "nonce too low" or — worse — both txs landing in different blocks
//! with the second one stuck "already known".
//!
//! The fix is to centralise nonce allocation in Postgres:
//!
//! ```text
//! BEGIN;
//!   SELECT next_nonce FROM wallet_nonces
//!     WHERE chain_id = $1 AND signer_address = $2
//!     FOR UPDATE;            -- blocks concurrent allocators on the same row
//!   UPDATE wallet_nonces SET next_nonce = next_nonce + 1, updated_at = NOW()
//!     WHERE chain_id = $1 AND signer_address = $2;
//! COMMIT;                    -- second allocator unblocks here, sees N+1
//! ```
//!
//! On first call after a restart (no row yet) we **seed** from
//! `eth_getTransactionCount(signer, "pending")` — that's the only place the
//! RPC value is consulted; from then on Postgres is the authoritative
//! source. Seed is itself wrapped in `ON CONFLICT DO NOTHING` so a race
//! between two cold-starting isolates produces exactly one row.
//!
//! ## Why FOR UPDATE and not advisory locks
//!
//! pg_advisory_xact_lock is cheaper but address-hash collisions across
//! chains would block unrelated allocators. The composite PK (chain_id,
//! signer_address) + `FOR UPDATE` gives exact per-EOA serialization with
//! zero collision risk and the row-level lock is automatically released on
//! commit/rollback.

use alloy_primitives::Address;
use async_trait::async_trait;
use sqlx::PgPool;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NonceError {
    #[error("nonce store transport: {0}")]
    Transport(String),
    /// First-call seed read failed — we couldn't ask the chain for the
    /// initial value. Bubbled up so the caller writes a `wallet_txs` row
    /// with `status='rejected'` rather than a phantom dispatch.
    #[error("nonce seed failed: {0}")]
    Seed(String),
}

/// The single method the signing path needs from a nonce store. `seed_from`
/// is invoked exactly once per (chain_id, signer) on a cold row; the impl
/// must guarantee at-most-once seeding even under concurrent first-call
/// pressure (the Postgres impl uses `INSERT ... ON CONFLICT DO NOTHING`).
#[async_trait]
pub trait NonceProvider: Send + Sync {
    /// Allocate the next nonce for `(chain_id, signer)`. On first call for
    /// a never-seen pair, invoke `seed_from` to fetch the initial value
    /// from the chain (`eth_getTransactionCount(signer, "pending")`).
    ///
    /// Returns the **nonce to use** (NOT `next_nonce`). Post-call the row's
    /// `next_nonce` is whatever was returned + 1.
    async fn allocate(
        &self,
        chain_id: u64,
        signer: Address,
        seed_from: &dyn NonceSeed,
    ) -> Result<u64, NonceError>;
}

/// The chain-side seed source. Implemented by [`crate::wallet::rpc::WalletRpc`]
/// — kept as a separate trait so the nonce store doesn't depend on the full
/// `WalletRpc` surface (and tests can swap in a stub that returns a fixed
/// number without spinning up a mock RPC).
#[async_trait]
pub trait NonceSeed: Send + Sync {
    /// `eth_getTransactionCount(signer, "pending")` — the seed value for a
    /// brand-new row in `wallet_nonces`.
    async fn pending_tx_count(&self, chain_id: u64, signer: Address) -> Result<u64, NonceError>;
}

// ---------------------------------------------------------------------------
// In-memory implementation (tests)
// ---------------------------------------------------------------------------

/// In-memory `NonceProvider` for unit tests. Uses a `Mutex<HashMap>` so the
/// concurrency test in `tests/wallet_nonce_concurrency.rs` exercises real
/// synchronization (not just happy-path serial dispense).
#[derive(Debug, Default)]
pub struct InMemoryNonceProvider {
    state: tokio::sync::Mutex<HashMap<(u64, Address), u64>>,
}

impl InMemoryNonceProvider {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inspector: returns the **next** nonce (NOT the last-issued one).
    /// `None` if no row yet.
    pub async fn peek(&self, chain_id: u64, signer: Address) -> Option<u64> {
        self.state.lock().await.get(&(chain_id, signer)).copied()
    }
}

#[async_trait]
impl NonceProvider for InMemoryNonceProvider {
    async fn allocate(
        &self,
        chain_id: u64,
        signer: Address,
        seed_from: &dyn NonceSeed,
    ) -> Result<u64, NonceError> {
        let mut guard = self.state.lock().await;
        let key = (chain_id, signer);
        let next = if let Some(n) = guard.get(&key).copied() {
            n
        } else {
            // First call — seed from the chain. Done while holding the lock
            // so two concurrent first-callers can't double-seed.
            seed_from.pending_tx_count(chain_id, signer).await?
        };
        guard.insert(key, next.saturating_add(1));
        Ok(next)
    }
}

/// Test stub for [`NonceSeed`] — returns a fixed value.
#[derive(Debug, Clone, Copy)]
pub struct FixedSeed(pub u64);

#[async_trait]
impl NonceSeed for FixedSeed {
    async fn pending_tx_count(&self, _chain_id: u64, _signer: Address) -> Result<u64, NonceError> {
        Ok(self.0)
    }
}

/// Test stub for [`NonceSeed`] — always errors.
#[derive(Debug, Clone, Copy)]
pub struct FailingSeed;

#[async_trait]
impl NonceSeed for FailingSeed {
    async fn pending_tx_count(&self, _chain_id: u64, _signer: Address) -> Result<u64, NonceError> {
        Err(NonceError::Seed("simulated RPC outage".into()))
    }
}

/// Snapshot the in-memory state for assertions.
#[derive(Debug, Default)]
pub struct PoisonNonceProvider; // counter that always errors

#[async_trait]
impl NonceProvider for PoisonNonceProvider {
    async fn allocate(
        &self,
        _chain_id: u64,
        _signer: Address,
        _seed_from: &dyn NonceSeed,
    ) -> Result<u64, NonceError> {
        Err(NonceError::Transport("simulated".into()))
    }
}

// ---------------------------------------------------------------------------
// Postgres implementation (prod)
// ---------------------------------------------------------------------------

/// Postgres-backed `NonceProvider`. One row per (chain_id, signer) in
/// `wallet_nonces`. Each `allocate` opens a transaction, takes a row-level
/// lock via `FOR UPDATE`, returns the current `next_nonce`, and bumps the
/// row by 1 before committing — so concurrent allocators serialize behind
/// the row lock and produce a strict 0,1,2,… sequence with no gaps.
#[derive(Debug, Clone)]
pub struct PgNonceProvider {
    pool: PgPool,
}

impl PgNonceProvider {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl NonceProvider for PgNonceProvider {
    async fn allocate(
        &self,
        chain_id: u64,
        signer: Address,
        seed_from: &dyn NonceSeed,
    ) -> Result<u64, NonceError> {
        let signer_bytes = signer.as_slice();
        let chain_id_i64 =
            i64::try_from(chain_id).map_err(|e| NonceError::Transport(format!("chain_id overflow: {e}")))?;

        // Seed-if-missing path. INSERT ... ON CONFLICT DO NOTHING so two
        // concurrent first-callers produce exactly one row; the loser's
        // INSERT is a no-op and they fall through to the SELECT FOR UPDATE.
        let row_exists: Option<(bool,)> = sqlx::query_as(
            "SELECT TRUE FROM wallet_nonces WHERE chain_id = $1 AND signer_address = $2",
        )
        .bind(chain_id_i64)
        .bind(signer_bytes)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| NonceError::Transport(format!("seed-check: {e}")))?;

        if row_exists.is_none() {
            let seed = seed_from.pending_tx_count(chain_id, signer).await?;
            let seed_i64 = i64::try_from(seed)
                .map_err(|e| NonceError::Transport(format!("nonce seed overflow: {e}")))?;
            sqlx::query(
                "INSERT INTO wallet_nonces (chain_id, signer_address, next_nonce) \
                 VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
            )
            .bind(chain_id_i64)
            .bind(signer_bytes)
            .bind(seed_i64)
            .execute(&self.pool)
            .await
            .map_err(|e| NonceError::Transport(format!("seed-insert: {e}")))?;
        }

        // Allocate inside a transaction with FOR UPDATE so concurrent
        // allocators serialise on the row lock.
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| NonceError::Transport(format!("begin: {e}")))?;

        let (current,): (i64,) = sqlx::query_as(
            "SELECT next_nonce FROM wallet_nonces \
             WHERE chain_id = $1 AND signer_address = $2 FOR UPDATE",
        )
        .bind(chain_id_i64)
        .bind(signer_bytes)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| NonceError::Transport(format!("select-for-update: {e}")))?;

        sqlx::query(
            "UPDATE wallet_nonces SET next_nonce = next_nonce + 1, updated_at = NOW() \
             WHERE chain_id = $1 AND signer_address = $2",
        )
        .bind(chain_id_i64)
        .bind(signer_bytes)
        .execute(&mut *tx)
        .await
        .map_err(|e| NonceError::Transport(format!("update: {e}")))?;

        tx.commit()
            .await
            .map_err(|e| NonceError::Transport(format!("commit: {e}")))?;

        u64::try_from(current)
            .map_err(|e| NonceError::Transport(format!("nonce underflow: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;

    fn signer() -> Address {
        address!("000000000000000000000000000000000000abcd")
    }

    #[tokio::test]
    async fn in_memory_seeds_then_increments() {
        let store = InMemoryNonceProvider::new();
        let seed = FixedSeed(42);
        assert_eq!(store.allocate(8453, signer(), &seed).await.unwrap(), 42);
        assert_eq!(store.allocate(8453, signer(), &seed).await.unwrap(), 43);
        assert_eq!(store.allocate(8453, signer(), &seed).await.unwrap(), 44);
        // peek returns next-to-issue.
        assert_eq!(store.peek(8453, signer()).await, Some(45));
    }

    #[tokio::test]
    async fn in_memory_independent_per_chain() {
        let store = InMemoryNonceProvider::new();
        let seed = FixedSeed(0);
        assert_eq!(store.allocate(8453, signer(), &seed).await.unwrap(), 0);
        assert_eq!(store.allocate(1, signer(), &seed).await.unwrap(), 0);
        assert_eq!(store.allocate(8453, signer(), &seed).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn in_memory_seed_failure_propagates() {
        let store = InMemoryNonceProvider::new();
        let err = store.allocate(8453, signer(), &FailingSeed).await.unwrap_err();
        assert!(matches!(err, NonceError::Seed(_)));
    }

    #[tokio::test]
    async fn in_memory_seed_not_called_after_first() {
        // The seed_from object can fail on subsequent calls and we must
        // still dispense from cached state.
        struct OnceSeed {
            called: tokio::sync::Mutex<bool>,
            initial: u64,
        }
        #[async_trait]
        impl NonceSeed for OnceSeed {
            async fn pending_tx_count(&self, _c: u64, _s: Address) -> Result<u64, NonceError> {
                let mut g = self.called.lock().await;
                if *g {
                    return Err(NonceError::Seed("must only be called once".into()));
                }
                *g = true;
                Ok(self.initial)
            }
        }

        let once = OnceSeed {
            called: tokio::sync::Mutex::new(false),
            initial: 7,
        };
        let store = InMemoryNonceProvider::new();
        assert_eq!(store.allocate(8453, signer(), &once).await.unwrap(), 7);
        assert_eq!(store.allocate(8453, signer(), &once).await.unwrap(), 8);
        assert_eq!(store.allocate(8453, signer(), &once).await.unwrap(), 9);
    }
}

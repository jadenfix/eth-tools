//! Daily-spend counter (plan §10.2 rail #5) — abstracted via a trait so the
//! rails can run end-to-end in tests without an Upstash/Redis instance.
//!
//! The production implementation is an Upstash REST client that issues
//! `INCRBY` + `EXPIRE` atomically (Upstash's REST API supports pipelines
//! that hit the same key). That client lands in the same PR as the actual
//! signing path — Phase 6 (rails only) ships the trait + an
//! [`InMemorySpendCounter`] test stub.
//!
//! ## Rollback semantics
//!
//! The rail order matters: the counter is incremented LAST, after every
//! other rail (kill switch, chain, recipient, gas, balance) has passed. If
//! the increment itself pushes us over `MAX_DAILY_SPEND_USD_CENTS`, the
//! rail decrements the counter back. If the signing path later fails
//! (network, RPC, on-chain revert) the caller (`sign::sign_and_send`) is
//! responsible for the rollback — see the rollback test in `policy.rs`.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::RwLock;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SpendCounterError {
    /// Underlying KV transport failure (network, auth, etc.). Treated by the
    /// rails as fail-closed: we'd rather refuse a tx than risk exceeding the
    /// cap because we couldn't read the counter.
    #[error("spend-counter transport: {0}")]
    Transport(String),
}

/// The Upstash-shaped surface the daily-cap rail needs. The KEY argument is
/// the full namespaced key from `wallet::daily_spend_key(vercel_env)`; the
/// trait does NOT do any prefixing of its own.
#[async_trait]
pub trait SpendCounter: Send + Sync {
    /// `INCRBY key amount` — returns the post-increment value. Caller is
    /// responsible for combining this with an EXPIRE on the same key.
    async fn incrby(&self, key: &str, amount: i64) -> Result<i64, SpendCounterError>;

    /// `DECRBY key amount` — rollback path. Returns the post-decrement value.
    async fn decrby(&self, key: &str, amount: i64) -> Result<i64, SpendCounterError>;

    /// `EXPIRE key ttl_seconds`. Idempotent — calling this every time is
    /// cheaper than maintaining a separate "did we set the TTL yet?" flag.
    async fn expire(&self, key: &str, ttl_seconds: i64) -> Result<(), SpendCounterError>;
}

/// In-memory test stub. Tracks counter values + records every operation for
/// assertions in rollback tests.
#[derive(Debug, Default)]
pub struct InMemorySpendCounter {
    state: RwLock<InnerState>,
}

#[derive(Debug, Default)]
struct InnerState {
    counters: HashMap<String, i64>,
    /// Append-only ledger of every op — used by tests to assert the right
    /// rollback path fired.
    ops: Vec<CounterOp>,
    /// If set, every call returns `Err(Transport(msg))` instead of touching
    /// state. Used to drive fail-closed tests.
    fail_with: Option<String>,
    /// Per-op failure switches — narrower than `fail_with` so tests can
    /// drive "incrby succeeds, expire fails" scenarios that exercise the
    /// counter-rollback path on EXPIRE failure.
    fail_expire_with: Option<String>,
    fail_decrby_with: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CounterOp {
    Incr { key: String, by: i64, after: i64 },
    Decr { key: String, by: i64, after: i64 },
    Expire { key: String, ttl: i64 },
}

impl InMemorySpendCounter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-seed a counter — useful for "what if the day's spend is already
    /// near the cap?" tests.
    pub fn with_initial(key: impl Into<String>, value: i64) -> Self {
        let s = Self::new();
        s.state
            .write()
            .expect("counter lock poisoned")
            .counters
            .insert(key.into(), value);
        s
    }

    pub fn fail_all_with(&self, msg: impl Into<String>) {
        self.state.write().expect("counter lock poisoned").fail_with = Some(msg.into());
    }

    /// Fail only `expire` calls — lets a test simulate the "incrby succeeded
    /// but EXPIRE failed" race the daily-cap rail rolls back on.
    pub fn fail_expire_with(&self, msg: impl Into<String>) {
        self.state.write().expect("counter lock poisoned").fail_expire_with = Some(msg.into());
    }

    /// Fail only `decrby` calls — lets a test simulate the worst case where
    /// the rollback `DECRBY` ALSO fails (counter inflated until TTL).
    pub fn fail_decrby_with(&self, msg: impl Into<String>) {
        self.state.write().expect("counter lock poisoned").fail_decrby_with = Some(msg.into());
    }

    pub fn value(&self, key: &str) -> i64 {
        self.state
            .read()
            .expect("counter lock poisoned")
            .counters
            .get(key)
            .copied()
            .unwrap_or(0)
    }

    pub fn ops(&self) -> Vec<CounterOp> {
        self.state.read().expect("counter lock poisoned").ops.clone()
    }
}

#[async_trait]
impl SpendCounter for InMemorySpendCounter {
    async fn incrby(&self, key: &str, amount: i64) -> Result<i64, SpendCounterError> {
        let mut guard = self
            .state
            .write()
            .map_err(|e| SpendCounterError::Transport(format!("lock poisoned: {e}")))?;
        if let Some(msg) = guard.fail_with.clone() {
            return Err(SpendCounterError::Transport(msg));
        }
        let entry = guard.counters.entry(key.to_string()).or_insert(0);
        *entry = entry.saturating_add(amount);
        let after = *entry;
        guard.ops.push(CounterOp::Incr {
            key: key.to_string(),
            by: amount,
            after,
        });
        Ok(after)
    }

    async fn decrby(&self, key: &str, amount: i64) -> Result<i64, SpendCounterError> {
        let mut guard = self
            .state
            .write()
            .map_err(|e| SpendCounterError::Transport(format!("lock poisoned: {e}")))?;
        if let Some(msg) = guard.fail_with.clone() {
            return Err(SpendCounterError::Transport(msg));
        }
        if let Some(msg) = guard.fail_decrby_with.clone() {
            return Err(SpendCounterError::Transport(msg));
        }
        let entry = guard.counters.entry(key.to_string()).or_insert(0);
        *entry = entry.saturating_sub(amount);
        let after = *entry;
        guard.ops.push(CounterOp::Decr {
            key: key.to_string(),
            by: amount,
            after,
        });
        Ok(after)
    }

    async fn expire(&self, key: &str, ttl_seconds: i64) -> Result<(), SpendCounterError> {
        let mut guard = self
            .state
            .write()
            .map_err(|e| SpendCounterError::Transport(format!("lock poisoned: {e}")))?;
        if let Some(msg) = guard.fail_with.clone() {
            return Err(SpendCounterError::Transport(msg));
        }
        if let Some(msg) = guard.fail_expire_with.clone() {
            return Err(SpendCounterError::Transport(msg));
        }
        guard.ops.push(CounterOp::Expire {
            key: key.to_string(),
            ttl: ttl_seconds,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn incrby_then_decrby_returns_to_initial() {
        let c = InMemorySpendCounter::new();
        assert_eq!(c.incrby("k", 30).await.unwrap(), 30);
        assert_eq!(c.incrby("k", 20).await.unwrap(), 50);
        assert_eq!(c.decrby("k", 50).await.unwrap(), 0);
        assert_eq!(c.value("k"), 0);
    }

    #[tokio::test]
    async fn fail_with_blocks_every_op() {
        let c = InMemorySpendCounter::new();
        c.fail_all_with("simulated");
        assert!(c.incrby("k", 1).await.is_err());
        assert!(c.decrby("k", 1).await.is_err());
        assert!(c.expire("k", 60).await.is_err());
    }

    #[tokio::test]
    async fn ops_are_recorded_in_order() {
        let c = InMemorySpendCounter::new();
        c.incrby("k", 10).await.unwrap();
        c.expire("k", 86_400).await.unwrap();
        c.decrby("k", 10).await.unwrap();
        let ops = c.ops();
        assert_eq!(ops.len(), 3);
        assert!(matches!(ops[0], CounterOp::Incr { .. }));
        assert!(matches!(ops[1], CounterOp::Expire { .. }));
        assert!(matches!(ops[2], CounterOp::Decr { .. }));
    }
}

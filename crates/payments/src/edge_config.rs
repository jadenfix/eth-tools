//! Vercel Edge Config client trait — plan §10.5 #6.
//!
//! Edge Config is a read-only KV store backed by a JSON blob replicated to
//! every Vercel edge location, p99 read ≈ 15 ms. We use exactly one key —
//! `wallet_enabled` — as the kill switch in front of every signed
//! transaction. Flipping that bool in the Vercel dashboard halts the wallet
//! crate in under a minute without a redeploy.
//!
//! The HTTP-backed implementation lands in the same PR as the actual
//! signing path (next milestone). For Phase 6 (rails only, signing stubbed)
//! this module ships:
//!  - The [`EdgeConfig`] trait — the only seam the rails see.
//!  - An [`InMemoryEdgeConfig`] test stub.
//!
//! Production code MUST inject the real client via the trait so the rails
//! never know which backend they're talking to. This same shape will let us
//! add a `LocalFileEdgeConfig` for `vercel dev` later without touching
//! `wallet::policy`.

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::RwLock;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EdgeConfigError {
    /// The Edge Config endpoint returned a network-level error. Treated by the
    /// rails as a "fail closed" signal — better to refuse a tx than to sign
    /// one when we can't verify the kill switch.
    #[error("edge config transport: {0}")]
    Transport(String),

    /// The key exists but its value is the wrong type (e.g. a string where
    /// we expected a bool). Indicates an operator misconfiguration; fail
    /// closed and surface the type mismatch.
    #[error("edge config type mismatch on key `{key}`: expected {expected}")]
    TypeMismatch { key: String, expected: &'static str },
}

#[async_trait]
pub trait EdgeConfig: Send + Sync {
    /// Returns the boolean at `key`, or `None` if the key is absent.
    ///
    /// `None` is distinct from `Some(false)` on purpose: the rails treat
    /// missing-key as "kill switch off" (i.e. the wallet is disabled),
    /// matching the production posture of "explicit-opt-in only."
    async fn get_bool(&self, key: &str) -> Result<Option<bool>, EdgeConfigError>;
}

/// Test/dev stub. NOT for production use — the rails inject this via the
/// trait so production gets the real HTTP client.
#[derive(Debug, Default)]
pub struct InMemoryEdgeConfig {
    inner: RwLock<HashMap<String, serde_json::Value>>,
}

impl InMemoryEdgeConfig {
    pub fn new() -> Self {
        Self::default()
    }

    /// Convenience constructor: pre-load a single bool.
    pub fn with_bool(key: impl Into<String>, value: bool) -> Self {
        let s = Self::new();
        s.set_bool(key, value);
        s
    }

    pub fn set_bool(&self, key: impl Into<String>, value: bool) {
        self.inner
            .write()
            .expect("edge-config lock poisoned")
            .insert(key.into(), serde_json::Value::Bool(value));
    }

    /// Inject a non-bool value at `key` — used by tests that need to drive
    /// the `TypeMismatch` branch.
    pub fn set_raw(&self, key: impl Into<String>, value: serde_json::Value) {
        self.inner
            .write()
            .expect("edge-config lock poisoned")
            .insert(key.into(), value);
    }
}

#[async_trait]
impl EdgeConfig for InMemoryEdgeConfig {
    async fn get_bool(&self, key: &str) -> Result<Option<bool>, EdgeConfigError> {
        let guard = self
            .inner
            .read()
            .map_err(|e| EdgeConfigError::Transport(format!("lock poisoned: {e}")))?;
        match guard.get(key) {
            None => Ok(None),
            Some(serde_json::Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(EdgeConfigError::TypeMismatch {
                key: key.to_string(),
                expected: "bool",
            }),
        }
    }
}

/// Always-fails stub — pin for tests that must observe the fail-closed path.
#[derive(Debug, Default)]
pub struct UnreachableEdgeConfig;

#[async_trait]
impl EdgeConfig for UnreachableEdgeConfig {
    async fn get_bool(&self, _key: &str) -> Result<Option<bool>, EdgeConfigError> {
        Err(EdgeConfigError::Transport("simulated outage".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_key_returns_none() {
        let ec = InMemoryEdgeConfig::new();
        assert_eq!(ec.get_bool("wallet_enabled").await.unwrap(), None);
    }

    #[tokio::test]
    async fn round_trip_bool() {
        let ec = InMemoryEdgeConfig::with_bool("wallet_enabled", true);
        assert_eq!(ec.get_bool("wallet_enabled").await.unwrap(), Some(true));
        ec.set_bool("wallet_enabled", false);
        assert_eq!(ec.get_bool("wallet_enabled").await.unwrap(), Some(false));
    }

    #[tokio::test]
    async fn wrong_type_is_explicit_error() {
        let ec = InMemoryEdgeConfig::new();
        ec.set_raw("wallet_enabled", serde_json::json!("yes"));
        let err = ec.get_bool("wallet_enabled").await.unwrap_err();
        assert!(matches!(err, EdgeConfigError::TypeMismatch { .. }));
    }

    #[tokio::test]
    async fn unreachable_stub_always_errors() {
        let ec = UnreachableEdgeConfig;
        assert!(ec.get_bool("wallet_enabled").await.is_err());
    }
}

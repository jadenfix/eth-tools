//! Best-effort cache invalidation against Upstash Redis (HTTP REST).
//!
//! W6 (wallet_rotation_watcher) uses this to evict
//! `agent_wallet:{chain}:{agent_id}` keys whenever a Transfer or
//! MetadataSet event lands. The invalidation is **best-effort**: a
//! failed DELETE must NOT fail the worker, because the chain-level
//! invariant (the DB row is already updated/cleared inside the same
//! transaction the event triggered) is what's authoritative. The cache
//! exists only as a read-side latency optimisation.
//!
//! ## Production vs no-op
//!
//! `UpstashKv::from_env()` returns:
//!   - `UpstashKv::Real { … }` if BOTH `UPSTASH_REDIS_REST_URL` and
//!     `UPSTASH_REDIS_REST_TOKEN` are set,
//!   - `UpstashKv::NoOp` otherwise (logs a warning the first time it's
//!     called per process; subsequent calls are silent).
//!
//! ## Test seam
//!
//! Tests inject a [`CountingKv`] that records every `del()` call so the
//! W6 test can assert "the cache invalidation was attempted with the
//! right key" without needing a live Redis.

use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum KvError {
    #[error("upstash transport error: {0}")]
    Transport(String),
    #[error("upstash returned non-success status: {0}")]
    Status(u16),
}

/// Pluggable cache invalidator. The W6 worker calls `del()` once per
/// affected agent; production wires the Upstash impl, tests wire a
/// counter.
#[async_trait]
pub trait KvInvalidator: Send + Sync {
    /// DELETE `key`. Returning `Err` does NOT abort the worker — the
    /// caller logs + swallows. We propagate the error so tests can
    /// assert on it, and so the production impl can hand back useful
    /// context for the log line.
    async fn del(&self, key: &str) -> Result<(), KvError>;
}

/// Canonical cache key shape per plan §3 W6:
/// `agent_wallet:{chain_id}:{agent_id_decimal}`.
pub fn agent_wallet_key(chain_id: i64, agent_id_decimal: &str) -> String {
    format!("agent_wallet:{chain_id}:{agent_id_decimal}")
}

/// Production Upstash Redis client over HTTP REST.
///
/// Upstash's REST API takes a single `DEL` command at
/// `POST {url}/del/{key}` with `Authorization: Bearer {token}`. We use
/// a short timeout (2s) because cache invalidation is on the critical
/// path of a chain-event hot loop — we'd rather skip the invalidation
/// than block the worker on a slow Redis.
pub struct UpstashKv {
    client: reqwest::Client,
    url: String,
    token: String,
}

impl UpstashKv {
    /// Build from env. Returns `None` if either env var is missing —
    /// callers wrap into [`KvNoOp`] in that case so production deploys
    /// without Upstash configured don't crash.
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("UPSTASH_REDIS_REST_URL").ok()?;
        let token = std::env::var("UPSTASH_REDIS_REST_TOKEN").ok()?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .ok()?;
        Some(Self { client, url, token })
    }
}

#[async_trait]
impl KvInvalidator for UpstashKv {
    async fn del(&self, key: &str) -> Result<(), KvError> {
        // Upstash REST: POST {base}/del/{key} returns {"result": <count>}.
        // URL-encoding the key would mangle the colons we use as
        // separators; Upstash accepts raw colons in the path.
        let url = format!("{}/del/{}", self.url.trim_end_matches('/'), key);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| KvError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(KvError::Status(resp.status().as_u16()));
        }
        Ok(())
    }
}

/// No-op invalidator. Returns `Ok(())` for every `del()` call. Used in
/// production when Upstash env vars are absent (and in tests that don't
/// care about cache semantics).
pub struct KvNoOp;

#[async_trait]
impl KvInvalidator for KvNoOp {
    async fn del(&self, _key: &str) -> Result<(), KvError> {
        Ok(())
    }
}

/// Build the "right thing" for the current environment: real Upstash if
/// configured, no-op otherwise. The first warn() log on cold start
/// records the choice so operators can audit.
pub fn from_env_or_noop() -> Arc<dyn KvInvalidator> {
    match UpstashKv::from_env() {
        Some(real) => {
            tracing::info!("kv: using Upstash Redis REST invalidator");
            Arc::new(real)
        }
        None => {
            tracing::warn!(
                "kv: UPSTASH_REDIS_REST_URL or _TOKEN unset — using no-op invalidator"
            );
            Arc::new(KvNoOp)
        }
    }
}

pub use test_helpers::CountingKv;

/// Helpers that integration tests in `crates/workers/tests/` need access
/// to. Kept `#[doc(hidden)]` so the public rustdoc surface stays small;
/// `pub` so binary-target integration tests can name the type.
#[doc(hidden)]
pub mod test_helpers {
    use super::*;
    use tokio::sync::Mutex;

    /// Test KV that records every `del()` call into an in-memory `Vec`.
    /// Tests inspect `keys()` to assert the worker invalidated what we
    /// expected. Not part of the public API — only `pub` so the
    /// `crates/workers/tests/` binaries can reach it.
    #[derive(Default)]
    pub struct CountingKv {
        seen: Mutex<Vec<String>>,
    }

    impl CountingKv {
        pub fn new() -> Self {
            Self::default()
        }
        pub async fn keys(&self) -> Vec<String> {
            self.seen.lock().await.clone()
        }
    }

    #[async_trait]
    impl KvInvalidator for CountingKv {
        async fn del(&self, key: &str) -> Result<(), KvError> {
            self.seen.lock().await.push(key.to_owned());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_shape_matches_plan() {
        assert_eq!(agent_wallet_key(8453, "42"), "agent_wallet:8453:42");
        // Large agent ids (uint256 in decimal) are passed through unchanged.
        let huge = "1000000000000000000000000000000000000000";
        let k = agent_wallet_key(1, huge);
        assert!(k.ends_with(huge));
        assert!(k.starts_with("agent_wallet:1:"));
    }

    #[tokio::test]
    async fn noop_always_succeeds() {
        let kv = KvNoOp;
        assert!(kv.del("anything").await.is_ok());
    }

    #[tokio::test]
    async fn counting_kv_records_calls() {
        let kv = CountingKv::new();
        kv.del("a").await.unwrap();
        kv.del("b").await.unwrap();
        let seen = kv.keys().await;
        assert_eq!(seen, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn from_env_returns_none_when_unset() {
        // Snapshot + restore. Other tests in this binary mutate env.
        let prev_url = std::env::var("UPSTASH_REDIS_REST_URL").ok();
        let prev_tok = std::env::var("UPSTASH_REDIS_REST_TOKEN").ok();
        std::env::remove_var("UPSTASH_REDIS_REST_URL");
        std::env::remove_var("UPSTASH_REDIS_REST_TOKEN");
        assert!(UpstashKv::from_env().is_none());
        if let Some(v) = prev_url {
            std::env::set_var("UPSTASH_REDIS_REST_URL", v);
        }
        if let Some(v) = prev_tok {
            std::env::set_var("UPSTASH_REDIS_REST_TOKEN", v);
        }
    }
}

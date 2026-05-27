//! Rotating Ethereum RPC provider with per-provider circuit breakers.
//!
//! Implementation of plan §14.1:
//!   - **Primary:** Alchemy Base — 300M compute units / mo free.
//!   - **Fallback:** QuickNode.
//!   - **Last resort:** `https://mainnet.base.org` public RPC.
//!   - Trip a per-provider circuit after **5 consecutive 429/5xx errors in 30s**,
//!     open for 30s, then **half-open** to probe with a single request; close on
//!     success. While open, the rotator skips that provider entirely.
//!   - `eth_getLogs` paginated at **2 000 blocks per call** (Alchemy hard limit).
//!   - Reorg safety: workers process up to `head - 12` blocks; refetch the tail
//!     on the next tick.
//!
//! ## Why our own `RpcProvider` trait instead of `Arc<dyn alloy::Provider>`?
//!
//! alloy 1.x's `Provider` trait is generic over a `Network` associated type
//! and carries a fat surface (signing, subscriptions, fillers, …). For PR #1
//! we only need two methods (`get_block_number`, `get_logs`) and ironclad
//! testability without an HTTP layer. Defining our own thin trait gives us:
//!   1. **Pure-Rust mocking** — no wiremock plumbing, the circuit-breaker test
//!      drives a `Mutex<VecDeque<Result>>` directly.
//!   2. **Stable trait-object shape** — `Arc<dyn RpcProvider + Send + Sync>`
//!      is one bound, no Network generic to thread.
//!   3. **A clean seam for future providers** (a Substreams-backed indexer
//!      could implement this trait without pretending to be a full alloy
//!      provider).
//!
//! The real alloy-backed implementation lives in PR #2 once `crates/workers`
//! actually needs to talk to Base. This PR ships the abstraction + breaker +
//! pagination logic that the 8 workers will share.

use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy::rpc::types::{Filter, Log};
use alloy_primitives::{Address, U256};
use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::Mutex;

/// Current monotonic instant — wired through `tokio::time::Instant::now()` so
/// `#[tokio::test(start_paused = true)]` can drive the breaker forward with
/// `tokio::time::advance(...)` instead of real wall-clock sleeps.
/// In production this is exactly `std::time::Instant::now()`.
#[inline]
pub(crate) fn mono_now() -> Instant {
    tokio::time::Instant::now().into_std()
}

pub mod health;

pub use health::ProviderHealth;

// ---------------------------------------------------------------------------
// Tunables (plan §14.1)
// ---------------------------------------------------------------------------

/// Alchemy's `eth_getLogs` hard limit is 2_000 blocks per call. Other providers
/// are more permissive but we pin to the strictest so we never have to
/// special-case.
pub const MAX_LOGS_PER_CALL: u64 = 2_000;

/// Default `confirmations` argument to [`confirmed_head`]. Process blocks up
/// to `head - REORG_SAFETY_BLOCKS`; refetch the tail on the next tick. Twelve
/// blocks (~24 s on Base) is conservative and matches Coinbase's own
/// finality recommendation for Base mainnet.
pub const REORG_SAFETY_BLOCKS: u64 = 12;

/// Trip the circuit after this many consecutive transient errors in the window.
pub const BREAKER_THRESHOLD: u32 = 5;

/// Sliding window in which `BREAKER_THRESHOLD` failures trip the breaker.
pub const BREAKER_WINDOW: Duration = Duration::from_secs(30);

/// How long the breaker stays open before allowing a half-open probe.
pub const BREAKER_COOLDOWN: Duration = Duration::from_secs(30);

/// Rolling window used by `health()`.
pub const HEALTH_WINDOW: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum RpcError {
    /// Transient — caller should fall back to the next provider. Includes
    /// 429 rate-limit, 5xx, connection reset, timeout.
    #[error("transient rpc error: {0}")]
    Transient(String),

    /// Permanent — invalid request, decode failure, etc. Caller should NOT
    /// fall back; the next provider would return the same error.
    #[error("permanent rpc error: {0}")]
    Permanent(String),

    /// Every configured provider's circuit is open. Caller should back off.
    #[error("all providers exhausted (last error: {last})")]
    AllProvidersOpen { last: String },
}

impl RpcError {
    pub fn is_transient(&self) -> bool {
        matches!(self, RpcError::Transient(_) | RpcError::AllProvidersOpen { .. })
    }
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Minimal RPC surface needed by the 8 workers in PR #1. Trait objects of
/// this type are what `RotatingProvider` rotates over.
#[async_trait]
pub trait RpcProvider: Send + Sync {
    /// A human-readable name used in logs + health reports
    /// (e.g. "alchemy", "quicknode", "public-base").
    fn name(&self) -> &str;

    /// `eth_blockNumber`.
    async fn get_block_number(&self) -> Result<u64, RpcError>;

    /// `eth_getLogs` for the **exact** filter the caller supplied. Pagination
    /// is the rotator's job (see `RotatingProvider::get_logs_paginated`).
    async fn get_logs(&self, filter: &Filter) -> Result<Vec<Log>, RpcError>;

    /// `eth_getBalance` against the latest block. Returns balance in wei as
    /// a `U256`. Used by W8 (wallet_balance_keeper) to monitor the operator
    /// EOA. Default impl returns `RpcError::Transient` so legacy providers
    /// that haven't been upgraded surface as a rotator failover instead of
    /// silently returning zero (which would mask a missing-balance bug).
    async fn get_balance(&self, _addr: Address) -> Result<U256, RpcError> {
        Err(RpcError::Transient(
            "get_balance not implemented on this provider".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Breaker state
// ---------------------------------------------------------------------------

/// Per-provider circuit-breaker state. Kept inside a `Mutex<Vec<_>>` indexed
/// by provider position in the rotator's `providers` Vec.
#[derive(Debug, Clone, Default)]
pub struct BreakerState {
    /// Count of transient failures inside the current `BREAKER_WINDOW`.
    pub failures_in_window: u32,
    /// Timestamp of the first failure in the current window. Used to reset
    /// the counter once the window expires without tripping.
    pub window_started_at: Option<Instant>,
    /// `Some(t)` if the breaker is open (rejecting traffic); cleared on close.
    pub opened_at: Option<Instant>,
    /// True while a half-open probe is in flight. Prevents thundering-herd
    /// probes across concurrent callers.
    pub half_open_probe_inflight: bool,
    /// Outcomes seen in the last `HEALTH_WINDOW` — `(timestamp, success)`.
    /// Bounded to ~64 entries per provider; old entries pruned on insert.
    pub recent: std::collections::VecDeque<(Instant, bool)>,
}

impl BreakerState {
    /// Returns true if the breaker has tripped — the cooldown timer is
    /// orthogonal. A "closed" breaker is one where `opened_at == None`
    /// (which is the post-success state); an open breaker either rejects
    /// traffic outright (cooldown not yet elapsed) or admits a single
    /// half-open probe (cooldown elapsed, no probe in flight).
    pub fn is_open(&self, _now: Instant) -> bool {
        self.opened_at.is_some()
    }

    /// True when the breaker has been open at least `BREAKER_COOLDOWN` and no
    /// probe is currently in flight. Caller flips `half_open_probe_inflight`
    /// before issuing the probe.
    pub fn should_attempt_probe(&self, now: Instant) -> bool {
        match self.opened_at {
            Some(t) => now.duration_since(t) >= BREAKER_COOLDOWN && !self.half_open_probe_inflight,
            None => false,
        }
    }

    fn record_success(&mut self, now: Instant) {
        self.failures_in_window = 0;
        self.window_started_at = None;
        self.opened_at = None;
        self.half_open_probe_inflight = false;
        self.push_outcome(now, true);
    }

    fn record_transient_failure(&mut self, now: Instant) {
        // Reset the counter if the window has expired.
        if let Some(start) = self.window_started_at {
            if now.duration_since(start) > BREAKER_WINDOW {
                self.failures_in_window = 0;
                self.window_started_at = None;
            }
        }
        if self.window_started_at.is_none() {
            self.window_started_at = Some(now);
        }
        self.failures_in_window = self.failures_in_window.saturating_add(1);
        self.half_open_probe_inflight = false;

        if self.failures_in_window >= BREAKER_THRESHOLD {
            self.opened_at = Some(now);
        }
        self.push_outcome(now, false);
    }

    fn push_outcome(&mut self, now: Instant, ok: bool) {
        // Prune outcomes older than HEALTH_WINDOW before inserting.
        while let Some((t, _)) = self.recent.front() {
            if now.duration_since(*t) > HEALTH_WINDOW {
                self.recent.pop_front();
            } else {
                break;
            }
        }
        self.recent.push_back((now, ok));
        // Hard cap so a flapping provider can't grow memory unbounded.
        while self.recent.len() > 64 {
            self.recent.pop_front();
        }
    }

    pub(crate) fn success_rate(&self, now: Instant) -> f32 {
        let mut total = 0u32;
        let mut ok = 0u32;
        for (t, success) in &self.recent {
            if now.duration_since(*t) > HEALTH_WINDOW {
                continue;
            }
            total += 1;
            if *success {
                ok += 1;
            }
        }
        if total == 0 {
            1.0
        } else {
            ok as f32 / total as f32
        }
    }
}

// ---------------------------------------------------------------------------
// RotatingProvider
// ---------------------------------------------------------------------------

/// Rotates over an ordered list of providers with per-provider circuit
/// breakers. The first non-open provider gets traffic; on transient errors
/// the rotator falls through to the next one and increments the breaker.
pub struct RotatingProvider {
    pub(crate) providers: Vec<Arc<dyn RpcProvider>>,
    pub(crate) breaker_state: Mutex<Vec<BreakerState>>,
}

impl RotatingProvider {
    pub fn new(providers: Vec<Arc<dyn RpcProvider>>) -> Self {
        assert!(!providers.is_empty(), "RotatingProvider needs >=1 provider");
        let n = providers.len();
        Self {
            providers,
            breaker_state: Mutex::new(vec![BreakerState::default(); n]),
        }
    }

    /// Run `op` against the first non-open provider. On `RpcError::Transient`
    /// the breaker for that provider is incremented and the rotator falls
    /// through to the next one. Permanent errors short-circuit immediately
    /// (the next provider would return the same error).
    ///
    /// The closure is invoked with an `Arc<dyn RpcProvider>` so it can clone
    /// the provider into its own async block as needed.
    pub async fn call<F, Fut, T>(&self, op: F) -> Result<T, RpcError>
    where
        F: Fn(Arc<dyn RpcProvider>) -> Fut,
        Fut: std::future::Future<Output = Result<T, RpcError>>,
    {
        let now = mono_now();
        let mut last_error: Option<String> = None;

        // Snapshot which providers are eligible right now (closed or half-open).
        // We do NOT hold the lock across the await — open->half-open transitions
        // are intentionally racy on the cold path (worst case: two half-open
        // probes, both succeed, both close — harmless).
        //
        // `half_open_probe_inflight` is set *inside* the iteration loop just
        // before each half-open call, then cleared by record_success /
        // record_transient_failure. If an earlier closed-state provider
        // succeeds first we never set the inflight bit at all — so a
        // half-open provider can't get stuck "in flight" by an unrelated
        // success on another provider.
        let eligible: Vec<usize> = {
            let guard = self.breaker_state.lock().await;
            (0..self.providers.len())
                .filter(|&i| {
                    let st = &guard[i];
                    if !st.is_open(now) {
                        return true;
                    }
                    st.should_attempt_probe(now)
                })
                .collect()
        };

        if eligible.is_empty() {
            return Err(RpcError::AllProvidersOpen {
                last: "all circuits open".into(),
            });
        }

        for idx in eligible {
            // If this provider is in the half-open state, mark probe inflight
            // before issuing the request so a concurrent caller skips us.
            {
                let mut guard = self.breaker_state.lock().await;
                if guard[idx].is_open(mono_now()) {
                    guard[idx].half_open_probe_inflight = true;
                }
            }
            let provider = Arc::clone(&self.providers[idx]);
            let name = provider.name().to_string();
            let result = op(provider).await;
            let now_after = mono_now();
            match result {
                Ok(val) => {
                    let mut guard = self.breaker_state.lock().await;
                    guard[idx].record_success(now_after);
                    return Ok(val);
                }
                Err(RpcError::Transient(msg)) => {
                    tracing::warn!(
                        provider = %name,
                        error = %msg,
                        "transient rpc error; rotating to next provider"
                    );
                    let mut guard = self.breaker_state.lock().await;
                    guard[idx].record_transient_failure(now_after);
                    last_error = Some(format!("{name}: {msg}"));
                    continue;
                }
                Err(RpcError::Permanent(msg)) => {
                    // Don't increment the breaker — permanent errors aren't
                    // the provider's fault, and would repeat on the next one.
                    // Clear any inflight bit we set so the next caller can
                    // still probe.
                    let mut guard = self.breaker_state.lock().await;
                    guard[idx].half_open_probe_inflight = false;
                    return Err(RpcError::Permanent(msg));
                }
                Err(RpcError::AllProvidersOpen { last }) => {
                    // Shouldn't happen from a leaf op, but propagate.
                    return Err(RpcError::AllProvidersOpen { last });
                }
            }
        }

        Err(RpcError::AllProvidersOpen {
            last: last_error.unwrap_or_else(|| "no eligible providers".into()),
        })
    }

    /// Fetch the current `eth_blockNumber` with rotator failover.
    pub async fn get_block_number(&self) -> Result<u64, RpcError> {
        self.call(|p| async move { p.get_block_number().await }).await
    }

    /// Fetch the wei balance of `addr` at the latest block with rotator
    /// failover. Used by W8 to check the operator wallet's funding level.
    pub async fn get_balance(&self, addr: Address) -> Result<U256, RpcError> {
        self.call(move |p| async move { p.get_balance(addr).await }).await
    }

    /// Fetch logs matching `filter` across `[start_block, end_block]` in
    /// pages of `page_size` (max `MAX_LOGS_PER_CALL`). If a provider fails
    /// mid-pagination, the rotator's per-page call falls over to the next
    /// provider; the caller sees a continuous `Vec<Log>` in block order.
    ///
    /// Pages are **inclusive on both ends**; a 5000-block range
    /// `[0, 4999]` with `page_size=2000` yields three pages
    /// `[0, 1999] / [2000, 3999] / [4000, 4999]` — the final partial page is
    /// honored.
    pub async fn get_logs_paginated(
        &self,
        filter: Filter,
        start_block: u64,
        end_block: u64,
        page_size: u64,
    ) -> Result<Vec<Log>, RpcError> {
        if start_block > end_block {
            return Ok(Vec::new());
        }
        let page_size = page_size.clamp(1, MAX_LOGS_PER_CALL);

        let mut out = Vec::new();
        let mut cursor = start_block;
        while cursor <= end_block {
            // Inclusive page end. e.g. start=0, page_size=2000 -> [0, 1999].
            let page_end = cursor.saturating_add(page_size - 1).min(end_block);

            let page_filter = filter.clone().from_block(cursor).to_block(page_end);

            let logs = self
                .call(|p| {
                    let f = page_filter.clone();
                    async move { p.get_logs(&f).await }
                })
                .await?;
            out.extend(logs);

            // Saturating: cursor at u64::MAX + 1 would wrap; bail out.
            cursor = match page_end.checked_add(1) {
                Some(c) => c,
                None => break,
            };
        }
        Ok(out)
    }
}

/// Reorg-safety helper. Workers should treat `head - confirmations` as the
/// "safe to commit" tip and re-scan the tail on the next tick. The project
/// default for `confirmations` is [`REORG_SAFETY_BLOCKS`] (12, ~24s on
/// Base); pass an explicit value for chains with different reorg
/// characteristics.
///
/// Saturating subtraction so early-chain heads (test forks, anvil from block 0)
/// don't panic.
pub fn confirmed_head(head: u64, confirmations: u64) -> u64 {
    head.saturating_sub(confirmations)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmed_head_subtracts_confirmations() {
        assert_eq!(confirmed_head(1_000_000, 12), 999_988);
    }

    #[test]
    fn confirmed_head_saturates_at_zero() {
        assert_eq!(confirmed_head(5, 12), 0);
    }

    #[test]
    fn confirmed_head_with_project_default() {
        // The "canonical" worker call site: confirmed_head(head, REORG_SAFETY_BLOCKS).
        assert_eq!(confirmed_head(1_000_000, REORG_SAFETY_BLOCKS), 999_988);
    }

    #[test]
    fn breaker_records_success_resets_counter() {
        let mut bs = BreakerState::default();
        let now = mono_now();
        for _ in 0..4 {
            bs.record_transient_failure(now);
        }
        assert_eq!(bs.failures_in_window, 4);
        assert!(!bs.is_open(now));
        bs.record_success(now);
        assert_eq!(bs.failures_in_window, 0);
        assert!(!bs.is_open(now));
    }

    #[test]
    fn breaker_trips_after_threshold() {
        let mut bs = BreakerState::default();
        let now = mono_now();
        for _ in 0..BREAKER_THRESHOLD {
            bs.record_transient_failure(now);
        }
        assert!(bs.is_open(now));
    }

    #[test]
    fn breaker_window_resets_failures() {
        let mut bs = BreakerState::default();
        let t0 = Instant::now();
        bs.record_transient_failure(t0);
        bs.record_transient_failure(t0);
        assert_eq!(bs.failures_in_window, 2);
        // Beyond BREAKER_WINDOW -> next failure starts a fresh window.
        let t1 = t0 + BREAKER_WINDOW + Duration::from_secs(1);
        bs.record_transient_failure(t1);
        assert_eq!(bs.failures_in_window, 1);
        assert!(!bs.is_open(t1));
    }
}

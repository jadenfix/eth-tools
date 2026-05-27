//! Health snapshot for the rotating provider pool.
//!
//! Exposed via `RotatingProvider::health()` and consumed (in a later PR) by
//! `/api/v1/health` so dashboards can show per-provider status. The endpoint
//! wiring is deliberately NOT in this PR — we only expose the data.

use serde::{Deserialize, Serialize};

use crate::RotatingProvider;

/// Per-provider health snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderHealth {
    /// Provider name as reported by `RpcProvider::name()`.
    pub name: String,
    /// True if the circuit breaker is currently open (rejecting traffic).
    pub opened: bool,
    /// Success rate over the last `HEALTH_WINDOW` (60 s) — `0.0..=1.0`.
    /// `1.0` when no outcomes have been recorded yet (default-optimistic).
    pub success_rate_60s: f32,
}

impl RotatingProvider {
    /// Snapshot the breaker state for every provider. Lock is held only
    /// long enough to clone the relevant fields.
    pub async fn health(&self) -> Vec<ProviderHealth> {
        let now = crate::mono_now();
        let guard = self.breaker_state.lock().await;
        self.providers
            .iter()
            .zip(guard.iter())
            .map(|(p, st)| ProviderHealth {
                name: p.name().to_string(),
                opened: st.is_open(now),
                success_rate_60s: st.success_rate(now),
            })
            .collect()
    }
}

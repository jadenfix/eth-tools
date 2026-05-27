//! Hot-wallet balance lookup (plan §10.2 rail #6).
//!
//! Returns the wallet's current balance denominated in USD cents (integer —
//! the rail compares against `MAX_BALANCE_USD_CENTS: u32`, never against a
//! float). The real implementation queries alloy's HTTP provider for the
//! native ETH balance, converts via a price oracle, and rounds to cents.
//!
//! Phase 6 ships only the trait + test stubs; the alloy-backed implementation
//! lives alongside the signing path in the follow-up PR (see
//! `wallet::sign::sign_and_send` TODO).

use async_trait::async_trait;
use std::sync::RwLock;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BalanceError {
    /// RPC or price-oracle transport error. Fail-closed in the rail.
    #[error("balance lookup transport: {0}")]
    Transport(String),
}

/// The single method the balance rail needs from a provider. Keeping this
/// trait narrow (one method) means a `MockProvider` is a couple of lines and
/// the rail can be unit-tested with no HTTP layer.
#[async_trait]
pub trait BalanceProvider: Send + Sync {
    /// Returns the hot wallet's balance in **USD cents** (integer). The rail
    /// compares this directly against `MAX_BALANCE_USD_CENTS`; the conversion
    /// from wei -> cents happens inside the provider so the rail never sees
    /// a U256 or a float.
    async fn balance_usd_cents(&self) -> Result<u64, BalanceError>;
}

#[derive(Debug, Default)]
pub struct InMemoryBalanceProvider {
    state: RwLock<InnerState>,
}

#[derive(Debug, Default)]
struct InnerState {
    value: u64,
    fail_with: Option<String>,
}

impl InMemoryBalanceProvider {
    pub fn new(initial_cents: u64) -> Self {
        Self {
            state: RwLock::new(InnerState {
                value: initial_cents,
                fail_with: None,
            }),
        }
    }

    pub fn set(&self, cents: u64) {
        self.state.write().expect("balance lock poisoned").value = cents;
    }

    pub fn fail_with(&self, msg: impl Into<String>) {
        self.state.write().expect("balance lock poisoned").fail_with = Some(msg.into());
    }
}

#[async_trait]
impl BalanceProvider for InMemoryBalanceProvider {
    async fn balance_usd_cents(&self) -> Result<u64, BalanceError> {
        let guard = self
            .state
            .read()
            .map_err(|e| BalanceError::Transport(format!("lock poisoned: {e}")))?;
        if let Some(msg) = &guard.fail_with {
            return Err(BalanceError::Transport(msg.clone()));
        }
        Ok(guard.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_initial() {
        let p = InMemoryBalanceProvider::new(250);
        assert_eq!(p.balance_usd_cents().await.unwrap(), 250);
    }

    #[tokio::test]
    async fn set_updates_value() {
        let p = InMemoryBalanceProvider::new(0);
        p.set(499);
        assert_eq!(p.balance_usd_cents().await.unwrap(), 499);
    }

    #[tokio::test]
    async fn fail_with_propagates() {
        let p = InMemoryBalanceProvider::new(0);
        p.fail_with("simulated");
        assert!(p.balance_usd_cents().await.is_err());
    }
}

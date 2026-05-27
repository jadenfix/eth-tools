//! x402 configuration loaded from env at AppState construction.
//!
//! Two env vars control everything:
//! - `X402_FACILITATOR_URL` — defaults to `https://x402.org/facilitator`
//!   (the open-source CDP-compatible facilitator; works for mainnet + Base
//!   Sepolia). Prod can swap to `https://api.cdp.coinbase.com/platform/v2/x402`
//!   without code changes.
//! - `X402_PAY_TO_ADDRESS` — **required in production**. If absent or empty,
//!   the gate becomes a no-op (dev mode) and we log a warning at boot.
//!
//! ## Why we read env at startup, not per-request
//!
//! Per plan §10.5: log the wallet posture exactly once at boot so an operator
//! grepping logs can answer "is x402 on right now?" without parsing every
//! request. Per-request reads also let a typo in a Vercel-dashboard rename
//! flip every gated endpoint dark mid-traffic.

use alloy_primitives::Address;
use std::str::FromStr;

/// The default x402 facilitator URL. CDP also hosts
/// `https://api.cdp.coinbase.com/platform/v2/x402` but that requires CDP
/// API keys; `x402.org/facilitator` is the open endpoint used by the
/// reference impl and most third-party sellers.
///
/// Source: <https://docs.cdp.coinbase.com/x402/quickstart-for-sellers>.
pub const DEFAULT_FACILITATOR_URL: &str = "https://x402.org/facilitator";

#[derive(Clone, Debug)]
pub struct X402Config {
    /// `https://…` URL of the facilitator. Always set (defaults to
    /// [`DEFAULT_FACILITATOR_URL`]).
    pub facilitator_url: String,
    /// The address the facilitator settles to. `None` disables x402 entirely
    /// — `gate_payment` becomes an identity no-op and the handler runs.
    pub pay_to_address: Option<Address>,
}

impl X402Config {
    /// Build from env. Reads `X402_FACILITATOR_URL` (with default fallback)
    /// and `X402_PAY_TO_ADDRESS` (optional; empty = disabled). Logs a single
    /// `WARN` at boot if x402 is disabled, so operators can spot accidental
    /// dev-mode in prod logs.
    pub fn from_env_or_disabled() -> Self {
        let facilitator_url = std::env::var("X402_FACILITATOR_URL")
            .unwrap_or_else(|_| DEFAULT_FACILITATOR_URL.to_string());
        let pay_to_address = match std::env::var("X402_PAY_TO_ADDRESS").ok().as_deref() {
            None | Some("") => {
                tracing::warn!(
                    facilitator_url = %facilitator_url,
                    "x402 disabled (X402_PAY_TO_ADDRESS unset) — gated endpoints will accept all requests; \
                     set X402_PAY_TO_ADDRESS in production"
                );
                None
            }
            Some(raw) => match Address::from_str(raw) {
                Ok(addr) => {
                    tracing::info!(
                        facilitator_url = %facilitator_url,
                        pay_to = %format!("{:#x}", addr),
                        "x402 enabled"
                    );
                    Some(addr)
                }
                Err(e) => {
                    // Don't `panic!` at boot — Vercel would cold-start-loop.
                    // Refuse to enable x402 and log loudly; the operator will
                    // notice immediately when invocations stop being gated.
                    tracing::error!(
                        error = %e,
                        raw = %raw,
                        "X402_PAY_TO_ADDRESS is not a valid hex address — x402 will stay DISABLED"
                    );
                    None
                }
            },
        };
        Self {
            facilitator_url,
            pay_to_address,
        }
    }

    /// Explicit test constructor. Don't use from production code — only
    /// `from_env_or_disabled` should observe environment state.
    pub fn for_test(facilitator_url: impl Into<String>, pay_to: Address) -> Self {
        Self {
            facilitator_url: facilitator_url.into(),
            pay_to_address: Some(pay_to),
        }
    }

    /// Test helper for the dev-mode (disabled) path.
    pub fn disabled() -> Self {
        Self {
            facilitator_url: DEFAULT_FACILITATOR_URL.to_string(),
            pay_to_address: None,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.pay_to_address.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_has_no_pay_to() {
        let c = X402Config::disabled();
        assert!(!c.is_enabled());
        assert_eq!(c.facilitator_url, DEFAULT_FACILITATOR_URL);
    }

    #[test]
    fn for_test_is_enabled() {
        let addr =
            Address::from_str("0x209693Bc6afc0C5328bA36FaF03C514EF312287C").unwrap();
        let c = X402Config::for_test("http://mock.test", addr);
        assert!(c.is_enabled());
        assert_eq!(c.pay_to_address, Some(addr));
    }
}

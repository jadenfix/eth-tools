//! Wallet hard rails (plan §10).
//!
//! The five rails in [`policy`] bound the worst-case loss from a leaked
//! `EVM_PRIVATE_KEY` to under $5: only Base mainnet, only the three ERC-8004
//! registries, ≤ $5 balance, ≤ $1/day spend, ≤ 500k gas/tx, with an
//! Edge-Config kill switch in front of everything.
//!
//! ## Layering
//!
//! ```text
//!   sign::sign_and_send                — top-level entrypoint (signing stubbed)
//!     └─> policy::evaluate_all         — runs the five rails in fixed order
//!         ├─> kill_switch              — Edge Config bool
//!         ├─> allowed_chain            — chain_id == 8453
//!         ├─> allowed_recipient        — to ∈ {Identity, Reputation, Validation}
//!         ├─> gas_ceiling              — gas_limit ≤ 500k
//!         ├─> balance_ceiling          — wallet balance ≤ $5  (alloy provider)
//!         └─> daily_cap                — Upstash INCRBY ≤ $1   (rollback on later failure)
//! ```
//!
//! Each rail is a pure async function whose only side effect is the daily-cap
//! counter (and the rollback path when a downstream rail rejects after the
//! counter was bumped). Denials are written by the caller (`sign_and_send`)
//! via [`crate::denials::record_denial`] so the per-rail functions stay
//! testable without a Postgres pool.

use alloy_primitives::{address, Address};

pub mod balance;
pub mod daily_cap;
pub mod gas;
pub mod nonce;
pub mod policy;
pub mod rpc;
pub mod sign;
pub mod signer;
pub mod txs;

pub use policy::{
    evaluate_all, reconcile_actual_cost, Approved, PolicyContext, Rail, TransactionRequest,
    ESTIMATED_TX_COST_USD_CENTS,
};
pub use sign::sign_and_send;

/// Base mainnet only at MVP. Plan §10.5 control #3.
pub const ALLOWED_CHAIN_ID: u64 = 8453;

/// The three ERC-8004 registries are the only addresses we will sign
/// transactions to. Plan §10.5 control #2. Hard-coded in Rust (NOT config)
/// so a single compromised env var cannot widen the allowlist.
pub const ALLOWED_RECIPIENTS: &[Address] = &[
    address!("8004A169FB4a3325136EB29fA0ceB6D2e539a432"), // Identity
    address!("8004BAa17C55a88189AE136b182e5fdA19dE9b63"), // Reputation
    address!("8004Cc8439f36fd5F9F049D9fF86523Df6dAAB58"), // Validation
];

/// Legacy hex form of [`ALLOWED_RECIPIENTS`] — preserved so the existing
/// compile-time invariants in `lib.rs` keep working. Kept in lockstep with
/// `ALLOWED_RECIPIENTS` by `tests::allowed_recipients_match_hex_constants`.
pub const ALLOWED_RECIPIENTS_HEX: &[&str] = &[
    "8004A169FB4a3325136EB29fA0ceB6D2e539a432",
    "8004BAa17C55a88189AE136b182e5fdA19dE9b63",
    "8004Cc8439f36fd5F9F049D9fF86523Df6dAAB58",
];

/// $5 hard ceiling on the hot-wallet balance. W8 sweeps above this. §10.5 #4.
pub const MAX_BALANCE_USD_CENTS: u32 = 500;

/// $1/day spend cap, enforced by Upstash atomic INCRBY. §10.5 #5.
pub const MAX_DAILY_SPEND_USD_CENTS: u32 = 100;

/// Per-tx gas cap — refuse > 500k. Plan §10.2 rail #4.
pub const MAX_PER_TX_GAS: u64 = 500_000;

/// Vercel Edge Config key that gates every signed transaction. §10.5 #6.
pub const KILL_SWITCH_KEY: &str = "wallet_enabled";

/// Public env var name (never the private key). Logged at boot for sanity.
pub const PUBLIC_KEY_ENV: &str = "EVM_PUBLIC_KEY";

/// Private key env var. **NEVER LOG.** Sensitive in Vercel.
pub const PRIVATE_KEY_ENV: &str = "EVM_PRIVATE_KEY";

/// The Vercel env-var name we read to pick prod-vs-preview behaviour. Outside
/// production every wallet call returns `WALLET_DISABLED_NON_PROD` immediately
/// (plan §10 — wallet is prod-only at MVP).
pub const VERCEL_ENV_VAR: &str = "VERCEL_ENV";

/// Value of `VERCEL_ENV` that unlocks the wallet path.
pub const VERCEL_PRODUCTION: &str = "production";

/// Upstash key shape per plan §14.3 (rate-limit namespacing). The first two
/// segments are `rl:{vercel_env}` so prod + preview share one Redis without
/// cross-pollination.
pub fn daily_spend_key(vercel_env: &str) -> String {
    format!("rl:{vercel_env}:wallet:spend:today")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_spend_key_namespaces_by_env() {
        assert_eq!(daily_spend_key("production"), "rl:production:wallet:spend:today");
        assert_eq!(daily_spend_key("preview"), "rl:preview:wallet:spend:today");
        // Distinct envs MUST produce distinct keys.
        assert_ne!(daily_spend_key("production"), daily_spend_key("preview"));
    }
}

//! The five wallet hard rails — pure functions, fully testable in isolation.
//!
//! Per plan §10.2 the rails are (in order of evaluation):
//!  1. Kill switch (Vercel Edge Config bool)
//!  2. Allowed chain (Base 8453)
//!  3. Allowed recipient (∈ {Identity, Reputation, Validation})
//!  4. Gas ceiling (≤ 500k)
//!  5. Balance ceiling (≤ $5)
//!  6. Daily spend cap (≤ $1, Upstash INCRBY — incremented LAST so the
//!     rollback path is bounded to "only this rail can leak counter state")
//!
//! Plus the env-aware short-circuit: any non-production `VERCEL_ENV` returns
//! `WALLET_DISABLED_NON_PROD` before any rail runs (plan §10 — wallet is
//! prod-only at MVP).
//!
//! ## Rail order is load-bearing
//!
//! The cheapest rails (chain, gas, recipient — pure compares) run first.
//! Network-hitting rails (kill switch, balance, daily cap) run after, so a
//! malformed tx never burns an RPC call.
//!
//! The daily-cap rail runs LAST, and is the only rail with side effects.
//! That ordering means an early-rail rejection (e.g. wrong chain) cannot
//! leave a stale counter increment behind, so the rollback contract is:
//! *"increment, then if THIS rail rejects (cap exceeded), decrement
//! immediately."* If a future rail or the signer fails AFTER the increment
//! lands, the caller (`sign_and_send`) must invoke
//! [`PolicyContext::rollback_daily_cap`].
//!
//! ## `result_large_err` carve-out
//!
//! `DeniedReason` is ~160 bytes (a `String` code + evaluator + policy_version
//! plus two `Option<serde_json::Value>` and a hint). Clippy's
//! `result_large_err` lint asks us to `Box<DeniedReason>` it — but
//! `DeniedReason` is the **canonical error envelope** for the whole platform
//! per plan section 9.1, returned identically from `crates/api`, `crates/mcp`,
//! the rate-limit layer, and these rails. Boxing it here would diverge the
//! rails' signature from every other denial site in the codebase and force
//! callers to wrap/unwrap. The signed-tx path is also a once-per-request
//! (slow) operation, so the 160-byte stack hit is invisible. We opt out at
//! the module level with a comment so reviewers see WHY.

#![allow(clippy::result_large_err)]

use crate::edge_config::{EdgeConfig, EdgeConfigError};
use crate::wallet::balance::{BalanceError, BalanceProvider};
use crate::wallet::daily_cap::{SpendCounter, SpendCounterError};
use crate::wallet::{
    daily_spend_key, ALLOWED_CHAIN_ID, ALLOWED_RECIPIENTS, KILL_SWITCH_KEY, MAX_BALANCE_USD_CENTS,
    MAX_DAILY_SPEND_USD_CENTS, MAX_PER_TX_GAS, VERCEL_PRODUCTION,
};
use alloy_primitives::{Address, U256};
use eth_tools_core::DeniedReason;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Estimated per-tx cost in USD cents — plan §10.2 uses
/// `estimate_cost_cents(&tx).await?`. Phase 6 ships a flat estimate so the
/// rails compile + test end-to-end; the dynamic estimator lands with the
/// alloy-backed signing path. **Integer**, never a float.
pub const ESTIMATED_TX_COST_USD_CENTS: i64 = 5;

/// TTL on the daily-spend counter (24h). Matches the plan §10.2 EXPIRE call.
pub const DAILY_CAP_TTL_SECONDS: i64 = 86_400;

/// Minimal transaction shape the rails inspect. Mirrors the fields the plan
/// pulls off of alloy's `TransactionRequest` (chain_id / to / gas_limit /
/// value) without forcing the rails to depend on the full alloy network
/// types. The signer path constructs an `alloy::rpc::types::TransactionRequest`
/// from this struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionRequest {
    /// EVM chain id. `None` is treated as "unspecified" — rejected by the
    /// chain rail because we refuse to default a chain for the user.
    pub chain_id: Option<u64>,
    /// Recipient. `None` is rejected (contract-creation txs are never on
    /// the allowlist).
    pub to: Option<Address>,
    /// Per-tx gas ceiling. `None` means "let the RPC estimate" — but we
    /// require an explicit number so the gas rail has a definite value to
    /// check. `None` is rejected.
    pub gas_limit: Option<u64>,
    /// Native-token value (wei). Optional; defaults to zero. Currently
    /// unused by the rails (the recipient allowlist is the binding
    /// constraint at MVP) but tracked so the signer has it.
    pub value: Option<U256>,
}

impl TransactionRequest {
    pub fn new(chain_id: u64, to: Address, gas_limit: u64) -> Self {
        Self {
            chain_id: Some(chain_id),
            to: Some(to),
            gas_limit: Some(gas_limit),
            value: None,
        }
    }
}

/// The five rails, named for telemetry. Each enum variant is the
/// `evaluator` string on the `DeniedReason` envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rail {
    EnvGate,
    KillSwitch,
    AllowedChain,
    AllowedRecipient,
    GasCeiling,
    BalanceCeiling,
    DailyCap,
}

impl Rail {
    pub const fn evaluator(self) -> &'static str {
        match self {
            Rail::EnvGate => "wallet.env",
            Rail::KillSwitch => "wallet.kill",
            Rail::AllowedChain => "wallet.chain",
            Rail::AllowedRecipient => "wallet.allowlist",
            Rail::GasCeiling => "wallet.gas",
            Rail::BalanceCeiling => "wallet.balance",
            Rail::DailyCap => "wallet.cap",
        }
    }

    pub const fn code(self) -> &'static str {
        match self {
            Rail::EnvGate => "WALLET_DISABLED_NON_PROD",
            Rail::KillSwitch => "KILL_SWITCH_ACTIVE",
            Rail::AllowedChain => "WRONG_CHAIN",
            Rail::AllowedRecipient => "RECIPIENT_NOT_ALLOWED",
            Rail::GasCeiling => "GAS_TOO_HIGH",
            Rail::BalanceCeiling => "OVERFUNDED",
            Rail::DailyCap => "DAILY_CAP",
        }
    }
}

/// All the runtime context the rails need. Built once per request by
/// `sign_and_send` from the cold-start cache.
pub struct PolicyContext<'a> {
    pub vercel_env: &'a str,
    pub edge_config: &'a dyn EdgeConfig,
    pub spend_counter: &'a dyn SpendCounter,
    pub balance_provider: &'a dyn BalanceProvider,
    /// Per-tx cost in cents that will be charged against the daily cap.
    /// Caller can override the default (`ESTIMATED_TX_COST_USD_CENTS`) once
    /// a real gas estimator lands in the signing PR.
    pub cost_cents: i64,
}

impl<'a> PolicyContext<'a> {
    /// Builds a context with the default per-tx cost estimate.
    pub fn new(
        vercel_env: &'a str,
        edge_config: &'a dyn EdgeConfig,
        spend_counter: &'a dyn SpendCounter,
        balance_provider: &'a dyn BalanceProvider,
    ) -> Self {
        Self {
            vercel_env,
            edge_config,
            spend_counter,
            balance_provider,
            cost_cents: ESTIMATED_TX_COST_USD_CENTS,
        }
    }

    /// Roll back a previously-committed daily-cap increment. The signing
    /// path MUST call this if anything between the cap rail and an on-chain
    /// confirmation fails. Returns the post-decrement counter value (mostly
    /// for telemetry; callers ignore the value).
    pub async fn rollback_daily_cap(&self) -> Result<i64, SpendCounterError> {
        let key = daily_spend_key(self.vercel_env);
        self.spend_counter.decrby(&key, self.cost_cents).await
    }
}

/// Successful evaluation — the caller is cleared to sign + send.
#[derive(Debug, Clone)]
pub struct Approved {
    pub tx: TransactionRequest,
    /// Cents charged against the daily cap by this evaluation. Stored so the
    /// signing path can roll back the exact amount on failure (defends
    /// against a future PR changing `cost_cents` mid-flight).
    pub committed_cents: i64,
    /// Counter value after the increment — telemetry only.
    pub daily_spend_after_cents: i64,
}

// ---------------------------------------------------------------------------
// Per-rail pure functions
// ---------------------------------------------------------------------------

/// Rail 0 — env gate. Wallet is **prod-only** at MVP. Non-prod environments
/// short-circuit BEFORE any other rail runs (so a preview deploy can't even
/// hit Edge Config). Plan §10 + §11.5.
pub fn env_gate(vercel_env: &str) -> Result<(), DeniedReason> {
    if vercel_env == VERCEL_PRODUCTION {
        Ok(())
    } else {
        Err(DeniedReason::new(Rail::EnvGate.code(), Rail::EnvGate.evaluator())
            .with_got(json!({ "vercel_env": vercel_env }))
            .with_expected(json!({ "vercel_env": VERCEL_PRODUCTION }))
            .with_hint("wallet only operates in production at MVP"))
    }
}

/// Rail 1 — Edge Config kill switch. `None` (key missing) is treated as
/// "wallet disabled" — fail-closed default.
pub async fn kill_switch(edge: &dyn EdgeConfig) -> Result<(), DeniedReason> {
    let enabled = edge
        .get_bool(KILL_SWITCH_KEY)
        .await
        .map_err(edge_config_to_denied)?
        .unwrap_or(false);
    if enabled {
        Ok(())
    } else {
        Err(
            DeniedReason::new(Rail::KillSwitch.code(), Rail::KillSwitch.evaluator())
                .with_got(json!({ "wallet_enabled": false }))
                .with_expected(json!({ "wallet_enabled": true }))
                .with_hint("flip Vercel Edge Config `wallet_enabled` to true"),
        )
    }
}

fn edge_config_to_denied(e: EdgeConfigError) -> DeniedReason {
    DeniedReason::new(Rail::KillSwitch.code(), Rail::KillSwitch.evaluator())
        .with_got(json!({ "error": e.to_string() }))
        .with_hint("edge config unreachable — failing closed")
}

/// Rail 2 — chain allowlist. Hard constant; no config override.
pub fn allowed_chain(tx: &TransactionRequest) -> Result<(), DeniedReason> {
    match tx.chain_id {
        Some(id) if id == ALLOWED_CHAIN_ID => Ok(()),
        other => Err(
            DeniedReason::new(Rail::AllowedChain.code(), Rail::AllowedChain.evaluator())
                .with_got(json!({ "chain_id": other }))
                .with_expected(json!({ "chain_id": ALLOWED_CHAIN_ID })),
        ),
    }
}

/// Rail 3 — recipient allowlist. The `to` field MUST be one of the three
/// ERC-8004 registry addresses hard-coded in [`crate::wallet::ALLOWED_RECIPIENTS`].
pub fn allowed_recipient(tx: &TransactionRequest) -> Result<(), DeniedReason> {
    match tx.to {
        None => Err(
            DeniedReason::new(Rail::AllowedRecipient.code(), Rail::AllowedRecipient.evaluator())
                .with_got(json!({ "to": null }))
                .with_hint("contract-creation txs are never on the allowlist"),
        ),
        Some(addr) if ALLOWED_RECIPIENTS.contains(&addr) => Ok(()),
        Some(addr) => Err(DeniedReason::new(
            Rail::AllowedRecipient.code(),
            Rail::AllowedRecipient.evaluator(),
        )
        .with_got(json!({ "to": format!("{:#x}", addr) }))
        .with_expected(json!({
            "to_in": ALLOWED_RECIPIENTS
                .iter()
                .map(|a| format!("{:#x}", a))
                .collect::<Vec<_>>(),
        }))),
    }
}

/// Rail 4 — per-tx gas ceiling.
pub fn gas_ceiling(tx: &TransactionRequest) -> Result<(), DeniedReason> {
    match tx.gas_limit {
        None => Err(
            DeniedReason::new(Rail::GasCeiling.code(), Rail::GasCeiling.evaluator())
                .with_got(json!({ "gas_limit": null }))
                .with_hint("explicit gas_limit required so the rail has a number to check"),
        ),
        Some(g) if g <= MAX_PER_TX_GAS => Ok(()),
        Some(g) => Err(
            DeniedReason::new(Rail::GasCeiling.code(), Rail::GasCeiling.evaluator())
                .with_got(json!({ "gas_limit": g }))
                .with_expected(json!({ "gas_limit_max": MAX_PER_TX_GAS })),
        ),
    }
}

/// Rail 5 — balance ceiling. Reject if the wallet is over-funded (means W8
/// hasn't swept). Bounded loss is the whole point.
pub async fn balance_ceiling(provider: &dyn BalanceProvider) -> Result<(), DeniedReason> {
    let cents = provider.balance_usd_cents().await.map_err(balance_to_denied)?;
    if cents <= u64::from(MAX_BALANCE_USD_CENTS) {
        Ok(())
    } else {
        Err(
            DeniedReason::new(Rail::BalanceCeiling.code(), Rail::BalanceCeiling.evaluator())
                .with_got(json!({ "balance_cents": cents }))
                .with_expected(json!({ "balance_cents_max": MAX_BALANCE_USD_CENTS })),
        )
    }
}

fn balance_to_denied(e: BalanceError) -> DeniedReason {
    DeniedReason::new(Rail::BalanceCeiling.code(), Rail::BalanceCeiling.evaluator())
        .with_got(json!({ "error": e.to_string() }))
        .with_hint("balance provider unreachable — failing closed")
}

/// Rail 6 — daily spend cap. **MUTATES** the Upstash counter: increments by
/// `cost_cents`, then either approves or decrements back to the pre-call value
/// if the post-increment total exceeds `MAX_DAILY_SPEND_USD_CENTS`. Returns
/// the post-increment counter value on success so the caller can stash it on
/// `Approved.daily_spend_after_cents`.
///
/// This rail runs LAST in `evaluate_all`. If a later step (the on-chain
/// signing path) fails, the caller MUST invoke
/// [`PolicyContext::rollback_daily_cap`].
pub async fn daily_cap(
    counter: &dyn SpendCounter,
    vercel_env: &str,
    cost_cents: i64,
) -> Result<i64, DeniedReason> {
    let key = daily_spend_key(vercel_env);
    let after = counter
        .incrby(&key, cost_cents)
        .await
        .map_err(spend_counter_to_denied)?;
    // Set/refresh TTL after each increment — cheap and means a brand-new
    // counter always carries the right expiry without a separate "first
    // write" branch.
    counter
        .expire(&key, DAILY_CAP_TTL_SECONDS)
        .await
        .map_err(spend_counter_to_denied)?;

    if after > i64::from(MAX_DAILY_SPEND_USD_CENTS) {
        // Roll back our own increment so the next caller sees the pre-call
        // value. Best-effort: a transport failure here would leave the
        // counter inflated, but that's strictly safer than letting it stay
        // inflated AND approving the tx. The decrby error is swallowed into
        // the denial details for telemetry.
        let rollback_result = counter.decrby(&key, cost_cents).await;
        let rollback_after = rollback_result.ok();
        return Err(
            DeniedReason::new(Rail::DailyCap.code(), Rail::DailyCap.evaluator())
                .with_got(json!({
                    "spent_after_cents": after,
                    "rolled_back_to_cents": rollback_after,
                }))
                .with_expected(json!({ "spend_cents_max": MAX_DAILY_SPEND_USD_CENTS })),
        );
    }
    Ok(after)
}

fn spend_counter_to_denied(e: SpendCounterError) -> DeniedReason {
    DeniedReason::new(Rail::DailyCap.code(), Rail::DailyCap.evaluator())
        .with_got(json!({ "error": e.to_string() }))
        .with_hint("spend counter unreachable — failing closed")
}

// ---------------------------------------------------------------------------
// Top-level: evaluate all rails in fixed order.
// ---------------------------------------------------------------------------

/// Runs every rail in the order documented at the top of this module. On
/// success returns an [`Approved`] carrying the post-increment counter value
/// so the caller can record it on `wallet_txs`. On failure returns the
/// `DeniedReason` from the FIRST rail that rejected — strictly first-fail so
/// telemetry is deterministic.
///
/// **Cost contract:** the daily-cap counter is incremented as part of this
/// call. If the signing path subsequently fails the caller is responsible
/// for calling [`PolicyContext::rollback_daily_cap`].
pub async fn evaluate_all(tx: TransactionRequest, ctx: &PolicyContext<'_>) -> Result<Approved, DeniedReason> {
    // 0. Env gate (cheapest — sync, no I/O).
    env_gate(ctx.vercel_env)?;

    // 1-4. Cheap rails (sync compares + one Edge-Config fetch).
    //
    // Order: chain/recipient/gas come first because they're sync; if the tx
    // is obviously malformed we never reach for the network at all.
    allowed_chain(&tx)?;
    allowed_recipient(&tx)?;
    gas_ceiling(&tx)?;
    kill_switch(ctx.edge_config).await?;

    // 5. Balance — single RPC call.
    balance_ceiling(ctx.balance_provider).await?;

    // 6. Daily cap — LAST, because it has side effects (the only rail that
    //    does). Per the rollback contract in the module docs, if anything
    //    above this line rejects the counter is untouched.
    let after = daily_cap(ctx.spend_counter, ctx.vercel_env, ctx.cost_cents).await?;

    Ok(Approved {
        tx,
        committed_cents: ctx.cost_cents,
        daily_spend_after_cents: after,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge_config::{InMemoryEdgeConfig, UnreachableEdgeConfig};
    use crate::wallet::balance::InMemoryBalanceProvider;
    use crate::wallet::daily_cap::{CounterOp, InMemorySpendCounter};
    use alloy_primitives::address;

    fn ok_tx() -> TransactionRequest {
        TransactionRequest::new(ALLOWED_CHAIN_ID, ALLOWED_RECIPIENTS[0], MAX_PER_TX_GAS - 1)
    }

    // -- env_gate ----------------------------------------------------------

    #[test]
    fn env_gate_allows_production() {
        env_gate("production").unwrap();
    }

    #[test]
    fn env_gate_blocks_preview() {
        let err = env_gate("preview").unwrap_err();
        assert_eq!(err.code, Rail::EnvGate.code());
        assert_eq!(err.evaluator, Rail::EnvGate.evaluator());
    }

    #[test]
    fn env_gate_blocks_empty_string() {
        // An unset VERCEL_ENV defaults to "" in callers — must still fail.
        env_gate("").unwrap_err();
    }

    // -- kill_switch -------------------------------------------------------

    #[tokio::test]
    async fn kill_switch_passes_when_true() {
        let ec = InMemoryEdgeConfig::with_bool(KILL_SWITCH_KEY, true);
        kill_switch(&ec).await.unwrap();
    }

    #[tokio::test]
    async fn kill_switch_rejects_when_false() {
        let ec = InMemoryEdgeConfig::with_bool(KILL_SWITCH_KEY, false);
        let err = kill_switch(&ec).await.unwrap_err();
        assert_eq!(err.code, Rail::KillSwitch.code());
    }

    #[tokio::test]
    async fn kill_switch_rejects_when_missing_fail_closed() {
        // Plan §10: missing key == disabled (fail-closed default).
        let ec = InMemoryEdgeConfig::new();
        let err = kill_switch(&ec).await.unwrap_err();
        assert_eq!(err.code, Rail::KillSwitch.code());
    }

    #[tokio::test]
    async fn kill_switch_fails_closed_on_transport_error() {
        let err = kill_switch(&UnreachableEdgeConfig).await.unwrap_err();
        assert_eq!(err.code, Rail::KillSwitch.code());
        assert_eq!(err.evaluator, Rail::KillSwitch.evaluator());
    }

    // -- allowed_chain -----------------------------------------------------

    #[test]
    fn allowed_chain_passes_base_mainnet() {
        let tx = TransactionRequest::new(ALLOWED_CHAIN_ID, ALLOWED_RECIPIENTS[0], 100_000);
        allowed_chain(&tx).unwrap();
    }

    #[test]
    fn allowed_chain_rejects_other_chains() {
        let tx = TransactionRequest::new(1, ALLOWED_RECIPIENTS[0], 100_000);
        let err = allowed_chain(&tx).unwrap_err();
        assert_eq!(err.code, Rail::AllowedChain.code());
    }

    #[test]
    fn allowed_chain_rejects_missing_chain_id() {
        let tx = TransactionRequest {
            chain_id: None,
            ..ok_tx()
        };
        let err = allowed_chain(&tx).unwrap_err();
        assert_eq!(err.code, Rail::AllowedChain.code());
    }

    // -- allowed_recipient -------------------------------------------------

    #[test]
    fn allowed_recipient_passes_for_each_registry() {
        for addr in ALLOWED_RECIPIENTS {
            let tx = TransactionRequest::new(ALLOWED_CHAIN_ID, *addr, 100_000);
            allowed_recipient(&tx).unwrap();
        }
    }

    #[test]
    fn allowed_recipient_rejects_random_address() {
        let attacker = address!("0000000000000000000000000000000000000bad");
        let tx = TransactionRequest::new(ALLOWED_CHAIN_ID, attacker, 100_000);
        let err = allowed_recipient(&tx).unwrap_err();
        assert_eq!(err.code, Rail::AllowedRecipient.code());
    }

    #[test]
    fn allowed_recipient_rejects_missing_to() {
        let tx = TransactionRequest { to: None, ..ok_tx() };
        let err = allowed_recipient(&tx).unwrap_err();
        assert_eq!(err.code, Rail::AllowedRecipient.code());
    }

    // -- gas_ceiling -------------------------------------------------------

    #[test]
    fn gas_ceiling_passes_at_exact_limit() {
        let tx = TransactionRequest::new(ALLOWED_CHAIN_ID, ALLOWED_RECIPIENTS[0], MAX_PER_TX_GAS);
        gas_ceiling(&tx).unwrap();
    }

    #[test]
    fn gas_ceiling_rejects_one_over() {
        let tx = TransactionRequest::new(ALLOWED_CHAIN_ID, ALLOWED_RECIPIENTS[0], MAX_PER_TX_GAS + 1);
        let err = gas_ceiling(&tx).unwrap_err();
        assert_eq!(err.code, Rail::GasCeiling.code());
    }

    #[test]
    fn gas_ceiling_rejects_missing() {
        let tx = TransactionRequest {
            gas_limit: None,
            ..ok_tx()
        };
        gas_ceiling(&tx).unwrap_err();
    }

    // -- balance_ceiling ---------------------------------------------------

    #[tokio::test]
    async fn balance_ceiling_passes_under_ceiling() {
        let p = InMemoryBalanceProvider::new(u64::from(MAX_BALANCE_USD_CENTS) - 1);
        balance_ceiling(&p).await.unwrap();
    }

    #[tokio::test]
    async fn balance_ceiling_passes_at_exact_ceiling() {
        let p = InMemoryBalanceProvider::new(u64::from(MAX_BALANCE_USD_CENTS));
        balance_ceiling(&p).await.unwrap();
    }

    #[tokio::test]
    async fn balance_ceiling_rejects_overfunded() {
        let p = InMemoryBalanceProvider::new(u64::from(MAX_BALANCE_USD_CENTS) + 1);
        let err = balance_ceiling(&p).await.unwrap_err();
        assert_eq!(err.code, Rail::BalanceCeiling.code());
    }

    #[tokio::test]
    async fn balance_ceiling_fails_closed_on_transport_error() {
        let p = InMemoryBalanceProvider::new(0);
        p.fail_with("simulated");
        balance_ceiling(&p).await.unwrap_err();
    }

    // -- daily_cap ---------------------------------------------------------

    #[tokio::test]
    async fn daily_cap_passes_under_cap() {
        let c = InMemorySpendCounter::new();
        let after = daily_cap(&c, "production", 10).await.unwrap();
        assert_eq!(after, 10);
        // EXPIRE was set as a side effect.
        assert!(c
            .ops()
            .iter()
            .any(|op| matches!(op, CounterOp::Expire { ttl, .. } if *ttl == DAILY_CAP_TTL_SECONDS)));
    }

    #[tokio::test]
    async fn daily_cap_rejects_and_rolls_back_when_over_cap() {
        // Pre-load the counter so the next increment is over the cap.
        let key = daily_spend_key("production");
        let c = InMemorySpendCounter::with_initial(&key, i64::from(MAX_DAILY_SPEND_USD_CENTS));
        let err = daily_cap(&c, "production", 1).await.unwrap_err();
        assert_eq!(err.code, Rail::DailyCap.code());
        // After rejection the counter must be at the pre-call value.
        assert_eq!(c.value(&key), i64::from(MAX_DAILY_SPEND_USD_CENTS));
    }

    #[tokio::test]
    async fn daily_cap_fails_closed_on_transport_error() {
        let c = InMemorySpendCounter::new();
        c.fail_all_with("simulated");
        daily_cap(&c, "production", 5).await.unwrap_err();
    }

    // -- evaluate_all ------------------------------------------------------

    fn good_ctx<'a>(
        edge: &'a InMemoryEdgeConfig,
        counter: &'a InMemorySpendCounter,
        balance: &'a InMemoryBalanceProvider,
    ) -> PolicyContext<'a> {
        PolicyContext::new("production", edge, counter, balance)
    }

    #[tokio::test]
    async fn evaluate_all_happy_path() {
        let edge = InMemoryEdgeConfig::with_bool(KILL_SWITCH_KEY, true);
        let counter = InMemorySpendCounter::new();
        let balance = InMemoryBalanceProvider::new(100);
        let ctx = good_ctx(&edge, &counter, &balance);
        let approved = evaluate_all(ok_tx(), &ctx).await.unwrap();
        assert_eq!(approved.committed_cents, ESTIMATED_TX_COST_USD_CENTS);
        assert_eq!(approved.daily_spend_after_cents, ESTIMATED_TX_COST_USD_CENTS);
        // Counter was bumped — exactly once.
        let key = daily_spend_key("production");
        assert_eq!(counter.value(&key), ESTIMATED_TX_COST_USD_CENTS);
    }

    #[tokio::test]
    async fn evaluate_all_non_prod_short_circuits_before_counter() {
        let edge = InMemoryEdgeConfig::with_bool(KILL_SWITCH_KEY, true);
        let counter = InMemorySpendCounter::new();
        let balance = InMemoryBalanceProvider::new(100);
        let mut ctx = good_ctx(&edge, &counter, &balance);
        ctx.vercel_env = "preview";
        let err = evaluate_all(ok_tx(), &ctx).await.unwrap_err();
        assert_eq!(err.code, Rail::EnvGate.code());
        // Crucially: the counter is untouched in non-prod (no ops recorded).
        assert!(counter.ops().is_empty());
    }

    #[tokio::test]
    async fn evaluate_all_earlier_rail_failure_does_not_touch_counter() {
        // Bad chain — the chain rail fires before the cap rail, so the
        // counter must remain at zero (proves rail ordering is correct).
        let edge = InMemoryEdgeConfig::with_bool(KILL_SWITCH_KEY, true);
        let counter = InMemorySpendCounter::new();
        let balance = InMemoryBalanceProvider::new(100);
        let ctx = good_ctx(&edge, &counter, &balance);
        let bad = TransactionRequest::new(1, ALLOWED_RECIPIENTS[0], 100_000);
        let err = evaluate_all(bad, &ctx).await.unwrap_err();
        assert_eq!(err.code, Rail::AllowedChain.code());
        assert_eq!(counter.value(&daily_spend_key("production")), 0);
    }

    #[tokio::test]
    async fn evaluate_all_balance_failure_does_not_touch_counter() {
        // The balance rail runs JUST BEFORE the daily cap — make sure a
        // balance-rail rejection doesn't leak counter state.
        let edge = InMemoryEdgeConfig::with_bool(KILL_SWITCH_KEY, true);
        let counter = InMemorySpendCounter::new();
        let balance = InMemoryBalanceProvider::new(u64::from(MAX_BALANCE_USD_CENTS) + 1);
        let ctx = good_ctx(&edge, &counter, &balance);
        let err = evaluate_all(ok_tx(), &ctx).await.unwrap_err();
        assert_eq!(err.code, Rail::BalanceCeiling.code());
        assert!(counter.ops().is_empty());
    }

    #[tokio::test]
    async fn rollback_daily_cap_decrements_by_committed_cents() {
        // Simulates the signing path failing AFTER the cap rail committed
        // an increment. The caller invokes rollback_daily_cap; we assert
        // the counter ends up where it started.
        let edge = InMemoryEdgeConfig::with_bool(KILL_SWITCH_KEY, true);
        let counter = InMemorySpendCounter::new();
        let balance = InMemoryBalanceProvider::new(100);
        let ctx = good_ctx(&edge, &counter, &balance);
        let approved = evaluate_all(ok_tx(), &ctx).await.unwrap();
        assert!(approved.committed_cents > 0);
        let key = daily_spend_key("production");
        assert_eq!(counter.value(&key), approved.committed_cents);

        // Caller's fault-handling path:
        ctx.rollback_daily_cap().await.unwrap();
        assert_eq!(counter.value(&key), 0);
    }

    #[tokio::test]
    async fn no_panic_on_zero_cost() {
        // Edge case: a downstream PR might wire cost_cents=0. The rails
        // should accept it (counter goes up by 0, still under cap).
        let edge = InMemoryEdgeConfig::with_bool(KILL_SWITCH_KEY, true);
        let counter = InMemorySpendCounter::new();
        let balance = InMemoryBalanceProvider::new(0);
        let mut ctx = good_ctx(&edge, &counter, &balance);
        ctx.cost_cents = 0;
        let approved = evaluate_all(ok_tx(), &ctx).await.unwrap();
        assert_eq!(approved.committed_cents, 0);
        assert_eq!(approved.daily_spend_after_cents, 0);
    }
}

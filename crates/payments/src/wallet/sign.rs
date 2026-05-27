//! Wallet signing entrypoint — **rails only; signing is stubbed**.
//!
//! Phase 6 implements every defensive check from plan §10.2 but stops short
//! of actually constructing a `PrivateKeySigner` or touching the chain.
//! Phase 6 part 2 wires alloy + the Coinbase CDP / Alchemy provider; the
//! rails interface above is stable so that change is additive.

use eth_tools_core::DeniedReason;
use sqlx::PgPool;

use crate::denials::{record_denial, DenialRecord};
use crate::wallet::policy::{evaluate_all, PolicyContext, TransactionRequest};

/// Public entrypoint used by callers (the upcoming x402 settlement path,
/// CLI sweep helpers, etc.). Runs every rail; on failure writes a row to
/// `denials` so the audit trail outlives Vercel's 1-day log retention
/// (plan §10.2 trailer + §10.5 #10). Signing is **explicitly stubbed**:
/// on success this fn panics with `todo!`, NOT silently no-ops, so a
/// caller wired up before the signing PR lands fails loudly.
///
/// `request_path` is the originating HTTP path (or a synthetic
/// `"cli:sweep"` for non-HTTP callers) — written verbatim into the
/// `denials.request_path` column for grep-ability.
pub async fn sign_and_send(
    tx: TransactionRequest,
    ctx: &PolicyContext<'_>,
    db: &PgPool,
    request_path: &str,
) -> Result<alloy_primitives::TxHash, DeniedReason> {
    match evaluate_all(tx, ctx).await {
        Ok(_approved) => {
            // Phase 6 part 2 — wires:
            //   let signer = PrivateKeySigner::from_str(&std::env::var(PRIVATE_KEY_ENV)?)?;
            //   let provider = ProviderBuilder::new().wallet(signer).on_http(rpc_url());
            //   let receipt = provider.send_transaction(approved.tx).await?
            //       .with_required_confirmations(1).get_receipt().await?;
            //   record_wallet_tx(db, &receipt).await?;
            //   Ok(receipt.transaction_hash)
            //
            // The rails above (including the daily-cap counter increment in
            // `evaluate_all`) have already committed; if the signer fails
            // the caller MUST invoke `ctx.rollback_daily_cap()` before
            // returning the error to the user.
            todo!("phase 6 part 2: alloy signing + receipt persistence")
        }
        Err(reason) => {
            // Best-effort audit write. A db hiccup here MUST NOT mask the
            // policy decision — log + drop. Plan §10.2: the wallet refuses
            // the tx; the audit row is a secondary obligation.
            if let Err(e) = record_denial(
                db,
                DenialRecord {
                    code: reason.code.clone(),
                    evaluator: reason.evaluator.clone(),
                    api_key_id: ctx.api_key_id,
                    request_path: request_path.to_string(),
                    details: Some(serde_json::to_value(&reason).unwrap_or(serde_json::Value::Null)),
                },
            )
            .await
            {
                tracing::error!(
                    error = %e,
                    code = %reason.code,
                    evaluator = %reason.evaluator,
                    "failed to record wallet denial to audit table"
                );
            }
            Err(reason)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge_config::InMemoryEdgeConfig;
    use crate::wallet::balance::InMemoryBalanceProvider;
    use crate::wallet::daily_cap::InMemorySpendCounter;
    use crate::wallet::policy::TransactionRequest;
    use crate::wallet::{ALLOWED_CHAIN_ID, ALLOWED_RECIPIENTS, KILL_SWITCH_KEY, MAX_PER_TX_GAS};

    // We can't `sign_and_send` happy-path without a Postgres pool AND it
    // would `todo!()` anyway. The integration test in
    // `tests/wallet_rails_e2e.rs` covers the denial-writing path against a
    // real Postgres. Unit-level we only assert that the function compiles
    // and dispatches correctly via the underlying `evaluate_all`.

    #[tokio::test]
    async fn rails_compose_through_evaluate_all() {
        let edge = InMemoryEdgeConfig::with_bool(KILL_SWITCH_KEY, true);
        let counter = InMemorySpendCounter::new();
        let balance = InMemoryBalanceProvider::new(100);
        let ctx = PolicyContext::new("production", &edge, &counter, &balance);
        let bad = TransactionRequest::new(1, ALLOWED_RECIPIENTS[0], MAX_PER_TX_GAS);
        // We sidestep sign_and_send (no DB here) but assert evaluate_all
        // -- which sign_and_send delegates to -- rejects the bad tx.
        let _ = ALLOWED_CHAIN_ID;
        let err = evaluate_all(bad, &ctx).await.unwrap_err();
        assert_eq!(err.code, "WRONG_CHAIN");
    }
}

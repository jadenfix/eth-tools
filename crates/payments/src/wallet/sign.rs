//! Wallet signing entrypoint — rails + alloy signing + Postgres nonce + reconcile.
//!
//! Pipeline (each step strictly ordered):
//!
//! ```text
//!   evaluate_all(tx, ctx)            — kill/chain/recipient/gas/balance/cap
//!     │
//!     ├─ Err(reason)  → record_denial → return Err
//!     │
//!     ▼
//!   require signer + rpc + nonce_store on ctx
//!     │
//!     ├─ missing      → SIGNING_DISABLED denial → rollback cap → return Err
//!     │
//!     ▼
//!   rpc.chain_id()              — confirm rpc chain == ALLOWED_CHAIN_ID
//!   rpc.eip1559_fees() + clamp  — gas-cap second pass (rpc may suggest
//!                                  fees above MAX_FEE_PER_GAS_WEI)
//!   nonce_store.allocate(...)   — Postgres FOR UPDATE, seed via rpc once
//!   signer.sign(unsigned)       — alloy LocalSigner builds EIP-1559 envelope
//!
//!   wallet_txs INSERT (status='submitted', nonce, tx_hash)
//!   rpc.send_raw_transaction()  — pushes to mempool
//!   rpc.wait_for_receipt(60s)   — block for receipt
//!     │
//!     ├─ Timeout      → keep wallet_txs as 'submitted', do NOT rollback nonce
//!     │
//!     ▼
//!   wallet_txs UPDATE (status='confirmed'|'reverted', gas_used, fee_usdc)
//!   reconcile_actual_cost(...)  — counter delta from estimate
//!   return Ok(tx_hash)
//! ```
//!
//! ## Failure modes and what gets persisted
//!
//! | Step           | wallet_txs | denials | counter | nonce  |
//! |----------------|------------|---------|---------|--------|
//! | rail rejection |  none      |  row    | clean   | clean  |
//! | signer missing |  none      |  row    | rolled  | clean  |
//! | wrong chain    |  none      |  row    | rolled  | clean  |
//! | gas-cap (rpc)  |  none      |  row    | rolled  | clean  |
//! | nonce alloc    |  none      |  row    | rolled  | clean  |
//! | sign           |  none      |  row    | rolled  | spent* |
//! | send (5xx)     |  none      |  row    | rolled  | spent* |
//! | send (reject)  |  rejected  |  row    | rolled  | spent* |
//! | timeout        |  submitted |  none   | KEPT    | spent  |
//! | revert         |  reverted  |  none   | reconc  | spent  |
//! | success        |  confirmed |  none   | reconc  | spent  |
//!
//! *"spent" + "we'd really like the cap counter rolled back" — see the
//! cap-rollback discussion. The nonce is gone in any case; the next
//! allocator skips that value.
//!
//! ## Why we don't release nonces on failure
//!
//! Even on a `send` failure we can't be sure the tx didn't land — the RPC
//! 503 might be a connection drop AFTER the mempool ack. Releasing the
//! nonce would let the next tx use the same value, causing one of them to
//! be permanently rejected. Safer to burn the nonce and let the next tx
//! pick `N+1`.

use std::time::Duration;

use alloy_primitives::U256;
use eth_tools_core::DeniedReason;
use serde_json::json;
use sqlx::PgPool;
use tracing::Instrument;

use crate::denials::{record_denial, DenialRecord};
use crate::wallet::gas::{
    check_worst_case_cost, clamp_gas_price, exceeds_fee_ceiling, wei_cost_to_cents,
};
use crate::wallet::nonce::NonceError;
use crate::wallet::policy::{evaluate_all, reconcile_actual_cost, PolicyContext, TransactionRequest};
use crate::wallet::rpc::{SeedAdapter, WalletRpcError};
use crate::wallet::signer::{SignerError, UnsignedTx};
use crate::wallet::txs::{finalize_receipt, insert_submitted, WalletTxStatus};
use crate::wallet::ALLOWED_CHAIN_ID;

/// Hard timeout for the receipt fetch. Plan §10.2 calls for "1 confirmation
/// within 60s"; if the receipt doesn't arrive the row stays in `submitted`
/// for the W7 reconciler to pick up later.
pub const RECEIPT_TIMEOUT: Duration = Duration::from_secs(60);

/// Public entrypoint. Runs every rail; on success signs + sends + reconciles.
/// Writes to `denials` on rail failure and to `wallet_txs` on every tx that
/// reached the dispatch path.
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
    // Span carries everything that's safe to log — never the private key,
    // never the raw signed bytes.
    let span = tracing::info_span!(
        "wallet.sign_and_send",
        chain_id = ?tx.chain_id,
        to = ?tx.to,
        gas_limit = ?tx.gas_limit,
        request_path = %request_path,
    );
    sign_and_send_inner(tx, ctx, db, request_path).instrument(span).await
}

async fn sign_and_send_inner(
    tx: TransactionRequest,
    ctx: &PolicyContext<'_>,
    db: &PgPool,
    request_path: &str,
) -> Result<alloy_primitives::TxHash, DeniedReason> {
    // 1. Rails. Counter is bumped iff every rail passes.
    let approved = match evaluate_all(tx.clone(), ctx).await {
        Ok(a) => a,
        Err(reason) => return Err(write_denial(db, ctx, request_path, reason).await),
    };

    // 2. Signing collaborators present?
    let (Some(signer), Some(rpc), Some(nonces)) = (ctx.signer, ctx.wallet_rpc, ctx.nonce_store)
    else {
        let reason = DeniedReason::new("SIGNING_DISABLED", "wallet.sign")
            .with_hint("PolicyContext missing signer/rpc/nonce_store — call with_signing()");
        rollback_with_log(ctx, "missing collaborators").await;
        return Err(write_denial(db, ctx, request_path, reason).await);
    };

    let signer_addr = signer.address();

    // 3. Sanity-check RPC chain matches the rail's allowed chain. If the
    //    RPC is pointed at the wrong network the rail-level chain check
    //    (which inspects the request) won't catch it.
    let rpc_chain = match rpc.chain_id().await {
        Ok(c) => c,
        Err(e) => {
            rollback_with_log(ctx, &format!("rpc chain_id fail: {e}")).await;
            let reason = DeniedReason::new("RPC_UNREACHABLE", "wallet.rpc")
                .with_got(json!({ "error": e.to_string() }))
                .with_hint("eth_chainId failed; rotator is exhausted");
            return Err(write_denial(db, ctx, request_path, reason).await);
        }
    };
    if rpc_chain != ALLOWED_CHAIN_ID {
        rollback_with_log(ctx, "rpc on wrong chain").await;
        let reason = DeniedReason::new("RPC_WRONG_CHAIN", "wallet.rpc")
            .with_got(json!({ "rpc_chain_id": rpc_chain }))
            .with_expected(json!({ "chain_id": ALLOWED_CHAIN_ID }))
            .with_hint("RPC endpoint is misconfigured");
        return Err(write_denial(db, ctx, request_path, reason).await);
    }

    // 4. Gas pricing — second-pass gas-cap check against the live fee
    //    market. The rail-level gas_ceiling check covers gas_limit;
    //    THIS check covers gas_price. Both are required to bound cost.
    let rpc_fees = match rpc.eip1559_fees().await {
        Ok(f) => f,
        Err(e) => {
            rollback_with_log(ctx, &format!("fee history fail: {e}")).await;
            let reason = DeniedReason::new("RPC_UNREACHABLE", "wallet.rpc")
                .with_got(json!({ "error": e.to_string() }))
                .with_hint("eth_feeHistory failed");
            return Err(write_denial(db, ctx, request_path, reason).await);
        }
    };
    if exceeds_fee_ceiling(rpc_fees) {
        rollback_with_log(ctx, "rpc gas fee above cap").await;
        let reason = DeniedReason::new("GAS_PRICE_TOO_HIGH", "wallet.gas")
            .with_got(json!({
                "max_fee_per_gas_wei": rpc_fees.max_fee_per_gas.to_string(),
            }))
            .with_expected(json!({
                "max_fee_per_gas_wei_ceiling": crate::wallet::gas::MAX_FEE_PER_GAS_WEI.to_string(),
            }))
            .with_hint("Base mainnet congested; refusing tx");
        return Err(write_denial(db, ctx, request_path, reason).await);
    }
    let gas = clamp_gas_price(rpc_fees);
    // Worst-case envelope check — gas_limit × max_fee must stay under the
    // daily cap (in cents).
    if let Err(overshoot) = check_worst_case_cost(approved.tx.gas_limit.unwrap_or(0), gas) {
        rollback_with_log(ctx, "worst-case envelope above cap").await;
        let reason = DeniedReason::new("ENVELOPE_TOO_LARGE", "wallet.gas")
            .with_got(json!({ "overshoot_cents": overshoot }))
            .with_hint("worst-case gas_limit × max_fee_per_gas exceeds daily cap");
        return Err(write_denial(db, ctx, request_path, reason).await);
    }

    // 5. Nonce — Postgres FOR UPDATE allocator, seed once from RPC.
    let seed = SeedAdapter(rpc);
    let nonce = match nonces.allocate(ALLOWED_CHAIN_ID, signer_addr, &seed).await {
        Ok(n) => n,
        Err(NonceError::Seed(msg)) => {
            rollback_with_log(ctx, &format!("nonce seed fail: {msg}")).await;
            let reason = DeniedReason::new("NONCE_SEED_FAILED", "wallet.nonce")
                .with_got(json!({ "error": msg }))
                .with_hint("first eth_getTransactionCount failed");
            return Err(write_denial(db, ctx, request_path, reason).await);
        }
        Err(NonceError::Transport(msg)) => {
            rollback_with_log(ctx, &format!("nonce store fail: {msg}")).await;
            let reason = DeniedReason::new("NONCE_STORE_UNREACHABLE", "wallet.nonce")
                .with_got(json!({ "error": msg }))
                .with_hint("wallet_nonces table unreachable");
            return Err(write_denial(db, ctx, request_path, reason).await);
        }
    };

    // 6. Sign. Nonce is now "spent" — we will not release it even on later
    //    failure; the next allocator picks N+1.
    let unsigned = UnsignedTx::from_rail(&approved.tx, nonce, gas);
    let selector = unsigned.selector();
    let value_wei = unsigned.value_wei;
    let signed = match signer.sign(unsigned).await {
        Ok(s) => s,
        Err(SignerError::KeyMissing(env)) => {
            rollback_with_log(ctx, &format!("signer key missing: {env}")).await;
            let reason = DeniedReason::new("SIGNER_KEY_MISSING", "wallet.sign")
                .with_got(json!({ "env": env }))
                .with_hint("EVM_PRIVATE_KEY not in env");
            return Err(write_denial(db, ctx, request_path, reason).await);
        }
        Err(e) => {
            rollback_with_log(ctx, &format!("sign fail: {e}")).await;
            let reason = DeniedReason::new("SIGNER_ERROR", "wallet.sign")
                .with_got(json!({ "error": e.to_string() }))
                .with_hint("signer failed to produce signature");
            return Err(write_denial(db, ctx, request_path, reason).await);
        }
    };

    // 7. wallet_txs INSERT (status='submitted'). Captures the row id so the
    //    UPDATE can target precisely. NEVER log signed.raw; the tx_hash is
    //    safe (public on chain) but the raw payload contains the signature
    //    bytes which combined with the unsigned hash could leak private key
    //    bits — defensive-only, the signature is mathematically not the key.
    let wallet_tx_id = match insert_submitted(
        db,
        ALLOWED_CHAIN_ID,
        nonce,
        approved.tx.to.expect("rail passed"),
        selector,
        value_wei,
        signed.tx_hash,
        ctx.api_key_id,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            // DB hiccup — log + drop (the rails decision is what's
            // load-bearing). We still try to send the tx since the signed
            // payload is bound to the burned nonce.
            tracing::error!(error = %e, tx_hash = %signed.tx_hash, "wallet_txs INSERT failed; continuing to send");
            -1
        }
    };

    // 8. Submit raw tx.
    let submitted_hash = match rpc.send_raw_transaction(&signed.raw).await {
        Ok(h) => h,
        Err(WalletRpcError::Rejected(msg)) => {
            // We can flip the row to 'rejected' so reconciliation knows the
            // tx never made it to the mempool.
            if wallet_tx_id >= 0 {
                let _ = sqlx::query(
                    "UPDATE wallet_txs SET status = 'rejected', reject_reason = $1 WHERE id = $2",
                )
                .bind(&msg)
                .bind(wallet_tx_id)
                .execute(db)
                .await;
            }
            rollback_with_log(ctx, &format!("send rejected: {msg}")).await;
            let reason = DeniedReason::new("TX_REJECTED", "wallet.send")
                .with_got(json!({ "error": msg }))
                .with_hint("rpc rejected at mempool");
            return Err(write_denial(db, ctx, request_path, reason).await);
        }
        Err(WalletRpcError::Transport(msg)) => {
            rollback_with_log(ctx, &format!("send transport: {msg}")).await;
            let reason = DeniedReason::new("RPC_UNREACHABLE", "wallet.send")
                .with_got(json!({ "error": msg }))
                .with_hint("send_raw_transaction transport error");
            return Err(write_denial(db, ctx, request_path, reason).await);
        }
        Err(WalletRpcError::Timeout(_)) => {
            // send shouldn't time out — only wait_for_receipt does. Treat
            // as transport.
            rollback_with_log(ctx, "send timeout").await;
            let reason = DeniedReason::new("RPC_UNREACHABLE", "wallet.send")
                .with_hint("send_raw_transaction timed out");
            return Err(write_denial(db, ctx, request_path, reason).await);
        }
    };

    // 9. Wait for confirmation. On timeout: leave wallet_txs as 'submitted',
    //    return Err but do NOT touch counter (we can't reconcile without a
    //    receipt; if the tx eventually confirms the W7 reconciler picks it
    //    up).
    let receipt = match rpc
        .wait_for_receipt(submitted_hash, 1, RECEIPT_TIMEOUT)
        .await
    {
        Ok(r) => r,
        Err(WalletRpcError::Timeout(d)) => {
            tracing::warn!(
                tx_hash = %submitted_hash,
                timeout_secs = d.as_secs(),
                "receipt timeout; leaving wallet_txs as submitted for reconciler"
            );
            let reason = DeniedReason::new("RECEIPT_TIMEOUT", "wallet.receipt")
                .with_got(json!({ "tx_hash": format!("{:#x}", submitted_hash) }))
                .with_hint("tx submitted but no receipt within 60s; reconciler will catch up");
            // CRITICAL: do NOT rollback the cap counter here. The tx may
            // still confirm; reconciliation will adjust on the next pass.
            return Err(write_denial(db, ctx, request_path, reason).await);
        }
        Err(e) => {
            tracing::error!(error = %e, tx_hash = %submitted_hash, "wait_for_receipt error");
            let reason = DeniedReason::new("RPC_UNREACHABLE", "wallet.receipt")
                .with_got(json!({ "error": e.to_string() }))
                .with_hint("wait_for_receipt transport error");
            return Err(write_denial(db, ctx, request_path, reason).await);
        }
    };

    // 10. Final status + reconciliation. Even on revert we paid gas — the
    //     counter still needs to reflect actual cost.
    let final_status = if receipt.success {
        WalletTxStatus::Confirmed
    } else {
        WalletTxStatus::Reverted
    };
    let actual_cents = wei_cost_to_cents(receipt.gas_used, receipt.effective_gas_price);

    if wallet_tx_id >= 0 {
        if let Err(e) = finalize_receipt(db, wallet_tx_id, final_status, receipt.gas_used, actual_cents).await
        {
            tracing::error!(
                error = %e,
                wallet_tx_id,
                tx_hash = %submitted_hash,
                "wallet_txs UPDATE failed; row may stay in 'submitted'"
            );
        }
    }

    if let Err(e) = reconcile_actual_cost(ctx, approved.committed_cents, actual_cents).await {
        tracing::error!(
            error = %e.code,
            tx_hash = %submitted_hash,
            actual_cents,
            estimated_cents = approved.committed_cents,
            "reconcile_actual_cost failed; daily cap may drift"
        );
    }

    if !receipt.success {
        // Revert → denial. We still wrote the receipt + reconciled the
        // counter (we paid gas), but the caller deserves a structured error.
        let reason = DeniedReason::new("TX_REVERTED", "wallet.receipt")
            .with_got(json!({
                "tx_hash": format!("{:#x}", submitted_hash),
                "gas_used": receipt.gas_used,
            }))
            .with_hint("contract execution reverted on-chain; gas was still charged");
        return Err(write_denial(db, ctx, request_path, reason).await);
    }

    Ok(submitted_hash)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Roll back the daily-cap counter and log on failure. Idempotent at the
/// counter level (decrby always applies — the counter can go slightly
/// negative for a few seconds if a concurrent reconcile races, but the
/// next caller's incrby fixes it).
async fn rollback_with_log(ctx: &PolicyContext<'_>, why: &str) {
    if let Err(e) = ctx.rollback_daily_cap().await {
        tracing::error!(
            error = %e,
            why = %why,
            "daily-cap rollback failed; counter may be inflated until TTL"
        );
    }
}

/// Best-effort denial-row write — never propagates a DB error to the user.
/// Returns the original reason unchanged so the caller can `Err(...)` it.
async fn write_denial(
    db: &PgPool,
    ctx: &PolicyContext<'_>,
    request_path: &str,
    reason: DeniedReason,
) -> DeniedReason {
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
    reason
}

/// Bridge so we can short-circuit `value_wei` for the (unused) value field.
#[doc(hidden)]
#[allow(dead_code)]
fn _value_or_zero(v: Option<U256>) -> U256 {
    v.unwrap_or(U256::ZERO)
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

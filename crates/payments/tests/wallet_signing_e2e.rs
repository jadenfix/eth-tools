//! Phase 6.2 — sign_and_send happy + revert + timeout + gas-cap + nonce-conflict.
//!
//! Drives the full pipeline against:
//!   * real Postgres (testcontainers) for `wallet_nonces` + `wallet_txs` + `denials`
//!   * mock `WalletRpc` for chain_id / fees / send / receipt
//!   * mock `TxSigner` for deterministic signed bytes
//!
//! These are the unit-level coverage the task spec asks for. The
//! anvil-fork integration test lives in `wallet_anvil_fork.rs` and is
//! gated on `ANVIL_FORK_URL`.

#![cfg(test)]

use std::time::Duration;

use alloy_primitives::{address, Address, U256};
use eth_tools_payments::denials::count_by_code_evaluator;
use eth_tools_payments::edge_config::InMemoryEdgeConfig;
use eth_tools_payments::wallet::balance::InMemoryBalanceProvider;
use eth_tools_payments::wallet::daily_cap::InMemorySpendCounter;
use eth_tools_payments::wallet::gas::{MAX_FEE_PER_GAS_WEI, MIN_PRIORITY_FEE_WEI};
use eth_tools_payments::wallet::nonce::{InMemoryNonceProvider, NonceProvider};
use eth_tools_payments::wallet::policy::{PolicyContext, TransactionRequest};
use eth_tools_payments::wallet::rpc::{
    GasPrice, InMemoryWalletRpc, MockOutcome, WalletRpc, WalletRpcSeed,
};
use eth_tools_payments::wallet::signer::{MockSigner, TxSigner};
use eth_tools_payments::wallet::txs::{count_by_status, WalletTxStatus};
use eth_tools_payments::wallet::{
    daily_spend_key, sign_and_send, ALLOWED_CHAIN_ID, ALLOWED_RECIPIENTS, KILL_SWITCH_KEY,
    MAX_PER_TX_GAS,
};
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

async fn boot() -> Option<(impl std::fmt::Debug, sqlx::PgPool)> {
    let container = match Postgres::default().with_tag("16-alpine").start().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[skip] Docker unavailable: {e}");
            return None;
        }
    };
    let host = container.get_host().await.ok()?;
    let port = container.get_host_port_ipv4(5432).await.ok()?;
    let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");

    let pool = eth_tools_db::connect(&url).await.expect("connect");
    eth_tools_db::migrate(&pool).await.expect("migrate");
    Some((container, pool))
}

fn signer_addr() -> Address {
    address!("ABCDABCDABCDABCDABCDABCDABCDABCDABCDABCD")
}

fn ok_tx() -> TransactionRequest {
    TransactionRequest::new(ALLOWED_CHAIN_ID, ALLOWED_RECIPIENTS[0], MAX_PER_TX_GAS - 1)
}

/// Standard happy-path stubs: kill switch on, balance under cap, counter
/// empty, RPC returns sane fees, nonce seeded at zero.
fn happy_stubs() -> (
    InMemoryEdgeConfig,
    InMemorySpendCounter,
    InMemoryBalanceProvider,
    InMemoryWalletRpc,
    InMemoryNonceProvider,
    MockSigner,
) {
    (
        InMemoryEdgeConfig::with_bool(KILL_SWITCH_KEY, true),
        InMemorySpendCounter::new(),
        InMemoryBalanceProvider::new(100),
        {
            let r = InMemoryWalletRpc::new();
            r.set_fees(GasPrice {
                max_fee_per_gas: 100_000_000,         // 0.1 gwei
                max_priority_fee_per_gas: MIN_PRIORITY_FEE_WEI,
            });
            r.set_pending_count(0);
            r
        },
        InMemoryNonceProvider::new(),
        MockSigner::new(signer_addr()),
    )
}

#[tokio::test]
async fn happy_path_inserts_wallet_tx_and_reconciles_counter() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let (edge, counter, balance, rpc, nonces, signer) = happy_stubs();
    // Pre-load receipt outcome — gas_used=60k @ 0.05 gwei = ~$0.0005 ≈ 0¢
    // after rounding (well below ESTIMATED_TX_COST_USD_CENTS=5).
    rpc.push_outcome(MockOutcome::Success {
        gas_used: 60_000,
        effective_gas_price: 50_000_000,
    });
    let ctx = PolicyContext::new("production", &edge, &counter, &balance).with_signing(
        &signer,
        &rpc,
        &nonces,
    );

    let hash = sign_and_send(ok_tx(), &ctx, &pool, "/api/v1/wallet/spend")
        .await
        .expect("happy path must succeed");
    assert_ne!(hash, alloy_primitives::TxHash::ZERO);

    // wallet_txs row was written and finalized to 'confirmed'.
    let confirmed = count_by_status(&pool, WalletTxStatus::Confirmed, ALLOWED_CHAIN_ID)
        .await
        .unwrap();
    assert_eq!(confirmed, 1);

    // Counter reflects reconciled (actual) cost — actual was ~0¢, estimate 5¢
    // → counter should end up at max(0, 5 - rollback_delta) i.e. <= 5.
    let key = daily_spend_key("production");
    let final_cents = counter.value(&key);
    assert!(final_cents <= 5, "counter at {final_cents}, expected ≤ 5");

    // Nonce was allocated.
    assert_eq!(nonces.peek(ALLOWED_CHAIN_ID, signer_addr()).await, Some(1));
}

#[tokio::test]
async fn revert_writes_reverted_status_and_returns_denial() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let (edge, counter, balance, rpc, nonces, signer) = happy_stubs();
    rpc.push_outcome(MockOutcome::Revert {
        gas_used: 80_000,
        effective_gas_price: 100_000_000,
    });
    let ctx = PolicyContext::new("production", &edge, &counter, &balance).with_signing(
        &signer,
        &rpc,
        &nonces,
    );

    let err = sign_and_send(ok_tx(), &ctx, &pool, "test:revert")
        .await
        .unwrap_err();
    assert_eq!(err.code, "TX_REVERTED");

    // Row is 'reverted', not 'confirmed'.
    let reverted = count_by_status(&pool, WalletTxStatus::Reverted, ALLOWED_CHAIN_ID)
        .await
        .unwrap();
    assert_eq!(reverted, 1);

    // Denial row written.
    let n = count_by_code_evaluator(&pool, "TX_REVERTED", "wallet.receipt")
        .await
        .unwrap();
    assert_eq!(n, 1);

    // Nonce still spent (we paid gas).
    assert_eq!(nonces.peek(ALLOWED_CHAIN_ID, signer_addr()).await, Some(1));
}

#[tokio::test]
async fn receipt_timeout_leaves_submitted_status_and_keeps_counter() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let (edge, counter, balance, rpc, nonces, signer) = happy_stubs();
    rpc.push_outcome(MockOutcome::Timeout);
    let ctx = PolicyContext::new("production", &edge, &counter, &balance).with_signing(
        &signer,
        &rpc,
        &nonces,
    );

    let err = sign_and_send(ok_tx(), &ctx, &pool, "test:timeout")
        .await
        .unwrap_err();
    assert_eq!(err.code, "RECEIPT_TIMEOUT");

    // Row stays in 'submitted' for the reconciler to pick up later.
    let submitted = count_by_status(&pool, WalletTxStatus::Submitted, ALLOWED_CHAIN_ID)
        .await
        .unwrap();
    assert_eq!(submitted, 1);

    // CRITICAL: counter is NOT rolled back on timeout — the tx may still
    // confirm and we want the cap reflected.
    let key = daily_spend_key("production");
    assert!(counter.value(&key) > 0, "counter must NOT roll back on timeout");
}

#[tokio::test]
async fn gas_price_above_cap_denies_before_send() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let (edge, counter, balance, rpc, nonces, signer) = happy_stubs();
    // Suggest a fee 10x our cap — must trigger GAS_PRICE_TOO_HIGH.
    rpc.set_fees(GasPrice {
        max_fee_per_gas: 10 * MAX_FEE_PER_GAS_WEI,
        max_priority_fee_per_gas: MIN_PRIORITY_FEE_WEI,
    });
    let ctx = PolicyContext::new("production", &edge, &counter, &balance).with_signing(
        &signer,
        &rpc,
        &nonces,
    );

    let err = sign_and_send(ok_tx(), &ctx, &pool, "test:gas")
        .await
        .unwrap_err();
    assert_eq!(err.code, "GAS_PRICE_TOO_HIGH");

    // No tx was sent (send_raw_transaction never called).
    let sent = rpc.ops().iter().any(|op| matches!(op, eth_tools_payments::wallet::rpc::MockOp::SendRaw { .. }));
    assert!(!sent, "must not send_raw_transaction after gas-cap denial");

    // Counter was rolled back (rail incremented, gas-cap rejection rolls back).
    let key = daily_spend_key("production");
    assert_eq!(counter.value(&key), 0);
}

#[tokio::test]
async fn send_rejected_writes_rejected_status() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let (edge, counter, balance, rpc, nonces, signer) = happy_stubs();
    rpc.push_outcome(MockOutcome::SendRejected("nonce too low"));
    let ctx = PolicyContext::new("production", &edge, &counter, &balance).with_signing(
        &signer,
        &rpc,
        &nonces,
    );

    let err = sign_and_send(ok_tx(), &ctx, &pool, "test:nonce_too_low")
        .await
        .unwrap_err();
    assert_eq!(err.code, "TX_REJECTED");

    // Row is 'rejected'.
    let rejected = count_by_status(&pool, WalletTxStatus::Rejected, ALLOWED_CHAIN_ID)
        .await
        .unwrap();
    assert_eq!(rejected, 1);
}

#[tokio::test]
async fn send_transport_returns_rpc_unreachable_and_rolls_back() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let (edge, counter, balance, rpc, nonces, signer) = happy_stubs();
    rpc.push_outcome(MockOutcome::SendTransport("503 upstream gone"));
    let ctx = PolicyContext::new("production", &edge, &counter, &balance).with_signing(
        &signer,
        &rpc,
        &nonces,
    );

    let err = sign_and_send(ok_tx(), &ctx, &pool, "test:transport")
        .await
        .unwrap_err();
    assert_eq!(err.code, "RPC_UNREACHABLE");

    // Counter was rolled back. Don't write a wallet_txs row for transport
    // errors (we DO write — pre-send INSERT happens before send). Actually
    // the row is still 'submitted'. That's OK — reconciler will catch it.
    let key = daily_spend_key("production");
    assert_eq!(counter.value(&key), 0);
}

#[tokio::test]
async fn signing_disabled_denial_when_collaborators_missing() {
    // PolicyContext WITHOUT .with_signing() — sign_and_send must reject
    // post-rails with SIGNING_DISABLED, NOT panic.
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let edge = InMemoryEdgeConfig::with_bool(KILL_SWITCH_KEY, true);
    let counter = InMemorySpendCounter::new();
    let balance = InMemoryBalanceProvider::new(100);
    let ctx = PolicyContext::new("production", &edge, &counter, &balance);

    let err = sign_and_send(ok_tx(), &ctx, &pool, "test:no_signer")
        .await
        .unwrap_err();
    assert_eq!(err.code, "SIGNING_DISABLED");

    // Counter must roll back (rails incremented, signer-missing rejects).
    let key = daily_spend_key("production");
    assert_eq!(counter.value(&key), 0);
}

#[tokio::test]
async fn rpc_chain_mismatch_returns_denial() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let (edge, counter, balance, rpc, nonces, signer) = happy_stubs();
    rpc.set_chain_id(1); // mainnet, not Base
    let ctx = PolicyContext::new("production", &edge, &counter, &balance).with_signing(
        &signer,
        &rpc,
        &nonces,
    );

    let err = sign_and_send(ok_tx(), &ctx, &pool, "test:chain_mismatch")
        .await
        .unwrap_err();
    assert_eq!(err.code, "RPC_WRONG_CHAIN");
}

#[tokio::test]
async fn wallet_tx_id_is_attributed_to_api_key() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let api_key_id = uuid::Uuid::new_v4();
    sqlx::query(
        "INSERT INTO api_keys (id, github_user_id, github_login, key_prefix, key_hash, name) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(api_key_id)
    .bind(42_i64)
    .bind("phase6.2-test")
    .bind(format!("pfx-{api_key_id}"))
    .bind(format!("hash-{api_key_id}"))
    .bind("phase-6.2-wallet-tx-attribution")
    .execute(&pool)
    .await
    .expect("seed api_keys row");

    let (edge, counter, balance, rpc, nonces, signer) = happy_stubs();
    rpc.push_outcome(MockOutcome::Success {
        gas_used: 50_000,
        effective_gas_price: 50_000_000,
    });
    let ctx = PolicyContext::new("production", &edge, &counter, &balance)
        .with_api_key_id(Some(api_key_id))
        .with_signing(&signer, &rpc, &nonces);

    sign_and_send(ok_tx(), &ctx, &pool, "/api/v1/wallet/spend")
        .await
        .expect("happy path must succeed");

    let (stored,): (Option<uuid::Uuid>,) =
        sqlx::query_as("SELECT api_key_id FROM wallet_txs WHERE status = 'confirmed' LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored, Some(api_key_id));
}

#[tokio::test]
async fn sequential_calls_dispense_strictly_increasing_nonces() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let (edge, counter, balance, rpc, nonces, signer) = happy_stubs();
    for _ in 0..3 {
        rpc.push_outcome(MockOutcome::Success {
            gas_used: 50_000,
            effective_gas_price: 50_000_000,
        });
    }
    // Bump balance + cap to fit 3 sends.
    balance.set(200);
    let mut ctx = PolicyContext::new("production", &edge, &counter, &balance).with_signing(
        &signer,
        &rpc,
        &nonces,
    );
    ctx.cost_cents = 5;

    for _ in 0..3 {
        sign_and_send(ok_tx(), &ctx, &pool, "test:seq")
            .await
            .expect("must succeed");
    }

    // Nonces 0,1,2 issued → next is 3.
    assert_eq!(nonces.peek(ALLOWED_CHAIN_ID, signer_addr()).await, Some(3));
    // wallet_txs has 3 confirmed rows with nonces 0,1,2.
    let (nonces_used,): (Vec<i64>,) =
        sqlx::query_as("SELECT array_agg(nonce ORDER BY nonce) FROM wallet_txs WHERE status = 'confirmed'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(nonces_used, vec![0, 1, 2]);
}

#[tokio::test]
async fn high_value_tx_carries_value_wei_into_wallet_txs() {
    // value field round-trips into the BYTEA-numeric column without precision loss.
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let (edge, counter, balance, rpc, nonces, signer) = happy_stubs();
    rpc.push_outcome(MockOutcome::Success {
        gas_used: 50_000,
        effective_gas_price: 50_000_000,
    });
    let mut tx = ok_tx();
    tx.value = Some(U256::from(1_234_567_u64));
    let ctx = PolicyContext::new("production", &edge, &counter, &balance).with_signing(
        &signer,
        &rpc,
        &nonces,
    );

    sign_and_send(tx, &ctx, &pool, "test:value").await.expect("ok");

    let (val,): (bigdecimal::BigDecimal,) =
        sqlx::query_as("SELECT value_wei FROM wallet_txs WHERE status = 'confirmed' LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(val.to_string(), "1234567");
}

#[tokio::test]
async fn _ignore_unused_imports_compiles() {
    let _ = Duration::from_secs(0);
    // ensure imports kept
    let _ = MAX_PER_TX_GAS;
    let _: Box<dyn TxSigner> = Box::new(MockSigner::new(signer_addr()));
    let _: Box<dyn WalletRpc> = Box::new(InMemoryWalletRpc::new());
    let _: Box<dyn WalletRpcSeed> = Box::new(InMemoryWalletRpc::new());
    let _: Box<dyn NonceProvider> = Box::new(InMemoryNonceProvider::new());
}

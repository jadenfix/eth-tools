//! End-to-end wallet-rails integration test.
//!
//! Boots a real Postgres via testcontainers, runs every migration, and
//! exercises [`eth_tools_payments::wallet::sign_and_send`] against the full
//! rail chain with stubbed Edge Config / spend counter / balance provider.
//! Asserts that every rejection writes a `denials` row with the correct
//! `code` + `evaluator` + `details` JSONB shape.
//!
//! Skipped automatically if Docker is unavailable.

#![cfg(test)]

use alloy_primitives::{address, Address};
use eth_tools_payments::denials::count_by_code_evaluator;
use eth_tools_payments::edge_config::InMemoryEdgeConfig;
use eth_tools_payments::wallet::balance::InMemoryBalanceProvider;
use eth_tools_payments::wallet::daily_cap::InMemorySpendCounter;
use eth_tools_payments::wallet::policy::{PolicyContext, TransactionRequest};
use eth_tools_payments::wallet::{
    daily_spend_key, sign_and_send, ALLOWED_CHAIN_ID, ALLOWED_RECIPIENTS, KILL_SWITCH_KEY,
    MAX_BALANCE_USD_CENTS, MAX_DAILY_SPEND_USD_CENTS, MAX_PER_TX_GAS,
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

fn ok_tx() -> TransactionRequest {
    TransactionRequest::new(ALLOWED_CHAIN_ID, ALLOWED_RECIPIENTS[0], MAX_PER_TX_GAS - 1)
}

/// Helper that builds a wallet-rails context with every stub set to the
/// "everything is happy" configuration (kill switch on, balance under the
/// ceiling, counter empty).
fn happy_stubs() -> (InMemoryEdgeConfig, InMemorySpendCounter, InMemoryBalanceProvider) {
    (
        InMemoryEdgeConfig::with_bool(KILL_SWITCH_KEY, true),
        InMemorySpendCounter::new(),
        InMemoryBalanceProvider::new(u64::from(MAX_BALANCE_USD_CENTS) - 50),
    )
}

#[tokio::test]
async fn denial_row_for_kill_switch_off() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let edge = InMemoryEdgeConfig::with_bool(KILL_SWITCH_KEY, false);
    let counter = InMemorySpendCounter::new();
    let balance = InMemoryBalanceProvider::new(0);
    let ctx = PolicyContext::new("production", &edge, &counter, &balance);
    let err = sign_and_send(ok_tx(), &ctx, &pool, "test:kill_switch")
        .await
        .unwrap_err();
    assert_eq!(err.code, "KILL_SWITCH_ACTIVE");
    assert_eq!(err.evaluator, "wallet.kill");

    let n = count_by_code_evaluator(&pool, "KILL_SWITCH_ACTIVE", "wallet.kill")
        .await
        .unwrap();
    assert_eq!(n, 1, "must write exactly one denials row");

    // The counter must NOT have been touched (kill switch fires before
    // daily cap).
    assert!(counter.ops().is_empty());
}

#[tokio::test]
async fn denial_row_for_wrong_chain() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let (edge, counter, balance) = happy_stubs();
    let ctx = PolicyContext::new("production", &edge, &counter, &balance);
    let bad = TransactionRequest::new(1, ALLOWED_RECIPIENTS[0], 100_000);
    let err = sign_and_send(bad, &ctx, &pool, "test:wrong_chain")
        .await
        .unwrap_err();
    assert_eq!(err.code, "WRONG_CHAIN");
    assert_eq!(err.evaluator, "wallet.chain");

    let n = count_by_code_evaluator(&pool, "WRONG_CHAIN", "wallet.chain")
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn denial_row_for_bad_recipient() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let (edge, counter, balance) = happy_stubs();
    let ctx = PolicyContext::new("production", &edge, &counter, &balance);
    let attacker: Address = address!("0000000000000000000000000000000000000bad");
    let bad = TransactionRequest::new(ALLOWED_CHAIN_ID, attacker, 100_000);
    let err = sign_and_send(bad, &ctx, &pool, "test:bad_to").await.unwrap_err();
    assert_eq!(err.code, "RECIPIENT_NOT_ALLOWED");

    let n = count_by_code_evaluator(&pool, "RECIPIENT_NOT_ALLOWED", "wallet.allowlist")
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn denial_row_for_gas_too_high() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let (edge, counter, balance) = happy_stubs();
    let ctx = PolicyContext::new("production", &edge, &counter, &balance);
    let bad = TransactionRequest::new(ALLOWED_CHAIN_ID, ALLOWED_RECIPIENTS[0], MAX_PER_TX_GAS + 1);
    let err = sign_and_send(bad, &ctx, &pool, "test:gas").await.unwrap_err();
    assert_eq!(err.code, "GAS_TOO_HIGH");

    let n = count_by_code_evaluator(&pool, "GAS_TOO_HIGH", "wallet.gas")
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn denial_row_for_overfunded_balance() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let edge = InMemoryEdgeConfig::with_bool(KILL_SWITCH_KEY, true);
    let counter = InMemorySpendCounter::new();
    let balance = InMemoryBalanceProvider::new(u64::from(MAX_BALANCE_USD_CENTS) + 1);
    let ctx = PolicyContext::new("production", &edge, &counter, &balance);
    let err = sign_and_send(ok_tx(), &ctx, &pool, "test:overfunded")
        .await
        .unwrap_err();
    assert_eq!(err.code, "OVERFUNDED");
    assert_eq!(err.evaluator, "wallet.balance");

    let n = count_by_code_evaluator(&pool, "OVERFUNDED", "wallet.balance")
        .await
        .unwrap();
    assert_eq!(n, 1);

    // Counter must be untouched — balance rail runs before daily cap.
    assert!(counter.ops().is_empty());
}

#[tokio::test]
async fn denial_row_for_daily_cap_exceeded() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let edge = InMemoryEdgeConfig::with_bool(KILL_SWITCH_KEY, true);
    let key = daily_spend_key("production");
    let counter = InMemorySpendCounter::with_initial(&key, i64::from(MAX_DAILY_SPEND_USD_CENTS));
    let balance = InMemoryBalanceProvider::new(100);
    let ctx = PolicyContext::new("production", &edge, &counter, &balance);
    let err = sign_and_send(ok_tx(), &ctx, &pool, "test:daily_cap")
        .await
        .unwrap_err();
    assert_eq!(err.code, "DAILY_CAP");
    assert_eq!(err.evaluator, "wallet.cap");

    let n = count_by_code_evaluator(&pool, "DAILY_CAP", "wallet.cap")
        .await
        .unwrap();
    assert_eq!(n, 1);

    // CRITICAL: the counter must be exactly back at its pre-call value.
    assert_eq!(counter.value(&key), i64::from(MAX_DAILY_SPEND_USD_CENTS));
}

#[tokio::test]
async fn denial_row_for_non_prod_env_short_circuit() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let (edge, counter, balance) = happy_stubs();
    let mut ctx = PolicyContext::new("production", &edge, &counter, &balance);
    ctx.vercel_env = "preview";
    let err = sign_and_send(ok_tx(), &ctx, &pool, "test:env").await.unwrap_err();
    assert_eq!(err.code, "WALLET_DISABLED_NON_PROD");
    assert_eq!(err.evaluator, "wallet.env");

    let n = count_by_code_evaluator(&pool, "WALLET_DISABLED_NON_PROD", "wallet.env")
        .await
        .unwrap();
    assert_eq!(n, 1);

    // Nothing else — no Edge Config call, no counter touch.
    assert!(counter.ops().is_empty());
}

#[tokio::test]
async fn details_jsonb_carries_expected_shape() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let (edge, counter, balance) = happy_stubs();
    let ctx = PolicyContext::new("production", &edge, &counter, &balance);
    let bad = TransactionRequest::new(1, ALLOWED_RECIPIENTS[0], 100_000);
    sign_and_send(bad, &ctx, &pool, "test:details").await.unwrap_err();

    let (details,): (serde_json::Value,) = sqlx::query_as(
        "SELECT details FROM denials WHERE code = 'WRONG_CHAIN' AND evaluator = 'wallet.chain' ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    // The full `DeniedReason` is round-tripped into the JSONB column.
    assert_eq!(details["code"], "WRONG_CHAIN");
    assert_eq!(details["evaluator"], "wallet.chain");
    assert_eq!(details["got"]["chain_id"], 1);
    assert_eq!(details["expected"]["chain_id"], 8453);
}

#[tokio::test]
async fn request_path_is_recorded_verbatim() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let (edge, counter, balance) = happy_stubs();
    let ctx = PolicyContext::new("production", &edge, &counter, &balance);
    let bad = TransactionRequest::new(99999, ALLOWED_RECIPIENTS[0], 100_000);
    sign_and_send(bad, &ctx, &pool, "/api/v1/wallet/spend")
        .await
        .unwrap_err();

    let (path,): (String,) = sqlx::query_as(
        "SELECT request_path FROM denials WHERE code = 'WRONG_CHAIN' ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(path, "/api/v1/wallet/spend");
}

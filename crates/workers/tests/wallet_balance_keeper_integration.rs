//! Integration test for W8 — `wallet_balance_keeper` (balance + watchdog + cursor lag).
//!
//! Exercises the three concerns:
//!   1. Balance check with mock provider returning various wei → assert
//!      sweep queued / alert raised as expected.
//!   2. Watchdog: insert worker_runs rows with various ages → assert
//!      correct count + severity of `worker.stale` alerts.
//!   3. Cursor lag: insert cursors with various last_block values → assert
//!      `cursor.lag` alerts only fire for cursors > MAX_CURSOR_LAG_BLOCKS
//!      behind head.

#![cfg(test)]

use std::sync::Arc;

use alloy_primitives::U256;
use async_trait::async_trait;
use chrono::{Duration, Utc};
use eth_tools_db::cursors;
use eth_tools_rpc::{RotatingProvider, RpcError, RpcProvider};
use eth_tools_workers::context::WorkerDeps;
use eth_tools_workers::{wallet_balance_keeper, WorkerContext};
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

async fn boot() -> Option<(impl std::fmt::Debug, eth_tools_db::Pool)> {
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

async fn seed_chain(pool: &eth_tools_db::Pool) {
    sqlx::query(
        "INSERT INTO chains (chain_id, name, identity_registry, reputation_registry, validation_registry, rpc_url, is_testnet)
         VALUES (8453, 'base', '\\x00'::bytea, '\\x00'::bytea, '\\x00'::bytea, 'http://stub', false)
         ON CONFLICT DO NOTHING",
    )
    .execute(pool)
    .await
    .unwrap();
}

/// Mock provider whose balance/head responses are configurable per test.
struct MockProvider {
    balance_wei: U256,
    head: u64,
}

#[async_trait]
impl RpcProvider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }
    async fn get_block_number(&self) -> Result<u64, RpcError> {
        Ok(self.head)
    }
    async fn get_logs(
        &self,
        _f: &alloy::rpc::types::Filter,
    ) -> Result<Vec<alloy::rpc::types::Log>, RpcError> {
        Ok(vec![])
    }
    async fn get_balance(&self, _addr: alloy_primitives::Address) -> Result<U256, RpcError> {
        Ok(self.balance_wei)
    }
}

fn ctx(pool: eth_tools_db::Pool, balance_wei: U256, head: u64) -> WorkerContext {
    let rpc = Arc::new(RotatingProvider::new(vec![Arc::new(MockProvider {
        balance_wei,
        head,
    })]));
    let _ = WorkerDeps {
        pool: pool.clone(),
        rpc: rpc.clone(),
    };
    WorkerContext {
        pool,
        rpc,
        vercel_env: "production".into(),
        dryrun: false,
        force: false,
    }
}

async fn count_alerts(pool: &eth_tools_db::Pool, source: &str) -> i64 {
    let (c,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT FROM alerts WHERE source = $1",
    )
    .bind(source)
    .fetch_one(pool)
    .await
    .unwrap();
    c
}

async fn count_alerts_by_severity(pool: &eth_tools_db::Pool, source: &str, sev: &str) -> i64 {
    let (c,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT FROM alerts WHERE source = $1 AND severity = $2",
    )
    .bind(source)
    .bind(sev)
    .fetch_one(pool)
    .await
    .unwrap();
    c
}

#[tokio::test]
async fn w8_balance_alert_on_low() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    seed_chain(&pool).await;

    // Signer set, balance = 0.0001 ETH at $3500 = 35 cents < $1 floor.
    std::env::set_var("WALLET_SIGNER_ADDRESS", "0x0000000000000000000000000000000000000001");
    std::env::remove_var("ETH_USD_PRICE_CENTS");
    std::env::remove_var("WALLET_SAFE_ADDRESS");

    let dust = U256::from(100_000_000_000_000u128); // 1e14 wei = 0.0001 ETH
    let ctx_dust = ctx(pool.clone(), dust, 1_000_000);

    let summary = wallet_balance_keeper::run(&ctx_dust, 8453).await.expect("run");
    assert!(summary.ok);

    // One wallet.low alert; no sweep queued.
    assert_eq!(count_alerts(&pool, "wallet.low").await, 1);
    assert_eq!(count_alerts_by_severity(&pool, "wallet.low", "warn").await, 1);

    let (queued_sweeps,): (i64,) =
        sqlx::query_as("SELECT COUNT(*)::BIGINT FROM wallet_txs WHERE status = 'queued'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(queued_sweeps, 0, "no sweep for low balance");

    std::env::remove_var("WALLET_SIGNER_ADDRESS");
}

#[tokio::test]
async fn w8_balance_sweep_on_high() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    seed_chain(&pool).await;

    std::env::set_var("WALLET_SIGNER_ADDRESS", "0x0000000000000000000000000000000000000002");

    // 1 ETH at $3500 = 350_000 cents, well above the $5 sweep threshold.
    let rich = U256::from(1_000_000_000_000_000_000u128);
    let ctx_rich = ctx(pool.clone(), rich, 1_000_000);

    let summary = wallet_balance_keeper::run(&ctx_rich, 8453).await.expect("run");
    assert!(summary.ok);

    // One queued sweep tx; no wallet.low alert.
    let (queued,): (i64,) =
        sqlx::query_as("SELECT COUNT(*)::BIGINT FROM wallet_txs WHERE status = 'queued'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(queued, 1, "sweep queued");
    assert_eq!(count_alerts(&pool, "wallet.low").await, 0);

    std::env::remove_var("WALLET_SIGNER_ADDRESS");
}

#[tokio::test]
async fn w8_watchdog_alerts_on_stale_and_missing() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    seed_chain(&pool).await;
    // No signer configured → balance check skipped.
    std::env::remove_var("WALLET_SIGNER_ADDRESS");

    // registry_scraper: fresh ok=true 1 minute ago — healthy (cadence 2 → stale > 6).
    let fresh = Utc::now() - Duration::minutes(1);
    sqlx::query(
        "INSERT INTO worker_runs (worker_name, vercel_env, started_at, finished_at, ok, dryrun)
         VALUES ('registry_scraper', 'production', $1, $1, TRUE, FALSE)",
    )
    .bind(fresh)
    .execute(&pool)
    .await
    .unwrap();

    // manifest_fetcher: ok=true 1 hour ago — stale (cadence 15 → stale > 45 min).
    let stale = Utc::now() - Duration::minutes(60);
    sqlx::query(
        "INSERT INTO worker_runs (worker_name, vercel_env, started_at, finished_at, ok, dryrun)
         VALUES ('manifest_fetcher', 'production', $1, $1, TRUE, FALSE)",
    )
    .bind(stale)
    .execute(&pool)
    .await
    .unwrap();

    // endpoint_prober: ONLY ok=false runs — counts as "never had ok=true".
    sqlx::query(
        "INSERT INTO worker_runs (worker_name, vercel_env, started_at, finished_at, ok, error, dryrun)
         VALUES ('endpoint_prober', 'production', NOW(), NOW(), FALSE, 'boom', FALSE)",
    )
    .execute(&pool)
    .await
    .unwrap();

    // The other 4 workers have NO rows at all → also alerted.

    let c = ctx(pool.clone(), U256::ZERO, 1_000_000);
    let summary = wallet_balance_keeper::run(&c, 8453).await.expect("run");
    assert!(summary.ok);

    // Expected alerts: manifest_fetcher (stale) + endpoint_prober (never)
    // + 4 others (never) = 6.
    let stale_count = count_alerts(&pool, "worker.stale").await;
    assert_eq!(stale_count, 6, "stale + never-ok counts");
    // All worker.stale alerts must be severity=crit.
    assert_eq!(
        count_alerts_by_severity(&pool, "worker.stale", "crit").await,
        6
    );
    // No alert for the healthy one.
    let healthy_alerts: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT FROM alerts WHERE source = 'worker.stale'
          AND body->>'worker' = 'registry_scraper'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(healthy_alerts.0, 0, "fresh worker NOT alerted");
}

#[tokio::test]
async fn w8_cursor_lag_alerts() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    seed_chain(&pool).await;
    std::env::remove_var("WALLET_SIGNER_ADDRESS");

    // Three cursors. Head = 1_000_000.
    let mut conn = pool.acquire().await.unwrap();
    cursors::advance(&mut conn, "fresh", 999_950, 0).await.unwrap(); // lag 50, OK
    cursors::advance(&mut conn, "stale", 999_800, 0).await.unwrap(); // lag 200, ALERT
    cursors::advance(&mut conn, "ancient", 500_000, 0).await.unwrap(); // huge lag, ALERT
    drop(conn);

    let c = ctx(pool.clone(), U256::ZERO, 1_000_000);
    let _ = wallet_balance_keeper::run(&c, 8453).await.expect("run");

    let lag_alerts = count_alerts(&pool, "cursor.lag").await;
    assert_eq!(lag_alerts, 2, "two cursors > 100 blocks behind head");
    assert_eq!(count_alerts_by_severity(&pool, "cursor.lag", "warn").await, 2);
}

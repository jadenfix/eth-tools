//! Integration test: spin up a real Postgres via testcontainers, run all
//! migrations, load `fixtures/seed.sql`, exercise `agents::list/get_one`.
//!
//! Skipped automatically if Docker isn't available (testcontainers returns
//! an error which we treat as "skipped"). Locally requires `docker` running.

#![cfg(test)]

use bigdecimal::BigDecimal;
use std::str::FromStr;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

const SEED_SQL: &str = include_str!("../../../fixtures/seed.sql");

async fn boot() -> Option<(impl std::fmt::Debug, eth_tools_db::Pool)> {
    // Pin Postgres 16 — matches docker-compose.yml and gives us native
    // `gen_random_uuid()` (added to core in 13). Default tag could float
    // to an older image where `gen_random_uuid` requires pgcrypto.
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
    sqlx::raw_sql(SEED_SQL).execute(&pool).await.expect("seed");
    Some((container, pool))
}

#[tokio::test]
async fn list_returns_seeded_rows() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let rows = eth_tools_db::agents::list(
        &pool,
        eth_tools_db::agents::ListParams {
            chain_id: None,
            limit: 100,
            after: None,
        },
    )
    .await
    .expect("list");
    assert!(rows.len() >= 6, "got {} rows, expected ≥6", rows.len());
}

#[tokio::test]
async fn list_filtered_by_chain() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let base = eth_tools_db::agents::list(
        &pool,
        eth_tools_db::agents::ListParams {
            chain_id: Some(8453),
            limit: 100,
            after: None,
        },
    )
    .await
    .unwrap();
    let sepolia = eth_tools_db::agents::list(
        &pool,
        eth_tools_db::agents::ListParams {
            chain_id: Some(84532),
            limit: 100,
            after: None,
        },
    )
    .await
    .unwrap();
    assert!(base.iter().all(|r| r.chain_id == 8453));
    assert!(sepolia.iter().all(|r| r.chain_id == 84532));
    assert!(base.len() >= 4);
    assert!(sepolia.len() >= 2);
}

#[tokio::test]
async fn list_keyset_pagination_no_duplicates() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let mut all = Vec::new();
    let mut cursor: Option<eth_tools_db::agents::KeysetCursor> = None;
    loop {
        let page = eth_tools_db::agents::list(
            &pool,
            eth_tools_db::agents::ListParams {
                chain_id: None,
                limit: 2,
                after: cursor.clone(),
            },
        )
        .await
        .unwrap();
        if page.is_empty() {
            break;
        }
        let last = page.last().unwrap();
        cursor = Some(eth_tools_db::agents::KeysetCursor {
            updated_at: last.updated_at,
            chain_id: last.chain_id,
            agent_id: last.agent_id.clone(),
        });
        all.extend(page);
    }
    // No duplicates: every (chain, agent) pair appears exactly once.
    let mut keys: Vec<_> = all.iter().map(|r| (r.chain_id, r.agent_id.clone())).collect();
    keys.sort();
    let unique = keys.len();
    keys.dedup();
    assert_eq!(unique, keys.len(), "duplicates across pages");
    assert!(unique >= 6, "expected ≥6 distinct agents, got {unique}");
}

#[tokio::test]
async fn get_one_roundtrip() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let id = BigDecimal::from_str("42").unwrap();
    let row = eth_tools_db::agents::get_one(&pool, 8453, &id).await.unwrap();
    let row = row.expect("agent (8453, 42) must exist in seed");
    assert_eq!(row.chain_id, 8453);
    assert_eq!(row.agent_id, id);
    assert_eq!(
        row.agent_uri.as_deref(),
        Some("https://wallet-risk.example/agent.json")
    );

    // Non-existent → None, not error.
    let missing = eth_tools_db::agents::get_one(&pool, 8453, &BigDecimal::from_str("999999").unwrap())
        .await
        .unwrap();
    assert!(missing.is_none());
}

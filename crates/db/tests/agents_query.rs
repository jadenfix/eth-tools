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

/// Regression for the keyset filter: the previous tuple `<` form silently
/// dropped rows whose `(chain_id, agent_id)` was *smaller* than the cursor
/// on a tied `updated_at`, because Postgres tuple comparison is purely
/// lexicographic and has no mixed-direction semantics for
/// `ORDER BY updated_at DESC, chain_id ASC, agent_id ASC`.
///
/// We construct four rows that share an `updated_at` and `chain_id` and page
/// with `limit=1` — every row must be returned exactly once.
#[tokio::test]
async fn list_keyset_handles_tied_updated_at() {
    let Some((_c, pool)) = boot().await else {
        return;
    };

    // Use a fixed `updated_at` for all four rows on the same chain to force
    // the tie-breaker columns to do the work.
    let tied_at = chrono::Utc::now();
    sqlx::query(
        "INSERT INTO agents (chain_id, agent_id, owner, agent_uri, agent_wallet, registered_at, updated_at)
         VALUES
           (8453, 9001, decode('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','hex'), NULL, NULL, $1, $1),
           (8453, 9002, decode('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','hex'), NULL, NULL, $1, $1),
           (8453, 9003, decode('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','hex'), NULL, NULL, $1, $1),
           (8453, 9004, decode('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','hex'), NULL, NULL, $1, $1)",
    )
    .bind(tied_at)
    .execute(&pool)
    .await
    .expect("insert tied rows");

    // Page through ONLY the chain we just stuffed, with limit=1.
    let mut seen: Vec<BigDecimal> = Vec::new();
    let mut cursor: Option<eth_tools_db::agents::KeysetCursor> = None;
    for _ in 0..32 {
        let page = eth_tools_db::agents::list(
            &pool,
            eth_tools_db::agents::ListParams {
                chain_id: Some(8453),
                limit: 1,
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
        seen.extend(page.into_iter().map(|r| r.agent_id));
    }

    // All four of the synthetic agent_ids must appear exactly once. (Other
    // seeded base agents may also appear — we only assert presence of 9001-4
    // and no duplicates among them.)
    for id in ["9001", "9002", "9003", "9004"] {
        let want = BigDecimal::from_str(id).unwrap();
        let count = seen.iter().filter(|got| **got == want).count();
        assert_eq!(count, 1, "agent_id={id} expected exactly once, saw {count}");
    }
}

#[tokio::test]
async fn count_by_chain_matches_per_chain_count() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let grouped = eth_tools_db::agents::count_by_chain(&pool).await.unwrap();
    // Cross-check against the per-chain count() helper.
    for (chain_id, n) in &grouped {
        let direct = eth_tools_db::agents::count(&pool, Some(*chain_id)).await.unwrap();
        assert_eq!(direct, *n, "count_by_chain mismatch on chain {chain_id}");
    }
    // grouped should be ≤ 2 since seed only populates Base + Base Sepolia.
    assert!(grouped.len() <= eth_tools_core::CHAINS.len());
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

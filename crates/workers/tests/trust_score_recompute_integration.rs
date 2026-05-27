//! Integration test for W7 — `trust_score_recompute`.
//!
//! Seeds three agents with different signal mixes and verifies:
//!   - Agent A (good rep, good liveness, valid manifest, fresh)
//!     gets a high score.
//!   - Agent B (mid rep, no probes, invalid manifest) gets a low-mid score.
//!   - Agent C (no signals at all, only `agents` row) gets NO trust_scores row.
//!
//! Skipped automatically if Docker isn't available.

#![cfg(test)]

use bigdecimal::BigDecimal;
use eth_tools_db::trust_scores;
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

/// Seed `chains` so the FK from `agents` resolves. Returns the chain_id used.
async fn seed_chain(pool: &eth_tools_db::Pool) -> i64 {
    let chain_id: i64 = 8453;
    sqlx::query(
        "INSERT INTO chains (chain_id, name, identity_registry, reputation_registry, validation_registry, rpc_url, is_testnet)
         VALUES ($1, 'base', '\\x00'::bytea, '\\x00'::bytea, '\\x00'::bytea, 'http://stub', false)
         ON CONFLICT (chain_id) DO NOTHING",
    )
    .bind(chain_id)
    .execute(pool)
    .await
    .expect("seed chain");
    chain_id
}

async fn seed_agent(pool: &eth_tools_db::Pool, chain_id: i64, agent_id: i64, days_old: i64) {
    let id = BigDecimal::from(agent_id);
    let registered_at = chrono::Utc::now() - chrono::Duration::days(days_old);
    sqlx::query(
        "INSERT INTO agents (chain_id, agent_id, owner, agent_uri, agent_wallet, registered_at, updated_at)
         VALUES ($1, $2, '\\x01'::bytea, 'http://example/agent', NULL, $3, $3)",
    )
    .bind(chain_id)
    .bind(&id)
    .bind(registered_at)
    .execute(pool)
    .await
    .expect("seed agent");
}

async fn seed_feedback(
    pool: &eth_tools_db::Pool,
    chain_id: i64,
    agent_id: i64,
    value: i64,
    decimals: i16,
    feedback_index: i64,
) {
    let id = BigDecimal::from(agent_id);
    let value_bd = BigDecimal::from(value);
    sqlx::query(
        "INSERT INTO feedback
            (chain_id, agent_id, client_address, feedback_index, value, value_decimals,
             tag1, tag2, endpoint, feedback_uri, feedback_hash, is_revoked, tx_hash, block_number)
         VALUES ($1, $2, '\\x02'::bytea, $3, $4, $5, NULL, NULL, NULL, NULL, NULL, FALSE, '\\x03'::bytea, 1)",
    )
    .bind(chain_id)
    .bind(&id)
    .bind(feedback_index)
    .bind(value_bd)
    .bind(decimals)
    .execute(pool)
    .await
    .expect("seed feedback");
}

async fn seed_probe(pool: &eth_tools_db::Pool, chain_id: i64, agent_id: i64, ok: bool) {
    let id = BigDecimal::from(agent_id);
    sqlx::query(
        "INSERT INTO endpoint_probes
            (chain_id, agent_id, service_name, endpoint, probed_at, status_code, latency_ms, ok, error)
         VALUES ($1, $2, 'svc', 'http://x', NOW(), 200, 10, $3, NULL)",
    )
    .bind(chain_id)
    .bind(&id)
    .bind(ok)
    .execute(pool)
    .await
    .expect("seed probe");
}

async fn seed_manifest(pool: &eth_tools_db::Pool, chain_id: i64, agent_id: i64, status: &str) {
    let id = BigDecimal::from(agent_id);
    let sha = vec![agent_id as u8; 32];
    sqlx::query(
        "INSERT INTO manifests
            (chain_id, agent_id, fetched_at, source_uri, raw_bytes_sha256, raw_keccak256, parsed, validation_status, validation_errors, blob_url)
         VALUES ($1, $2, NOW(), 'http://x', $3, $3, NULL, $4, NULL, NULL)",
    )
    .bind(chain_id)
    .bind(&id)
    .bind(&sha)
    .bind(status)
    .execute(pool)
    .await
    .expect("seed manifest");
}

#[tokio::test]
async fn w7_recompute_writes_scores_with_correct_breakdown() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let chain_id = seed_chain(&pool).await;

    // Agent A — strong signals across the board.
    //   reputation: value=80, decimals=0 -> 80 → (80+100)/2 = 90
    //   liveness:   2/2 ok = 100
    //   manifest:   valid = 100
    //   recency:    180 days → ln(181)/ln(366)*100 ≈ 88.04
    //   composite:  0.4*90 + 0.3*100 + 0.2*100 + 0.1*88 ≈ 94.8 → 95
    seed_agent(&pool, chain_id, 1, 180).await;
    seed_feedback(&pool, chain_id, 1, 80, 0, 0).await;
    seed_probe(&pool, chain_id, 1, true).await;
    seed_probe(&pool, chain_id, 1, true).await;
    seed_manifest(&pool, chain_id, 1, "valid").await;

    // Agent B — middling signals.
    //   reputation: value=0 -> (0+100)/2 = 50
    //   liveness:   1/2 = 50
    //   manifest:   invalid = 0
    //   recency:    30 days → ln(31)/ln(366) * 100 ≈ 58.18
    //   composite:  0.4*50 + 0.3*50 + 0.2*0 + 0.1*58 ≈ 40.8 → 41
    seed_agent(&pool, chain_id, 2, 30).await;
    seed_feedback(&pool, chain_id, 2, 0, 0, 0).await;
    seed_probe(&pool, chain_id, 2, true).await;
    seed_probe(&pool, chain_id, 2, false).await;
    seed_manifest(&pool, chain_id, 2, "invalid").await;

    // Agent C — zero signals. Should NOT get a trust_scores row.
    seed_agent(&pool, chain_id, 3, 5).await;

    // Run recompute.
    let rows = trust_scores::recompute_v1(&pool).await.expect("recompute");
    assert_eq!(rows, 2, "exactly 2 scored agents (C is excluded for no signals)");

    // Verify A.
    let row_a: (i32, serde_json::Value) = sqlx::query_as(
        "SELECT score, components FROM trust_scores
          WHERE chain_id = $1 AND agent_id = 1 AND scorer_version = 'v1'",
    )
    .bind(chain_id)
    .fetch_one(&pool)
    .await
    .expect("fetch A");
    assert!(
        row_a.0 >= 90 && row_a.0 <= 100,
        "agent A score should be ≥ 90, got {}",
        row_a.0
    );
    let comp_a = row_a.1.as_object().expect("components obj");
    assert!(comp_a.contains_key("reputation_pct"));
    assert!(comp_a.contains_key("liveness_pct"));
    assert!(comp_a.contains_key("manifest_pct"));
    assert!(comp_a.contains_key("recency_pct"));
    assert!(comp_a.contains_key("weights"));
    assert_eq!(
        comp_a["manifest_pct"].as_f64().unwrap(),
        100.0,
        "A manifest valid"
    );

    // Verify B.
    let row_b: (i32, serde_json::Value) = sqlx::query_as(
        "SELECT score, components FROM trust_scores
          WHERE chain_id = $1 AND agent_id = 2 AND scorer_version = 'v1'",
    )
    .bind(chain_id)
    .fetch_one(&pool)
    .await
    .expect("fetch B");
    assert!(
        row_b.0 >= 30 && row_b.0 <= 55,
        "agent B mid-tier, got {}",
        row_b.0
    );
    let comp_b = row_b.1.as_object().unwrap();
    assert_eq!(comp_b["manifest_pct"].as_f64().unwrap(), 0.0, "B manifest invalid");
    assert!(comp_b["liveness_pct"].as_f64().unwrap() > 40.0); // ~50%
    assert!(comp_b["liveness_pct"].as_f64().unwrap() < 60.0);

    // Verify C absent.
    let count_c: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT FROM trust_scores WHERE chain_id = $1 AND agent_id = 3",
    )
    .bind(chain_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count_c.0, 0, "zero-signal agent C should NOT be scored");

    // Run again — should be idempotent (UPDATE not duplicate).
    let rows2 = trust_scores::recompute_v1(&pool).await.expect("recompute 2");
    assert_eq!(rows2, 2, "second pass still writes 2 rows (UPSERT)");
    let total: (i64,) = sqlx::query_as("SELECT COUNT(*)::BIGINT FROM trust_scores")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(total.0, 2, "no duplicates after second run");

    // Revoke A's feedback → its reputation drops; score moves accordingly.
    sqlx::query("UPDATE feedback SET is_revoked = TRUE WHERE agent_id = 1")
        .execute(&pool)
        .await
        .unwrap();
    trust_scores::recompute_v1(&pool).await.unwrap();
    let row_a2: (i32, serde_json::Value) = sqlx::query_as(
        "SELECT score, components FROM trust_scores
          WHERE chain_id = $1 AND agent_id = 1 AND scorer_version = 'v1'",
    )
    .bind(chain_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    // With reputation removed, raw_avg defaults to 0, reputation_pct = 0.
    // Composite drops to 0.3*100 + 0.2*100 + 0.1*88 ≈ 58.8 → 59.
    assert!(row_a2.0 < row_a.0, "score must drop after revoking feedback");
    assert_eq!(row_a2.1["reputation_pct"].as_f64().unwrap(), 0.0);
}

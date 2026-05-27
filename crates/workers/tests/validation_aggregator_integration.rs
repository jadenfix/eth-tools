//! End-to-end test for W5 (`validation_aggregator::run`).
//!
//! Covers (plan §3 row W5):
//!   1. ValidationRequest INSERT (with response columns NULL).
//!   2. ValidationResponse upsert (response/uri/hash/tag all populated).
//!   3. Latest-status-only: a second Response overwrites the row in place.
//!   4. Idempotency: re-running over the same range yields no new rows.
//!   5. Reorg safety: to_block never exceeds head - REORG_SAFETY_BLOCKS.
//!
//! Skipped if Docker is unavailable.

#![cfg(test)]

mod common;

use std::sync::Arc;

use alloy::primitives::{address, b256, Address, U256};
use bigdecimal::BigDecimal;
use eth_tools_core::chains;
use eth_tools_core::events::{VALIDATION_REQUEST_TOPIC, VALIDATION_RESPONSE_TOPIC};
use eth_tools_db::cursors;
use eth_tools_rpc::REORG_SAFETY_BLOCKS;
use eth_tools_workers::validation_aggregator;

use crate::common::{
    boot_pg, ctx_for, validation_request_log, validation_response_log, MockProvider,
};

const CHAIN_ID: u64 = 8453;

fn val_addr() -> Address {
    Address::from(chains::by_id(CHAIN_ID).unwrap().validation_registry)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn w5_validation_aggregator_e2e() {
    let Some((_c, pool)) = boot_pg().await else {
        return;
    };

    // Above Base genesis (worker clamps from_block to genesis floor).
    let genesis = chains::by_id(CHAIN_ID).unwrap().genesis_block;
    let head = genesis + 250;
    let target = head - REORG_SAFETY_BLOCKS;
    let seed_from = target.saturating_sub(100);

    // Seed cursors.
    {
        let mut conn = pool.acquire().await.unwrap();
        for topic in [VALIDATION_REQUEST_TOPIC, VALIDATION_RESPONSE_TOPIC] {
            let key = cursors::cursor_key(
                CHAIN_ID as i64,
                &val_addr().into_array(),
                &topic.0,
            );
            cursors::advance(&mut conn, &key, seed_from as i64 - 1, 0)
                .await
                .unwrap();
        }
    }

    let validator = address!("0x000000000000000000000000000000000000DEAD");
    let agent_a = U256::from(7);
    let req_hash = b256!("0x4242424242424242424242424242424242424242424242424242424242424242");
    let resp_hash = b256!("0x5252525252525252525252525252525252525252525252525252525252525252");
    let tx1 = b256!("0xabababababababababababababababababababababababababababababababab");

    let vreq = validation_request_log(
        val_addr(),
        validator,
        agent_a,
        "ipfs://req/1",
        req_hash,
        target - 3,
        0,
        tx1,
    );
    let vresp_v1 = validation_response_log(
        val_addr(),
        validator,
        agent_a,
        req_hash,
        77,
        "ipfs://resp/1",
        resp_hash,
        "audit-pass",
        target - 2,
        0,
        tx1,
    );

    // ----- Phase 1: request + response in same tick ---------------------
    let mock = Arc::new(MockProvider::new("primary"));
    mock.push_block_number(Ok(head));
    mock.push_logs(Ok(vec![vreq.clone()])); // ValidationRequest page
    mock.push_logs(Ok(vec![vresp_v1.clone()])); // ValidationResponse page

    let ctx = ctx_for(pool.clone(), mock);
    let summary = validation_aggregator::run(&ctx, CHAIN_ID).await.expect("run");
    assert!(summary.ok);
    assert_eq!(summary.rows_in, 2);
    assert_eq!(summary.rows_out, 2);

    // Verify row state.
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*)::BIGINT FROM validations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "request_hash PK collapses Request+Response to 1 row");
    let (val_addr_db, agent_db, request_uri, response, tag): (
        Vec<u8>,
        BigDecimal,
        String,
        Option<i16>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT validator_addr, agent_id, request_uri, response, tag
           FROM validations WHERE chain_id = $1 AND request_hash = $2",
    )
    .bind(CHAIN_ID as i64)
    .bind(req_hash.0.to_vec())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(val_addr_db, validator.0.to_vec());
    assert_eq!(agent_db, BigDecimal::from(7));
    assert_eq!(request_uri, "ipfs://req/1");
    assert_eq!(response, Some(77));
    assert_eq!(tag.as_deref(), Some("audit-pass"));

    // Cursors at `target`.
    {
        let mut conn = pool.acquire().await.unwrap();
        for topic in [VALIDATION_REQUEST_TOPIC, VALIDATION_RESPONSE_TOPIC] {
            let key = cursors::cursor_key(
                CHAIN_ID as i64,
                &val_addr().into_array(),
                &topic.0,
            );
            let c = cursors::get(&mut conn, &key).await.unwrap().unwrap();
            assert_eq!(c.last_block as u64, target);
        }
    }

    // ----- Phase 2: idempotency (replay same range, no new rows) ---------
    {
        let mut conn = pool.acquire().await.unwrap();
        for topic in [VALIDATION_REQUEST_TOPIC, VALIDATION_RESPONSE_TOPIC] {
            let key = cursors::cursor_key(
                CHAIN_ID as i64,
                &val_addr().into_array(),
                &topic.0,
            );
            cursors::advance(&mut conn, &key, seed_from as i64 - 1, 0)
                .await
                .unwrap();
        }
    }
    let mock2 = Arc::new(MockProvider::new("primary"));
    mock2.push_block_number(Ok(head));
    mock2.push_logs(Ok(vec![vreq])); // same request
    mock2.push_logs(Ok(vec![vresp_v1])); // same response
    let ctx2 = ctx_for(pool.clone(), mock2);
    let _ = validation_aggregator::run(&ctx2, CHAIN_ID).await.expect("idem");
    let (count2,): (i64,) = sqlx::query_as("SELECT COUNT(*)::BIGINT FROM validations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count2, 1, "no duplicate rows on replay");

    // ----- Phase 3: latest-status-only — second response overwrites ------
    let head2 = genesis + 400;
    let target2 = head2 - REORG_SAFETY_BLOCKS;
    let resp_hash2 = b256!("0x6363636363636363636363636363636363636363636363636363636363636363");
    let vresp_v2 = validation_response_log(
        val_addr(),
        validator,
        agent_a,
        req_hash,
        42, // different score
        "ipfs://resp/2",
        resp_hash2,
        "audit-refined",
        target2 - 3,
        0,
        tx1,
    );
    {
        let mut conn = pool.acquire().await.unwrap();
        for topic in [VALIDATION_REQUEST_TOPIC, VALIDATION_RESPONSE_TOPIC] {
            let key = cursors::cursor_key(
                CHAIN_ID as i64,
                &val_addr().into_array(),
                &topic.0,
            );
            cursors::advance(&mut conn, &key, target as i64, 0).await.unwrap();
        }
    }
    let mock3 = Arc::new(MockProvider::new("primary"));
    mock3.push_block_number(Ok(head2));
    mock3.push_logs(Ok(vec![])); // no new requests
    mock3.push_logs(Ok(vec![vresp_v2]));
    let ctx3 = ctx_for(pool.clone(), mock3);
    let _ = validation_aggregator::run(&ctx3, CHAIN_ID).await.expect("overwrite");

    let (response2, tag2, resp_hash_db): (Option<i16>, Option<String>, Option<Vec<u8>>) =
        sqlx::query_as(
            "SELECT response, tag, response_hash FROM validations
              WHERE chain_id = $1 AND request_hash = $2",
        )
        .bind(CHAIN_ID as i64)
        .bind(req_hash.0.to_vec())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(response2, Some(42), "latest response overwrites");
    assert_eq!(tag2.as_deref(), Some("audit-refined"));
    assert_eq!(resp_hash_db.as_deref(), Some(&resp_hash2.0[..]));
    let (final_count,): (i64,) = sqlx::query_as("SELECT COUNT(*)::BIGINT FROM validations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(final_count, 1, "still one row (latest-status-only)");

    // ----- Phase 4: reorg safety -----------------------------------------
    let head3 = genesis + 600;
    let target3 = head3 - REORG_SAFETY_BLOCKS;
    {
        let mut conn = pool.acquire().await.unwrap();
        for topic in [VALIDATION_REQUEST_TOPIC, VALIDATION_RESPONSE_TOPIC] {
            let key = cursors::cursor_key(
                CHAIN_ID as i64,
                &val_addr().into_array(),
                &topic.0,
            );
            cursors::advance(&mut conn, &key, target2 as i64, 0).await.unwrap();
        }
    }
    let mock4 = Arc::new(MockProvider::new("primary"));
    mock4.push_block_number(Ok(head3));
    mock4.push_logs(Ok(vec![]));
    mock4.push_logs(Ok(vec![]));
    let ctx4 = ctx_for(pool.clone(), mock4.clone());
    let _ = validation_aggregator::run(&ctx4, CHAIN_ID).await.expect("reorg-safe");
    for f in mock4.get_logs_calls_snapshot() {
        let to_block = f.get_to_block().expect("to_block");
        assert!(
            to_block <= target3,
            "scraped to_block {to_block} > target {target3} (within reorg buffer)"
        );
    }
}

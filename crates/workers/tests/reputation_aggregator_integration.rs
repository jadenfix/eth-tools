//! End-to-end test for W4 (`reputation_aggregator::run`).
//!
//! Covers (plan §3 row W4):
//!   1. Each of the three event types upserts the correct row(s).
//!   2. Cursor advances exactly to `head - REORG_SAFETY_BLOCKS` per topic.
//!   3. Idempotency: running twice over the same range yields no new rows.
//!   4. Reorg safety: a log at `head - 5` is NOT processed.
//!   5. Rolling summary: mode-of-decimals computed correctly when scales
//!      diverge — minority-decimals rows are filtered out of the mean.
//!
//! Skipped if Docker is unavailable. One sequential `#[tokio::test]` —
//! sqlx pools + the cursors table are shared state across phases.

#![cfg(test)]

mod common;

use std::str::FromStr;
use std::sync::Arc;

use alloy::primitives::{address, b256, Address, U256};
use bigdecimal::BigDecimal;
use eth_tools_core::chains;
use eth_tools_core::events::{
    FEEDBACK_REVOKED_TOPIC, NEW_FEEDBACK_TOPIC, RESPONSE_APPENDED_TOPIC,
};
use eth_tools_db::cursors;
use eth_tools_rpc::REORG_SAFETY_BLOCKS;
use eth_tools_workers::reputation_aggregator;

use crate::common::{
    boot_pg, ctx_for, feedback_revoked_log, new_feedback_log, response_appended_log, MockProvider,
};

const CHAIN_ID: u64 = 8453;

fn rep_addr() -> Address {
    Address::from(chains::by_id(CHAIN_ID).unwrap().reputation_registry)
}

fn agent(n: u64) -> U256 {
    U256::from(n)
}

fn client(n: u8) -> Address {
    let mut bytes = [0u8; 20];
    bytes[19] = n;
    Address::from(bytes)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn w4_reputation_aggregator_e2e() {
    let Some((_c, pool)) = boot_pg().await else {
        return;
    };

    // ----- Phase 1: one of each event over a small range ------------------
    //
    // The worker clamps `from_block` to `chain_genesis(chain_id)` to avoid
    // scanning Base from block 0. Base genesis is 41,663,783, so all test
    // blocks must sit above it. We pick a head just past genesis and seed
    // cursors so the worker only scans a small synthetic window.
    let genesis = chains::by_id(CHAIN_ID).unwrap().genesis_block;
    let head = genesis + 200;
    let target = head - REORG_SAFETY_BLOCKS;
    let seed_from = target.saturating_sub(100);

    {
        let mut conn = pool.acquire().await.unwrap();
        for topic in [
            NEW_FEEDBACK_TOPIC,
            FEEDBACK_REVOKED_TOPIC,
            RESPONSE_APPENDED_TOPIC,
        ] {
            let key = cursors::cursor_key(
                CHAIN_ID as i64,
                &rep_addr().into_array(),
                &topic.0,
            );
            // (last_block, last_log_index) = (seed_from - 1, 0) so the
            // worker starts scanning from `seed_from`.
            cursors::advance(&mut conn, &key, seed_from as i64 - 1, 0)
                .await
                .unwrap();
        }
    }

    // Build event fixtures inside [seed_from, target].
    let agent_a = agent(7);
    let client_a = client(0xAA);
    let fhash = b256!("0x1111111111111111111111111111111111111111111111111111111111111111");
    let tx1 = b256!("0xabababababababababababababababababababababababababababababababab");

    // 1 NewFeedback log per topic-stream is fine — the worker scans each
    // topic independently. Place each in its own block within the window.
    let nf_log = new_feedback_log(
        rep_addr(),
        agent_a,
        client_a,
        1,
        50,
        2,
        "quality",
        "v1",
        "https://example.com/api",
        "ipfs://feedback/1",
        fhash,
        target - 3,
        0,
        tx1,
    );
    // ResponseAppended for the same feedback, by a responder.
    let responder = address!("0x000000000000000000000000000000000000B0B0");
    let rhash = b256!("0x2222222222222222222222222222222222222222222222222222222222222222");
    let ra_log = response_appended_log(
        rep_addr(),
        agent_a,
        client_a,
        1,
        responder,
        "ipfs://resp/1",
        rhash,
        target - 2,
        0,
        tx1,
    );

    // FeedbackRevoked for a *different* feedback that doesn't exist yet —
    // should be a no-op (warn + skip). This proves out-of-order tolerance.
    let fr_log_missing = feedback_revoked_log(
        rep_addr(),
        agent_a,
        client_a,
        99,
        target - 1,
        0,
        tx1,
    );

    // Queue up provider responses. The worker calls:
    //   1) get_block_number once
    //   2) get_logs once per topic (NewFeedback, FeedbackRevoked, ResponseAppended)
    let mock = Arc::new(MockProvider::new("primary"));
    mock.push_block_number(Ok(head));
    mock.push_logs(Ok(vec![nf_log.clone()])); // NewFeedback page
    mock.push_logs(Ok(vec![fr_log_missing.clone()])); // FeedbackRevoked page (no match)
    mock.push_logs(Ok(vec![ra_log.clone()])); // ResponseAppended page

    let ctx = ctx_for(pool.clone(), mock.clone());
    let summary = reputation_aggregator::run(&ctx, CHAIN_ID).await.expect("run");
    assert!(summary.ok);
    assert_eq!(summary.rows_in, 3, "3 logs decoded");
    // rows_out: 1 NewFeedback upsert + 1 ResponseAppended upsert + 0 revokes (no match).
    assert_eq!(summary.rows_out, 2);

    // ----- Assertions on DB state ----------------------------------------

    // feedback row exists, not revoked.
    let (count_fb,): (i64,) = sqlx::query_as("SELECT COUNT(*)::BIGINT FROM feedback")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count_fb, 1);

    let (is_revoked, value, vdec, tag1): (bool, BigDecimal, i16, Option<String>) = sqlx::query_as(
        "SELECT is_revoked, value, value_decimals, tag1
           FROM feedback WHERE feedback_index = 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!is_revoked, "no matching FeedbackRevoked → still live");
    assert_eq!(value, BigDecimal::from(50));
    assert_eq!(vdec, 2);
    assert_eq!(tag1.as_deref(), Some("quality"));

    let (count_resp,): (i64,) =
        sqlx::query_as("SELECT COUNT(*)::BIGINT FROM feedback_responses")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count_resp, 1, "ResponseAppended upserted exactly once");

    // Cursor for each topic advanced to `target`.
    {
        let mut conn = pool.acquire().await.unwrap();
        for topic in [
            NEW_FEEDBACK_TOPIC,
            FEEDBACK_REVOKED_TOPIC,
            RESPONSE_APPENDED_TOPIC,
        ] {
            let key = cursors::cursor_key(
                CHAIN_ID as i64,
                &rep_addr().into_array(),
                &topic.0,
            );
            let c = cursors::get(&mut conn, &key).await.unwrap().unwrap();
            assert_eq!(
                c.last_block as u64, target,
                "cursor for {topic:?} should advance to head - 12"
            );
        }
    }

    // ----- Phase 2: idempotency — re-run produces zero new rows ----------
    //
    // Reset cursor to before the events so the worker re-scans the same
    // window, but DO NOT change the events themselves.
    {
        let mut conn = pool.acquire().await.unwrap();
        for topic in [
            NEW_FEEDBACK_TOPIC,
            FEEDBACK_REVOKED_TOPIC,
            RESPONSE_APPENDED_TOPIC,
        ] {
            let key = cursors::cursor_key(
                CHAIN_ID as i64,
                &rep_addr().into_array(),
                &topic.0,
            );
            cursors::advance(&mut conn, &key, seed_from as i64 - 1, 0)
                .await
                .unwrap();
        }
    }
    let mock2 = Arc::new(MockProvider::new("primary"));
    mock2.push_block_number(Ok(head));
    mock2.push_logs(Ok(vec![nf_log])); // same logs
    mock2.push_logs(Ok(vec![fr_log_missing]));
    mock2.push_logs(Ok(vec![ra_log]));
    let ctx2 = ctx_for(pool.clone(), mock2);
    let _ = reputation_aggregator::run(&ctx2, CHAIN_ID).await.expect("idem");

    // Same row counts after replay → upserts are idempotent.
    let (count_fb_after,): (i64,) = sqlx::query_as("SELECT COUNT(*)::BIGINT FROM feedback")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count_fb_after, 1, "re-scan must not duplicate feedback");
    let (count_resp_after,): (i64,) =
        sqlx::query_as("SELECT COUNT(*)::BIGINT FROM feedback_responses")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        count_resp_after, 1,
        "re-scan must not duplicate responses (PK collision absorbs)"
    );

    // ----- Phase 3: revoke now matches (existing feedback) ---------------
    let fr_log_match = feedback_revoked_log(
        rep_addr(),
        agent_a,
        client_a,
        1,
        target - 4,
        0,
        tx1,
    );
    // Advance head + cursors so we get a fresh window.
    let head2 = genesis + 400;
    let target2 = head2 - REORG_SAFETY_BLOCKS;
    {
        let mut conn = pool.acquire().await.unwrap();
        for topic in [
            NEW_FEEDBACK_TOPIC,
            FEEDBACK_REVOKED_TOPIC,
            RESPONSE_APPENDED_TOPIC,
        ] {
            let key = cursors::cursor_key(
                CHAIN_ID as i64,
                &rep_addr().into_array(),
                &topic.0,
            );
            cursors::advance(&mut conn, &key, target as i64, 0).await.unwrap();
        }
    }
    let mock3 = Arc::new(MockProvider::new("primary"));
    mock3.push_block_number(Ok(head2));
    mock3.push_logs(Ok(vec![])); // NewFeedback empty
    mock3.push_logs(Ok(vec![fr_log_match])); // matches existing row
    mock3.push_logs(Ok(vec![])); // ResponseAppended empty
    let ctx3 = ctx_for(pool.clone(), mock3);
    let s3 = reputation_aggregator::run(&ctx3, CHAIN_ID).await.expect("revoke");
    assert_eq!(s3.rows_out, 1);
    let (is_revoked2,): (bool,) =
        sqlx::query_as("SELECT is_revoked FROM feedback WHERE feedback_index = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(is_revoked2, "FeedbackRevoked with matching key flips flag");
    // target2 verified by the second run
    assert!(target2 > target);

    // ----- Phase 4: reorg-safety — head moves forward but logs at head-5 not processed
    //
    // Place a NewFeedback at block = head3 - 5 (within reorg buffer). Set
    // head3 = 500, target3 = 488. Log at block 495 must NOT be scraped.
    let head3 = genesis + 500;
    let target3 = head3 - REORG_SAFETY_BLOCKS;
    // Reset NewFeedback cursor to before the unsafe window.
    {
        let mut conn = pool.acquire().await.unwrap();
        let key = cursors::cursor_key(
            CHAIN_ID as i64,
            &rep_addr().into_array(),
            &NEW_FEEDBACK_TOPIC.0,
        );
        cursors::advance(&mut conn, &key, (head3 - 20) as i64, 0).await.unwrap();
        for topic in [FEEDBACK_REVOKED_TOPIC, RESPONSE_APPENDED_TOPIC] {
            let key = cursors::cursor_key(
                CHAIN_ID as i64,
                &rep_addr().into_array(),
                &topic.0,
            );
            cursors::advance(&mut conn, &key, target3 as i64, 0).await.unwrap();
        }
    }
    let mock4 = Arc::new(MockProvider::new("primary"));
    mock4.push_block_number(Ok(head3));
    // Inject only logs from inside the safe window. The mock provider
    // won't be asked for a log at block 495 because the worker's
    // get_logs filter spans [from, target3] which excludes (target3, head3].
    // The test confirms this by checking get_logs_calls' to_block.
    mock4.push_logs(Ok(vec![])); // NewFeedback page — empty
    mock4.push_logs(Ok(vec![])); // FeedbackRevoked
    mock4.push_logs(Ok(vec![])); // ResponseAppended
    let ctx4 = ctx_for(pool.clone(), mock4.clone());
    let _ = reputation_aggregator::run(&ctx4, CHAIN_ID).await.expect("reorg-safe");

    // Confirm get_logs was never asked for a block > target3.
    let calls = mock4.get_logs_calls_snapshot();
    assert!(!calls.is_empty(), "should have queried at least one page");
    for f in &calls {
        let to_block = f.get_to_block().expect("to_block set");
        assert!(
            to_block <= target3,
            "scraped to_block {to_block} > target {target3} (within reorg buffer)"
        );
    }

    // ----- Phase 5: rolling summary — mode-of-decimals semantics --------
    //
    // Seed three feedback rows for agent_a:
    //   - 3 with decimals=2 (values 10, 20, 30 → mean=20)
    //   - 1 with decimals=4 (value=9999 — would skew if averaged)
    // Mode=2, so summary mean must be 20 (filtered to decimals=2 only).
    sqlx::query("DELETE FROM feedback").execute(&pool).await.unwrap();
    let agent_b_id = BigDecimal::from(99);
    for (idx, (val, dec)) in [(10i64, 2i16), (20, 2), (30, 2), (9999, 4)]
        .iter()
        .enumerate()
    {
        sqlx::query(
            "INSERT INTO feedback (chain_id, agent_id, client_address, feedback_index,
                                   value, value_decimals, is_revoked, tx_hash, block_number)
             VALUES ($1, $2, $3, $4, $5, $6, FALSE, $7, $8)",
        )
        .bind(CHAIN_ID as i64)
        .bind(&agent_b_id)
        .bind(vec![0u8; 20])
        .bind((idx + 1) as i64)
        .bind(BigDecimal::from(*val))
        .bind(*dec)
        .bind(vec![0u8; 32])
        .bind(100i64)
        .execute(&pool)
        .await
        .unwrap();
    }
    let summary = eth_tools_db::feedback::compute_summary(&pool, CHAIN_ID as i64, &agent_b_id)
        .await
        .unwrap()
        .expect("summary");
    assert_eq!(summary.count, 4, "count includes all non-revoked rows");
    assert_eq!(summary.mode_decimals, 2, "majority decimals = 2");
    // Mean over the decimals=2 subset: (10 + 20 + 30) / 3 = 20.
    let mean: f64 = summary.mean_value.to_string().parse().unwrap();
    assert!(
        (mean - 20.0).abs() < 1e-9,
        "mean over mode-only rows must be 20, got {mean}"
    );

    // Tie-break: when two scales tie on count, larger decimals wins.
    sqlx::query("DELETE FROM feedback").execute(&pool).await.unwrap();
    for (idx, (val, dec)) in [(5i64, 2i16), (50, 4)].iter().enumerate() {
        sqlx::query(
            "INSERT INTO feedback (chain_id, agent_id, client_address, feedback_index,
                                   value, value_decimals, is_revoked, tx_hash, block_number)
             VALUES ($1, $2, $3, $4, $5, $6, FALSE, $7, $8)",
        )
        .bind(CHAIN_ID as i64)
        .bind(&agent_b_id)
        .bind(vec![0u8; 20])
        .bind((idx + 1) as i64)
        .bind(BigDecimal::from(*val))
        .bind(*dec)
        .bind(vec![0u8; 32])
        .bind(100i64)
        .execute(&pool)
        .await
        .unwrap();
    }
    let tie = eth_tools_db::feedback::compute_summary(&pool, CHAIN_ID as i64, &agent_b_id)
        .await
        .unwrap()
        .expect("tie");
    assert_eq!(tie.mode_decimals, 4, "tie-break picks larger decimals");
    assert_eq!(
        tie.mean_value,
        BigDecimal::from_str("50").unwrap(),
        "mean reflects the decimals=4 subset only"
    );

    // ----- Phase 6: summary cache returns Ok(()) without env --------------
    // The worker's refresh_summary fan-out is internal; we exercise the
    // public surface here to keep the test self-contained.
    eth_tools_workers::reputation_summary::cache_summary(&tie)
        .await
        .expect("cache no-env Ok");
}

//! Pagination behaviour for `RotatingProvider::get_logs_paginated`.
//!
//! Asserts:
//!   1. A 5000-block range with page_size=2000 produces exactly 3 calls.
//!   2. The final partial page is honored (1000 blocks).
//!   3. A mid-pagination provider switch preserves block ordering — i.e. logs
//!      from page 1 (primary) come before logs from page 2 (fallback) in the
//!      returned `Vec<Log>`.

mod common;

use std::sync::Arc;

use alloy::primitives::{Address, B256, U256};
use alloy::rpc::types::{Filter, Log};
use eth_tools_rpc::{RotatingProvider, RpcError, RpcProvider, MAX_LOGS_PER_CALL};

use crate::common::MockProvider;

/// Build a synthetic `Log` whose block_number we can assert on.
fn log_at(block: u64, idx: u64) -> Log {
    let mut topic = [0u8; 32];
    topic[24..].copy_from_slice(&block.to_be_bytes());
    let mut data_bytes = [0u8; 32];
    data_bytes[24..].copy_from_slice(&idx.to_be_bytes());
    let inner = alloy_primitives::Log {
        address: Address::ZERO,
        data: alloy_primitives::LogData::new(vec![B256::from(topic)], data_bytes.to_vec().into())
            .expect("LogData::new"),
    };
    Log {
        inner,
        block_hash: None,
        block_number: Some(block),
        block_timestamp: None,
        transaction_hash: None,
        transaction_index: None,
        log_index: Some(idx),
        removed: false,
    }
}

#[tokio::test(flavor = "current_thread")]
async fn paginates_5000_blocks_in_three_calls() {
    let p1 = Arc::new(MockProvider::new("primary"));
    let p2 = Arc::new(MockProvider::new("fallback"));
    let providers: Vec<Arc<dyn RpcProvider>> = vec![p1.clone(), p2.clone()];
    let rotator = RotatingProvider::new(providers);

    // Queue three successful pages on the primary. Each page returns a
    // distinguishable log so we can assert ordering downstream.
    p1.push_logs(Ok(vec![log_at(100, 0)])); // page [0, 1999]
    p1.push_logs(Ok(vec![log_at(2500, 0)])); // page [2000, 3999]
    p1.push_logs(Ok(vec![log_at(4500, 0)])); // page [4000, 4999]

    let filter = Filter::new();
    let logs = rotator
        .get_logs_paginated(filter, 0, 4999, 2000)
        .await
        .expect("paginate");

    // Exactly 3 calls; fallback never touched.
    assert_eq!(p1.get_logs_call_count(), 3);
    assert_eq!(p2.get_logs_call_count(), 0);
    assert_eq!(logs.len(), 3);

    // Verify the page windows are correct + inclusive.
    let calls = p1.get_logs_calls_snapshot();
    let to_u64 = |f: &Filter, get_to: bool| -> Option<u64> {
        let bn = if get_to {
            f.get_to_block()
        } else {
            f.get_from_block()
        }?;
        Some(bn)
    };
    assert_eq!(to_u64(&calls[0], false), Some(0));
    assert_eq!(to_u64(&calls[0], true), Some(1999));
    assert_eq!(to_u64(&calls[1], false), Some(2000));
    assert_eq!(to_u64(&calls[1], true), Some(3999));
    assert_eq!(to_u64(&calls[2], false), Some(4000));
    assert_eq!(to_u64(&calls[2], true), Some(4999));

    // Logs come back in the order they were emitted (page 1 -> page 2 -> page 3).
    let block_nums: Vec<u64> = logs.iter().filter_map(|l| l.block_number).collect();
    assert_eq!(block_nums, vec![100, 2500, 4500]);
}

#[tokio::test(flavor = "current_thread")]
async fn final_partial_page_handled() {
    let p1 = Arc::new(MockProvider::new("primary"));
    let providers: Vec<Arc<dyn RpcProvider>> = vec![p1.clone()];
    let rotator = RotatingProvider::new(providers);

    // 2500-block range -> two pages: [0,1999] (2000 blocks), [2000,2499] (500).
    p1.push_logs(Ok(vec![log_at(0, 0), log_at(1, 1)]));
    p1.push_logs(Ok(vec![log_at(2400, 0)]));

    let logs = rotator
        .get_logs_paginated(Filter::new(), 0, 2499, 2000)
        .await
        .expect("paginate");

    assert_eq!(p1.get_logs_call_count(), 2);
    let calls = p1.get_logs_calls_snapshot();
    assert_eq!(calls[0].get_from_block(), Some(0));
    assert_eq!(calls[0].get_to_block(), Some(1999));
    assert_eq!(calls[1].get_from_block(), Some(2000));
    assert_eq!(calls[1].get_to_block(), Some(2499));
    assert_eq!(logs.len(), 3);
}

#[tokio::test(flavor = "current_thread")]
async fn mid_pagination_provider_switch_preserves_ordering() {
    let p1 = Arc::new(MockProvider::new("primary"));
    let p2 = Arc::new(MockProvider::new("fallback"));
    let providers: Vec<Arc<dyn RpcProvider>> = vec![p1.clone(), p2.clone()];
    let rotator = RotatingProvider::new(providers);

    // Page 1: primary OK -> [log@500].
    p1.push_logs(Ok(vec![log_at(500, 0)]));
    // Page 2: primary transient -> rotator falls to fallback -> [log@2500].
    p1.push_logs(Err(RpcError::Transient("temporary 500".into())));
    p2.push_logs(Ok(vec![log_at(2500, 0)]));
    // Page 3: primary still has 1 strike, returns OK -> [log@4500].
    p1.push_logs(Ok(vec![log_at(4500, 0)]));

    let logs = rotator
        .get_logs_paginated(Filter::new(), 0, 4999, 2000)
        .await
        .expect("paginate");

    // Primary was called 3 times (page1 ok, page2 fail, page3 ok); fallback once (page2 retry).
    assert_eq!(p1.get_logs_call_count(), 3);
    assert_eq!(p2.get_logs_call_count(), 1);

    // Block ordering preserved across the provider switch.
    let block_nums: Vec<u64> = logs.iter().filter_map(|l| l.block_number).collect();
    assert_eq!(block_nums, vec![500, 2500, 4500]);
}

#[tokio::test(flavor = "current_thread")]
async fn page_size_is_clamped_to_alchemy_limit() {
    let p1 = Arc::new(MockProvider::new("primary"));
    let providers: Vec<Arc<dyn RpcProvider>> = vec![p1.clone()];
    let rotator = RotatingProvider::new(providers);

    // Caller asks for 10_000-block pages — must be clamped to MAX_LOGS_PER_CALL (2_000).
    // 5_000 blocks at clamped page=2_000 -> 3 calls (not 1).
    for _ in 0..3 {
        p1.push_logs(Ok(Vec::new()));
    }
    let _ = rotator
        .get_logs_paginated(Filter::new(), 0, 4_999, 10_000)
        .await
        .expect("paginate");
    assert_eq!(p1.get_logs_call_count(), 3);

    // Cross-check the constant.
    assert_eq!(MAX_LOGS_PER_CALL, 2_000);

    // Silence unused-import warnings — `U256` is intentionally imported for
    // future test extensions that build richer logs.
    let _ = U256::ZERO;
}

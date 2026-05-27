//! Concurrency test for `PgNonceProvider`.
//!
//! Spawns N concurrent `allocate` calls against ONE shared Postgres row +
//! asserts the dispensed nonces are exactly `0..N` with no gaps and no
//! duplicates. This is the load-bearing invariant for the Vercel
//! multi-isolate scenario: two isolates dispatching concurrent txs MUST
//! get distinct sequential nonces or the second one is rejected by the
//! mempool.

#![cfg(test)]

use std::collections::HashSet;
use std::sync::Arc;

use alloy_primitives::{address, Address};
use eth_tools_payments::wallet::nonce::{
    FixedSeed, InMemoryNonceProvider, NonceProvider, PgNonceProvider,
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

fn signer() -> Address {
    address!("ABCDABCDABCDABCDABCDABCDABCDABCDABCDABCD")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn pg_allocator_under_concurrent_load_is_strictly_sequential() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    const N: usize = 5;
    let store = Arc::new(PgNonceProvider::new(pool.clone()));
    let seed = Arc::new(FixedSeed(0));

    let mut handles = Vec::with_capacity(N);
    for _ in 0..N {
        let s = Arc::clone(&store);
        let sd = Arc::clone(&seed);
        handles.push(tokio::spawn(async move {
            s.allocate(8453, signer(), sd.as_ref()).await
        }));
    }

    let mut dispensed = Vec::with_capacity(N);
    for h in handles {
        dispensed.push(h.await.unwrap().expect("allocate must succeed"));
    }
    dispensed.sort_unstable();
    assert_eq!(dispensed, (0..N as u64).collect::<Vec<_>>(), "must dispense 0..N");

    let unique: HashSet<u64> = dispensed.iter().copied().collect();
    assert_eq!(unique.len(), N, "must have N distinct nonces");

    // Final stored next_nonce in the table == N.
    let (next_nonce,): (i64,) = sqlx::query_as(
        "SELECT next_nonce FROM wallet_nonces WHERE chain_id = $1 AND signer_address = $2",
    )
    .bind(8453_i64)
    .bind(signer().as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(next_nonce, N as i64);
}

#[tokio::test]
async fn pg_allocator_seeds_then_increments_serially() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let store = PgNonceProvider::new(pool.clone());
    let seed = FixedSeed(42);

    assert_eq!(store.allocate(8453, signer(), &seed).await.unwrap(), 42);
    assert_eq!(store.allocate(8453, signer(), &seed).await.unwrap(), 43);
    assert_eq!(store.allocate(8453, signer(), &seed).await.unwrap(), 44);

    let (next_nonce,): (i64,) = sqlx::query_as(
        "SELECT next_nonce FROM wallet_nonces WHERE chain_id = $1 AND signer_address = $2",
    )
    .bind(8453_i64)
    .bind(signer().as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(next_nonce, 45);
}

#[tokio::test]
async fn pg_allocator_independent_per_chain() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    let store = PgNonceProvider::new(pool);
    let seed = FixedSeed(0);

    assert_eq!(store.allocate(8453, signer(), &seed).await.unwrap(), 0);
    assert_eq!(store.allocate(1, signer(), &seed).await.unwrap(), 0);
    assert_eq!(store.allocate(8453, signer(), &seed).await.unwrap(), 1);
    assert_eq!(store.allocate(1, signer(), &seed).await.unwrap(), 1);
}

#[tokio::test]
async fn in_memory_allocator_under_concurrent_load_is_strictly_sequential() {
    // Mirror of the pg test for the in-memory store — proves the trait
    // shape supports both backends with the same concurrency guarantees.
    const N: usize = 20;
    let store = Arc::new(InMemoryNonceProvider::new());
    let seed = Arc::new(FixedSeed(0));

    let mut handles = Vec::with_capacity(N);
    for _ in 0..N {
        let s = Arc::clone(&store);
        let sd = Arc::clone(&seed);
        handles.push(tokio::spawn(async move {
            s.allocate(8453, signer(), sd.as_ref()).await
        }));
    }

    let mut dispensed = Vec::with_capacity(N);
    for h in handles {
        dispensed.push(h.await.unwrap().expect("allocate must succeed"));
    }
    dispensed.sort_unstable();
    assert_eq!(dispensed, (0..N as u64).collect::<Vec<_>>());
}

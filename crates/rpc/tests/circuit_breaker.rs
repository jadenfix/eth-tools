//! Drive `RotatingProvider` against two mock providers and assert:
//!   1. Five consecutive transient errors trip provider 1's breaker.
//!   2. While provider 1 is open, traffic falls through to provider 2.
//!   3. After 30s of breaker cooldown, provider 1 is probed (half-open).
//!   4. A successful probe closes the breaker.
//!
//! Uses `tokio::time::pause()` so the test runs instantly instead of waiting
//! 30s of real wall-clock time.

mod common;

use std::sync::Arc;
use std::time::Duration;

use eth_tools_rpc::{RotatingProvider, RpcError, RpcProvider, BREAKER_COOLDOWN, BREAKER_THRESHOLD};

use crate::common::MockProvider;

fn build_rotator() -> (Arc<MockProvider>, Arc<MockProvider>, RotatingProvider) {
    let p1 = Arc::new(MockProvider::new("primary"));
    let p2 = Arc::new(MockProvider::new("fallback"));
    let providers: Vec<Arc<dyn RpcProvider>> = vec![p1.clone(), p2.clone()];
    let rotator = RotatingProvider::new(providers);
    (p1, p2, rotator)
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn breaker_trips_after_five_failures_then_falls_back() {
    let (p1, p2, rotator) = build_rotator();

    // Five transient errors from primary, each backstopped by a primary
    // success that we DON'T need (primary is the first-in-list, so it gets
    // every call until tripped). We also queue five fallback successes so the
    // rotator has something to fall through to on each call.
    for _ in 0..BREAKER_THRESHOLD {
        p1.push_block_number(Err(RpcError::Transient("429".into())));
        p2.push_block_number(Ok(123_456));
    }

    for _ in 0..BREAKER_THRESHOLD {
        let n = rotator.get_block_number().await.expect("fallback should succeed");
        assert_eq!(n, 123_456);
    }

    // After 5 transient errors primary should be open.
    let health = rotator.health().await;
    assert_eq!(health[0].name, "primary");
    assert!(health[0].opened, "primary breaker should be OPEN");
    assert!(!health[1].opened, "fallback breaker should be CLOSED");

    // While primary is open, the next call goes straight to fallback and we
    // do NOT consume any primary queue entry — assert that by only queuing
    // a single fallback response.
    p2.push_block_number(Ok(999));
    assert_eq!(rotator.get_block_number().await.unwrap(), 999);

    // Advance just under cooldown — primary should still be open.
    tokio::time::advance(BREAKER_COOLDOWN - Duration::from_secs(1)).await;
    p2.push_block_number(Ok(1_000));
    assert_eq!(rotator.get_block_number().await.unwrap(), 1_000);
    assert!(rotator.health().await[0].opened);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn half_open_probe_closes_breaker_on_success() {
    let (p1, p2, rotator) = build_rotator();

    // Trip primary.
    for _ in 0..BREAKER_THRESHOLD {
        p1.push_block_number(Err(RpcError::Transient("500".into())));
        p2.push_block_number(Ok(1));
    }
    for _ in 0..BREAKER_THRESHOLD {
        let _ = rotator.get_block_number().await.unwrap();
    }
    assert!(rotator.health().await[0].opened);

    // Advance past cooldown to allow the half-open probe.
    tokio::time::advance(BREAKER_COOLDOWN + Duration::from_secs(1)).await;

    // The next call should hit primary (half-open probe). Queue a success there.
    p1.push_block_number(Ok(42));
    // No fallback response queued — if the rotator wrongly skips primary, we panic.
    let result = rotator.get_block_number().await.unwrap();
    assert_eq!(result, 42);

    let health = rotator.health().await;
    assert!(
        !health[0].opened,
        "primary breaker should be CLOSED after successful probe"
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn all_open_returns_all_providers_open() {
    let (p1, p2, rotator) = build_rotator();

    // Trip both providers in lockstep: each call hits primary, fails, falls to
    // fallback, fails — both breakers increment.
    for _ in 0..BREAKER_THRESHOLD {
        p1.push_block_number(Err(RpcError::Transient("429".into())));
        p2.push_block_number(Err(RpcError::Transient("429".into())));
        let err = rotator.get_block_number().await.unwrap_err();
        // Both transient -> rotator exhausts the list and returns AllProvidersOpen.
        assert!(matches!(err, RpcError::AllProvidersOpen { .. }));
    }

    // Both breakers should be open now.
    let health = rotator.health().await;
    assert!(health[0].opened);
    assert!(health[1].opened);

    // Next call: no eligible providers, no responses consumed.
    let err = rotator.get_block_number().await.unwrap_err();
    assert!(matches!(err, RpcError::AllProvidersOpen { .. }));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn half_open_probe_failure_rearms_cooldown() {
    // Regression: previously, a failed half-open probe reset the failure
    // counter to 0, decided the (now zero) counter didn't meet threshold, and
    // left `opened_at` at the original trip time — so `should_attempt_probe`
    // immediately returned true again. Result: every call after the first
    // cooldown spammed the dead provider with another probe.
    let (p1, p2, rotator) = build_rotator();

    // Trip primary.
    for _ in 0..BREAKER_THRESHOLD {
        p1.push_block_number(Err(RpcError::Transient("500".into())));
        p2.push_block_number(Ok(1));
    }
    for _ in 0..BREAKER_THRESHOLD {
        let _ = rotator.get_block_number().await.unwrap();
    }
    assert!(rotator.health().await[0].opened);

    // Advance past cooldown, then issue ONE failed probe.
    tokio::time::advance(BREAKER_COOLDOWN + Duration::from_secs(1)).await;
    p1.push_block_number(Err(RpcError::Transient("still down".into())));
    p2.push_block_number(Ok(2));
    let n = rotator.get_block_number().await.unwrap();
    assert_eq!(n, 2, "fallback handles call after primary's probe fails");
    assert!(
        rotator.health().await[0].opened,
        "primary still open after failed probe"
    );

    // The NEXT call within cooldown must NOT issue a new probe to primary.
    // If we queue no primary response and one fallback response, a regression
    // would consume from p1 (panic) or call into the empty p1 queue.
    p2.push_block_number(Ok(3));
    let n = rotator.get_block_number().await.unwrap();
    assert_eq!(n, 3, "call routed to fallback, NOT a re-probe to primary");

    // Advance the full cooldown again — now a fresh probe is allowed.
    tokio::time::advance(BREAKER_COOLDOWN + Duration::from_secs(1)).await;
    p1.push_block_number(Ok(99));
    let n = rotator.get_block_number().await.unwrap();
    assert_eq!(n, 99, "after full cooldown, primary is probed again");
    assert!(!rotator.health().await[0].opened, "successful probe closed primary");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn permanent_error_does_not_trip_breaker() {
    let (p1, _p2, rotator) = build_rotator();

    // Five permanent errors should NOT trip the breaker (they're caller bugs,
    // not provider degradation; the next provider would return the same).
    for _ in 0..BREAKER_THRESHOLD {
        p1.push_block_number(Err(RpcError::Permanent("invalid params".into())));
    }
    for _ in 0..BREAKER_THRESHOLD {
        let err = rotator.get_block_number().await.unwrap_err();
        assert!(matches!(err, RpcError::Permanent(_)));
    }

    let health = rotator.health().await;
    assert!(!health[0].opened, "permanent errors must not trip breaker");
}

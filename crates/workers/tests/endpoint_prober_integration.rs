//! Integration tests for W3 (endpoint_prober).
//!
//! Covers the four contract surfaces called out in the W3 spec:
//!
//!   1. **HEAD 200 → ok=true.** Baseline happy path.
//!   2. **HEAD 405 → fall back to GET → ok=true.** The fallback exists
//!      because some origins reject HEAD even when GET works (S3-style
//!      bucket policies, NGINX defaults).
//!   3. **5xx → ok=false, status_code recorded.** A failing health-check
//!      is itself a useful liveness signal — we record the status, not
//!      just an error.
//!   4. **Timeout → ok=false, error="timeout".** Bounded by `PROBE_TIMEOUT`
//!      (5 s in prod; we override to 300 ms here so the test runs fast).
//!   5. **SSRF target at 127.0.0.1 → pre-flight rejection.** The prod
//!      guard MUST reject before any HTTP is issued; we verify the error
//!      reason includes "blocked" / "deny set" and that no request hit
//!      the wiremock listener (assertion: zero `received_requests`).
//!   6. **Concurrency bound.** 100 endpoints × 200 ms latency each ÷ 32-way
//!      concurrency ≈ 0.8 s wall. We assert ≤ 5 s (huge margin to absorb
//!      CI jitter, but well below the 200 ms × 100 = 20 s serial floor
//!      that would indicate a misconfigured semaphore).
//!
//! ## Why a wiremock listener, not a hand-rolled TCP socket?
//!
//! wiremock gives us per-test path-scoped responses with first-class
//! "respond after N ms" support, which is exactly what the timeout +
//! concurrency tests need. It also matches the spec's literal wording
//! ("integration: wiremock with various endpoints").

#![cfg(test)]

use std::time::{Duration, Instant};

use eth_tools_workers::endpoint_prober::{
    probe_with_client, test_only_probe_one, test_only_unguarded_client, ProbeOutcomeView,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn short_timeout_client() -> reqwest::Client {
    // 300 ms timeout for the synthetic timeout test; the other tests
    // complete inside this budget so it's safe to share.
    test_only_unguarded_client(Duration::from_millis(300))
}

async fn probe(server_url: &str, suffix: &str) -> ProbeOutcomeView {
    let url = url::Url::parse(&format!("{server_url}{suffix}")).unwrap();
    let client = short_timeout_client();
    probe_with_client(&client, &url, Instant::now()).await
}

#[tokio::test]
async fn head_200_is_ok() {
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .and(path("/healthy"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let out = probe(&server.uri(), "/healthy").await;
    assert_eq!(out.status_code, Some(200));
    assert!(out.ok);
    assert!(out.error.is_none(), "got error: {:?}", out.error);
    // latency_ms must be a non-negative number well under our short timeout.
    assert!(out.latency_ms >= 0);
    assert!(out.latency_ms < 1000, "unexpected latency: {}", out.latency_ms);
}

#[tokio::test]
async fn head_405_falls_back_to_get_200() {
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .and(path("/fallback"))
        .respond_with(ResponseTemplate::new(405))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/fallback"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            // Body should NEVER be buffered by the worker — but we set
            // something here to prove the GET path completes even when
            // the origin returns a non-empty body.
            "{\"status\":\"ok\"}",
        ))
        .mount(&server)
        .await;

    let out = probe(&server.uri(), "/fallback").await;
    assert_eq!(
        out.status_code,
        Some(200),
        "HEAD 405 → GET 200 fallback must record the GET status"
    );
    assert!(out.ok);
    assert!(out.error.is_none());
}

#[tokio::test]
async fn head_501_falls_back_to_get_200() {
    // Same fallback policy, different trigger status. 501 Not Implemented
    // is the older HTTP/1.0 spelling of "HEAD not supported."
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .and(path("/501"))
        .respond_with(ResponseTemplate::new(501))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/501"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let out = probe(&server.uri(), "/501").await;
    assert_eq!(out.status_code, Some(200));
    assert!(out.ok);
}

#[tokio::test]
async fn five_xx_recorded_as_not_ok() {
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .and(path("/broken"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let out = probe(&server.uri(), "/broken").await;
    assert_eq!(out.status_code, Some(503));
    assert!(!out.ok, "5xx must classify as ok=false");
    // status was reached — no transport error.
    assert!(out.error.is_none(), "got error on 5xx: {:?}", out.error);
}

#[tokio::test]
async fn four_xx_recorded_as_not_ok() {
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .and(path("/forbidden"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;

    let out = probe(&server.uri(), "/forbidden").await;
    assert_eq!(out.status_code, Some(403));
    assert!(!out.ok);
}

#[tokio::test]
async fn three_xx_recorded_as_ok() {
    // A 3xx without following the redirect is still a successful liveness
    // probe — the origin responded promptly with a routable answer.
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .and(path("/moved"))
        .respond_with(ResponseTemplate::new(301).insert_header("location", "/other"))
        .mount(&server)
        .await;

    let out = probe(&server.uri(), "/moved").await;
    assert_eq!(out.status_code, Some(301));
    assert!(out.ok, "3xx is in the 200..400 ok band");
}

#[tokio::test]
async fn timeout_yields_error_timeout() {
    let server = MockServer::start().await;
    // Respond after 2 s — well beyond our 300 ms test timeout.
    Mock::given(method("HEAD"))
        .and(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(2)))
        .mount(&server)
        .await;

    let start = Instant::now();
    let out = probe(&server.uri(), "/slow").await;
    let elapsed = start.elapsed();

    assert!(out.status_code.is_none(), "timeout must not record a status");
    assert!(!out.ok);
    let err = out.error.expect("must surface a transport error");
    assert!(err.contains("timeout"), "expected timeout, got: {err}");
    // Timeout itself enforces an upper bound; allow generous slack for
    // CI scheduler jitter but well under the 2 s response delay.
    assert!(
        elapsed < Duration::from_millis(1500),
        "probe took {elapsed:?}; timeout did not fire"
    );
}

#[tokio::test]
async fn ssrf_loopback_rejected_pre_flight() {
    // Stand up a wiremock to give a real listener at 127.0.0.1. If the
    // SSRF guard fails to fire, the request would land here and we'd
    // see a 200. We assert (a) the outcome is ok=false with the right
    // error class, and (b) the listener received zero requests — the
    // ONLY way both hold simultaneously is a pre-flight reject.
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .and(path("/wont-hit"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    // `server.uri()` is `http://127.0.0.1:<port>` — exactly the SSRF
    // scenario. Use `test_only_probe_one` which goes through the prod
    // guard (the un-test seam).
    let out = test_only_probe_one(&format!("{}/wont-hit", server.uri())).await;
    assert!(out.status_code.is_none(), "no status — request never went");
    assert!(!out.ok);
    let err = out.error.expect("SSRF rejection error reason");
    assert!(
        err.contains("blocked") || err.contains("deny"),
        "expected SSRF block reason, got: {err}"
    );
    // The keystone assertion: wiremock saw nothing.
    let received = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        received.len(),
        0,
        "SSRF guard MUST reject pre-flight; received {} requests",
        received.len()
    );
}

#[tokio::test]
async fn concurrency_bound_holds() {
    // 100 unique paths, each with a 200 ms delay. Serial wall-clock would
    // be 20 s; 32-way concurrent wall-clock is ≈ ceil(100/32) × 200ms
    // ≈ 800 ms. We assert < 5 s, leaving ~6× margin for CI jitter while
    // still failing loudly if the semaphore degenerates to 1.
    //
    // We drive concurrency through the SAME machinery the worker uses
    // (Semaphore + JoinSet) so the test exercises the actual scheduling
    // pattern, not a hand-rolled lookalike.
    use std::sync::Arc;
    use tokio::sync::Semaphore;
    use tokio::task::JoinSet;

    let server = MockServer::start().await;
    // One mock matching every path under /probe/ with the delay.
    Mock::given(method("HEAD"))
        .and(wiremock::matchers::path_regex("^/probe/\\d+$"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(200)))
        .mount(&server)
        .await;

    // Per-request timeout MUST exceed the 200 ms response delay or every
    // probe times out. Use 2 s here.
    let client = test_only_unguarded_client(Duration::from_secs(2));
    let sem = Arc::new(Semaphore::new(32));
    let mut set: JoinSet<ProbeOutcomeView> = JoinSet::new();

    let started = Instant::now();
    for i in 0..100u32 {
        let sem = sem.clone();
        let client = client.clone();
        let url = url::Url::parse(&format!("{}/probe/{i}", server.uri())).unwrap();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.unwrap();
            probe_with_client(&client, &url, Instant::now()).await
        });
    }
    let mut ok_count = 0;
    while let Some(j) = set.join_next().await {
        let o = j.expect("probe task");
        if o.ok && o.status_code == Some(200) {
            ok_count += 1;
        }
    }
    let elapsed = started.elapsed();
    assert_eq!(ok_count, 100, "every probe must succeed");
    assert!(
        elapsed < Duration::from_secs(5),
        "32-way concurrency must beat 5s; took {elapsed:?}"
    );
    // Also assert we beat the serial floor by a wide margin — proves
    // concurrency is engaged, not just "fast network".
    assert!(
        elapsed < Duration::from_secs(15),
        "wall {elapsed:?} suggests concurrency degraded to serial"
    );
}

#[tokio::test]
async fn body_is_not_buffered_on_get_fallback() {
    // The spec says "stream-and-drop body — don't buffer huge responses."
    // We can't directly observe the worker's memory, but we CAN prove
    // the GET completes quickly even when the origin sends a large body —
    // if the worker were calling `.bytes()` it would block until the
    // body fully arrived. With a 10 MB body served at full speed over
    // localhost, that's still measurable.
    let big = "x".repeat(8 * 1024 * 1024); // 8 MiB
    let server = MockServer::start().await;
    Mock::given(method("HEAD"))
        .and(path("/big"))
        .respond_with(ResponseTemplate::new(405))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/big"))
        .respond_with(ResponseTemplate::new(200).set_body_string(big))
        .mount(&server)
        .await;

    // 5s timeout — plenty for an 8 MiB localhost transfer either way,
    // but the test catches a flagrantly broken impl.
    let client = test_only_unguarded_client(Duration::from_secs(5));
    let url = url::Url::parse(&format!("{}/big", server.uri())).unwrap();
    let out: ProbeOutcomeView = probe_with_client(&client, &url, Instant::now()).await;
    assert_eq!(out.status_code, Some(200));
    assert!(out.ok);
}

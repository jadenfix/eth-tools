//! Integration tests for `POST /api/v1/invoke` + the x402 gate.
//!
//! These run the full Axum router against a wiremock-stood-up fake
//! facilitator — proving the 402 challenge body shape, the
//! verify → settle round-trip, the replay path, and the dev-mode bypass
//! all the way from HTTP request to HTTP response.
//!
//! We use `PgPool::connect_lazy` so these tests don't need Docker — the
//! invoke handler never touches the pool. The first DB call would fail
//! (no real Postgres), but the gate short-circuits before that.

use axum::body::{to_bytes, Body};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use eth_tools_api::{router, AppState, X402Config};
use eth_tools_db::Pool;
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A throwaway pool that never opens a connection. Safe for handlers that
/// don't query the DB (like `invoke`).
fn lazy_pool() -> Pool {
    PgPoolOptions::new()
        .max_connections(1)
        // localhost:1 is reserved & guaranteed to refuse — if the handler
        // ever tries to use it, the test will fail loudly instead of
        // silently hanging.
        .connect_lazy("postgres://nobody:nobody@127.0.0.1:1/none")
        .expect("lazy pool builds")
}

fn pay_to() -> alloy_primitives::Address {
    "0x209693Bc6afc0C5328bA36FaF03C514EF312287C"
        .parse()
        .unwrap()
}

fn dummy_payment_header() -> String {
    STANDARD.encode(r#"{"signature":"0x00","authorization":{}}"#)
}

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn post_without_payment_returns_402_with_challenge_body() {
    let state = AppState::with_x402(
        lazy_pool(),
        X402Config::for_test("http://unused.invalid", pay_to()),
    );
    let app = router(state);
    let req = http::Request::builder()
        .method("POST")
        .uri("/api/v1/invoke")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 402);

    let v = body_json(resp).await;
    assert_eq!(v["x402Version"], 1);
    assert_eq!(v["error"], "X-PAYMENT header is required");
    let accepts = v["accepts"].as_array().expect("accepts array");
    assert_eq!(accepts.len(), 1);
    let req0 = &accepts[0];
    assert_eq!(req0["scheme"], "exact");
    assert_eq!(req0["network"], "eip155:8453");
    assert_eq!(req0["maxAmountRequired"], "10000"); // 1 cent
    assert_eq!(req0["resource"], "https://eth-tools.dev/api/v1/invoke");
    assert!(req0["payTo"].as_str().unwrap().starts_with("0x"));
}

#[tokio::test]
async fn post_with_valid_payment_returns_200_and_receipt_header() {
    let facilitator = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/verify"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "isValid": true,
            "payer": "0xAGENT"
        })))
        .mount(&facilitator)
        .await;
    Mock::given(method("POST"))
        .and(path("/settle"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true,
            "payer": "0xAGENT",
            "transaction": "0xfeedface",
            "network": "base"
        })))
        .mount(&facilitator)
        .await;

    let state = AppState::with_x402(
        lazy_pool(),
        X402Config::for_test(facilitator.uri(), pay_to()),
    );
    let app = router(state);

    let req = http::Request::builder()
        .method("POST")
        .uri("/api/v1/invoke")
        .header("content-type", "application/json")
        .header("x-payment", dummy_payment_header())
        .body(Body::from(r#"{"echo":"hi"}"#))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("x-payment-receipt").unwrap(),
        "0xfeedface"
    );

    let v = body_json(resp).await;
    assert_eq!(v["ok"], true);
    assert_eq!(v["echo"], "hi");
    assert_eq!(v["settlement_tx"], "0xfeedface");
}

#[tokio::test]
async fn post_with_replayed_nonce_returns_402_with_extra_replay() {
    let facilitator = MockServer::start().await;
    // /verify catches the replay (cheaper than letting /settle eat gas)
    Mock::given(method("POST"))
        .and(path("/verify"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "isValid": false,
            "invalidReason": "nonce already used"
        })))
        .mount(&facilitator)
        .await;

    let state = AppState::with_x402(
        lazy_pool(),
        X402Config::for_test(facilitator.uri(), pay_to()),
    );
    let app = router(state);

    let req = http::Request::builder()
        .method("POST")
        .uri("/api/v1/invoke")
        .header("x-payment", dummy_payment_header())
        .body(Body::from(""))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 402);

    let v = body_json(resp).await;
    assert_eq!(v["x402Version"], 1);
    assert_eq!(v["extra"]["replay"], true);
    // The accepts[] still includes a fresh challenge so the agent can pick a
    // new nonce and retry without a second round-trip.
    assert_eq!(v["accepts"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn post_when_settle_replays_returns_402_with_extra_replay() {
    // Variant: verify OK but settle reports a replay (e.g. concurrent
    // submission won the race to chain). Same outcome — 402 with extra.
    let facilitator = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/verify"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"isValid": true})))
        .mount(&facilitator)
        .await;
    Mock::given(method("POST"))
        .and(path("/settle"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "success": false,
            "errorReason": "nonce already used"
        })))
        .mount(&facilitator)
        .await;

    let state = AppState::with_x402(
        lazy_pool(),
        X402Config::for_test(facilitator.uri(), pay_to()),
    );
    let app = router(state);

    let req = http::Request::builder()
        .method("POST")
        .uri("/api/v1/invoke")
        .header("x-payment", dummy_payment_header())
        .body(Body::from(""))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 402);
    let v = body_json(resp).await;
    assert_eq!(v["extra"]["replay"], true);
}

#[tokio::test]
async fn post_when_facilitator_unreachable_returns_502() {
    let state = AppState::with_x402(
        lazy_pool(),
        // Port 1 is reserved + unbound → connection refused.
        X402Config::for_test("http://127.0.0.1:1", pay_to()),
    );
    let app = router(state);

    let req = http::Request::builder()
        .method("POST")
        .uri("/api/v1/invoke")
        .header("x-payment", dummy_payment_header())
        .body(Body::from(""))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 502, "facilitator down should be 502, not 402");

    let v = body_json(resp).await;
    // Typed error envelope — clients pattern-match on `error.code`.
    assert_eq!(v["error"]["code"], "FACILITATOR_UNAVAILABLE");
    assert_eq!(v["error"]["evaluator"], "api.x402");
}

#[tokio::test]
async fn dev_mode_bypass_lets_invoke_through_without_payment() {
    // No X402_PAY_TO_ADDRESS configured → gate is a no-op.
    let state = AppState::with_x402(lazy_pool(), X402Config::disabled());
    let app = router(state);

    let req = http::Request::builder()
        .method("POST")
        .uri("/api/v1/invoke")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"echo":"dev"}"#))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200, "dev mode must bypass the gate");
    assert!(
        resp.headers().get("x-payment-receipt").is_none(),
        "no receipt header in dev mode (no settlement happened)"
    );

    let v = body_json(resp).await;
    assert_eq!(v["ok"], true);
    assert_eq!(v["echo"], "dev");
    assert!(v.get("settlement_tx").is_none() || v["settlement_tx"].is_null());
}

#[tokio::test]
async fn dev_mode_ignores_a_present_x_payment_header() {
    // Even if the buyer SENDS X-PAYMENT, dev mode must not try to verify
    // it — we have no facilitator and would 502.
    let state = AppState::with_x402(lazy_pool(), X402Config::disabled());
    let app = router(state);

    let req = http::Request::builder()
        .method("POST")
        .uri("/api/v1/invoke")
        .header("x-payment", "ignored-in-dev")
        .body(Body::from(""))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn post_with_empty_x_payment_returns_402_not_502() {
    // Empty header is a client bug — challenge them again, don't 502.
    let facilitator = MockServer::start().await;
    let state = AppState::with_x402(
        lazy_pool(),
        X402Config::for_test(facilitator.uri(), pay_to()),
    );
    let app = router(state);

    let req = http::Request::builder()
        .method("POST")
        .uri("/api/v1/invoke")
        .header("x-payment", "")
        .body(Body::from(""))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 402);
}

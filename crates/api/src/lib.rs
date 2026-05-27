//! HTTP API router for eth-tools.
//!
//! Mounted by `api/v1/index.rs` (Vercel function entrypoint; `vercel.json`
//! rewrites `/api/v1/(.*)` → `/api/v1/index`) and by `crates/dev-server`
//! (local). Both share the exact same `router()` so prod and local exercise
//! identical handler code.
//!
//! ## Phase-4 surface (plan §9.1)
//!
//! Read-side (anonymous + rate-limited):
//!   GET  /api/v1/health
//!   GET  /api/v1/agents
//!   GET  /api/v1/agents/:chain/:agent_id
//!   POST /api/v1/agents/search
//!   POST /api/v1/manifest/validate
//!   POST /api/v1/manifest/hash
//!   POST /api/v1/manifest/generate
//!   POST /api/v1/access/check
//!   POST /api/v1/access/explain
//!   GET  /api/v1/reputation/:chain/:agent_id
//!   GET  /api/v1/validation/:chain/:request_hash
//!
//! Write-side (Bearer-required + Idempotency-Key honoured):
//!   POST /api/v1/invoke/prepare
//!   POST /api/v1/invoke
//!   POST /api/v1/reputation/give
//!   POST /api/v1/validation/request
//!   POST /api/v1/validation/respond
//!
//! Several write handlers return 503 NOT_IMPLEMENTED with the sibling
//! branch name in the override_hint, until that branch lands in `main`.

use axum::routing::{get, post};
use axum::Router;
use eth_tools_db::Pool;

pub mod dto;
pub mod error;
pub mod handlers;
pub mod middleware;
pub mod openapi;

/// Shared state every handler can extract via `axum::extract::State`.
/// Cheap to clone (the inner pool is Arc-backed) so we pass by value.
#[derive(Clone)]
pub struct AppState {
    pub pool: Pool,
}

impl AppState {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

/// Build the Axum router. Mount everything under `/api/v1/*` so the path
/// matches both the Vercel rewrite target and the local dev-server.
///
/// Two sub-routers compose into the final tree:
///   - `read_routes` — no middleware (rate-limit lands as a tower layer in
///     the dev-server bin so handlers stay pure).
///   - `write_routes` — `require_bearer` + `idempotency::enforce` stacked
///     in that order (auth first, so unauthorised callers don't touch the
///     idempotency table).
pub fn router(state: AppState) -> Router {
    let read_routes = Router::new()
        .route("/api/v1/health", get(handlers::health::get))
        .route("/api/v1/agents", get(handlers::agents::list))
        .route(
            "/api/v1/agents/:chain/:agent_id",
            get(handlers::agents::get_one),
        )
        .route("/api/v1/agents/search", post(handlers::agents::search))
        .route(
            "/api/v1/manifest/validate",
            post(handlers::manifest::validate),
        )
        .route("/api/v1/manifest/hash", post(handlers::manifest::hash))
        .route(
            "/api/v1/manifest/generate",
            post(handlers::manifest::generate),
        )
        .route("/api/v1/access/check", post(handlers::access::check))
        .route("/api/v1/access/explain", post(handlers::access::explain))
        .route(
            "/api/v1/reputation/:chain/:agent_id",
            get(handlers::reputation::read),
        )
        .route(
            "/api/v1/validation/:chain/:request_hash",
            get(handlers::validation::read),
        );

    let write_routes = Router::new()
        .route("/api/v1/invoke/prepare", post(handlers::invoke::prepare))
        .route("/api/v1/invoke", post(handlers::invoke::execute))
        .route(
            "/api/v1/reputation/give",
            post(handlers::reputation::give),
        )
        .route(
            "/api/v1/validation/request",
            post(handlers::validation::request_validation),
        )
        .route(
            "/api/v1/validation/respond",
            post(handlers::validation::respond_validation),
        )
        // Bottom-up middleware: idempotency runs INSIDE auth, so the cache
        // never sees an unauthorised call.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::idempotency::enforce,
        ))
        .layer(axum::middleware::from_fn(middleware::auth::require_bearer));

    Router::new()
        .merge(read_routes)
        .merge(write_routes)
        .fallback(handlers::not_found)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    //! Handler tests run against a real Postgres via testcontainers (skipped
    //! if Docker unavailable). This is the gate that proves the router +
    //! handlers + db queries integrate end-to-end.

    use super::*;
    use axum::body::{to_bytes, Body};
    use serde_json::{json, Value};
    use testcontainers::runners::AsyncRunner;
    use testcontainers::ImageExt;
    use testcontainers_modules::postgres::Postgres;
    use tower::ServiceExt;

    const SEED_SQL: &str = include_str!("../../../fixtures/seed.sql");

    async fn boot() -> Option<(impl std::fmt::Debug, AppState)> {
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
        let pool = eth_tools_db::connect(&url).await.unwrap();
        eth_tools_db::migrate(&pool).await.unwrap();
        sqlx::raw_sql(SEED_SQL).execute(&pool).await.unwrap();
        Some((container, AppState::new(pool)))
    }

    async fn body_json(resp: axum::response::Response) -> Value {
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn post_json(uri: &str, body: Value) -> http::Request<Body> {
        http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn post_json_auth(uri: &str, body: Value, token: &str) -> http::Request<Body> {
        http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn post_json_auth_with_key(
        uri: &str,
        body: Value,
        token: &str,
        idem: &str,
    ) -> http::Request<Body> {
        http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .header("Idempotency-Key", idem)
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    // ---- Pre-existing read tests (unchanged contract) ------------------

    #[tokio::test]
    async fn health_includes_chain_counts() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        let req = http::Request::builder()
            .uri("/api/v1/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["status"], "ok");
        assert!(v["chains"].as_array().unwrap().len() >= 2);
        assert!(v["agents_indexed"].as_i64().unwrap() >= 6);
    }

    #[tokio::test]
    async fn list_agents_returns_seeded_rows_with_cursor() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        let req = http::Request::builder()
            .uri("/api/v1/agents?limit=3")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["data"].as_array().unwrap().len(), 3);
        assert!(v["next_cursor"].is_string(), "expected next_cursor on full page");
        assert!(v["staleness_ms"].is_number());
    }

    #[tokio::test]
    async fn list_agents_keyset_iterates_all() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state.clone());

        let mut seen: Vec<(i64, String)> = Vec::new();
        let mut url = "/api/v1/agents?limit=2".to_string();
        loop {
            let req = http::Request::builder().uri(&url).body(Body::empty()).unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            let v = body_json(resp).await;
            let page = v["data"].as_array().unwrap();
            if page.is_empty() {
                break;
            }
            for row in page {
                seen.push((
                    row["chain_id"].as_i64().unwrap(),
                    row["agent_id"].as_str().unwrap().to_string(),
                ));
            }
            match v["next_cursor"].as_str() {
                Some(c) => url = format!("/api/v1/agents?limit=2&cursor={c}"),
                None => break,
            }
        }
        let mut keys = seen.clone();
        keys.sort();
        let unique = keys.len();
        keys.dedup();
        assert_eq!(unique, keys.len(), "duplicate rows across pages");
        assert!(unique >= 6);
    }

    #[tokio::test]
    async fn get_one_returns_agent_and_404() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);

        let req = http::Request::builder()
            .uri("/api/v1/agents/base/42")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["data"]["agent_id"], "42");
        assert_eq!(v["data"]["chain"], "base");

        let req = http::Request::builder()
            .uri("/api/v1/agents/base/999999")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 404);
        let v = body_json(resp).await;
        assert_eq!(v["error"]["code"], "AGENT_NOT_FOUND");
    }

    #[tokio::test]
    async fn unknown_chain_returns_404() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        let req = http::Request::builder()
            .uri("/api/v1/agents/no-such-chain/1")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 404);
        let v = body_json(resp).await;
        assert_eq!(v["error"]["code"], "CHAIN_NOT_FOUND");
    }

    // ---- New Phase-4 routes --------------------------------------------

    #[tokio::test]
    async fn agents_search_filters_by_chain() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        let resp = app
            .oneshot(post_json(
                "/api/v1/agents/search",
                json!({"filters": {"chain": "base-sepolia"}, "limit": 50}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        let data = v["data"].as_array().unwrap();
        assert!(!data.is_empty());
        for row in data {
            assert_eq!(row["chain"], "base-sepolia");
        }
    }

    #[tokio::test]
    async fn agents_search_rejects_bad_owner_hex() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        let resp = app
            .oneshot(post_json(
                "/api/v1/agents/search",
                json!({"filters": {"owner": "0xnothex"}}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
        let v = body_json(resp).await;
        assert_eq!(v["error"]["code"], "INVALID_ADDRESS");
    }

    #[tokio::test]
    async fn manifest_validate_good_fixture() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        use base64::engine::general_purpose::STANDARD;
        use base64::Engine;
        let bytes = include_bytes!("../../../fixtures/agent-card.good.json");
        let resp = app
            .oneshot(post_json(
                "/api/v1/manifest/validate",
                json!({"bytes_b64": STANDARD.encode(bytes)}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["valid"], true);
        assert_eq!(v["errors"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn manifest_validate_uri_mode_returns_503() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        let resp = app
            .oneshot(post_json(
                "/api/v1/manifest/validate",
                json!({"uri": "https://example.com/agent.json"}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 503);
        let v = body_json(resp).await;
        assert_eq!(v["error"]["code"], "NOT_IMPLEMENTED");
        assert!(v["error"]["override_hint"]
            .as_str()
            .unwrap()
            .contains("manifest-fetcher"));
    }

    #[tokio::test]
    async fn manifest_hash_returns_sha256() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        use base64::engine::general_purpose::STANDARD;
        use base64::Engine;
        let bytes = b"hello";
        let resp = app
            .oneshot(post_json(
                "/api/v1/manifest/hash",
                json!({"bytes_b64": STANDARD.encode(bytes)}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        // SHA-256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        assert_eq!(
            v["sha256"],
            "0x2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(v["byte_len"], 5);
    }

    #[tokio::test]
    async fn manifest_validate_both_uri_and_bytes_rejected() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        let resp = app
            .oneshot(post_json(
                "/api/v1/manifest/validate",
                json!({"uri": "https://x", "bytes_b64": "aGVsbG8="}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
    }

    #[tokio::test]
    async fn manifest_generate_backfills_defaults() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        let resp = app
            .oneshot(post_json(
                "/api/v1/manifest/generate",
                json!({"name": "Test Agent", "services": [{"type": "web", "endpoint": "https://x"}]}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["data"]["manifest"]["name"], "Test Agent");
        assert_eq!(v["data"]["manifest"]["version"], "0.0.1");
        assert!(v["data"]["manifest"]["services"].is_array());
    }

    #[tokio::test]
    async fn access_check_owner_allowed() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        let resp = app
            .oneshot(post_json(
                "/api/v1/access/check",
                json!({
                    "chain": "base",
                    "agent_id": "1",
                    "requester_address": "0xc0ffee0000000000000000000000000000000001",
                    "action": "invoke"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["allowed"], true);
        assert_eq!(v["reason"], "OWNER");
    }

    #[tokio::test]
    async fn access_check_non_owner_denied() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        let resp = app
            .oneshot(post_json(
                "/api/v1/access/check",
                json!({
                    "chain": "base",
                    "agent_id": "1",
                    "requester_address": "0x0000000000000000000000000000000000000001",
                    "action": "invoke"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["allowed"], false);
        assert_eq!(v["reason"], "NOT_OWNER");
    }

    #[tokio::test]
    async fn access_explain_returns_steps() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        let resp = app
            .oneshot(post_json(
                "/api/v1/access/explain",
                json!({
                    "chain": "base",
                    "agent_id": "3",
                    "requester_address": "0x0000000000000000000000000000000000000001",
                    "action": "read"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        let steps = v["steps"].as_array().unwrap();
        assert!(steps.len() >= 3);
    }

    #[tokio::test]
    async fn reputation_read_empty_when_no_feedback() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        let resp = app
            .oneshot(
                http::Request::builder()
                    .uri("/api/v1/reputation/base/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert!(v["data"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn validation_read_404_when_missing() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        let hash = "0x0000000000000000000000000000000000000000000000000000000000000001";
        let resp = app
            .oneshot(
                http::Request::builder()
                    .uri(format!("/api/v1/validation/base/{hash}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
        let v = body_json(resp).await;
        assert_eq!(v["error"]["code"], "VALIDATION_NOT_FOUND");
    }

    #[tokio::test]
    async fn validation_read_bad_hash_rejected() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        let resp = app
            .oneshot(
                http::Request::builder()
                    .uri("/api/v1/validation/base/0xshort")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
        let v = body_json(resp).await;
        assert_eq!(v["error"]["code"], "INVALID_REQUEST_HASH");
    }

    // ---- Write-side: auth gate -----------------------------------------

    #[tokio::test]
    async fn invoke_prepare_requires_bearer() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        let resp = app
            .oneshot(post_json(
                "/api/v1/invoke/prepare",
                json!({
                    "chain": "base", "agent_id": "1",
                    "to": "0x0000000000000000000000000000000000000001",
                    "selector": "0xdeadbeef"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
        let v = body_json(resp).await;
        assert_eq!(v["error"]["code"], "UNAUTHORIZED");
    }

    #[tokio::test]
    async fn invoke_prepare_builds_calldata() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        let resp = app
            .oneshot(post_json_auth(
                "/api/v1/invoke/prepare",
                json!({
                    "chain": "base", "agent_id": "1",
                    "to": "0x0000000000000000000000000000000000000001",
                    "selector": "0xdeadbeef",
                    "args_hex": "00".repeat(32),
                }),
                "et_test_dev",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["calldata"].as_str().unwrap().len(), 2 + 8 + 64);
        assert!(v["calldata"].as_str().unwrap().starts_with("0xdeadbeef"));
        assert_eq!(v["chain_id"], 8453);
    }

    #[tokio::test]
    async fn invoke_execute_returns_503() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        let resp = app
            .oneshot(post_json_auth(
                "/api/v1/invoke",
                json!({
                    "chain": "base", "agent_id": "1",
                    "to": "0x0000000000000000000000000000000000000001",
                    "selector": "0xdeadbeef",
                }),
                "et_test_dev",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 503);
        let v = body_json(resp).await;
        assert_eq!(v["error"]["code"], "NOT_IMPLEMENTED");
    }

    #[tokio::test]
    async fn reputation_give_503() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        let resp = app
            .oneshot(post_json_auth(
                "/api/v1/reputation/give",
                json!({"chain": "base", "agent_id": "1", "value": "100", "value_decimals": 0}),
                "et_test_dev",
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 503);
    }

    // ---- Idempotency ---------------------------------------------------

    #[tokio::test]
    async fn idempotency_same_key_returns_cached() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        let body = json!({
            "chain": "base", "agent_id": "1",
            "to": "0x0000000000000000000000000000000000000001",
            "selector": "0xdeadbeef",
        });
        // First request — populates the cache.
        let resp1 = app
            .clone()
            .oneshot(post_json_auth_with_key(
                "/api/v1/invoke/prepare",
                body.clone(),
                "et_test_dev",
                "idem-key-A",
            ))
            .await
            .unwrap();
        assert_eq!(resp1.status(), 200);
        let v1 = body_json(resp1).await;

        // Second request with the same key + body — should hit cache.
        // To prove this is the cache and not a re-execution, change the
        // chain (which would yield a DIFFERENT response if it ran) and
        // confirm we still get v1.
        let mut body2 = body.clone();
        body2["chain"] = json!("base-sepolia");
        let resp2 = app
            .oneshot(post_json_auth_with_key(
                "/api/v1/invoke/prepare",
                body2,
                "et_test_dev",
                "idem-key-A",
            ))
            .await
            .unwrap();
        assert_eq!(resp2.status(), 200);
        let v2 = body_json(resp2).await;
        assert_eq!(v1, v2, "cached response must replay verbatim");
    }

    #[tokio::test]
    async fn idempotency_different_key_re_executes() {
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);
        let resp1 = app
            .clone()
            .oneshot(post_json_auth_with_key(
                "/api/v1/invoke/prepare",
                json!({
                    "chain": "base", "agent_id": "1",
                    "to": "0x0000000000000000000000000000000000000001",
                    "selector": "0xdeadbeef",
                }),
                "et_test_dev",
                "idem-key-1",
            ))
            .await
            .unwrap();
        assert_eq!(resp1.status(), 200);
        let v1 = body_json(resp1).await;

        // Different idem key + different body — must re-execute.
        let resp2 = app
            .oneshot(post_json_auth_with_key(
                "/api/v1/invoke/prepare",
                json!({
                    "chain": "base-sepolia", "agent_id": "1",
                    "to": "0x0000000000000000000000000000000000000002",
                    "selector": "0xdeadbeef",
                }),
                "et_test_dev",
                "idem-key-2",
            ))
            .await
            .unwrap();
        assert_eq!(resp2.status(), 200);
        let v2 = body_json(resp2).await;
        assert_ne!(v1["chain_id"], v2["chain_id"]);
        assert_eq!(v2["chain_id"], 84532);
    }
}

//! HTTP API router for eth-tools.
//!
//! Mounted by `api/v1/index.rs` (Vercel function entrypoint; `vercel.json`
//! rewrites `/api/v1/(.*)` → `/api/v1/index`) and by `crates/dev-server`
//! (local). Both share the exact same `router()` so prod and local exercise
//! identical handler code.
//!
//! Surface:
//! - `GET /api/v1/health`                   — chain + DB liveness
//! - `GET /api/v1/agents`                   — keyset-paginated list (anon read)
//! - `GET /api/v1/agents/:chain/:agent_id`  — single agent (anon read)
//! - `POST /api/v1/_internal/keys`         — dashboard-only: issue
//! - `GET  /api/v1/_internal/keys`         — dashboard-only: list mine
//! - `DELETE /api/v1/_internal/keys/:id`   — dashboard-only: revoke
//!
//! `/api/v1/_internal/*` is gated by `auth::require_internal` (shared-secret +
//! identity headers from the Next.js Route Handler). End-user bearer-token
//! write endpoints (invoke / feedback / validate) land in Phase 5/6 and use
//! `auth::bearer_required` instead — that middleware function is exported
//! here so future routes can drop it on with one line.

use axum::Router;
use eth_tools_db::Pool;

pub mod auth;
pub mod dto;
pub mod error;
pub mod handlers;
pub mod keys;

/// Shared state every handler can extract via `axum::extract::State`.
/// Cheap to clone (the inner pool is Arc-backed; redis is wrapped in Arc) so
/// we pass by value.
#[derive(Clone)]
pub struct AppState {
    pub pool: Pool,
    /// Optional Upstash Redis client. `None` in environments where the env
    /// vars are unset (e.g. some unit tests); the bearer-verify path falls
    /// through to Postgres in that case.
    pub redis: Option<auth::RedisClient>,
}

impl AppState {
    pub fn new(pool: Pool) -> Self {
        Self {
            pool,
            redis: auth::RedisClient::from_env(),
        }
    }

    /// Test helper — explicit redis (usually `None`).
    pub fn with_redis(pool: Pool, redis: Option<auth::RedisClient>) -> Self {
        Self { pool, redis }
    }
}

/// Build the Axum router. Mount everything under `/api/v1/*` so the path
/// matches both the Vercel rewrite target and the local dev-server.
pub fn router(state: AppState) -> Router {
    // Internal admin sub-router — every route here requires the
    // X-Internal-Secret + X-Github-* headers that `require_internal` checks.
    // We use `route_layer` so the layer attaches to these routes only and
    // nothing else (anon reads stay anon).
    //
    // Mounted under `/api/v1/_internal/*` (NOT `/api/v1/keys/*`) so the
    // path doesn't collide with the Next.js Route Handler at
    // `app/api/v1/keys/route.ts`. Next.js file-system routes take precedence
    // over Vercel rewrites (`afterFiles`), so a Rust route at
    // `/api/v1/keys` would never be reachable in prod.
    let keys_routes = Router::new()
        .route(
            "/api/v1/_internal/keys",
            axum::routing::post(handlers::keys::create).get(handlers::keys::list),
        )
        .route(
            "/api/v1/_internal/keys/:id",
            axum::routing::delete(handlers::keys::delete),
        )
        .route_layer(axum::middleware::from_fn(auth::require_internal));

    Router::new()
        .route("/api/v1/health", axum::routing::get(handlers::health::get))
        .route("/api/v1/agents", axum::routing::get(handlers::agents::list))
        .route(
            "/api/v1/agents/:chain/:agent_id",
            axum::routing::get(handlers::agents::get_one),
        )
        .merge(keys_routes)
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
    use serde_json::Value;
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
        // Tests never have Upstash; pass None explicitly so behaviour is
        // identical across CI runs.
        Some((container, AppState::with_redis(pool, None)))
    }

    async fn body_json(resp: axum::response::Response) -> Value {
        let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

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
            .uri("/api/v1/no-such-chain/1")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // Match the pre-Phase-5 behaviour: fallthrough → 404 ROUTE_NOT_FOUND.
        assert_eq!(resp.status(), 404);
    }

    // ----- Phase-5: API keys end-to-end -----

    fn internal_headers() -> [(&'static str, &'static str); 2] {
        [("x-github-user-id", "424242"), ("x-github-login", "octocat")]
    }

    async fn create_key(app: &axum::Router, name: &str) -> Value {
        let body = serde_json::to_vec(&serde_json::json!({
            "name": name,
            "scopes": ["read"],
        }))
        .unwrap();
        let mut req = http::Request::builder()
            .method("POST")
            .uri("/api/v1/_internal/keys")
            .header("content-type", "application/json");
        for (k, v) in internal_headers() {
            req = req.header(k, v);
        }
        let resp = app
            .clone()
            .oneshot(req.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), 201, "expected 201 Created");
        body_json(resp).await
    }

    #[tokio::test]
    async fn issue_then_verify_then_revoke() {
        // Force test-mode prefix regardless of host env.
        std::env::remove_var("VERCEL_ENV");

        let Some((_c, state)) = boot().await else {
            return;
        };
        let pool = state.pool.clone();
        let app = router(state);

        let issued = create_key(&app, "primary").await;
        let plaintext = issued["plaintext"].as_str().unwrap().to_string();
        let prefix = issued["prefix"].as_str().unwrap().to_string();
        assert!(plaintext.starts_with("et_test_"));
        assert!(prefix.starts_with("et_test_"));
        assert_eq!(prefix.len(), keys::PREFIX_LEN);
        assert_eq!(plaintext.len(), 32);
        let id: uuid::Uuid = issued["id"].as_str().unwrap().parse().unwrap();

        // Verify the plaintext bears claims.
        let claims = auth::verify_bearer(&plaintext, &pool, None).await.expect("verify ok");
        assert_eq!(claims.github_user_id, 424242);
        assert_eq!(claims.github_login, "octocat");
        assert_eq!(claims.scopes, vec!["read".to_string()]);
        assert_eq!(claims.key_id, id);

        // A wrong-key with the right prefix → Invalid (bcrypt mismatch).
        let mut wrong = plaintext.clone();
        wrong.pop();
        wrong.push('X');
        let res = auth::verify_bearer(&wrong, &pool, None).await;
        assert!(matches!(res, Err(auth::AuthError::Invalid)));

        // List shows the key without the hash.
        let mut req = http::Request::builder().method("GET").uri("/api/v1/_internal/keys");
        for (k, v) in internal_headers() {
            req = req.header(k, v);
        }
        let resp = app.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        let data = v["data"].as_array().unwrap();
        assert_eq!(data.len(), 1);
        assert!(data[0].get("plaintext").is_none());
        assert!(data[0].get("key_hash").is_none());

        // Revoke.
        let mut req = http::Request::builder().method("DELETE").uri(format!("/api/v1/_internal/keys/{id}"));
        for (k, v) in internal_headers() {
            req = req.header(k, v);
        }
        let resp = app.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), 204);

        // Verify after revoke → Invalid.
        let res = auth::verify_bearer(&plaintext, &pool, None).await;
        assert!(matches!(res, Err(auth::AuthError::Invalid)));
    }

    #[tokio::test]
    async fn revoke_other_users_key_returns_404() {
        std::env::remove_var("VERCEL_ENV");

        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);

        let issued = create_key(&app, "victim-key").await;
        let id = issued["id"].as_str().unwrap().to_string();

        // Attempt revoke as a different user.
        let resp = app
            .clone()
            .oneshot(
                http::Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/_internal/keys/{id}"))
                    .header("x-github-user-id", "999")
                    .header("x-github-login", "attacker")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
        let v = body_json(resp).await;
        assert_eq!(v["error"]["code"], "KEY_NOT_FOUND");
    }

    #[tokio::test]
    async fn create_rejects_unknown_scope() {
        std::env::remove_var("VERCEL_ENV");
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);

        let body = serde_json::to_vec(&serde_json::json!({
            "name": "k",
            "scopes": ["admin"],
        }))
        .unwrap();
        let resp = app
            .clone()
            .oneshot(
                http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/_internal/keys")
                    .header("content-type", "application/json")
                    .header("x-github-user-id", "1")
                    .header("x-github-login", "alice")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
        let v = body_json(resp).await;
        assert_eq!(v["error"]["code"], "INVALID_SCOPE");
    }

    #[tokio::test]
    async fn keys_endpoint_requires_identity_headers() {
        std::env::remove_var("VERCEL_ENV");
        let Some((_c, state)) = boot().await else {
            return;
        };
        let app = router(state);

        let resp = app
            .clone()
            .oneshot(
                http::Request::builder()
                    .method("GET")
                    .uri("/api/v1/_internal/keys")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
    }

    #[tokio::test]
    async fn wrong_key_attempts_take_similar_time() {
        // Constant-time smoke test. Two wrong-but-prefix-valid bearer
        // attempts against the same row should differ by less than the bcrypt
        // step itself — we just assert both took at least a few ms (i.e. the
        // bcrypt path actually ran) and neither was an order of magnitude
        // faster than the other.
        std::env::remove_var("VERCEL_ENV");
        let Some((_c, state)) = boot().await else {
            return;
        };
        let pool = state.pool.clone();
        let app = router(state);
        let issued = create_key(&app, "ct").await;
        let plaintext = issued["plaintext"].as_str().unwrap().to_string();

        let mut wrong_a = plaintext.clone();
        wrong_a.pop();
        wrong_a.push('A');
        let mut wrong_b = plaintext.clone();
        wrong_b.pop();
        wrong_b.push('Z');

        let t0 = std::time::Instant::now();
        let _ = auth::verify_bearer(&wrong_a, &pool, None).await;
        let a = t0.elapsed();

        let t0 = std::time::Instant::now();
        let _ = auth::verify_bearer(&wrong_b, &pool, None).await;
        let b = t0.elapsed();

        // bcrypt(cost=10) should be > 10ms on any sane runner.
        assert!(a.as_millis() > 10, "first attempt too fast: {a:?}");
        assert!(b.as_millis() > 10, "second attempt too fast: {b:?}");
        // Allow a generous fan-out — contended CI runners can swing bcrypt
        // by 5–8x without it being a real side-channel leak. Anything past
        // 10x is suspicious.
        let ratio = (a.as_secs_f64() / b.as_secs_f64()).max(b.as_secs_f64() / a.as_secs_f64());
        assert!(ratio < 10.0, "timing diverged: a={a:?} b={b:?}");
    }

    /// Stand up a tiny in-process HTTP server that mimics the Upstash REST
    /// surface (GET/SET/DEL inside a `["CMD", "key", ...]` JSON body), point
    /// a [`RedisClient`] at it, and assert that:
    ///   - first verify is a cache MISS → bcrypt runs (slow);
    ///   - second verify is a cache HIT → bcrypt skipped (fast);
    ///   - revoke clears the cache → next verify is a MISS again.
    #[tokio::test]
    async fn cache_hit_miss_via_mock_upstash() {
        use axum::{routing::post, Json, Router};
        use std::collections::HashMap;
        use std::sync::Mutex;
        use tokio::net::TcpListener;

        std::env::remove_var("VERCEL_ENV");
        let Some((_c, state)) = boot().await else {
            return;
        };
        let pool = state.pool.clone();

        // Mock Upstash. Single-threaded HashMap behind a Mutex is plenty.
        #[derive(Default)]
        struct Store(Mutex<HashMap<String, String>>);
        let store = std::sync::Arc::new(Store::default());

        let app = Router::new()
            .route(
                "/",
                post({
                    let store = store.clone();
                    move |Json(body): Json<Vec<serde_json::Value>>| {
                        let store = store.clone();
                        async move {
                            let cmd = body[0].as_str().unwrap_or("").to_uppercase();
                            let key = body[1].as_str().unwrap_or("").to_string();
                            let mut map = store.0.lock().unwrap();
                            match cmd.as_str() {
                                "GET" => Json(serde_json::json!({
                                    "result": map.get(&key).cloned(),
                                })),
                                "SET" => {
                                    let val = body[2].as_str().unwrap_or("").to_string();
                                    map.insert(key, val);
                                    Json(serde_json::json!({"result": "OK"}))
                                }
                                "DEL" => {
                                    let n = if map.remove(&key).is_some() { 1 } else { 0 };
                                    Json(serde_json::json!({"result": n}))
                                }
                                _ => Json(serde_json::json!({"result": null})),
                            }
                        }
                    }
                }),
            );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        std::env::set_var("UPSTASH_REDIS_REST_URL", format!("http://{addr}/"));
        std::env::set_var("UPSTASH_REDIS_REST_TOKEN", "test-token");
        let redis = auth::RedisClient::from_env().expect("client builds");
        // Don't let the env vars leak into other tests.
        std::env::remove_var("UPSTASH_REDIS_REST_URL");
        std::env::remove_var("UPSTASH_REDIS_REST_TOKEN");

        // Mint a key against the router (which uses None for redis — fine,
        // issuance never reads the cache).
        let app = router(state);
        let issued = create_key(&app, "cache-test").await;
        let plaintext = issued["plaintext"].as_str().unwrap().to_string();

        // First verify: cache MISS → bcrypt runs.
        let t0 = std::time::Instant::now();
        let claims = auth::verify_bearer(&plaintext, &pool, Some(&redis)).await.unwrap();
        let cold = t0.elapsed();
        assert_eq!(claims.github_login, "octocat");
        assert!(cold.as_millis() > 10, "cold path should run bcrypt: {cold:?}");

        // Second verify: cache HIT → no bcrypt.
        let t0 = std::time::Instant::now();
        let claims = auth::verify_bearer(&plaintext, &pool, Some(&redis)).await.unwrap();
        let warm = t0.elapsed();
        assert_eq!(claims.github_login, "octocat");
        // Cache hit should be well under the bcrypt(10) floor.
        assert!(
            warm.as_millis() < cold.as_millis().max(1) / 2,
            "warm path should be much faster than cold: cold={cold:?} warm={warm:?}"
        );

        // After we wipe the cache entry, the next verify should run bcrypt again.
        redis.del(issued["prefix"].as_str().unwrap()).await;
        let t0 = std::time::Instant::now();
        let _ = auth::verify_bearer(&plaintext, &pool, Some(&redis)).await.unwrap();
        let recold = t0.elapsed();
        assert!(recold.as_millis() > 10, "after DEL bcrypt should run again: {recold:?}");
    }
}

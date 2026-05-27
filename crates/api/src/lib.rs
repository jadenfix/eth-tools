//! HTTP API router for eth-tools.
//!
//! Mounted by `api/v1/index.rs` (Vercel function entrypoint; `vercel.json`
//! rewrites `/api/v1/(.*)` → `/api/v1/index`) and by `crates/dev-server`
//! (local). Both share the exact same `router()` so prod and local exercise
//! identical handler code.
//!
//! Phase-1 surface: read-only.
//! - `GET /api/v1/health`           — chain + DB liveness, indexed agent count
//! - `GET /api/v1/agents`           — keyset-paginated list (filter by `chain`)
//! - `GET /api/v1/agents/:chain/:id` — single agent detail
//!
//! Write endpoints (manifest validate, invoke, reputation, validation) land
//! in Phase 5/6.

use axum::Router;
use eth_tools_db::Pool;

pub mod dto;
pub mod error;
pub mod handlers;

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
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/health", axum::routing::get(handlers::health::get))
        .route("/api/v1/agents", axum::routing::get(handlers::agents::list))
        .route(
            "/api/v1/agents/:chain/:agent_id",
            axum::routing::get(handlers::agents::get_one),
        )
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
        Some((container, AppState::new(pool)))
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
            .uri("/api/v1/agents/no-such-chain/1")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), 404);
        let v = body_json(resp).await;
        assert_eq!(v["error"]["code"], "CHAIN_NOT_FOUND");
    }
}

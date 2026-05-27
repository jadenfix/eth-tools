//! End-to-end integration tests for the MCP server.
//!
//! Two suites:
//!  1. **Tool dispatch** — boots `EthToolsServer` on a `tokio::io::duplex`
//!     pair, drives it with an rmcp `ClientHandler`, runs `tools/list` and
//!     two `tools/call` invocations (`hash_manifest`, `validate_manifest`).
//!     These tools are DB-free, so the test runs without Docker.
//!  2. **Auth round-trip** — exercises the axum `router(state)` via
//!     `tower::ServiceExt::oneshot`: missing token → 401, bad token → 401,
//!     good token → 200. The auth middleware runs before the rmcp service,
//!     so we can stub the pool with testcontainers (skipped if Docker is
//!     unavailable).

use eth_tools_mcp::{auth::AuthConfig, server::AppState, EthToolsServer};
use rmcp::{
    handler::client::ClientHandler,
    model::{CallToolRequestParams, ClientInfo},
    serve_client, serve_server,
};

// -----------------------------------------------------------------------------
// 1. Tool dispatch (no DB required)

#[derive(Debug, Clone, Default)]
struct DummyClient;

impl ClientHandler for DummyClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::default()
    }
}

/// A tiny stand-in pool. We can't truly run DB-backed tools without
/// Postgres, but `hash_manifest` and `validate_manifest` are pure compute
/// — they never touch the pool — so an unconnected pool is fine here as
/// long as we don't call any other tool. Constructing a real `PgPool`
/// without connecting requires `PgPoolOptions::connect_lazy_with`, which
/// is the right tool for the job (no network, no Docker).
fn lazy_pool() -> sqlx::PgPool {
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    // 127.0.0.1:1 is an unreachable port — the lazy pool defers connection
    // until first use, and our test never triggers a use.
    let opts = PgConnectOptions::new()
        .host("127.0.0.1")
        .port(1)
        .username("u")
        .password("p")
        .database("d");
    PgPoolOptions::new().max_connections(1).connect_lazy_with(opts)
}

#[tokio::test]
async fn tools_list_returns_all_eight_phase5_tools() -> anyhow::Result<()> {
    let (server_io, client_io) = tokio::io::duplex(8192);

    let server = EthToolsServer::new(AppState::new(lazy_pool()));
    let server_handle = tokio::spawn(async move {
        // `serve_server` requires the trait via blanket impl on
        // `ServerHandler`. The `?` propagates the wire-level handshake.
        serve_server(server, server_io).await
    });

    let client = serve_client(DummyClient, client_io).await?;
    let listed = client.list_tools(Default::default()).await?;
    let names: Vec<&str> = listed.tools.iter().map(|t| t.name.as_ref()).collect();

    for expected in [
        "find_agent",
        "inspect_agent",
        "search_agents",
        "validate_manifest",
        "hash_manifest",
        "read_feedback",
        "read_validation",
        "health",
    ] {
        assert!(
            names.contains(&expected),
            "tools/list missing `{expected}`; got: {names:?}"
        );
    }
    assert_eq!(names.len(), 8, "unexpected tool count: {names:?}");

    client.cancel().await?;
    let _ = server_handle.await;
    Ok(())
}

#[tokio::test]
async fn call_hash_manifest_returns_known_sha256() -> anyhow::Result<()> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};

    let (server_io, client_io) = tokio::io::duplex(8192);
    let server = EthToolsServer::new(AppState::new(lazy_pool()));
    let server_handle = tokio::spawn(async move { serve_server(server, server_io).await });
    let client = serve_client(DummyClient, client_io).await?;

    let body = B64.encode(b"hello");
    let res = client
        .call_tool(
            CallToolRequestParams::new("hash_manifest")
                .with_arguments(serde_json::json!({"bytes": body}).as_object().cloned().unwrap()),
        )
        .await?;

    // The tool emits one text-content block of pretty-printed JSON.
    let text = res
        .content
        .first()
        .and_then(|c| c.raw.as_text())
        .map(|t| t.text.as_str())
        .expect("expected text content");
    let parsed: serde_json::Value = serde_json::from_str(text)?;
    assert_eq!(
        parsed["data"]["sha256"].as_str().unwrap(),
        "0x2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
    assert_eq!(parsed["data"]["length"].as_u64().unwrap(), 5);

    client.cancel().await?;
    let _ = server_handle.await;
    Ok(())
}

#[tokio::test]
async fn call_validate_manifest_rejects_missing_name() -> anyhow::Result<()> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};

    let (server_io, client_io) = tokio::io::duplex(8192);
    let server = EthToolsServer::new(AppState::new(lazy_pool()));
    let server_handle = tokio::spawn(async move { serve_server(server, server_io).await });
    let client = serve_client(DummyClient, client_io).await?;

    let body = serde_json::to_vec(&serde_json::json!({
        "endpoints": [{"protocol": "https", "url": "https://example.com/x"}]
    }))?;
    let res = client
        .call_tool(
            CallToolRequestParams::new("validate_manifest").with_arguments(
                serde_json::json!({"bytes": B64.encode(&body)})
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        )
        .await?;
    let text = res
        .content
        .first()
        .and_then(|c| c.raw.as_text())
        .map(|t| t.text.as_str())
        .expect("expected text content");
    let parsed: serde_json::Value = serde_json::from_str(text)?;
    assert_eq!(parsed["data"]["valid"], false);
    let errors = parsed["data"]["errors"].as_array().unwrap();
    assert!(!errors.is_empty(), "expected at least one schema error");

    client.cancel().await?;
    let _ = server_handle.await;
    Ok(())
}

// -----------------------------------------------------------------------------
// 2. Auth round-trip (uses the axum router directly; no rmcp client needed)
//
// We hit `/` (the rmcp service root) with an empty JSON-RPC payload. The
// auth middleware decides 401-vs-pass-through *before* the rmcp service
// sees the body, so even a malformed body counts as a "good" response for
// the auth gate's purposes (we only assert on status, not body shape).

mod auth {
    use super::*;
    use axum::body::Body;
    use http::{Request, StatusCode};
    use tower::ServiceExt;

    fn router_with(token: &str) -> axum::Router {
        // Use `EthToolsServer::new` directly with the lazy pool. The auth
        // middleware short-circuits before any DB call would happen.
        let state = AppState::new(lazy_pool());
        let mcp_service = rmcp::transport::streamable_http_server::tower::StreamableHttpService::<
            EthToolsServer,
            rmcp::transport::streamable_http_server::session::local::LocalSessionManager,
        >::new(
            {
                let s = state.clone();
                move || Ok(EthToolsServer::new(s.clone()))
            },
            std::sync::Arc::new(
                rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default(),
            ),
            // Share the same allowed-hosts config as production so the
            // tests exercise the real DNS-rebinding guard surface.
            eth_tools_mcp::build_mcp_config(),
        );
        let cfg = std::sync::Arc::new(AuthConfig::with_token(token));
        axum::Router::new()
            .nest_service("/", mcp_service)
            .layer(axum::middleware::from_fn_with_state(
                cfg,
                eth_tools_mcp::auth::bearer_token,
            ))
    }

    fn empty_post() -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/json")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn missing_token_is_401() {
        let app = router_with("secret-token");
        let resp = app.oneshot(empty_post()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        // RFC 6750 §3 — the WWW-Authenticate header MUST be present on 401.
        assert!(resp.headers().get("www-authenticate").is_some());
    }

    #[tokio::test]
    async fn wrong_token_is_401() {
        let app = router_with("secret-token");
        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header("authorization", "Bearer not-the-secret")
            .header("content-type", "application/json")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    fn initialize_body() -> serde_json::Value {
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "auth-test", "version": "0"}
            },
            "id": 1
        })
    }

    fn initialize_request(host: &str, token: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/")
            // rmcp's StreamableHttpService validates Host vs `allowed_hosts`.
            // `tower::ServiceExt::oneshot` doesn't synthesize a Host header
            // like a real client would, so we set it explicitly.
            .header("host", host)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(Body::from(serde_json::to_vec(&initialize_body()).unwrap()))
            .unwrap()
    }

    #[tokio::test]
    async fn good_token_initialize_returns_2xx() {
        // The spec's literal "good token → 200" requires a *valid* MCP
        // initialize request through the gate, not just a passed-through
        // empty body. Hit the public prod hostname (no `127.0.0.1`
        // workaround) so this test locks in the deploy-ready allowlist.
        let app = router_with("secret-token");
        let resp = app
            .oneshot(initialize_request("eth-tools.dev", "secret-token"))
            .await
            .unwrap();
        let status = resp.status();
        // Auth gate succeeded → middleware passed to rmcp → rmcp returned a
        // 2xx handshake response. If the gate had rejected, status would be
        // 401; the strict equality below proves both halves.
        assert!(status.is_success(), "expected 2xx after auth gate, got {status}");
        assert_ne!(
            status,
            StatusCode::UNAUTHORIZED,
            "good token must not be rejected by auth middleware"
        );
    }

    /// Regression lock against B1 (Phase-5 MCP deep review): the rmcp
    /// DNS-rebinding guard must admit `Host: eth-tools.dev` so prod
    /// requests dispatch instead of returning 403.
    #[tokio::test]
    async fn host_eth_tools_dev_passes() {
        let app = router_with("secret-token");
        let resp = app
            .oneshot(initialize_request("eth-tools.dev", "secret-token"))
            .await
            .unwrap();
        let status = resp.status();
        assert_ne!(
            status,
            StatusCode::FORBIDDEN,
            "rmcp host allowlist must admit `eth-tools.dev`, got {status}"
        );
        assert!(
            status.is_success(),
            "initialize via prod Host header should 2xx, got {status}"
        );
    }
}

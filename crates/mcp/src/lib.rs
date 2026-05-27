//! MCP server for eth-tools, built on `rmcp` 1.x Streamable HTTP transport.
//!
//! # Surface (Phase-5)
//!
//! Eight **read-side** tools plus four **write-side dry-run previews**
//! (plan §9.2). The write tools (`register_agent`, `give_feedback`,
//! `set_agent_uri`, `request_validation`) encode calldata + run the two
//! pure wallet rails (chain + recipient allowlist) and return a preview;
//! they NEVER broadcast in this PR. Phase 6.2 swaps the preview path for
//! `payments::wallet::sign_and_send`.
//!
//! Remaining write-side tools — `invoke`, `respond_validation`,
//! `set_agent_wallet`, `mcp_from_agent_card`, `wallet_status`,
//! `revoke_feedback`, `estimate_gas`, `generate_manifest`, `check_access`,
//! `explain_access` — defer until the rest of the Phase-6/7 stack lands.
//! See `TOOL_NAMES` below for the full forward-looking list and the inline
//! `TODO(phase-6.2)` markers in `server.rs`.
//!
//! # Architecture
//!
//! [`EthToolsServer`] (in [`server`]) holds the shared [`AppState`] (DB pool)
//! and dispatches tool calls via rmcp's `#[tool_router]` macro. Each tool's
//! input struct lives in `tools::<area>` and derives both
//! [`schemars::JsonSchema`] (so rmcp can expose the tool's input schema
//! over `tools/list`) and [`serde::Deserialize`] (so rmcp can decode the
//! JSON args from `tools/call`).
//!
//! # Mounting
//!
//! [`router`] returns an `axum::Router` that wraps the rmcp Streamable
//! HTTP service plus the bearer-token auth middleware. Mount with
//! `Router::merge` from the Vercel entrypoint.
//!
//! # Auth
//!
//! [`auth`] reads `MCP_BEARER_TOKEN` from the environment. Missing or
//! wrong token → 401. The token is checked via constant-time compare to
//! avoid timing side channels. (Real OAuth-PR + DCR negotiation lands
//! when `crates/api::auth` arrives — track via the TODO in `auth.rs`.)

pub mod auth;
pub mod server;
pub mod tools;

pub use server::{AppState, EthToolsServer};

use axum::Router;
use rmcp::transport::{
    streamable_http_server::{session::local::LocalSessionManager, tower::StreamableHttpService},
    StreamableHttpServerConfig,
};
use std::sync::Arc;

/// Legacy bootstrap constant retained for backward compatibility with the
/// `api/mcp/index.rs` stub from PR #1 (it serialized this list as the
/// service-discovery placeholder). The real tool registry is the
/// `#[tool_router]` derive on [`EthToolsServer`], not this string list.
/// Remove this once the dashboard switches to `tools/list` over MCP.
pub const TOOL_NAMES: &[&str] = &[
    // Phase-5 read-side (implemented):
    "find_agent",
    "inspect_agent",
    "search_agents",
    "validate_manifest",
    "hash_manifest",
    "read_feedback",
    "read_validation",
    "health",
    // Phase-5 write-side dry-run previews (implemented; broadcast in 6.2):
    "register_agent",
    "give_feedback",
    "set_agent_uri",
    "request_validation",
    // Phase-6+ remaining write-side (TODO_WRITE_SIDE):
    "invoke",
    "estimate_gas",
    "revoke_feedback",
    "respond_validation",
    "set_agent_wallet",
    "mcp_from_agent_card",
    "wallet_status",
    "generate_manifest",
    "check_access",
    "explain_access",
];

/// Default host allowlist for the rmcp DNS-rebinding guard.
///
/// rmcp 1.7's `StreamableHttpServerConfig::default()` ships
/// `["localhost", "127.0.0.1", "::1"]`, which 403s every inbound request
/// in production (where the `Host` header is `eth-tools.dev` or a Vercel
/// preview URL). We keep the loopback entries (so local dev keeps
/// working) and add the two well-known prod hostnames; operators add
/// per-deploy preview URLs via `MCP_ALLOWED_HOSTS`.
const DEFAULT_ALLOWED_HOSTS: &[&str] = &[
    "localhost",
    "127.0.0.1",
    "::1",
    "eth-tools.dev",
    "preview.eth-tools.dev",
];

/// Build the `StreamableHttpServerConfig` used by both the production
/// router and the integration tests, with the allowed-hosts list patched
/// to admit the prod hostnames (and any extras from `MCP_ALLOWED_HOSTS`).
///
/// rmcp performs exact authority matching (host + optional port; no glob
/// support — see `host_is_allowed` in
/// `rmcp::transport::streamable_http_server::tower`). Vercel preview
/// deploys (`*-git-*.vercel.app`) therefore must populate
/// `MCP_ALLOWED_HOSTS` with the specific deploy hostname during the build
/// step. This is documented in `.env.example`.
pub fn build_mcp_config() -> StreamableHttpServerConfig {
    let mut hosts: Vec<String> = DEFAULT_ALLOWED_HOSTS.iter().map(|s| (*s).to_string()).collect();
    if let Ok(extra) = std::env::var("MCP_ALLOWED_HOSTS") {
        for h in extra.split(',').map(str::trim).filter(|h| !h.is_empty()) {
            hosts.push(h.to_string());
        }
    }
    StreamableHttpServerConfig::default().with_allowed_hosts(hosts)
}

/// Build the public-facing `axum::Router` for the MCP endpoint.
///
/// Mounts the rmcp Streamable HTTP service at the root and gates every
/// request through [`auth::bearer_token`]. The caller is expected to
/// `nest` or `merge` this router under the desired URL prefix (the Vercel
/// rewrite already targets `/api/mcp/*` so the entrypoint passes `""`).
pub fn router(state: AppState) -> Router {
    // `StreamableHttpService::new` takes a *factory closure* because every
    // session creates a fresh server instance. We clone the AppState into
    // the closure — `AppState::pool` is `Arc`-backed, so this is cheap.
    let state_for_factory = state.clone();
    let mcp_service: StreamableHttpService<EthToolsServer, LocalSessionManager> = StreamableHttpService::new(
        move || Ok(EthToolsServer::new(state_for_factory.clone())),
        Arc::new(LocalSessionManager::default()),
        build_mcp_config(),
    );

    let auth_state = Arc::new(auth::AuthConfig::from_env());

    Router::new()
        // rmcp's StreamableHttpService implements `tower::Service`, so
        // `nest_service` (not `nest`) is the correct mount verb.
        .nest_service("/", mcp_service)
        .layer(axum::middleware::from_fn_with_state(
            auth_state,
            auth::bearer_token,
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_allowed_hosts_include_prod_and_loopback() {
        // Lock in the prod-ready default. If someone re-adds the bare
        // rmcp default (loopback-only), this test fires.
        assert!(DEFAULT_ALLOWED_HOSTS.contains(&"eth-tools.dev"));
        assert!(DEFAULT_ALLOWED_HOSTS.contains(&"preview.eth-tools.dev"));
        assert!(DEFAULT_ALLOWED_HOSTS.contains(&"127.0.0.1"));
    }

    #[test]
    fn build_mcp_config_includes_defaults() {
        // SAFETY: this test mutates a process-wide env var. The mutation
        // is scoped within the test and removed before exit; other tests
        // do not read `MCP_ALLOWED_HOSTS`, so the race window is benign.
        unsafe { std::env::remove_var("MCP_ALLOWED_HOSTS") };
        let cfg = build_mcp_config();
        assert!(cfg.allowed_hosts.iter().any(|h| h == "eth-tools.dev"));
        assert!(cfg.allowed_hosts.iter().any(|h| h == "127.0.0.1"));
    }

    #[test]
    fn build_mcp_config_honors_env_override() {
        unsafe { std::env::set_var("MCP_ALLOWED_HOSTS", "foo.vercel.app, bar.vercel.app") };
        let cfg = build_mcp_config();
        assert!(cfg.allowed_hosts.iter().any(|h| h == "foo.vercel.app"));
        assert!(cfg.allowed_hosts.iter().any(|h| h == "bar.vercel.app"));
        // Defaults must still be present so dev + prod hostnames both work.
        assert!(cfg.allowed_hosts.iter().any(|h| h == "eth-tools.dev"));
        unsafe { std::env::remove_var("MCP_ALLOWED_HOSTS") };
    }

    #[test]
    fn tool_names_includes_all_phase5_tools() {
        // The 8 read-side + 4 write-side tools that this PR ships must be
        // present in the public constant so downstream consumers
        // (dashboard, llms.txt generator) can enumerate them without
        // booting the rmcp server.
        for t in [
            // Read-side
            "find_agent",
            "inspect_agent",
            "search_agents",
            "validate_manifest",
            "hash_manifest",
            "read_feedback",
            "read_validation",
            "health",
            // Write-side dry-run previews
            "register_agent",
            "give_feedback",
            "set_agent_uri",
            "request_validation",
        ] {
            assert!(TOOL_NAMES.contains(&t), "missing tool: {t}");
        }
    }
}

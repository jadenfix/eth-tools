//! MCP server for eth-tools, built on `rmcp` 1.x Streamable HTTP transport.
//!
//! # Surface (Phase-5)
//!
//! Eight **read-side** tools (plan §9.2). Write-side tools — `invoke`,
//! `give_feedback`, `request_validation`, `respond_validation`,
//! `register_agent`, `set_agent_uri`, `set_agent_wallet`,
//! `mcp_from_agent_card`, `wallet_status` — defer until the Phase-6 wallet
//! crate lands. See `TOOL_NAMES` below for the full forward-looking list
//! and the inline `TODO(phase-6)` markers in `tools/mod.rs`.
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
    // Phase-6+ write-side (TODO_WRITE_SIDE):
    "invoke",
    "estimate_gas",
    "give_feedback",
    "revoke_feedback",
    "request_validation",
    "respond_validation",
    "register_agent",
    "set_agent_uri",
    "set_agent_wallet",
    "mcp_from_agent_card",
    "wallet_status",
    "generate_manifest",
    "check_access",
    "explain_access",
];

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
        StreamableHttpServerConfig::default(),
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
    fn tool_names_includes_all_phase5_tools() {
        // The 8 read-side tools that this PR ships must be present in the
        // public constant so downstream consumers (dashboard, llms.txt
        // generator) can enumerate them without booting the rmcp server.
        for t in [
            "find_agent",
            "inspect_agent",
            "search_agents",
            "validate_manifest",
            "hash_manifest",
            "read_feedback",
            "read_validation",
            "health",
        ] {
            assert!(TOOL_NAMES.contains(&t), "missing tool: {t}");
        }
    }
}

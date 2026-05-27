//! `EthToolsServer` — the rmcp `ServerHandler` that dispatches every tool
//! call. The handler holds the shared [`AppState`] (DB pool) and forwards
//! to the per-area helpers in [`crate::tools`].
//!
//! # Why one giant struct?
//!
//! rmcp's `#[tool_router]` derive generates a single `ToolRouter<Self>` per
//! impl block, and `#[tool_handler]` wires that one router into the
//! `ServerHandler` trait. Splitting tools across multiple structs is
//! possible (each gets its own session) but pointless here — all 8
//! read-side tools share the same DB pool and have no per-session state.

use crate::tools;
use eth_tools_db::Pool;
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler,
};

/// Shared state every tool can clone cheaply. `Pool` is `Arc`-backed inside
/// `sqlx`, so this is a pointer copy.
#[derive(Clone, Debug)]
pub struct AppState {
    pub pool: Pool,
}

impl AppState {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

/// The MCP server. One instance per session (rmcp creates fresh ones via
/// the factory closure passed to `StreamableHttpService::new`).
#[derive(Clone)]
pub struct EthToolsServer {
    state: AppState,
    // Read by the generated code from `#[tool_handler]` (rmcp 1.7) — the
    // derive scans for a field of this exact type by name. Without this
    // allow, rustc flags it as dead because the macro expansion happens
    // before deadness analysis can see the reads.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl std::fmt::Debug for EthToolsServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthToolsServer")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

#[tool_router]
impl EthToolsServer {
    pub fn new(state: AppState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    // ----- 1. find_agent — exact lookup by id (calls inspect_agent under
    // the hood when chain is also supplied; otherwise scans both chains).
    #[tool(description = "Locate an ERC-8004 agent by id substring or owner. \
                       Returns the first match per chain (use search_agents for full result sets).")]
    async fn find_agent(
        &self,
        Parameters(args): Parameters<tools::agents::FindAgentArgs>,
    ) -> Result<CallToolResult, McpError> {
        tools::agents::find_agent(&self.state.pool, args)
            .await
            .map(to_json_result)
            .map_err(to_mcp_err)
    }

    // ----- 2. inspect_agent — full agent detail by `(chain, agent_id)`.
    #[tool(description = "Fetch the full record for one ERC-8004 agent: owner, \
                       agent_uri, agent_wallet, registration metadata.")]
    async fn inspect_agent(
        &self,
        Parameters(args): Parameters<tools::agents::InspectAgentArgs>,
    ) -> Result<CallToolResult, McpError> {
        tools::agents::inspect_agent(&self.state.pool, args)
            .await
            .map(to_json_result)
            .map_err(to_mcp_err)
    }

    // ----- 3. search_agents — filtered list with ILIKE on agent_uri.
    #[tool(description = "Search the agent index by free-text query and \
                       optional filters (chain, owner, has_manifest, has_endpoint).")]
    async fn search_agents(
        &self,
        Parameters(args): Parameters<tools::agents::SearchAgentsArgs>,
    ) -> Result<CallToolResult, McpError> {
        tools::agents::search_agents(&self.state.pool, args)
            .await
            .map(to_json_result)
            .map_err(to_mcp_err)
    }

    // ----- 4. validate_manifest — JSON Schema check.
    #[tool(description = "Validate an ERC-8004 agent manifest against the v1 \
                       JSON schema. Pass either `uri` (fetched server-side) \
                       or base64-encoded `bytes`.")]
    async fn validate_manifest(
        &self,
        Parameters(args): Parameters<tools::manifest::ManifestArgs>,
    ) -> Result<CallToolResult, McpError> {
        tools::manifest::validate_manifest(args)
            .await
            .map(to_json_result)
            .map_err(to_mcp_err)
    }

    // ----- 5. hash_manifest — sha256.
    #[tool(description = "Compute the canonical sha256 hash of a manifest. \
                       Pass either `uri` (fetched server-side) or base64-encoded `bytes`.")]
    async fn hash_manifest(
        &self,
        Parameters(args): Parameters<tools::manifest::ManifestArgs>,
    ) -> Result<CallToolResult, McpError> {
        tools::manifest::hash_manifest(args)
            .await
            .map(to_json_result)
            .map_err(to_mcp_err)
    }

    // ----- 6. read_feedback — all on-chain reputation events for one agent.
    #[tool(description = "List on-chain reputation feedback for one agent. \
                       Returns up to 500 rows ordered by (client, index).")]
    async fn read_feedback(
        &self,
        Parameters(args): Parameters<tools::feedback::ReadFeedbackArgs>,
    ) -> Result<CallToolResult, McpError> {
        tools::feedback::read_feedback(&self.state.pool, args)
            .await
            .map(to_json_result)
            .map_err(to_mcp_err)
    }

    // ----- 7. read_validation — single validation lookup by request hash.
    #[tool(description = "Fetch one on-chain validation by its (chain, request_hash) pair.")]
    async fn read_validation(
        &self,
        Parameters(args): Parameters<tools::validation::ReadValidationArgs>,
    ) -> Result<CallToolResult, McpError> {
        tools::validation::read_validation(&self.state.pool, args)
            .await
            .map(to_json_result)
            .map_err(to_mcp_err)
    }

    // ----- 8. health — DB + chain + RPC liveness summary.
    #[tool(description = "MCP server liveness. Returns DB row counts per chain, \
                       last RPC success, and staleness vs newest agent row.")]
    async fn health(&self) -> Result<CallToolResult, McpError> {
        tools::health::health(&self.state.pool)
            .await
            .map(to_json_result)
            .map_err(to_mcp_err)
    }

    // ----- 9. register_agent — dry-run preview of `Identity.register(uri)`.
    //
    // Phase 6.2 will add a `dry_run: bool = true` argument to all four write
    // tools and, on `dry_run = false`, swap the rails-only preview for
    // `payments::wallet::sign_and_send`. Until then EVERY call is a preview;
    // we never broadcast a transaction from this PR.
    #[tool(description = "Dry-run preview for `Identity.register(string agentUri)`. \
                       Encodes calldata, runs the pure wallet rails (chain + \
                       recipient allowlist), and returns `{would_proceed, \
                       denied_reason?, calldata_preview, gas_estimate, \
                       estimated_cost_cents}`. Real signing lands in phase 6.2.")]
    async fn register_agent(
        &self,
        Parameters(args): Parameters<tools::writes::RegisterAgentArgs>,
    ) -> Result<CallToolResult, McpError> {
        tools::writes::register_agent(args)
            .map(to_json_result)
            .map_err(to_mcp_err)
    }

    // ----- 10. give_feedback — dry-run preview of `Reputation.giveFeedback`.
    #[tool(description = "Dry-run preview for `Reputation.giveFeedback(uint256, \
                       int8, uint8, bytes32, bytes32, string, string, bytes32)`. \
                       Encodes calldata + rails check; never broadcasts. \
                       Phase 6.2 enables real signing.")]
    async fn give_feedback(
        &self,
        Parameters(args): Parameters<tools::writes::GiveFeedbackArgs>,
    ) -> Result<CallToolResult, McpError> {
        tools::writes::give_feedback(args)
            .map(to_json_result)
            .map_err(to_mcp_err)
    }

    // ----- 11. set_agent_uri — dry-run preview of `Identity.setURI`.
    //
    // The rails confirm the recipient is the Identity registry; on-chain
    // ownership of `agent_id` is enforced by the registry itself when the
    // tx lands. API keys are not yet bound to wallets so we cannot
    // pre-flight the owner check from the MCP side.
    #[tool(description = "Dry-run preview for `Identity.setURI(uint256 agentId, \
                       string newUri)`. Caller ownership is enforced on-chain \
                       by the registry (API keys are not yet bound to wallets, \
                       so a successful preview does not guarantee a successful \
                       broadcast). Phase 6.2 enables real signing.")]
    async fn set_agent_uri(
        &self,
        Parameters(args): Parameters<tools::writes::SetAgentUriArgs>,
    ) -> Result<CallToolResult, McpError> {
        tools::writes::set_agent_uri(args)
            .map(to_json_result)
            .map_err(to_mcp_err)
    }

    // ----- 12. request_validation — dry-run preview of
    //                                `Validation.requestValidation`.
    #[tool(description = "Dry-run preview for `Validation.requestValidation(\
                       uint256 agentId, string requestURI)`. Phase 6.2 enables \
                       real signing.")]
    async fn request_validation(
        &self,
        Parameters(args): Parameters<tools::writes::RequestValidationArgs>,
    ) -> Result<CallToolResult, McpError> {
        tools::writes::request_validation(args)
            .map(to_json_result)
            .map_err(to_mcp_err)
    }

    // TODO(phase-6.2): swap each write tool's body for the signing path:
    //   - resolve the active hot-wallet via `wallet::current()`
    //   - estimate gas via `eth_tools_rpc::provider()`
    //   - call `payments::wallet::sign_and_send`
    //   - on `dry_run = true` (default) keep returning the preview shape
    //
    // The output schema already carries `would_proceed` + `denied_reason`,
    // so the swap is binary-compatible from the MCP-client side.
}

#[tool_handler]
impl ServerHandler for EthToolsServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_instructions(
                "eth-tools MCP server. Read-side tools for ERC-8004 agent \
             registry on Base + Base Sepolia. See /.well-known/oauth-protected-resource \
             for auth metadata."
                    .to_string(),
            )
    }
}

/// Serialize a tool's domain return value as a single text-content block
/// holding pretty-printed JSON. MCP clients (including the Anthropic
/// Claude.ai client) render JSON text blocks well; structured `Content::json`
/// is reserved for cases where the value is itself the artifact.
fn to_json_result<T: serde::Serialize>(v: T) -> CallToolResult {
    let body = serde_json::to_string_pretty(&v).unwrap_or_else(|e| {
        // Serialization shouldn't fail for our shapes, but if it does we
        // surface a structured error rather than panicking inside rmcp's
        // task — a panic here would kill the session.
        format!(r#"{{"error":"serialize: {e}"}}"#)
    });
    CallToolResult::success(vec![Content::text(body)])
}

/// Map our domain `ToolError` into rmcp's wire-level [`ErrorData`]. All
/// tool errors are surfaced as `INVALID_PARAMS` or `INTERNAL_ERROR`
/// (JSON-RPC codes -32602 / -32603); 4xx-style domain errors (not-found,
/// etc.) ride inside a successful `CallToolResult` so the client can
/// reason about them as data, not as a session-level fault.
fn to_mcp_err(e: tools::ToolError) -> McpError {
    use tools::ToolError::*;
    match e {
        InvalidInput(msg) => McpError::invalid_params(msg, None),
        Other(msg) => McpError::internal_error(msg, None),
    }
}

//! Tool implementations. Each module exposes:
//!  - one or more `*Args` input structs deriving `JsonSchema + Deserialize`
//!    (so rmcp's `#[tool]` macro can expose the input schema + decode args)
//!  - one or more `async fn …(pool, args) -> Result<impl Serialize, ToolError>`
//!    that the rmcp `EthToolsServer` calls.
//!
//! Tools talk to the DB and the in-process chain registry directly. They
//! never re-enter the HTTP router — that would force unnecessary header
//! plumbing through axum and lose typed errors at the boundary.

pub mod agents;
pub mod feedback;
pub mod health;
pub mod manifest;
pub mod validation;

use serde::Serialize;

/// Errors that bubble out of a tool implementation. Mapped to rmcp's
/// `ErrorData` in `server.rs::to_mcp_err`. Distinct from
/// `eth_tools_api::ApiError` because (a) the API crate is HTTP-flavored
/// (it carries `StatusCode`s) and (b) we want MCP tools to surface
/// domain-level "no such row" as **data inside a successful tool result**,
/// not as a JSON-RPC error. Only true input-validation and infra failures
/// become `ToolError`s.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("internal: {0}")]
    Other(String),
}

impl From<sqlx::Error> for ToolError {
    fn from(e: sqlx::Error) -> Self {
        // Log the full error server-side; only a short message goes to the
        // client (sqlx errors can leak table names and connection strings).
        tracing::error!(error = ?e, "tool db error");
        Self::Other("database error".into())
    }
}

/// Helper: every read tool wraps its data in a `{data, source}` envelope
/// so MCP clients can pattern-match the same way they do on REST endpoints.
/// `staleness_ms` is included only when the underlying query is timestamp-
/// aware; otherwise it's omitted.
#[derive(Debug, Serialize)]
pub struct DataEnvelope<T: Serialize> {
    pub data: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub staleness_ms: Option<i64>,
    pub source: &'static str,
}

impl<T: Serialize> DataEnvelope<T> {
    pub fn db(data: T) -> Self {
        Self {
            data,
            staleness_ms: None,
            source: "db",
        }
    }

    pub fn local(data: T) -> Self {
        Self {
            data,
            staleness_ms: None,
            source: "local",
        }
    }
}

/// Parse a `chain` argument that's either a chain name (`"base"`) or a
/// decimal chain id (string or integer JSON value). Used by every tool
/// that takes a chain parameter.
pub fn resolve_chain(s: &str) -> Result<&'static eth_tools_core::Chain, ToolError> {
    if let Ok(id) = s.parse::<u64>() {
        if let Some(c) = eth_tools_core::chains::by_id(id) {
            return Ok(c);
        }
    }
    eth_tools_core::chains::by_name(s).ok_or_else(|| {
        ToolError::InvalidInput(format!(
            "unknown chain `{s}` — supported: base, base-sepolia (or chain_id 8453, 84532)"
        ))
    })
}

/// `chain` field shared by several tool inputs. Modelled as a `String` so
/// the generated JSON schema is simple; the runtime parser accepts both
/// names and decimal ids. (rmcp 1.x's schemars 1.0 doesn't have a clean
/// way to model JSON-Schema `oneOf: [string, integer]` for a single field
/// without giving up `Deserialize` ergonomics, so we keep it stringly-typed
/// and document the union in the field's description.)
pub type ChainArg = String;

//! Emit the OpenAPI doc consumed by `openapi-typescript` to produce
//! `app/lib/api-types.ts`.
//!
//! Hand-rolled for Phase 1 to keep the diff small; `utoipa` integration on
//! every handler lands in a follow-up so we don't have to chase a moving
//! target while iterating on the API shape.
//!
//! Keep this in sync with `crates/api::router` whenever a route is added or
//! a DTO shape changes — CI's `pnpm gen:check` enforces that the committed
//! `app/openapi.json` and `app/lib/api-types.ts` match what this binary
//! produces.

use serde_json::json;

fn main() -> anyhow::Result<()> {
    let doc = json!({
        "openapi": "3.0.0",
        "info": {
            "title": "eth-tools API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Self-maintaining runtime for ERC-8004 trustless agents.",
            "license": { "name": "MIT" }
        },
        "servers": [
            { "url": "https://eth-tools.dev",         "description": "Production" },
            { "url": "https://preview.eth-tools.dev", "description": "Preview" },
            { "url": "http://localhost:3000",         "description": "Local dev" }
        ],
        "components": {
            "schemas": {
                "Agent": {
                    "type": "object",
                    "required": ["chain", "chain_id", "agent_id", "owner", "registered_at", "updated_at"],
                    "properties": {
                        "chain":         { "type": "string", "example": "base" },
                        "chain_id":      { "type": "integer", "format": "int64", "example": 8453 },
                        "agent_id":      { "type": "string", "description": "uint256 as decimal string", "example": "42" },
                        "owner":         { "type": "string", "description": "0x-prefixed 20-byte hex" },
                        "agent_uri":     { "type": "string", "nullable": true },
                        "agent_wallet":  { "type": "string", "nullable": true, "description": "0x-prefixed; cleared on ownership Transfer (spec gotcha)" },
                        "registered_at": { "type": "string", "format": "date-time" },
                        "updated_at":    { "type": "string", "format": "date-time" }
                    }
                },
                "ChainHealth": {
                    "type": "object",
                    "required": ["chain_id", "name", "is_testnet", "agents_indexed"],
                    "properties": {
                        "chain_id":       { "type": "integer", "format": "int64" },
                        "name":           { "type": "string" },
                        "is_testnet":     { "type": "boolean" },
                        "agents_indexed": { "type": "integer", "format": "int64" }
                    }
                },
                "Health": {
                    "type": "object",
                    "required": ["status", "version", "agents_indexed", "chains"],
                    "properties": {
                        "status":          { "type": "string", "enum": ["ok"] },
                        "version":         { "type": "string" },
                        "agents_indexed":  { "type": "integer", "format": "int64" },
                        "chains":          { "type": "array", "items": { "$ref": "#/components/schemas/ChainHealth" } }
                    }
                },
                "AgentList": {
                    "type": "object",
                    "required": ["data", "staleness_ms", "source"],
                    "properties": {
                        "data":         { "type": "array", "items": { "$ref": "#/components/schemas/Agent" } },
                        "next_cursor":  { "type": "string", "nullable": true, "description": "Opaque; pass verbatim as ?cursor= to page" },
                        "staleness_ms": { "type": "integer", "format": "int64" },
                        "source":       { "type": "string", "enum": ["db", "cache", "rpc"] }
                    }
                },
                "AgentDetail": {
                    "type": "object",
                    "required": ["data", "staleness_ms", "source"],
                    "properties": {
                        "data":         { "$ref": "#/components/schemas/Agent" },
                        "staleness_ms": { "type": "integer", "format": "int64" },
                        "source":       { "type": "string" }
                    }
                },
                "Error": {
                    "type": "object",
                    "required": ["error"],
                    "properties": {
                        "error": {
                            "type": "object",
                            "required": ["code", "policy_version", "evaluator"],
                            "properties": {
                                "code":           { "type": "string", "example": "AGENT_NOT_FOUND" },
                                "policy_version": { "type": "string", "example": "v1" },
                                "evaluator":      { "type": "string", "example": "api.lookup" },
                                "override_hint":  { "type": "string", "nullable": true }
                            }
                        }
                    }
                }
            }
        },
        "paths": {
            "/api/v1/health": {
                "get": {
                    "summary": "Service health, chain registry, indexed agent count",
                    "responses": {
                        "200": {
                            "description": "ok",
                            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Health" } } }
                        }
                    }
                }
            },
            "/api/v1/agents": {
                "get": {
                    "summary": "List indexed agents (keyset-paginated)",
                    "parameters": [
                        { "name": "chain",  "in": "query", "schema": { "type": "string" }, "description": "Filter by chain name (e.g. 'base') or chain_id (e.g. '8453')" },
                        { "name": "limit",  "in": "query", "schema": { "type": "integer", "default": 50, "minimum": 1, "maximum": 200 } },
                        { "name": "cursor", "in": "query", "schema": { "type": "string" }, "description": "Opaque keyset cursor from a prior response's next_cursor" }
                    ],
                    "responses": {
                        "200": {
                            "description": "ok",
                            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/AgentList" } } }
                        },
                        "400": {
                            "description": "invalid cursor or query",
                            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } }
                        }
                    }
                }
            },
            "/api/v1/agents/{chain}/{agent_id}": {
                "get": {
                    "summary": "Get a single agent by chain and id",
                    "parameters": [
                        { "name": "chain",    "in": "path", "required": true, "schema": { "type": "string" } },
                        { "name": "agent_id", "in": "path", "required": true, "schema": { "type": "string" }, "description": "uint256 as decimal string" }
                    ],
                    "responses": {
                        "200": {
                            "description": "ok",
                            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/AgentDetail" } } }
                        },
                        "404": {
                            "description": "agent or chain not found",
                            "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } }
                        }
                    }
                }
            }
        }
    });
    println!("{}", serde_json::to_string_pretty(&doc)?);
    Ok(())
}

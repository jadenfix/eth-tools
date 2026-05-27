//! Emits the OpenAPI doc consumed by `openapi-typescript` to produce
//! `app/lib/api-types.ts`. Bootstrap version emits a stub document so the
//! `pnpm gen:types` CI gate has something to read.

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
            { "url": "https://eth-tools.dev", "description": "Production" },
            { "url": "https://preview.eth-tools.dev", "description": "Preview" },
            { "url": "http://localhost:3000", "description": "Local dev" }
        ],
        "paths": {
            "/api/v1/health": {
                "get": {
                    "summary": "Service health and per-chain freshness",
                    "responses": { "200": { "description": "ok" } }
                }
            }
        }
    });
    println!("{}", serde_json::to_string_pretty(&doc)?);
    Ok(())
}

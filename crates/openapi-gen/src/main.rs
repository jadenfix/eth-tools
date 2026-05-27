//! Emit the OpenAPI doc consumed by `openapi-typescript` to produce
//! `app/lib/api-types.ts`.
//!
//! The doc itself lives next to the handlers in `crates/api::openapi::ApiDoc`,
//! built from `#[utoipa::path(...)]` annotations + `ToSchema` derives — so the
//! wire contract can never silently drift from the handler signatures.
//!
//! CI's `pnpm gen:check` re-runs this and asserts no diff against the
//! committed `app/openapi.json` and `app/lib/api-types.ts`.

use eth_tools_api::openapi::ApiDoc;
use utoipa::OpenApi;

fn main() -> anyhow::Result<()> {
    let mut doc = ApiDoc::openapi();
    // Bind the OpenAPI `info.version` to the workspace package version so
    // bumping `Cargo.toml` propagates to the spec without a manual edit.
    doc.info.version = env!("CARGO_PKG_VERSION").to_string();
    println!("{}", serde_json::to_string_pretty(&doc)?);
    Ok(())
}

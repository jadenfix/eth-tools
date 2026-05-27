//! `utoipa::OpenApi` doc — single source of truth for the wire contract.
//!
//! Consumed by `crates/openapi-gen` to dump `app/openapi.json`, which the
//! TypeScript client (`app/lib/api-types.ts`) is generated from. CI's
//! `pnpm gen:check` enforces that the committed JSON + TS files match what
//! this module produces.
//!
//! Every handler annotated with `#[utoipa::path(...)]` must be listed in the
//! `paths(...)` array below, and every schema referenced by those handlers
//! (transitively) must be listed in `components(schemas(...))`.

#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "eth-tools API",
        version = "0.0.1",
        description = "Self-maintaining runtime for ERC-8004 trustless agents.",
        license(name = "MIT")
    ),
    servers(
        (url = "https://eth-tools.dev", description = "Production"),
        (url = "https://preview.eth-tools.dev", description = "Preview"),
        (url = "http://localhost:3000", description = "Local dev"),
    ),
    paths(
        crate::handlers::health::get,
        crate::handlers::agents::list,
        crate::handlers::agents::get_one,
    ),
    components(schemas(
        crate::dto::AgentDto,
        crate::dto::AgentList,
        crate::dto::AgentDetail,
        crate::dto::ApiErrorBody,
        crate::dto::ApiErrorPayload,
        crate::handlers::health::Health,
        crate::handlers::health::ChainHealth,
    )),
    tags(
        (name = "health", description = "Liveness and chain registry"),
        (name = "agents", description = "Read-only ERC-8004 agent lookups"),
    ),
)]
pub struct ApiDoc;

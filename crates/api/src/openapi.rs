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
        // Read-side
        crate::handlers::health::get,
        crate::handlers::agents::list,
        crate::handlers::agents::get_one,
        crate::handlers::agents::search,
        crate::handlers::manifest::validate,
        crate::handlers::manifest::hash,
        crate::handlers::manifest::generate,
        crate::handlers::access::check,
        crate::handlers::access::explain,
        crate::handlers::reputation::read,
        crate::handlers::validation::read,
        // Write-side
        crate::handlers::invoke::prepare,
        crate::handlers::invoke::execute,
        crate::handlers::reputation::give,
        crate::handlers::validation::request_validation,
        crate::handlers::validation::respond_validation,
    ),
    components(schemas(
        // Existing
        crate::dto::AgentDto,
        crate::dto::AgentList,
        crate::dto::AgentDetail,
        crate::dto::ApiErrorBody,
        crate::dto::ApiErrorPayload,
        crate::handlers::health::Health,
        crate::handlers::health::ChainHealth,
        // Phase-4 — request DTOs
        crate::dto::SearchRequest,
        crate::dto::SearchFilters,
        crate::dto::ManifestInput,
        crate::dto::AccessRequest,
        crate::dto::ReputationGiveRequest,
        crate::dto::ValidationRequest,
        crate::dto::ValidationRespond,
        crate::dto::InvokePrepare,
        crate::dto::InvokeExecute,
        // Phase-4 — response DTOs
        crate::dto::ManifestValidateResponse,
        crate::dto::ManifestError,
        crate::dto::ManifestHashResponse,
        crate::dto::ManifestGenerateResponse,
        crate::dto::AccessCheckResponse,
        crate::dto::AccessExplainResponse,
        crate::dto::AccessStep,
        crate::dto::FeedbackDto,
        crate::dto::ReputationList,
        crate::dto::ValidationDto,
        crate::dto::ValidationDetail,
        crate::dto::InvokePrepareResponse,
    )),
    tags(
        (name = "health",     description = "Liveness and chain registry"),
        (name = "agents",     description = "ERC-8004 agent lookups + search"),
        (name = "manifest",   description = "Agent-card validation, hashing, canonicalisation"),
        (name = "access",     description = "Policy engine: allow/deny + explain"),
        (name = "reputation", description = "ERC-8004 feedback reads + writes"),
        (name = "validation", description = "ERC-8004 validation rows + writes"),
        (name = "invoke",     description = "Calldata prep + on-chain execution (stubbed until wallet rails)"),
    ),
)]
pub struct ApiDoc;

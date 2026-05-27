//! `eth-tools` CLI entry point.
//!
//! Scope (this PR): auth (login/whoami/logout), find, inspect, manifest
//! validate/hash, health. Other plan §9.3 commands (register, invoke, watch,
//! mcp, workers, wallet, backfill) are intentionally deferred.

use clap::{Parser, Subcommand};
use eth_tools_cli::client::DEFAULT_API_URL;
use eth_tools_cli::commands::{self, Ctx};
use eth_tools_cli::config::ConfigPaths;

#[derive(Parser)]
#[command(
    name = "eth-tools",
    version,
    about = "Self-maintaining runtime for ERC-8004 trustless agents"
)]
struct Cli {
    /// Override the API base URL. Falls back to `$ETH_TOOLS_API_URL` then the
    /// compiled-in default (`https://eth-tools.dev`).
    #[arg(long, global = true, env = "ETH_TOOLS_API_URL", default_value = DEFAULT_API_URL)]
    api_url: String,

    /// Emit raw JSON rather than the human-readable rendering.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage CLI credentials.
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    /// Search indexed agents by free-text query.
    Find {
        /// Query string forwarded to `POST /api/v1/agents/search`.
        query: String,
    },
    /// Inspect a single agent. Format: `<chain>/<agent_id>` (e.g. `base/42`).
    Inspect { reference: String },
    /// Agent-card / manifest tooling.
    Manifest {
        #[command(subcommand)]
        action: ManifestAction,
    },
    /// Hit `/api/v1/health` and render the chain registry.
    Health,
}

#[derive(Subcommand)]
enum AuthAction {
    /// Print the dashboard URL, open it in a browser, and store the pasted key.
    Login,
    /// Call `/api/v1/auth/whoami` with the stored token.
    Whoami,
    /// Remove the local credentials file.
    Logout,
}

#[derive(Subcommand)]
enum ManifestAction {
    /// POST a manifest file to `/api/v1/manifest/validate` and print JSON-pointer errors.
    Validate { path: String },
    /// POST a manifest file to `/api/v1/manifest/hash` and print sha256 + keccak256.
    Hash { path: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // Test/CI override: `$ETH_TOOLS_CONFIG_DIR` short-circuits XDG discovery
    // so that `assert_cmd` integration tests don't stomp on the developer's
    // real credentials file.
    let paths = match std::env::var("ETH_TOOLS_CONFIG_DIR") {
        Ok(p) if !p.is_empty() => ConfigPaths::at(p),
        _ => ConfigPaths::discover()?,
    };
    let ctx = Ctx {
        api_url: cli.api_url.clone(),
        json: cli.json,
        paths,
    };

    match cli.command {
        Command::Auth { action } => match action {
            AuthAction::Login => commands::auth::login(&ctx).await,
            AuthAction::Whoami => commands::auth::whoami(&ctx).await,
            AuthAction::Logout => commands::auth::logout(&ctx).await,
        },
        Command::Find { query } => commands::find::run(&ctx, &query).await,
        Command::Inspect { reference } => commands::inspect::run(&ctx, &reference).await,
        Command::Manifest { action } => match action {
            ManifestAction::Validate { path } => commands::manifest::validate(&ctx, &path).await,
            ManifestAction::Hash { path } => commands::manifest::hash(&ctx, &path).await,
        },
        Command::Health => commands::health::run(&ctx).await,
    }
}

//! `eth-tools` CLI entry point.
//!
//! Scope (post phase-7 completion): auth (login/whoami/logout), find,
//! inspect, manifest {validate,hash}, health, register, invoke, watch,
//! mcp {install,from-card}, workers status, wallet status, backfill.
//! All plan §9.3 commands (13 total) are now wired.

use clap::{Parser, Subcommand};
use eth_tools_cli::client::{validate_api_url, DEFAULT_API_URL};
use eth_tools_cli::commands::{self, Ctx};
use eth_tools_cli::config::ConfigPaths;

/// Clap value-parser that runs [`validate_api_url`] for `--api-url`, the env
/// var, AND the compiled-in default — so a misconfigured `$ETH_TOOLS_API_URL`
/// is rejected at startup rather than after a network round trip.
fn parse_api_url(s: &str) -> Result<String, String> {
    validate_api_url(s).map_err(|e| e.to_string())
}

#[derive(Parser)]
#[command(
    name = "eth-tools",
    version,
    about = "Self-maintaining runtime for ERC-8004 trustless agents"
)]
struct Cli {
    /// Override the API base URL. Falls back to `$ETH_TOOLS_API_URL` then the
    /// compiled-in default (`https://eth-tools.dev`). Must be `https://`, or
    /// `http://` against `127.0.0.1`/`localhost`/`::1`.
    #[arg(
        long,
        global = true,
        env = "ETH_TOOLS_API_URL",
        default_value = DEFAULT_API_URL,
        value_parser = parse_api_url,
    )]
    api_url: String,

    /// Bypass the safety check that refuses to open a browser for a
    /// non-default `--api-url`. Only meaningful for `auth login`.
    #[arg(long, global = true)]
    allow_untrusted_login: bool,

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
    /// Register a new ERC-8004 agent (interactive or via a manifest file).
    Register {
        /// Walk the user through name/description/skills/services prompts.
        #[arg(long, conflicts_with = "manifest")]
        interactive: bool,
        /// Path to a pre-assembled manifest JSON file (non-interactive).
        #[arg(long, conflicts_with = "interactive")]
        manifest: Option<String>,
    },
    /// Invoke a registered agent. Reference: `<chain>/<agent_id>`.
    Invoke {
        reference: String,
        /// JSON file forwarded as the agent's `input`.
        #[arg(long)]
        input: String,
        /// EIP-3009 payment authorization (for x402-gated agents).
        #[arg(long)]
        x_payment: Option<String>,
        /// Placeholder for client-side EIP-3009 signing (phase-6).
        #[arg(long)]
        auto_pay: bool,
    },
    /// Tail registry events for a chain (polls /api/v1/agents; SSE is TODO).
    Watch {
        chain: String,
        /// Override the poll interval in seconds (default 10).
        #[arg(long)]
        interval: Option<u64>,
        /// Stop after this many polls (test seam; default: never).
        #[arg(long, hide = true)]
        max_iters: Option<usize>,
    },
    /// MCP integrations: install client configs, generate Deno shims.
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },
    /// Background worker visibility.
    Workers {
        #[command(subcommand)]
        action: WorkersAction,
    },
    /// Wallet visibility (balance, spend, allowlist, kill switch).
    Wallet {
        #[command(subcommand)]
        action: WalletAction,
    },
    /// Backfill the registry index from a given block to current head.
    Backfill {
        #[arg(long)]
        chain: String,
        #[arg(long = "from-block")]
        from_block: u64,
        /// Print what would be scanned without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
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

#[derive(Subcommand)]
enum McpAction {
    /// Write the eth-tools stanza into Claude Desktop / Cursor MCP configs.
    Install,
    /// Fetch an agent card by URL and emit a Deno TS MCP shim.
    #[command(name = "from-card")]
    FromCard {
        url: String,
        /// Output path for the generated TS file (default: ./mcp-<slug>.ts).
        #[arg(long)]
        out: Option<String>,
    },
}

#[derive(Subcommand)]
enum WorkersAction {
    /// Render the worker-status table from `/api/v1/workers/status`.
    Status,
}

#[derive(Subcommand)]
enum WalletAction {
    /// Render wallet posture from `/api/v1/wallet/status`.
    Status,
}

/// Replace the default panic hook with a redacted one-liner so a Rust panic
/// never leaks a backtrace, source path, or local variable to the terminal.
/// Release builds also have `panic = "abort"`, so the process dies immediately
/// after; the explicit `exit(2)` is the fallback for `cargo test` / debug
/// builds where the hook would otherwise unwind.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("panicked");
        eprintln!("eth-tools: internal error: {msg}");
        std::process::exit(2);
    }));
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    install_panic_hook();
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
        allow_untrusted_login: cli.allow_untrusted_login,
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
        Command::Register {
            interactive,
            manifest,
        } => match (interactive, manifest) {
            (true, _) => commands::register::interactive(&ctx).await,
            (false, Some(p)) => commands::register::from_file(&ctx, &p).await,
            (false, None) => Err(anyhow::anyhow!(
                "register: pass either --interactive or --manifest <path>"
            )),
        },
        Command::Invoke {
            reference,
            input,
            x_payment,
            auto_pay,
        } => {
            commands::invoke::run(&ctx, &reference, &input, x_payment.as_deref(), auto_pay).await
        }
        Command::Watch {
            chain,
            interval,
            max_iters,
        } => commands::watch::run(&ctx, &chain, interval, max_iters).await,
        Command::Mcp { action } => match action {
            McpAction::Install => commands::mcp::install(&ctx).await,
            McpAction::FromCard { url, out } => {
                commands::mcp::from_card(&ctx, &url, out.as_deref()).await
            }
        },
        Command::Workers { action } => match action {
            WorkersAction::Status => commands::workers::status(&ctx).await,
        },
        Command::Wallet { action } => match action {
            WalletAction::Status => commands::wallet::status(&ctx).await,
        },
        Command::Backfill {
            chain,
            from_block,
            dry_run,
        } => commands::backfill::run(&ctx, &chain, from_block, dry_run).await,
    }
}

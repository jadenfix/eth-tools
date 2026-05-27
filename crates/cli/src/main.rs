//! `eth-tools` CLI. Bootstrap version: subcommands are declared but unimplemented.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "eth-tools",
    version,
    about = "Self-maintaining runtime for ERC-8004 trustless agents"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Authenticate against eth-tools.dev and store an API key locally.
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    /// Search agents by free-text query.
    Find { query: String },
    /// Inspect a single agent: `eth-tools inspect base/42`.
    Inspect { reference: String },
    /// Manifest tools.
    Manifest {
        #[command(subcommand)]
        action: ManifestAction,
    },
    /// Show worker telemetry.
    Workers {
        #[command(subcommand)]
        action: WorkersAction,
    },
    /// Wallet status.
    Wallet {
        #[command(subcommand)]
        action: WalletAction,
    },
}

#[derive(Subcommand)]
enum AuthAction {
    Login,
    Whoami,
    Logout,
}

#[derive(Subcommand)]
enum ManifestAction {
    Validate { path: String },
    Hash { path: String },
    Generate,
}

#[derive(Subcommand)]
enum WorkersAction {
    Status,
}

#[derive(Subcommand)]
enum WalletAction {
    Status,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Auth { action: _ }
        | Command::Find { .. }
        | Command::Inspect { .. }
        | Command::Manifest { .. }
        | Command::Workers { .. }
        | Command::Wallet { .. } => {
            eprintln!("eth-tools CLI bootstrap — subcommand not yet implemented");
            std::process::exit(2);
        }
    }
}

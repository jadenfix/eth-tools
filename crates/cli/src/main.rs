//! Read API and local manifest CLI. Unsupported mutating commands fail closed.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "eth-tools",
    version,
    about = "Self-maintaining runtime for ERC-8004 trustless agents"
)]
struct Cli {
    #[arg(long, env = "ETH_TOOLS_API_URL", default_value = "http://127.0.0.1:3000")]
    api_url: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Index one bounded confirmed Base identity-log page; no transactions signed.
    IndexRegistry {
        #[arg(long)]
        dryrun: bool,
    },
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
        Command::IndexRegistry { dryrun } => {
            let deps = eth_tools_workers::deps()
                .await
                .map_err(|_| anyhow::anyhow!("Indexer dependencies unavailable"))?;
            let context = eth_tools_workers::WorkerContext {
                pool: deps.pool.clone(),
                rpc: deps.rpc.clone(),
                vercel_env: "local".into(),
                dryrun,
                force: false,
            };
            let summary = eth_tools_workers::registry::run(context)
                .await
                .map_err(|_| anyhow::anyhow!("Indexer failed; no batch committed"))?;
            println!("{}", serde_json::to_string(&summary)?);
        }
        Command::Find { query } => {
            let mut value = read_api(&cli.api_url, &["api", "v1", "agents"]).await?;
            let rows = value["data"]
                .as_array_mut()
                .ok_or_else(|| anyhow::anyhow!("API agents response has no data array"))?;
            let needle = query.to_lowercase();
            rows.retain(|row| needle.is_empty() || row.to_string().to_lowercase().contains(&needle));
            println!("{value}");
        }
        Command::Inspect { reference } => {
            let (chain, id) = reference
                .split_once('/')
                .ok_or_else(|| anyhow::anyhow!("reference must be chain/id"))?;
            anyhow::ensure!(
                !chain.is_empty() && !id.is_empty() && id.bytes().all(|c| c.is_ascii_digit()),
                "reference must be chain/decimal-id"
            );
            println!(
                "{}",
                read_api(&cli.api_url, &["api", "v1", "agents", chain, id]).await?
            );
        }
        Command::Manifest {
            action: ManifestAction::Hash { path },
        } => {
            let bytes = std::fs::read(path)?;
            let hash = eth_tools_core::manifest::keccak256_raw_bytes(&bytes);
            print!("0x");
            for b in hash {
                print!("{b:02x}");
            }
            println!();
        }
        Command::Manifest {
            action: ManifestAction::Validate { path },
        } => {
            let card: eth_tools_core::manifest::AgentCard = serde_json::from_slice(&std::fs::read(path)?)?;
            anyhow::ensure!(!card.name.trim().is_empty(), "manifest name is required");
            for service in card.services {
                let url = reqwest::Url::parse(&service.endpoint)?;
                anyhow::ensure!(
                    matches!(url.scheme(), "http" | "https"),
                    "service endpoint must use HTTP(S)"
                );
            }
            println!("{{\"valid\":true,\"scope\":\"AgentCard structure and HTTP(S) endpoints\"}}");
        }
        _ => anyhow::bail!("This command is not implemented; no action was performed"),
    }
    Ok(())
}

async fn read_api(base: &str, segments: &[&str]) -> anyhow::Result<serde_json::Value> {
    let mut url = reqwest::Url::parse(base)?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https"),
        "API endpoint must use HTTP(S)"
    );
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("API endpoint cannot be a base URL"))?
        .clear()
        .extend(segments);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("API transport failed"))?;
    anyhow::ensure!(
        response.status().is_success(),
        "API HTTP status {}",
        response.status().as_u16()
    );
    Ok(response.json().await?)
}

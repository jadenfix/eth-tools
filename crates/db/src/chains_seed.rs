//! Seed the `chains` table from `eth_tools_core::chains::CHAINS`.
//!
//! Idempotent: `ON CONFLICT (chain_id) DO UPDATE` so re-running matches
//! whatever the latest core::chains constant says (rpc_url, name, addresses).
//! Called from dev-server boot and from migration 0002 (which embeds the same
//! data so prod and preview deploys don't need a separate seed step).

use eth_tools_core::CHAINS;
use sqlx::PgPool;

/// Default RPC URL to seed alongside the chain row. Production callers should
/// override via `update_rpc_url()` once the project's Alchemy/QuickNode URL
/// is in env. Public RPCs work for read-only operations.
fn default_rpc_url(chain_id: u64) -> &'static str {
    match chain_id {
        8453 => "https://mainnet.base.org",
        84532 => "https://sepolia.base.org",
        _ => "",
    }
}

pub async fn seed(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let mut affected = 0u64;
    for chain in CHAINS {
        let result = sqlx::query(
            "INSERT INTO chains
                (chain_id, name, identity_registry, reputation_registry, validation_registry, rpc_url, is_testnet)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (chain_id) DO UPDATE SET
                name                = EXCLUDED.name,
                identity_registry   = EXCLUDED.identity_registry,
                reputation_registry = EXCLUDED.reputation_registry,
                validation_registry = EXCLUDED.validation_registry,
                is_testnet          = EXCLUDED.is_testnet",
        )
        .bind(chain.chain_id as i64)
        .bind(chain.name)
        .bind(chain.identity_registry.as_slice())
        .bind(chain.reputation_registry.as_slice())
        .bind(chain.validation_registry.as_slice())
        .bind(default_rpc_url(chain.chain_id))
        .bind(chain.is_testnet)
        .execute(pool)
        .await?;
        affected += result.rows_affected();
    }
    Ok(affected)
}

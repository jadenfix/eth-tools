//! `eth-tools backfill --chain base --from-block <N>` — backfill the
//! registry index from `--from-block` to head, bypassing the 300s cron
//! limit. Useful for genesis ingest or for replaying a missed window.
//!
//! Implementation strategy (this PR):
//!
//!   * `--dry-run` (default-able): hit the configured RPC for the chain's
//!     current head block and the `from-block`, print the range we'd scan.
//!     Pure read; no DB writes; no API calls.
//!   * Non-dry-run: PRINT a clear "not yet exposed" message. The CLI
//!     intentionally does NOT speak directly to the prod database — that's
//!     a worker concern, and exposing CLI direct-DB writes would defeat the
//!     credential isolation the API/worker split enforces. Once the
//!     `registry_scraper` worker exposes a `run_range(from, to)` function
//!     OR an authenticated POST endpoint, this command grows a third arm.
//!
//! Env vars (mirror the worker layer):
//!   * `RPC_URL_PRIMARY` — required for the live-head probe
//!   * `RPC_URL_FALLBACK` — optional, used if primary errors

use crate::commands::Ctx;
use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

pub async fn run(_ctx: &Ctx, chain: &str, from_block: u64, dry_run: bool) -> Result<()> {
    let head = fetch_head_block(chain).await?;
    let to = head;
    if from_block > to {
        return Err(anyhow!(
            "--from-block {from_block} is ahead of current head {to} on {chain}"
        ));
    }
    let span = to - from_block;
    println!("chain:       {chain}");
    println!("from-block:  {from_block}");
    println!("to-block:    {to} (current head)");
    println!("span:        {span} blocks");
    if dry_run {
        println!("\n(dry-run) no writes performed. Pass without --dry-run to execute.");
        return Ok(());
    }
    eprintln!(
        "\nbackfill: non-dry-run execution requires the registry_scraper worker's\n\
         `run_range` entrypoint, which is not yet exposed for CLI use.\n\n\
         Workaround: trigger the cron with `?from_block={from_block}&force=1` via the\n\
         dashboard, or split the window into multiple cron-sized chunks.\n\n\
         Tracking: see plan §3.2 (backfill/replay opt-in)."
    );
    Ok(())
}

/// Fetch the chain head via `eth_blockNumber`. We accept the URL from
/// `RPC_URL_PRIMARY` (matching the worker convention); if that errors we
/// try `RPC_URL_FALLBACK` once before bailing.
async fn fetch_head_block(chain: &str) -> Result<u64> {
    let primary = std::env::var("RPC_URL_PRIMARY")
        .map_err(|_| anyhow!("RPC_URL_PRIMARY is not set; required for backfill"))?;
    if primary.trim().is_empty() {
        return Err(anyhow!("RPC_URL_PRIMARY is empty"));
    }
    let client = Client::builder()
        .user_agent(concat!("eth-tools-cli/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .context("build reqwest client for RPC")?;

    match probe_head(&client, &primary).await {
        Ok(b) => Ok(b),
        Err(e) => match std::env::var("RPC_URL_FALLBACK") {
            Ok(f) if !f.trim().is_empty() => {
                eprintln!("primary RPC failed ({e}); trying fallback for {chain}");
                probe_head(&client, &f).await
            }
            _ => Err(e),
        },
    }
}

async fn probe_head(client: &Client, url: &str) -> Result<u64> {
    let req = json!({
        "jsonrpc": "2.0",
        "method": "eth_blockNumber",
        "params": [],
        "id": 1
    });
    let resp = client
        .post(url)
        .json(&req)
        .send()
        .await
        .context("send eth_blockNumber")?;
    if !resp.status().is_success() {
        return Err(anyhow!("RPC {url} returned HTTP {}", resp.status()));
    }
    let body: Value = resp.json().await.context("parse RPC response")?;
    let hex = body
        .get("result")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("RPC response missing `result`: {body}"))?;
    parse_hex_u64(hex)
}

fn parse_hex_u64(s: &str) -> Result<u64> {
    let stripped = s.strip_prefix("0x").unwrap_or(s);
    u64::from_str_radix(stripped, 16).with_context(|| format!("parse hex block number {s:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_u64_accepts_0x_prefix() {
        assert_eq!(parse_hex_u64("0x1a").unwrap(), 26);
        assert_eq!(parse_hex_u64("1a").unwrap(), 26);
        assert_eq!(parse_hex_u64("0x0").unwrap(), 0);
    }

    #[test]
    fn parse_hex_u64_rejects_garbage() {
        assert!(parse_hex_u64("0xZZ").is_err());
        assert!(parse_hex_u64("not-hex").is_err());
    }
}

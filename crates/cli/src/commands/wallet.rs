//! `eth-tools wallet status` — render the user's wallet posture.
//!
//! Talks to `GET /api/v1/wallet/status`. Like `workers status`, the route is
//! not yet deployed at the time of this commit — 404 is treated as a
//! "not yet deployed, here's where to look" message rather than a hard
//! failure.

use crate::client::ApiError;
use crate::commands::Ctx;
use crate::output;
use anyhow::Result;
use reqwest::StatusCode;
use serde_json::Value;

pub async fn status(ctx: &Ctx) -> Result<()> {
    let result = ctx.client()?.wallet_status().await;
    match result {
        Ok(v) => {
            if ctx.json {
                output::print_json(&v);
            } else {
                render(&v);
            }
            Ok(())
        }
        Err(e) => {
            if let Some(api) = e.downcast_ref::<ApiError>() {
                if api.status == StatusCode::NOT_FOUND {
                    eprintln!(
                        "wallet status: endpoint /api/v1/wallet/status is not yet deployed.\n\
                         Check your wallet balance and spend in the dashboard at\n\
                         https://eth-tools.dev/account/wallet."
                    );
                    return Ok(());
                }
            }
            Err(e)
        }
    }
}

fn render(v: &Value) {
    // Expected wire shape (plan §6):
    //   { "balance_usdc": "...", "spend_today_usdc": "...",
    //     "allowlist": ["0x...", ...], "kill_switch": true|false }
    let balance = v
        .get("balance_usdc")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let spend = v
        .get("spend_today_usdc")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let kill = v
        .get("kill_switch")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    println!("balance USDC:     {balance}");
    println!("spend today USDC: {spend}");
    println!("kill switch:      {}", if kill { "ENGAGED" } else { "off" });
    if let Some(allow) = v.get("allowlist").and_then(Value::as_array) {
        println!("allowlisted recipients ({}):", allow.len());
        for r in allow {
            if let Some(s) = r.as_str() {
                println!("  - {s}");
            }
        }
    }
}

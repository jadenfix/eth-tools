//! `eth-tools health` — calls `GET /api/v1/health`.

use crate::commands::Ctx;
use crate::output;
use anyhow::Result;
use comfy_table::{Cell, Table};
use serde_json::Value;

pub async fn run(ctx: &Ctx) -> Result<()> {
    let v = ctx.client()?.health().await?;
    if ctx.json {
        output::print_json(&v);
        return Ok(());
    }
    render_human(&v);
    Ok(())
}

fn render_human(v: &Value) {
    let status = v.get("status").and_then(Value::as_str).unwrap_or("unknown");
    let version = v.get("version").and_then(Value::as_str).unwrap_or("unknown");
    let total = v.get("agents_indexed").and_then(Value::as_i64).unwrap_or(0);

    println!("status:  {status}");
    println!("version: {version}");
    println!("agents:  {total}");

    if let Some(chains) = v.get("chains").and_then(Value::as_array) {
        if !chains.is_empty() {
            let mut t = Table::new();
            t.set_header(vec!["chain", "chain_id", "testnet", "indexed"]);
            for c in chains {
                t.add_row(vec![
                    Cell::new(c.get("name").and_then(Value::as_str).unwrap_or("?")),
                    Cell::new(c.get("chain_id").and_then(Value::as_i64).unwrap_or(0)),
                    Cell::new(c.get("is_testnet").and_then(Value::as_bool).unwrap_or(false)),
                    Cell::new(c.get("agents_indexed").and_then(Value::as_i64).unwrap_or(0)),
                ]);
            }
            println!("{t}");
        }
    }
}

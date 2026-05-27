//! `eth-tools find <query>` — calls `POST /api/v1/agents/search`.

use crate::commands::Ctx;
use crate::output;
use anyhow::Result;
use comfy_table::{Cell, Table};
use serde_json::Value;

pub async fn run(ctx: &Ctx, query: &str) -> Result<()> {
    let v = ctx.client()?.search_agents(query).await?;
    if ctx.json {
        output::print_json(&v);
        return Ok(());
    }
    render_human(&v);
    Ok(())
}

fn render_human(v: &Value) {
    // Accept either a bare array or `{ "data": [...] }` for forward-compat
    // with the existing AgentList shape in api-types.ts.
    let rows = v
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| v.as_array())
        .cloned()
        .unwrap_or_default();

    if rows.is_empty() {
        println!("(no matches)");
        return;
    }

    let mut t = Table::new();
    t.set_header(vec!["chain", "agent_id", "owner", "agent_uri"]);
    for row in rows {
        t.add_row(vec![
            Cell::new(row.get("chain").and_then(Value::as_str).unwrap_or("?")),
            Cell::new(row.get("agent_id").and_then(Value::as_str).unwrap_or("?")),
            Cell::new(row.get("owner").and_then(Value::as_str).unwrap_or("?")),
            Cell::new(row.get("agent_uri").and_then(Value::as_str).unwrap_or("")),
        ]);
    }
    println!("{t}");
}

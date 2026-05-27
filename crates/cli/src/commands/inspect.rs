//! `eth-tools inspect <chain>/<agent_id>` — calls `GET /api/v1/agents/:chain/:id`.

use crate::commands::Ctx;
use crate::output;
use anyhow::{anyhow, Result};
use comfy_table::{Cell, Table};
use serde_json::Value;

pub async fn run(ctx: &Ctx, reference: &str) -> Result<()> {
    let (chain, agent_id) = parse_ref(reference)?;
    let v = ctx.client()?.inspect_agent(chain, agent_id).await?;
    if ctx.json {
        output::print_json(&v);
        return Ok(());
    }
    render_human(&v);
    Ok(())
}

fn parse_ref(s: &str) -> Result<(&str, &str)> {
    let (chain, id) = s
        .split_once('/')
        .ok_or_else(|| anyhow!("expected <chain>/<agent_id>, got `{s}`"))?;
    if chain.is_empty() || id.is_empty() {
        return Err(anyhow!("expected <chain>/<agent_id>, got `{s}`"));
    }
    Ok((chain, id))
}

fn render_human(v: &Value) {
    // The server returns `AgentDetail { data, source, staleness_ms }` per
    // api-types.ts. Be tolerant if the shape ever flattens.
    let data = v.get("data").unwrap_or(v);
    let mut t = Table::new();
    t.set_header(vec!["field", "value"]);

    let fields = [
        "chain",
        "chain_id",
        "agent_id",
        "owner",
        "agent_uri",
        "agent_wallet",
        "registered_at",
        "updated_at",
    ];
    for f in fields {
        let cell = match data.get(f) {
            Some(Value::Null) | None => String::from(""),
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
        };
        t.add_row(vec![Cell::new(f), Cell::new(cell)]);
    }
    println!("{t}");

    if let Some(src) = v.get("source").and_then(Value::as_str) {
        println!("source: {src}");
    }
    if let Some(stale) = v.get("staleness_ms").and_then(Value::as_i64) {
        println!("staleness_ms: {stale}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ref_ok() {
        assert_eq!(parse_ref("base/42").unwrap(), ("base", "42"));
    }

    #[test]
    fn parse_ref_rejects_bare() {
        assert!(parse_ref("base").is_err());
        assert!(parse_ref("/42").is_err());
        assert!(parse_ref("base/").is_err());
    }
}

//! `eth-tools manifest {validate, hash}` — agent-card tooling.

use crate::commands::Ctx;
use crate::output;
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

/// Read+parse a JSON manifest at `path`. Surfaced as its own function so the
/// CLI can fail fast with a clear error before paying for a network round
/// trip on garbage input.
fn load_manifest(path: &Path) -> Result<Value> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("read manifest {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parse manifest {} as JSON", path.display()))
}

pub async fn validate(ctx: &Ctx, path: &str) -> Result<()> {
    let manifest = load_manifest(Path::new(path))?;
    let v = ctx.client()?.manifest_validate(&manifest).await?;
    if ctx.json {
        output::print_json(&v);
        return Ok(());
    }
    render_validate(&v);
    Ok(())
}

pub async fn hash(ctx: &Ctx, path: &str) -> Result<()> {
    let manifest = load_manifest(Path::new(path))?;
    let v = ctx.client()?.manifest_hash(&manifest).await?;
    if ctx.json {
        output::print_json(&v);
        return Ok(());
    }
    render_hash(&v);
    Ok(())
}

fn render_validate(v: &Value) {
    // Expected wire shape: `{ "valid": bool, "errors": [{ "pointer": "/foo/0", "message": "..." }] }`.
    let ok = v.get("valid").and_then(Value::as_bool).unwrap_or(false);
    if ok {
        println!("manifest: ok");
        return;
    }
    println!("manifest: invalid");
    if let Some(errs) = v.get("errors").and_then(Value::as_array) {
        for e in errs {
            let ptr = e.get("pointer").and_then(Value::as_str).unwrap_or("/");
            let msg = e.get("message").and_then(Value::as_str).unwrap_or("(no message)");
            println!("  {ptr}: {msg}");
        }
    }
}

fn render_hash(v: &Value) {
    let sha = v.get("sha256").and_then(Value::as_str).unwrap_or("?");
    let kec = v.get("keccak256").and_then(Value::as_str).unwrap_or("?");
    println!("sha256:    {sha}");
    println!("keccak256: {kec}");
}

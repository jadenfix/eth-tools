//! `eth-tools workers status` — render a table of background worker health.
//!
//! Talks to `GET /api/v1/workers/status`. That route is NOT yet deployed at
//! the time of this commit (see plan §3 + phase-7 server work). When the
//! server returns 404 we print a friendly "not yet deployed" message
//! pointing the user at the dashboard instead of crashing with a raw HTTP
//! error — same posture as `wallet status`.

use crate::client::ApiError;
use crate::commands::Ctx;
use crate::output;
use anyhow::Result;
use comfy_table::{Cell, Table};
use reqwest::StatusCode;
use serde_json::Value;

pub async fn status(ctx: &Ctx) -> Result<()> {
    let result = ctx.client()?.workers_status().await;
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
                        "workers status: endpoint /api/v1/workers/status is not yet deployed.\n\
                         Track progress at https://eth-tools.dev/status or check the worker_runs\n\
                         table via the dashboard."
                    );
                    return Ok(());
                }
            }
            Err(e)
        }
    }
}

fn render(v: &Value) {
    // Expected wire shape:
    //   { "workers": [{ "name": "...", "last_ok_at": "...", "age_seconds": N,
    //                   "cursor_lag": N, "env": "production" }, ...] }
    let rows = v
        .get("workers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if rows.is_empty() {
        println!("(no workers reported)");
        return;
    }
    let mut t = Table::new();
    t.set_header(vec!["worker", "last_ok_at", "age (s)", "cursor lag", "env"]);
    for r in rows {
        t.add_row(vec![
            Cell::new(r.get("name").and_then(Value::as_str).unwrap_or("?")),
            Cell::new(r.get("last_ok_at").and_then(Value::as_str).unwrap_or("never")),
            Cell::new(r.get("age_seconds").and_then(Value::as_i64).unwrap_or(-1)),
            Cell::new(r.get("cursor_lag").and_then(Value::as_i64).unwrap_or(0)),
            Cell::new(r.get("env").and_then(Value::as_str).unwrap_or("?")),
        ]);
    }
    println!("{t}");
}

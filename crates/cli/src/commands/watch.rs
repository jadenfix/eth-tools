//! `eth-tools watch <chain>` — tail registry events for `<chain>`.
//!
//! Polling fallback (this PR): poll `GET /api/v1/agents?chain=<chain>&since=<cursor>`
//! every `--interval` seconds (default 10) and print newly indexed agents.
//! TODO(server): swap the poll loop for SSE / WebSocket once the server
//! exposes a streaming endpoint. The CLI surface (`watch <chain>`) is stable
//! either way — only the body of `run()` changes.
//!
//! The cursor is the maximum `agent_id` seen so far in the response array.
//! That mirrors the registry-scraper worker's monotonic ID semantics and
//! avoids depending on a server-side cursor token that doesn't exist yet.

use crate::commands::Ctx;
use crate::output;
use anyhow::Result;
use serde_json::Value;
use std::time::Duration;

/// Default poll interval (seconds). Conservative — keeps the request rate
/// well below the indexer's update frequency.
const DEFAULT_INTERVAL_SECS: u64 = 10;

pub async fn run(
    ctx: &Ctx,
    chain: &str,
    interval_secs: Option<u64>,
    max_iters: Option<usize>,
) -> Result<()> {
    let interval = Duration::from_secs(interval_secs.unwrap_or(DEFAULT_INTERVAL_SECS));
    let client = ctx.client()?;
    eprintln!(
        "watching {chain} (polling every {}s; TODO: upgrade to SSE)",
        interval.as_secs()
    );

    let mut cursor: Option<String> = None;
    let mut iter = 0usize;
    loop {
        let resp = client
            .list_agents_since(chain, cursor.as_deref())
            .await?;
        let rows = extract_rows(&resp);
        for row in &rows {
            if ctx.json {
                output::print_json(row);
            } else {
                render_row(row);
            }
        }
        cursor = next_cursor(&rows).or(cursor);

        iter += 1;
        if let Some(m) = max_iters {
            if iter >= m {
                break;
            }
        }
        tokio::time::sleep(interval).await;
    }
    Ok(())
}

/// Pull the list of agents out of the response. Accepts either a bare array
/// or `{ "data": [...] }`.
fn extract_rows(v: &Value) -> Vec<Value> {
    v.get("data")
        .and_then(Value::as_array)
        .or_else(|| v.as_array())
        .cloned()
        .unwrap_or_default()
}

/// Return the largest `agent_id` in the response, using a (length, lex)
/// ordering. This is correct for monotonic non-zero-padded *decimal* IDs
/// (the registry's actual semantic). Hex-style IDs (e.g. `0xff` → `0x100`)
/// would not order correctly here — that's an explicit trade-off until
/// the server exposes a proper cursor token.
fn next_cursor(rows: &[Value]) -> Option<String> {
    rows.iter()
        .filter_map(|r| r.get("agent_id").and_then(Value::as_str))
        .max_by_key(|s| (s.len(), s.to_string()))
        .map(String::from)
}

fn render_row(row: &Value) {
    let chain = row.get("chain").and_then(Value::as_str).unwrap_or("?");
    let id = row.get("agent_id").and_then(Value::as_str).unwrap_or("?");
    let owner = row.get("owner").and_then(Value::as_str).unwrap_or("?");
    let uri = row.get("agent_uri").and_then(Value::as_str).unwrap_or("");
    println!("[{chain}] {id}  owner={owner}  uri={uri}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_rows_handles_both_shapes() {
        let bare = json!([{ "agent_id": "1" }]);
        assert_eq!(extract_rows(&bare).len(), 1);
        let wrapped = json!({ "data": [{ "agent_id": "1" }, { "agent_id": "2" }] });
        assert_eq!(extract_rows(&wrapped).len(), 2);
        let empty = json!({});
        assert!(extract_rows(&empty).is_empty());
    }

    #[test]
    fn next_cursor_picks_largest_id() {
        let rows = vec![
            json!({ "agent_id": "1" }),
            json!({ "agent_id": "9" }),
            json!({ "agent_id": "42" }),
        ];
        // "42" has the longer length so wins under the (len, str) ordering;
        // this matches the indexer's intended monotonic semantic where new
        // IDs are at least as long as old ones.
        assert_eq!(next_cursor(&rows).as_deref(), Some("42"));
    }

    #[test]
    fn next_cursor_none_for_empty() {
        assert_eq!(next_cursor(&[]), None);
    }
}

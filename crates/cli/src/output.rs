//! Output formatting helpers shared across commands.

use serde_json::Value;

/// Pretty-print JSON to stdout. We always use `to_string_pretty` so that
/// `--json` output is human-diff-friendly without needing `jq`.
pub fn print_json(v: &Value) {
    match serde_json::to_string_pretty(v) {
        Ok(s) => println!("{s}"),
        Err(_) => println!("{v}"),
    }
}

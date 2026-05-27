//! `eth-tools invoke <chain>/<agent_id> --input ./input.json`.
//!
//! POSTs to `/api/v1/invoke`. On HTTP 402 (Payment Required) the server is
//! expected to return an x402 challenge envelope; we surface the next step
//! to the user instead of crashing. Two follow-up modes:
//!
//!   * `--x-payment <token>` — caller has already minted an EIP-3009
//!     authorization off-band; we re-POST with the `X-Payment` header.
//!   * `--auto-pay` — placeholder: prints a clear "not yet implemented"
//!     message. Client-side EIP-3009 signing lands with the wallet rails
//!     work in phase-6; the CLI surfaces the seam without implementing it.

use crate::client::ApiError;
use crate::commands::Ctx;
use crate::output;
use anyhow::{anyhow, Context, Result};
use reqwest::StatusCode;
use serde_json::Value;
use std::io::Read;
use std::path::Path;

/// Hard cap on the input payload (256 KiB). Matches the manifest cap; an
/// invocation input larger than this is almost certainly a misdirected
/// `--input /dev/zero` or a stray log file.
const INPUT_MAX_BYTES: usize = 256 * 1024;

pub async fn run(
    ctx: &Ctx,
    reference: &str,
    input_path: &str,
    x_payment: Option<&str>,
    auto_pay: bool,
) -> Result<()> {
    let (chain, agent_id) = parse_ref(reference)?;
    let input = load_input(Path::new(input_path))?;
    let body = serde_json::json!({
        "agent": format!("{chain}/{agent_id}"),
        "input": input,
    });

    let client = ctx.client()?;
    let result = match x_payment {
        Some(x) => client.invoke_with_payment(&body, x).await,
        None => client.invoke(&body).await,
    };

    match result {
        Ok(v) => {
            if ctx.json {
                output::print_json(&v);
            } else {
                render_human(&v);
            }
            Ok(())
        }
        Err(e) => {
            // 402: surface the challenge and explain next steps instead of
            // dumping a stack to the user.
            if let Some(api) = e.downcast_ref::<ApiError>() {
                if api.status == StatusCode::PAYMENT_REQUIRED {
                    eprintln!("payment required (HTTP 402)");
                    eprintln!("{}", api.message);
                    if auto_pay {
                        eprintln!(
                            "\n--auto-pay is not yet implemented: client-side \
                             EIP-3009 signing lands with phase-6 wallet rails."
                        );
                    } else {
                        eprintln!(
                            "\nNext step: re-run with `--x-payment <token>` once you've \
                             minted an EIP-3009 USDC authorization, OR pass `--auto-pay` \
                             (placeholder until phase-6 wallet signing lands)."
                        );
                    }
                    return Err(anyhow!("invoke: HTTP 402 — payment required"));
                }
            }
            Err(e)
        }
    }
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

fn load_input(path: &Path) -> Result<Value> {
    let f = std::fs::File::open(path)
        .with_context(|| format!("open input file {}", path.display()))?;
    let mut buf = Vec::with_capacity(INPUT_MAX_BYTES.min(64 * 1024));
    let n = f
        .take((INPUT_MAX_BYTES + 1) as u64)
        .read_to_end(&mut buf)
        .with_context(|| format!("read input file {}", path.display()))?;
    if n > INPUT_MAX_BYTES {
        return Err(anyhow!(
            "input file too large: exceeds {} KiB cap",
            INPUT_MAX_BYTES / 1024
        ));
    }
    let text = std::str::from_utf8(&buf)
        .with_context(|| format!("input file {} is not valid UTF-8", path.display()))?;
    serde_json::from_str(text)
        .with_context(|| format!("parse input file {} as JSON", path.display()))
}

fn render_human(v: &Value) {
    // Server contract not yet final; pretty-print until it lands. Best-effort
    // pull-outs for the common fields agents are expected to return.
    if let Some(out) = v.get("output") {
        println!("output:");
        output::print_json(out);
    } else {
        output::print_json(v);
    }
    if let Some(rid) = v.get("request_id").and_then(Value::as_str) {
        println!("request_id: {rid}");
    }
    if let Some(cost) = v.get("cost_usdc").and_then(Value::as_str) {
        println!("cost_usdc:  {cost}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn parse_ref_ok() {
        assert_eq!(parse_ref("base/42").unwrap(), ("base", "42"));
    }

    #[test]
    fn parse_ref_rejects_malformed() {
        assert!(parse_ref("base").is_err());
        assert!(parse_ref("/42").is_err());
        assert!(parse_ref("base/").is_err());
    }

    #[test]
    fn load_input_parses_json() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(br#"{"foo": "bar"}"#).unwrap();
        let v = load_input(f.path()).unwrap();
        assert_eq!(v["foo"], "bar");
    }

    #[test]
    fn load_input_rejects_oversize() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(&vec![b'a'; INPUT_MAX_BYTES + 1]).unwrap();
        let err = load_input(f.path()).unwrap_err();
        assert!(err.to_string().contains("too large"));
    }

    #[test]
    fn load_input_rejects_non_json() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"not json").unwrap();
        assert!(load_input(f.path()).is_err());
    }
}

//! `eth-tools manifest {validate, hash}` — agent-card tooling.

use crate::commands::Ctx;
use crate::output;
use anyhow::{Context, Result};
use serde_json::Value;
use std::io::Read;
use std::path::Path;

/// Hard cap on a manifest payload (256 KiB). The server-side validator caps
/// at the same value; loading more locally would just be wasted bytes — and
/// pointing the CLI at `/dev/zero` or a tarball of zeros must not OOM.
pub(crate) const MANIFEST_MAX_BYTES: usize = 256 * 1024;

/// Read+parse a JSON manifest at `path`. Surfaced as its own function so the
/// CLI can fail fast with a clear error before paying for a network round
/// trip on garbage input.
///
/// Uses a bounded reader (256 KiB) so a misdirected `--` to `/dev/zero` or a
/// gigabyte log file fails with a clear error within milliseconds instead of
/// driving the process OOM.
fn load_manifest(path: &Path) -> Result<Value> {
    let raw =
        read_capped(path, MANIFEST_MAX_BYTES).with_context(|| format!("read manifest {}", path.display()))?;
    let text = std::str::from_utf8(&raw)
        .with_context(|| format!("manifest {} is not valid UTF-8", path.display()))?;
    serde_json::from_str(text).with_context(|| format!("parse manifest {} as JSON", path.display()))
}

/// Read up to `limit` bytes from `path`. Returns `Err` if the file is larger
/// than `limit`. We deliberately read `limit + 1` and check the count, because
/// `Read::take(limit)` silently truncates — a 257 KiB file would otherwise
/// look identical to a 256 KiB file at this layer.
fn read_capped(path: &Path, limit: usize) -> std::io::Result<Vec<u8>> {
    let f = std::fs::File::open(path)?;
    let mut buf = Vec::with_capacity(limit.min(64 * 1024));
    let n = f.take((limit + 1) as u64).read_to_end(&mut buf)?;
    if n > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "manifest too large: exceeds {limit} bytes ({} KiB) cap",
                limit / 1024
            ),
        ));
    }
    Ok(buf)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn read_capped_accepts_small_file() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"hello").unwrap();
        let got = read_capped(f.path(), 1024).unwrap();
        assert_eq!(got, b"hello");
    }

    #[test]
    fn read_capped_accepts_exactly_at_limit() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(&[b'a'; 128]).unwrap();
        let got = read_capped(f.path(), 128).unwrap();
        assert_eq!(got.len(), 128);
    }

    #[test]
    fn read_capped_rejects_one_byte_over_limit() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(&[b'a'; 129]).unwrap();
        let err = read_capped(f.path(), 128).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("too large"));
    }

    #[cfg(unix)]
    #[test]
    fn read_capped_rejects_dev_zero_quickly() {
        let path = std::path::Path::new("/dev/zero");
        if !path.exists() {
            return; // CI sandbox without /dev/zero — skip.
        }
        let start = std::time::Instant::now();
        let err = read_capped(path, MANIFEST_MAX_BYTES).unwrap_err();
        let elapsed = start.elapsed();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "should fail fast, took {elapsed:?}"
        );
    }
}

//! On-disk credential storage for the eth-tools CLI.
//!
//! Layout (per `directories::ProjectDirs::from("dev", "eth-tools", "eth-tools")`):
//!   * Linux:   `$XDG_CONFIG_HOME/eth-tools/credentials.json`
//!   * macOS:   `~/Library/Application Support/dev.eth-tools.eth-tools/credentials.json`
//!   * Windows: `%APPDATA%\eth-tools\eth-tools\config\credentials.json`
//!
//! File is mode `0600` on unix; parent directory is `0700`. We never log the
//! token itself: `token` is a private field on `Credentials`, accessed only
//! via `token()` (borrow) or `into_token()` (consume), and `Debug` is
//! implemented manually to redact the secret.

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Logical wrapper so we can test against a temp dir without touching the real
/// XDG path during `cargo test`.
#[derive(Clone, Debug)]
pub struct ConfigPaths {
    root: PathBuf,
}

impl ConfigPaths {
    /// Resolve the platform default directory.
    pub fn discover() -> Result<Self> {
        let dirs = ProjectDirs::from("dev", "eth-tools", "eth-tools")
            .context("could not resolve a user config directory (no $HOME?)")?;
        Ok(Self {
            root: dirs.config_dir().to_path_buf(),
        })
    }

    /// Test-only / advanced override.
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn credentials_path(&self) -> PathBuf {
        self.root.join("credentials.json")
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Credentials {
    // Private: forces callers through `token()` / `into_token()` and prevents
    // accidental moves into log lines.
    token: String,
    /// Optional server the token was minted for; lets us warn when a token
    /// minted for one base URL is being sent to a different one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
}

impl Credentials {
    pub fn new(token: impl Into<String>, api_url: Option<String>) -> Self {
        Self {
            token: token.into(),
            api_url,
        }
    }

    /// Borrow the token. Avoid passing this into log / `format!` calls.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Consume the credentials, yielding the owned token string. Preferred
    /// when handing the value to an HTTP client builder.
    pub fn into_token(self) -> String {
        self.token
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("token", &"[REDACTED]")
            .field("api_url", &self.api_url)
            .finish()
    }
}

/// Load credentials from disk; `Ok(None)` if the file does not exist.
pub fn load(paths: &ConfigPaths) -> Result<Option<Credentials>> {
    let p = paths.credentials_path();
    if !p.exists() {
        return Ok(None);
    }
    let raw =
        std::fs::read_to_string(&p).with_context(|| format!("read credentials file {}", p.display()))?;
    let creds: Credentials =
        serde_json::from_str(&raw).with_context(|| format!("parse credentials file {}", p.display()))?;
    Ok(Some(creds))
}

/// Persist credentials with `0600` mode on unix and a `0700` parent dir.
pub fn save(paths: &ConfigPaths, creds: &Credentials) -> Result<()> {
    std::fs::create_dir_all(paths.root())
        .with_context(|| format!("create config dir {}", paths.root().display()))?;
    set_dir_mode(paths.root())?;

    let p = paths.credentials_path();
    let json = serde_json::to_string_pretty(creds).expect("Credentials serializes");
    write_secret_file(&p, json.as_bytes())?;
    Ok(())
}

/// Remove the credentials file. Missing file is not an error (idempotent).
///
/// Before unlinking we overwrite the on-disk bytes with zeros of the same
/// length. On COW filesystems (APFS, btrfs, ZFS) this is best-effort — the
/// previous block may still live on-disk — but it matches the `0600`/`0700`
/// hardening posture for the common ext4/xfs/HFS+ cases.
pub fn clear(paths: &ConfigPaths) -> Result<()> {
    let p = paths.credentials_path();
    // Best-effort shred. If shredding fails (e.g. permissions changed under
    // us), still try to unlink so logout is not blocked by an exotic FS state.
    if let Err(e) = shred_in_place(&p) {
        if e.kind() != std::io::ErrorKind::NotFound {
            // Log the shred failure to stderr but continue: leaving the token
            // on disk is strictly worse than leaving its plaintext bytes in a
            // sector that we already tried to overwrite once.
            eprintln!("warning: could not shred credentials file before deletion: {e}");
        }
    }
    match std::fs::remove_file(&p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("remove credentials file {}", p.display())),
    }
}

/// Open the credentials file in place and overwrite its bytes with zeros.
/// Returns `NotFound` if the file is missing so the caller can suppress that
/// case quietly.
///
/// Caveat (mirrors [`clear`]): on COW filesystems (APFS, btrfs, ZFS) and on
/// SSDs with wear-leveling, the previous block may still live on-disk after
/// the in-place write — the new zero bytes may be written to a *different*
/// physical block while the original is merely unmapped. This is a
/// best-effort hardening pass, not a guarantee of unrecoverability; it
/// matches the `0600`/`0700` posture for the common ext4/xfs/HFS+ cases.
fn shred_in_place(path: &Path) -> std::io::Result<()> {
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    let len = f.metadata()?.len();
    if len > 0 {
        f.seek(SeekFrom::Start(0))?;
        // Write in 8 KiB chunks; credentials files are tiny but this still
        // bounds peak memory in case someone hand-edits a giant file.
        let zeros = [0u8; 8 * 1024];
        let mut remaining = len as usize;
        while remaining > 0 {
            let n = remaining.min(zeros.len());
            f.write_all(&zeros[..n])?;
            remaining -= n;
        }
        f.sync_all()?;
    }
    Ok(())
}

#[cfg(unix)]
fn write_secret_file(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("open credentials file for write {}", path.display()))?;
    f.write_all(contents)
        .with_context(|| format!("write credentials file {}", path.display()))?;
    // Re-apply permissions in case the file already existed with looser bits.
    use std::os::unix::fs::PermissionsExt;
    let mut perms = f.metadata()?.permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_secret_file(path: &Path, contents: &[u8]) -> Result<()> {
    std::fs::write(path, contents).with_context(|| format!("write credentials file {}", path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn set_dir_mode(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o700);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_dir_mode(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn round_trip_save_load() {
        let dir = tempdir().unwrap();
        let paths = ConfigPaths::at(dir.path());
        assert!(load(&paths).unwrap().is_none());

        let creds = Credentials::new("tok_abc", Some("https://eth-tools.dev".into()));
        save(&paths, &creds).unwrap();

        let got = load(&paths).unwrap().expect("creds present after save");
        assert_eq!(got.token(), "tok_abc");
        assert_eq!(got.api_url.as_deref(), Some("https://eth-tools.dev"));
        // `into_token` consumes and yields the owned secret.
        assert_eq!(got.into_token(), "tok_abc");
    }

    #[test]
    fn debug_impl_redacts_token() {
        let creds = Credentials::new("super-secret-token", Some("https://x".into()));
        let rendered = format!("{:?}", creds);
        assert!(
            !rendered.contains("super-secret-token"),
            "Debug must not leak token, got: {rendered}"
        );
        assert!(rendered.contains("[REDACTED]"));
        assert!(rendered.contains("https://x"));

        // Containers (Option, Vec, tuples) MUST inherit the manual Debug —
        // i.e. they must format each element via `Credentials::fmt`, not
        // derive a generic structural Debug that leaks the private field.
        // Pin the contract so a future refactor away from `impl Debug` is
        // caught here instead of in a log file.
        let opt_rendered = format!("{:?}", Some(creds.clone()));
        assert!(
            !opt_rendered.contains("super-secret-token"),
            "Option<Credentials> Debug must not leak token, got: {opt_rendered}"
        );
        let vec_rendered = format!("{:?}", vec![creds.clone(), creds.clone()]);
        assert!(
            !vec_rendered.contains("super-secret-token"),
            "Vec<Credentials> Debug must not leak token, got: {vec_rendered}"
        );
    }

    #[test]
    fn clear_is_idempotent() {
        let dir = tempdir().unwrap();
        let paths = ConfigPaths::at(dir.path());
        // No file yet — must not error.
        clear(&paths).unwrap();

        save(&paths, &Credentials::new("tok", None)).unwrap();
        clear(&paths).unwrap();
        assert!(load(&paths).unwrap().is_none());

        // Idempotent: second clear is fine too.
        clear(&paths).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn credentials_file_is_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let paths = ConfigPaths::at(dir.path());
        save(&paths, &Credentials::new("tok", None)).unwrap();

        let mode = std::fs::metadata(paths.credentials_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "credentials file must be mode 0600, got {mode:o}");

        let dir_mode = std::fs::metadata(paths.root()).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "config dir must be mode 0700, got {dir_mode:o}");
    }

    #[test]
    fn clear_shreds_token_bytes_before_unlink() {
        // We can't easily observe the unlinked inode, but we can intercept the
        // shred step directly: write a sentinel, shred it, and verify the
        // bytes on disk are zero before the file is removed.
        let dir = tempdir().unwrap();
        let paths = ConfigPaths::at(dir.path());
        save(&paths, &Credentials::new("plaintext-secret", None)).unwrap();
        let p = paths.credentials_path();
        // Sanity: the secret is on disk before shred.
        let before = std::fs::read(&p).unwrap();
        assert!(
            before
                .windows(b"plaintext-secret".len())
                .any(|w| w == b"plaintext-secret"),
            "test setup: secret should be present on disk pre-shred"
        );

        super::shred_in_place(&p).unwrap();
        let after = std::fs::read(&p).unwrap();
        assert_eq!(after.len(), before.len(), "shred must preserve length");
        assert!(after.iter().all(|b| *b == 0), "all bytes must be zero post-shred");
    }

    #[cfg(unix)]
    #[test]
    fn overwrite_tightens_permissions() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let paths = ConfigPaths::at(dir.path());
        std::fs::create_dir_all(paths.root()).unwrap();
        // Pre-create a world-readable file.
        let p = paths.credentials_path();
        let mut f = std::fs::File::create(&p).unwrap();
        writeln!(f, "{{}}").unwrap();
        let mut perms = std::fs::metadata(&p).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&p, perms).unwrap();

        save(&paths, &Credentials::new("tok", None)).unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

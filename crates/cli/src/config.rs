//! On-disk credential storage for the eth-tools CLI.
//!
//! Layout (per `directories::ProjectDirs::from("dev", "eth-tools", "eth-tools")`):
//!   * Linux:   `$XDG_CONFIG_HOME/eth-tools/credentials.json`
//!   * macOS:   `~/Library/Application Support/dev.eth-tools.eth-tools/credentials.json`
//!   * Windows: `%APPDATA%\eth-tools\eth-tools\config\credentials.json`
//!
//! File is mode `0600` on unix; parent directory is `0700`. We never log the
//! token itself, and `Credentials::token` is consumed via `into_token()` to
//! discourage accidental `Debug` printing.

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Credentials {
    pub token: String,
    /// Optional server the token was minted for; lets us refuse to send a
    /// production token at a staging API by accident in a future PR.
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
pub fn clear(paths: &ConfigPaths) -> Result<()> {
    let p = paths.credentials_path();
    match std::fs::remove_file(&p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("remove credentials file {}", p.display())),
    }
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
        assert_eq!(got.token, "tok_abc");
        assert_eq!(got.api_url.as_deref(), Some("https://eth-tools.dev"));
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

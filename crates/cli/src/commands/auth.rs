//! `eth-tools auth {login, whoami, logout}`.
//!
//! Login flow (this PR): print the dashboard URL, try to open the user's
//! browser, then read a pasted API key (no echo) and persist it. The
//! `/api/v1/cli/auth/poll` endpoint is intentionally NOT called yet — it does
//! not exist server-side, and the prompt defers it to a follow-up PR.

use crate::client::DEFAULT_API_URL;
use crate::commands::Ctx;
use crate::config::{self, Credentials};
use crate::output;
use anyhow::Result;

/// Full, trusted URL the login flow points the user's browser at. Hard-coded
/// (NOT derived from `ctx.api_url`) so a hostile `--api-url http://phish.dev`
/// can't redirect the user to type their real eth-tools.dev API key into an
/// attacker-controlled page. The `--allow-untrusted-login` flag opts in to
/// the legacy behavior of opening `<ctx.api_url>/account/cli` for self-hosted
/// deployments.
const DEFAULT_DASHBOARD_URL: &str = "https://eth-tools.dev/account/cli";

/// Path on the dashboard that mints a CLI key, appended to `ctx.api_url`
/// only when the user has opted into an untrusted login flow.
const LOGIN_PATH: &str = "/account/cli";

/// Pluggable seam: hides `rpassword` (which reads from `/dev/tty`, not stdin,
/// and so can't be exercised through `assert_cmd`'s stdin pipe) and
/// `webbrowser::open` behind a trait so unit tests can inject canned values.
pub trait LoginIo {
    /// Prompt for the API key without echoing typed characters. Returns the
    /// raw input (trimmed by the caller).
    fn read_token(&mut self) -> Result<String>;
    /// Best-effort: launch the user's default browser at `url`. Errors are
    /// surfaced to the caller but never fatal — the URL is also printed.
    fn open_browser(&mut self, url: &str) -> Result<()>;
}

/// Production implementation: `rpassword` + `webbrowser`.
pub struct RealLoginIo;
impl LoginIo for RealLoginIo {
    fn read_token(&mut self) -> Result<String> {
        rpassword::prompt_password("Paste the API key here: ")
            .map_err(|e| anyhow::anyhow!("read API key from terminal: {e}"))
    }
    fn open_browser(&mut self, url: &str) -> Result<()> {
        webbrowser::open(url).map(|_| ()).map_err(Into::into)
    }
}

pub async fn login(ctx: &Ctx) -> Result<()> {
    login_with(ctx, &mut RealLoginIo).await
}

pub(crate) async fn login_with<I: LoginIo>(ctx: &Ctx, io_impl: &mut I) -> Result<()> {
    // Decide the browser URL using a TRUSTED constant by default. Only when
    // the user is intentionally running against a non-default `--api-url`
    // AND has passed `--allow-untrusted-login` do we honor that base URL.
    let using_default_api = ctx.api_url == DEFAULT_API_URL;
    let browser_url = if using_default_api {
        DEFAULT_DASHBOARD_URL.to_string()
    } else if ctx.allow_untrusted_login {
        eprintln!(
            "WARN: --api-url is non-default ({}); opening browser there because \
             --allow-untrusted-login was passed",
            ctx.api_url
        );
        format!("{}{}", ctx.api_url.trim_end_matches('/'), LOGIN_PATH)
    } else {
        anyhow::bail!(
            "refusing to open browser at non-default --api-url {}: pass \
             --allow-untrusted-login to override (self-hosted deployments)",
            ctx.api_url
        );
    };

    println!("Open this URL to mint a CLI API key:");
    println!("  {browser_url}");

    // Best-effort browser open. We don't fail if it can't launch — the user
    // can always copy/paste the URL.
    if let Err(e) = io_impl.open_browser(&browser_url) {
        eprintln!("(could not launch browser: {e}; copy/paste the URL above)");
    }

    let token = io_impl.read_token()?.trim().to_string();
    if token.is_empty() {
        anyhow::bail!("no API key entered; aborting");
    }

    let creds = Credentials::new(token, Some(ctx.api_url.clone()));
    config::save(&ctx.paths, &creds)?;
    println!("Saved credentials to {}", ctx.paths.credentials_path().display());
    Ok(())
}

pub async fn whoami(ctx: &Ctx) -> Result<()> {
    let creds = config::load(&ctx.paths)?;
    if creds.is_none() {
        anyhow::bail!("not logged in; run `eth-tools auth login` first");
    }
    let v = ctx.client()?.whoami().await?;
    if ctx.json {
        output::print_json(&v);
    } else {
        // Human-readable: print as pretty JSON until the server contract
        // settles. Cheaper than guessing fields the server has not defined.
        output::print_json(&v);
    }
    Ok(())
}

pub async fn logout(ctx: &Ctx) -> Result<()> {
    config::clear(&ctx.paths)?;
    println!("Logged out (cleared {}).", ctx.paths.credentials_path().display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigPaths;
    use std::cell::RefCell;
    use tempfile::tempdir;

    /// `RealLoginIo::read_token` calls `rpassword::prompt_password`, which
    /// opens `/dev/tty` directly (NOT stdin) — so an `assert_cmd` test that
    /// pipes `</dev/null` to the child doesn't actually exercise the error
    /// path on a developer box (where `/dev/tty` is still open).
    ///
    /// Strategy: only assert the no-tty error path when this process has
    /// *itself* lost its controlling tty (typical of CI sandboxes and
    /// `nohup`/`setsid` runners). On a developer box `/dev/tty` opens fine
    /// and `prompt_password` would block waiting for keyboard input, so we
    /// skip with a logged reason rather than hanging the test suite.
    #[cfg(unix)]
    #[test]
    fn real_login_io_surfaces_sensible_error_without_tty() {
        let tty_available = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .is_ok();
        if tty_available {
            eprintln!(
                "skipping real_login_io_surfaces_sensible_error_without_tty: \
                 /dev/tty is open (interactive shell). Rerun under `setsid` \
                 or in CI to exercise the no-tty path."
            );
            return;
        }
        let mut io = RealLoginIo;
        let err = io.read_token().expect_err("no tty => must error");
        let msg = err.to_string();
        assert!(
            msg.contains("read API key from terminal"),
            "error message should mention the terminal read step, got: {msg}"
        );
    }

    /// Records every interaction so tests can assert on order / arguments.
    struct FakeIo {
        token: String,
        opened: RefCell<Vec<String>>,
    }
    impl LoginIo for FakeIo {
        fn read_token(&mut self) -> Result<String> {
            Ok(self.token.clone())
        }
        fn open_browser(&mut self, url: &str) -> Result<()> {
            self.opened.borrow_mut().push(url.to_string());
            Ok(())
        }
    }

    fn ctx(api_url: &str, allow_untrusted: bool, tmp: &std::path::Path) -> Ctx {
        Ctx {
            api_url: api_url.to_string(),
            json: false,
            paths: ConfigPaths::at(tmp),
            allow_untrusted_login: allow_untrusted,
        }
    }

    #[tokio::test]
    async fn login_default_url_uses_trusted_constant_not_ctx_api_url() {
        let dir = tempdir().unwrap();
        let mut io = FakeIo {
            token: "tok_xyz".into(),
            opened: RefCell::new(vec![]),
        };
        // Default api_url means trusted dashboard URL must be opened.
        let c = ctx(DEFAULT_API_URL, false, dir.path());
        login_with(&c, &mut io).await.unwrap();
        let opened = io.opened.borrow();
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0], DEFAULT_DASHBOARD_URL);
        // Token was persisted.
        let creds = config::load(&c.paths).unwrap().unwrap();
        assert_eq!(creds.token(), "tok_xyz");
    }

    #[tokio::test]
    async fn login_refuses_untrusted_api_url_without_flag() {
        let dir = tempdir().unwrap();
        let mut io = FakeIo {
            token: "tok".into(),
            opened: RefCell::new(vec![]),
        };
        let c = ctx("https://attacker.example.com", false, dir.path());
        let err = login_with(&c, &mut io).await.unwrap_err();
        assert!(err.to_string().contains("--allow-untrusted-login"));
        assert!(io.opened.borrow().is_empty(), "must not open any browser");
        // Nothing was persisted.
        assert!(config::load(&c.paths).unwrap().is_none());
    }

    #[tokio::test]
    async fn login_honors_untrusted_flag_for_self_hosted() {
        let dir = tempdir().unwrap();
        let mut io = FakeIo {
            token: "tok".into(),
            opened: RefCell::new(vec![]),
        };
        let c = ctx("https://self.hosted.example.com", true, dir.path());
        login_with(&c, &mut io).await.unwrap();
        let opened = io.opened.borrow();
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0], "https://self.hosted.example.com/account/cli");
    }

    #[tokio::test]
    async fn login_rejects_empty_token() {
        let dir = tempdir().unwrap();
        let mut io = FakeIo {
            token: "   \n".into(),
            opened: RefCell::new(vec![]),
        };
        let c = ctx(DEFAULT_API_URL, false, dir.path());
        let err = login_with(&c, &mut io).await.unwrap_err();
        assert!(err.to_string().contains("no API key"));
        assert!(config::load(&c.paths).unwrap().is_none());
    }
}

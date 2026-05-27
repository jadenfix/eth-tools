//! Command implementations. One module per top-level command group.
pub mod auth;
pub mod backfill;
pub mod find;
pub mod health;
pub mod inspect;
pub mod invoke;
pub mod manifest;
pub mod mcp;
pub mod register;
pub mod wallet;
pub mod watch;
pub mod workers;

use crate::client::Client;
use crate::config::ConfigPaths;
use anyhow::Result;

/// Shared runtime context handed to every command.
pub struct Ctx {
    pub api_url: String,
    pub json: bool,
    pub paths: ConfigPaths,
    /// Opt-in: open the browser at a non-default `--api-url` during `auth
    /// login`. Without this, login refuses to navigate to an attacker-supplied
    /// host that might phish for an API key. See `commands::auth::login`.
    pub allow_untrusted_login: bool,
}

impl Ctx {
    /// Build a `Client`, threading in a saved token if present.
    ///
    /// If the loaded credentials carry an `api_url` field and it does not
    /// match `self.api_url`, we emit a warning to stderr. This is intentionally
    /// non-blocking: operators may legitimately switch staging↔prod against a
    /// long-lived dev token.
    pub fn client(&self) -> Result<Client> {
        let creds = crate::config::load(&self.paths)?;
        let token = creds.map(|c| {
            if let Some(creds_url) = c.api_url.as_deref() {
                if creds_url != self.api_url {
                    eprintln!(
                        "WARN: token was minted for {creds_url}; sending to {}",
                        self.api_url
                    );
                }
            }
            c.into_token()
        });
        Client::new(self.api_url.clone(), token)
    }

    /// Build a `Client` that ignores any saved token (login flow, etc.).
    pub fn anon_client(&self) -> Result<Client> {
        Client::new(self.api_url.clone(), None)
    }
}

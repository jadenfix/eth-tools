//! `eth-tools auth {login, whoami, logout}`.
//!
//! Login flow (this PR): print the dashboard URL, try to open the user's
//! browser, then read a pasted API key from stdin and persist it. The
//! `/api/v1/cli/auth/poll` endpoint is intentionally NOT called yet — it does
//! not exist server-side, and the prompt defers it to a follow-up PR.

use crate::commands::Ctx;
use crate::config::{self, Credentials};
use crate::output;
use anyhow::{Context, Result};
use std::io::{self, BufRead, Write};

/// Path on the dashboard that mints a CLI key. (Server-side support is a
/// follow-up PR; for now it's just a stable URL we direct the user to.)
const LOGIN_PATH: &str = "/account/cli";

pub async fn login(ctx: &Ctx) -> Result<()> {
    let url = format!("{}{}", ctx.api_url.trim_end_matches('/'), LOGIN_PATH);
    println!("Open this URL to mint a CLI API key:");
    println!("  {url}");

    // Best-effort browser open. We don't fail if it can't launch — the user
    // can always copy/paste the URL.
    if let Err(e) = webbrowser::open(&url) {
        eprintln!("(could not launch browser: {e}; copy/paste the URL above)");
    }

    print!("\nPaste the API key here and press Enter: ");
    io::stdout().flush().ok();

    let mut buf = String::new();
    let stdin = io::stdin();
    stdin
        .lock()
        .read_line(&mut buf)
        .context("read API key from stdin")?;
    let token = buf.trim().to_string();
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

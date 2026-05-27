//! `eth-tools mcp {install, from-card}`.
//!
//! `install` writes a stanza into the user's Claude Desktop / Cursor MCP
//! config files (preserving existing entries, writing a `.bak` alongside).
//! `from-card` fetches a remote agent card and emits a tiny Deno TS shim
//! that bridges MCP <-> the agent's A2A endpoint.

use crate::commands::Ctx;
use crate::config::Credentials;
use crate::{config, output};
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// MCP server URL the CLI configures clients to point at. The same URL the
/// dashboard surfaces under "MCP integration".
pub const ETH_TOOLS_MCP_URL: &str = "https://eth-tools.dev/api/mcp";

/// Possible MCP client config file locations. We probe each entry; missing
/// ones are silently ignored. Targeting both Claude Desktop and Cursor on
/// the three desktop OSes mirrors the matrix the prompt asks for.
fn candidate_paths() -> Vec<(&'static str, PathBuf)> {
    let mut out = Vec::new();
    let home = match dirs_home() {
        Some(h) => h,
        None => return out,
    };
    // ---- macOS ----
    #[cfg(target_os = "macos")]
    {
        out.push((
            "claude-desktop",
            home.join("Library/Application Support/Claude/claude_desktop_config.json"),
        ));
        out.push(("cursor", home.join(".cursor/mcp.json")));
    }
    // ---- Linux ----
    #[cfg(target_os = "linux")]
    {
        out.push((
            "claude-desktop",
            home.join(".config/Claude/claude_desktop_config.json"),
        ));
        out.push(("cursor", home.join(".cursor/mcp.json")));
    }
    // ---- Windows ----
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            out.push((
                "claude-desktop",
                std::path::PathBuf::from(appdata)
                    .join("Claude/claude_desktop_config.json"),
            ));
        }
        out.push(("cursor", home.join(".cursor/mcp.json")));
    }
    out
}

fn dirs_home() -> Option<PathBuf> {
    // Avoid pulling in another dep just for $HOME. `directories::UserDirs`
    // is already in the dependency tree via `directories` (used by
    // `config.rs`); reuse it.
    directories::UserDirs::new().map(|d| d.home_dir().to_path_buf())
}

/// `install` entrypoint. Discovers candidate config files, asks the user
/// which to update (or all/none), merges in our stanza, writes a `.bak`,
/// then writes the updated config.
pub async fn install(ctx: &Ctx) -> Result<()> {
    install_with(ctx, &mut RealMcpIo).await
}

/// Pluggable I/O — same pattern as `auth::LoginIo`.
pub trait McpIo {
    /// Yes/no confirmation. Default on empty input is `default`.
    fn confirm(&mut self, prompt: &str, default: bool) -> Result<bool>;
}

pub struct RealMcpIo;

impl McpIo for RealMcpIo {
    fn confirm(&mut self, prompt: &str, default: bool) -> Result<bool> {
        use std::io::{BufRead, Write};
        let suffix = if default { "[Y/n]" } else { "[y/N]" };
        print!("{prompt} {suffix} ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .context("read stdin")?;
        let t = line.trim().to_ascii_lowercase();
        if t.is_empty() {
            return Ok(default);
        }
        Ok(matches!(t.as_str(), "y" | "yes"))
    }
}

pub(crate) async fn install_with<I: McpIo>(ctx: &Ctx, io: &mut I) -> Result<()> {
    let token = config::load(&ctx.paths)?
        .ok_or_else(|| anyhow!("not logged in; run `eth-tools auth login` first"))?;
    install_to(
        ctx,
        io,
        &candidate_paths(),
        &token,
        ETH_TOOLS_MCP_URL,
    )
    .await
}

/// Test-injectable inner. Caller supplies the candidate paths and
/// credentials, so unit tests can point at a `TempDir` and skip the
/// real-config path entirely.
pub(crate) async fn install_to<I: McpIo>(
    _ctx: &Ctx,
    io: &mut I,
    candidates: &[(&str, PathBuf)],
    creds: &Credentials,
    mcp_url: &str,
) -> Result<()> {
    let present: Vec<&(&str, PathBuf)> =
        candidates.iter().filter(|(_, p)| p.exists()).collect();

    if present.is_empty() {
        eprintln!("No MCP-aware client configs were found in the standard locations:");
        for (label, p) in candidates {
            eprintln!("  {label}: {}", p.display());
        }
        eprintln!(
            "\nInstall instructions:\n  \
             - Claude Desktop: https://modelcontextprotocol.io/quickstart/user\n  \
             - Cursor:         https://docs.cursor.com/context/model-context-protocol\n\n\
             After installing, add the following stanza under \"mcpServers\":\n"
        );
        let stanza = stanza_for(creds, mcp_url);
        output::print_json(&stanza);
        return Ok(());
    }

    eprintln!("Found MCP-aware client configs:");
    for (label, p) in &present {
        eprintln!("  - {label}: {}", p.display());
    }

    if !io.confirm(
        "Update all of them with an eth-tools MCP entry?",
        true,
    )? {
        eprintln!("Aborted; no files were modified.");
        return Ok(());
    }

    let mut updated = Vec::new();
    for (label, p) in &present {
        match install_one(p, creds, mcp_url) {
            Ok(()) => updated.push((*label, (*p).clone())),
            Err(e) => eprintln!("  ! {label} ({}): {e}", p.display()),
        }
    }

    if updated.is_empty() {
        return Err(anyhow!("no client configs were updated"));
    }
    println!("Wrote eth-tools MCP stanza to:");
    for (label, p) in &updated {
        println!("  + {label}: {} (backup: {}.bak)", p.display(), p.display());
    }
    Ok(())
}

/// Build the JSON stanza we want under `mcpServers.eth-tools`.
fn stanza_for(creds: &Credentials, mcp_url: &str) -> Value {
    json!({
        "url": mcp_url,
        "headers": {
            "Authorization": format!("Bearer {}", creds.token())
        }
    })
}

/// Merge our stanza into the JSON at `path`, preserving every other entry.
/// Writes a `.bak` first (overwriting any prior `.bak`) so a misformatted
/// merge can be rolled back trivially.
pub(crate) fn install_one(path: &Path, creds: &Credentials, mcp_url: &str) -> Result<()> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read existing config {}", path.display()))?;
    let mut doc: Value = if raw.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&raw)
            .with_context(|| format!("parse existing config {} as JSON", path.display()))?
    };

    // `mcpServers` is the standard top-level key both Claude Desktop and
    // Cursor read from. Create it if missing; do NOT overwrite siblings.
    let obj = doc
        .as_object_mut()
        .ok_or_else(|| anyhow!("config root is not a JSON object: {}", path.display()))?;
    let servers = obj
        .entry("mcpServers".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let servers_obj = servers
        .as_object_mut()
        .ok_or_else(|| anyhow!("`mcpServers` is not a JSON object"))?;

    servers_obj.insert("eth-tools".to_string(), stanza_for(creds, mcp_url));

    // Back up the original, then write the merged doc.
    let bak = path.with_extension(
        path.extension()
            .map(|e| format!("{}.bak", e.to_string_lossy()))
            .unwrap_or_else(|| "bak".to_string()),
    );
    std::fs::copy(path, &bak)
        .with_context(|| format!("write backup {}", bak.display()))?;

    let pretty = serde_json::to_string_pretty(&doc).expect("Value serializes");
    std::fs::write(path, pretty)
        .with_context(|| format!("write merged config {}", path.display()))?;
    Ok(())
}

// ---------- from-card ----------

/// `eth-tools mcp from-card <url>` — fetch a remote agent card and emit
/// a Deno TS file that wraps it as an MCP server.
///
/// We deliberately keep this CLI-local rather than calling into
/// `eth_tools_mcp` — the MCP write-tools crate is still a stub today and
/// adding a placeholder there would force a test-surface ripple across
/// another crate for a CLI-only ergonomic.
pub async fn from_card(ctx: &Ctx, url: &str, out: Option<&str>) -> Result<()> {
    let card = ctx.client()?.fetch_agent_card(url).await?;
    let name = card
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("agent")
        .to_string();
    let endpoint = card
        .get("services")
        .and_then(Value::as_array)
        .and_then(|svcs| {
            svcs.iter()
                .find(|s| s.get("type").and_then(Value::as_str) == Some("A2A"))
                .or_else(|| svcs.first())
                .and_then(|s| s.get("endpoint").and_then(Value::as_str))
                .map(String::from)
        })
        .unwrap_or_else(|| "https://example.com/agent".to_string());

    let ts = render_ts_template(&name, &endpoint, url);

    let out_path = out
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("./mcp-{}.ts", slugify(&name))));
    std::fs::write(&out_path, &ts)
        .with_context(|| format!("write Deno TS file {}", out_path.display()))?;
    println!("wrote {} ({} bytes)", out_path.display(), ts.len());
    Ok(())
}

/// Tiny ~80-line Deno template. Parametrized so a future schema migration
/// only changes this function. Intentionally has no external Deno deps
/// other than the standard `@modelcontextprotocol/sdk` shim.
fn render_ts_template(name: &str, endpoint: &str, card_url: &str) -> String {
    let safe_name = name.replace('"', "\\\"");
    let safe_ep = endpoint.replace('"', "\\\"");
    let safe_card = card_url.replace('"', "\\\"");
    format!(
        r#"// Auto-generated by `eth-tools mcp from-card`.
// Source agent card: {safe_card}
// To run: deno run -A {{this_file}}
//
// This shim wraps a remote A2A endpoint as an MCP server so Claude Desktop /
// Cursor can talk to it via the standard transport.
import {{ Server }} from "npm:@modelcontextprotocol/sdk/server/index.js";
import {{ StdioServerTransport }} from "npm:@modelcontextprotocol/sdk/server/stdio.js";
import {{
  CallToolRequestSchema,
  ListToolsRequestSchema,
}} from "npm:@modelcontextprotocol/sdk/types.js";

const AGENT_NAME = "{safe_name}";
const AGENT_ENDPOINT = "{safe_ep}";

const server = new Server(
  {{ name: AGENT_NAME, version: "0.1.0" }},
  {{ capabilities: {{ tools: {{}} }} }},
);

server.setRequestHandler(ListToolsRequestSchema, async () => ({{
  tools: [{{
    name: "invoke",
    description: `Invoke ${{AGENT_NAME}} via its A2A endpoint`,
    inputSchema: {{
      type: "object",
      properties: {{
        input: {{ type: "object", description: "Free-form JSON forwarded to the agent" }},
      }},
      required: ["input"],
    }},
  }}],
}}));

server.setRequestHandler(CallToolRequestSchema, async (req) => {{
  if (req.params.name !== "invoke") {{
    throw new Error(`unknown tool: ${{req.params.name}}`);
  }}
  const body = JSON.stringify(req.params.arguments ?? {{}});
  const resp = await fetch(AGENT_ENDPOINT, {{
    method: "POST",
    headers: {{ "content-type": "application/json" }},
    body,
  }});
  const text = await resp.text();
  if (!resp.ok) {{
    throw new Error(`agent returned ${{resp.status}}: ${{text}}`);
  }}
  return {{ content: [{{ type: "text", text }}] }};
}});

await server.connect(new StdioServerTransport());
"#
    )
}

fn slugify(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn install_one_merges_preserving_other_entries() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("claude_desktop_config.json");
        std::fs::write(
            &cfg,
            r#"{
              "mcpServers": {
                "existing-other": {
                  "url": "https://other.example/mcp",
                  "headers": { "X": "Y" }
                }
              }
            }"#,
        )
        .unwrap();

        let creds = Credentials::new("tok_secret", None);
        install_one(&cfg, &creds, "https://eth-tools.dev/api/mcp").unwrap();

        let parsed: Value =
            serde_json::from_slice(&std::fs::read(&cfg).unwrap()).unwrap();
        let servers = parsed.get("mcpServers").unwrap();
        assert!(servers.get("existing-other").is_some(), "must preserve sibling");
        let stanza = servers.get("eth-tools").expect("our stanza is present");
        assert_eq!(
            stanza.get("url").and_then(Value::as_str),
            Some("https://eth-tools.dev/api/mcp")
        );
        assert_eq!(
            stanza
                .get("headers")
                .and_then(|h| h.get("Authorization"))
                .and_then(Value::as_str),
            Some("Bearer tok_secret")
        );

        // .bak file exists with the original contents.
        let bak = cfg.with_extension("json.bak");
        assert!(bak.exists(), "backup must be written: {}", bak.display());
        let bak_text = std::fs::read_to_string(&bak).unwrap();
        assert!(bak_text.contains("existing-other"));
        assert!(!bak_text.contains("eth-tools"), "bak must be the pre-merge doc");
    }

    #[test]
    fn install_one_creates_mcp_servers_key_if_missing() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("claude_desktop_config.json");
        std::fs::write(&cfg, "{}").unwrap();
        let creds = Credentials::new("tok", None);
        install_one(&cfg, &creds, "https://eth-tools.dev/api/mcp").unwrap();
        let parsed: Value = serde_json::from_slice(&std::fs::read(&cfg).unwrap()).unwrap();
        assert!(parsed.get("mcpServers").and_then(|s| s.get("eth-tools")).is_some());
    }

    #[test]
    fn install_one_rejects_non_object_root() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("c.json");
        std::fs::write(&cfg, "[]").unwrap();
        let err =
            install_one(&cfg, &Credentials::new("t", None), "https://x").unwrap_err();
        assert!(err.to_string().contains("not a JSON object"));
    }

    #[test]
    fn install_one_handles_empty_file() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("c.json");
        std::fs::write(&cfg, "").unwrap();
        install_one(&cfg, &Credentials::new("t", None), "https://x").unwrap();
        let parsed: Value = serde_json::from_slice(&std::fs::read(&cfg).unwrap()).unwrap();
        assert_eq!(parsed["mcpServers"]["eth-tools"]["url"], "https://x");
    }

    #[test]
    fn slugify_is_filesystem_safe() {
        assert_eq!(slugify("Risk Scorer / v2"), "risk-scorer---v2");
        assert_eq!(slugify("---abc---"), "abc");
    }

    #[test]
    fn ts_template_contains_endpoint_and_name() {
        let ts = render_ts_template("Risk Scorer", "https://api.example/a2a", "https://card.example/a.json");
        assert!(ts.contains("Risk Scorer"));
        assert!(ts.contains("https://api.example/a2a"));
        assert!(ts.contains("https://card.example/a.json"));
        // No unescaped double-quote injection.
        let injected = render_ts_template("foo\"; rm -rf /", "https://x", "https://y");
        assert!(injected.contains("foo\\\""));
    }

    struct YesIo;
    impl McpIo for YesIo {
        fn confirm(&mut self, _: &str, _: bool) -> Result<bool> {
            Ok(true)
        }
    }
    struct NoIo;
    impl McpIo for NoIo {
        fn confirm(&mut self, _: &str, _: bool) -> Result<bool> {
            Ok(false)
        }
    }

    fn ctx_at(tmp: &std::path::Path) -> Ctx {
        Ctx {
            api_url: "https://eth-tools.dev".to_string(),
            json: false,
            paths: crate::config::ConfigPaths::at(tmp),
            allow_untrusted_login: false,
        }
    }

    #[tokio::test]
    async fn install_to_falls_back_to_instructions_when_no_configs_present() {
        let dir = tempdir().unwrap();
        let ctx = ctx_at(dir.path());
        let creds = Credentials::new("tok", None);
        let missing = dir.path().join("does-not-exist.json");
        let candidates = vec![("nonesuch", missing)];
        // No client configs exist; must still succeed.
        install_to(&ctx, &mut YesIo, &candidates, &creds, "https://x")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn install_to_respects_user_decline() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("c.json");
        std::fs::write(&cfg, r#"{"mcpServers": {}}"#).unwrap();
        let ctx = ctx_at(dir.path());
        let creds = Credentials::new("tok", None);
        let candidates = vec![("test", cfg.clone())];
        install_to(&ctx, &mut NoIo, &candidates, &creds, "https://x")
            .await
            .unwrap();
        // File was not modified — still empty mcpServers, no eth-tools entry.
        let parsed: Value = serde_json::from_slice(&std::fs::read(&cfg).unwrap()).unwrap();
        assert!(parsed["mcpServers"].get("eth-tools").is_none());
        // And no backup was written.
        assert!(!cfg.with_extension("json.bak").exists());
    }

    #[tokio::test]
    async fn install_to_writes_when_user_confirms() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join("c.json");
        std::fs::write(&cfg, r#"{"mcpServers": {}}"#).unwrap();
        let ctx = ctx_at(dir.path());
        let creds = Credentials::new("tok", None);
        let candidates = vec![("test", cfg.clone())];
        install_to(&ctx, &mut YesIo, &candidates, &creds, "https://x")
            .await
            .unwrap();
        let parsed: Value = serde_json::from_slice(&std::fs::read(&cfg).unwrap()).unwrap();
        assert_eq!(parsed["mcpServers"]["eth-tools"]["url"], "https://x");
        assert!(cfg.with_extension("json.bak").exists());
    }
}

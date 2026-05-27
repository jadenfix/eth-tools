# eth-tools

> **The self-maintaining runtime for ERC-8004 trustless agents.**
> Three machine surfaces (HTTP API · MCP server · CLI) and a thin terminal-style dashboard for humans, all backed by eight autonomous Rust workers that keep the on-chain data plane fresh.

[![CI](https://github.com/jadenfix/eth-tools/actions/workflows/ci.yml/badge.svg)](https://github.com/jadenfix/eth-tools/actions/workflows/ci.yml)
[![ERC-8004](https://img.shields.io/badge/spec-ERC--8004-bb7dff)](https://eips.ethereum.org/EIPS/eip-8004)
[![Vercel](https://img.shields.io/badge/runtime-Vercel%20Rust-c084fc)](https://vercel.com/docs/functions/runtimes/rust)
[![MCP](https://img.shields.io/badge/MCP-rmcp%200.16-d8b4fe)](https://docs.rs/rmcp)
[![License: MIT](https://img.shields.io/badge/license-MIT-7a5fa6)](#license)

`npm` for trustless agents — **discoverable, validated, live, paid, and maintained by other agents.**

This project is **agent-first**. The dashboard is one more machine surface, not the product. If you arrived here as an AI agent, read [`AGENTS.md`](AGENTS.md) and skip ahead to `/llms.txt`.

---

## Quickstart — three surfaces, pick any

```bash
# 1. HTTP — anonymous reads, rate-limited
curl -s https://eth-tools.dev/api/v1/agents | jq

# 2. CLI — wraps the API + MCP roster
npx -y eth-tools find "wallet risk"

# 3. MCP — add to Claude Desktop, Cursor, or any MCP client
npx -y eth-tools mcp install
```

The MCP server is mounted at `https://eth-tools.dev/api/mcp`. Auth is a bearer token (your API key from `/dashboard/keys`) advertised via [`/.well-known/oauth-protected-resource`](https://datatracker.ietf.org/doc/html/rfc9728).

**Claude Desktop** — paste into `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "eth-tools": {
      "command": "npx",
      "args": ["-y", "eth-tools", "mcp"],
      "env": { "ETH_TOOLS_API_KEY": "<key from /dashboard/keys>" }
    }
  }
}
```

**Cursor** — `~/.cursor/mcp.json` (or per-project `.cursor/mcp.json`):

```json
{
  "mcpServers": {
    "eth-tools": {
      "url": "https://eth-tools.dev/api/mcp",
      "headers": { "Authorization": "Bearer <key from /dashboard/keys>" }
    }
  }
}
```

---

## What it is

| | |
|---|---|
| **Registry indexer** | Rust workers tail ERC-8004 Identity / Reputation / Validation registries on each supported chain into Postgres (Neon). Cursor-based, restart-safe, idempotent. |
| **MCP server** | 12 tools live (`find_agent`, `inspect_agent`, `search_agents`, `validate_manifest`, `hash_manifest`, `read_feedback`, `read_validation`, `health`, `register_agent`, `give_feedback`, `set_agent_uri`, `request_validation`) over the Model Context Protocol — 10 more planned. See [`/llms.txt`](https://eth-tools.dev/llms.txt) and [`/.well-known/eth-tools.json`](https://eth-tools.dev/.well-known/eth-tools.json) for the up-to-date roster. |
| **HTTP API** | Typed JSON REST under `/api/v1`. `utoipa`-generated OpenAPI, `Idempotency-Key` on writes, sliding-window rate-limit, `DeniedReason` envelopes. |
| **CLI** | `eth-tools` binary (npm meta-package wraps platform binaries). Mirrors the MCP roster plus workers, wallet, backfill. |
| **Paid-write relay** | Optional env-var wallet, capped per-tx. Five hard rails: kill switch, chain allowlist, recipient allowlist, balance ceiling, daily cap. |

## What it is not

- Not a new payment standard — writes go out as standard EVM transactions over x402.
- Not a UI fork of an existing explorer — the dashboard is thin and read-mostly; the data plane is the product.
- Not TEE / confidential compute — the wallet runs in a plain Vercel function behind the rails above. Blast radius is bounded, not zero.
- Not another reputation algorithm — we surface canonical events + a v0 composite; bring your own scorer.

---

## Architecture

```
   chains ─┬─►  api/cron/*  ──►  crates/workers ──►  Postgres (Neon)
           │    (8 jobs / Vercel cron)              │
           │                                        ▼
           │                                  Upstash Redis  (rate-limit, KV)
           │                                  Vercel Blob    (manifest snapshots)
           │                                  Edge Config    (kill switch)
           │
           └─►  api/mcp  ◄─── crates/mcp   ◄─┐
                api/v1   ◄─── crates/api    ◄─┼─── you (or your agent)
                                crates/cli   ◄─┘
                                crates/payments
                                (env-var wallet · 5 rails · x402)
```

**10 Vercel functions total** — 1 catch-all REST, 1 catch-all MCP, 8 cron entries. Cold-start budget &lt; 10 MB per binary. Everything else is shared via the workspace.

| Path | Role |
|---|---|
| `crates/core`     | ERC-8004 types · chain registry · manifest schema · `DeniedReason` |
| `crates/db`       | sqlx queries + migrations (canonical: `crates/db/migrations/0001_init.sql`) |
| `crates/rpc`      | `RotatingProvider` with circuit breaker (Alchemy → QuickNode → public) |
| `crates/workers`  | Shared worker logic; the 8 `api/cron/*.rs` files are thin wrappers |
| `crates/api`      | Axum router for the HTTP API (`#[utoipa::path]` annotations) |
| `crates/mcp`      | `rmcp` tool definitions + Streamable HTTP transport |
| `crates/payments` | x402 facilitator client + env-var wallet with the 5 hard rails |
| `crates/cli`      | `eth-tools` binary (clap) |
| `crates/dev-server` | Local-only Axum binary mounting prod handlers on `:3000` |
| `crates/openapi-gen` | Build-time tool — emits `public/openapi.json` |
| `api/v1/index.rs` | Catch-all REST function (Vercel rewrites `/api/v1/*` → here) |
| `api/mcp/index.rs` | Catch-all MCP function (Vercel rewrites `/api/mcp/*` → here) |
| `api/cron/*.rs`   | 8 cron entrypoints (paths static — Vercel cron requires it) |
| `app/`            | Next.js 15 — terminal-style landing + dashboard + .well-known + llms.txt |

---

## Status

**Pre-alpha.** Phase-by-phase rollout — see [`AGENTS.md`](AGENTS.md) for the active branch map. Public domain `eth-tools.dev` is live; production cron starts after the merge train completes.

- **Live now:** terminal-style landing · `/llms.txt` · `/.well-known/eth-tools.json` · `/.well-known/oauth-protected-resource` · `/openapi.json` · `/dashboard`
- **Behind the merge train:** the 12-tool MCP server (10 more planned), env-var wallet (Base only), x402 facilitator, 16 CLI subcommands, 8 cron workers
- **Cadence-once-live:** registry scrape every 2 min · manifest re-fetch every 15 min · endpoint prober every 30 min · reputation/validation roll-up every 5 min · trust-score recompute every hour · wallet sweep + watchdog daily

Track shipping in [GitHub Issues](https://github.com/jadenfix/eth-tools/issues). Live status at [`/dashboard/workers`](https://eth-tools.dev/dashboard/workers).

---

## Local development

```bash
git clone https://github.com/jadenfix/eth-tools.git
cd eth-tools
bash scripts/bootstrap.sh        # Postgres 16 + Dragonfly + Anvil via docker compose
# Terminal A — Rust API + crons on :3000
cargo run -p eth-tools-dev-server
# Terminal B — Next.js dashboard on :3001 (proxies /api/* → :3000)
pnpm dev
```

Open `http://localhost:3001` for the terminal-skinned landing. `vercel dev` is **not** used — the official Rust runtime doesn't support it.

### Common scripts

| Script | What it does |
|---|---|
| `pnpm gen:openapi` | Re-emits `public/openapi.json` from `crates/openapi-gen` |
| `pnpm gen:types`   | Re-emits `app/lib/api-types.ts` from `public/openapi.json` (CI gates on `git diff --exit-code`) |
| `pnpm typecheck`   | TypeScript check across the dashboard |
| `cargo test --workspace` | Rust unit + integration tests |
| `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings` | Pre-commit hygiene |

---

## Deployment

The repo is wired for a single Vercel project; bootstrap once per environment:

```bash
vercel login                    # jaden-8586 scope
vercel link                     # link this dir to the project
vercel env add EVM_PRIVATE_KEY production --sensitive
vercel env add CRON_SECRET     production --sensitive
vercel env add AUTH_SECRET     production --sensitive
vercel env add AUTH_GITHUB_ID  production --sensitive
vercel env add AUTH_GITHUB_SECRET production --sensitive
vercel env add MCP_BEARER_TOKEN   production --sensitive
vercel env add UPSTASH_REDIS_REST_URL    production
vercel env add UPSTASH_REDIS_REST_TOKEN  production --sensitive
vercel env add DATABASE_URL          production --sensitive
vercel env add DATABASE_URL_UNPOOLED production --sensitive
vercel env add RPC_URL_PRIMARY  production
vercel env add RPC_URL_FALLBACK production
vercel env add X402_FACILITATOR_URL  production
vercel env add X402_PAY_TO_ADDRESS   production
vercel --prod                   # ship
```

Pro plan is required (sub-daily cron). Marketplace integrations install Neon (branch-per-preview enabled), Upstash Redis (shared, namespaced by `VERCEL_ENV:VERCEL_GIT_COMMIT_REF`), Vercel Blob, and Edge Config.

Every cron entry no-ops outside production (`VERCEL_ENV != "production"`); manually replay with `?force=1` from the CLI or dashboard.

---

## For AI agents

Every surface here is meant for you:

- **`/llms.txt`** — concise machine-readable summary, llmstxt.org compliant.
- **`/openapi.json`** — typed REST contract, served with `Cache-Control: public, max-age=60`.
- **`/.well-known/eth-tools.json`** — self-describing kit metadata (surfaces, supported chains, repo).
- **`/.well-known/oauth-protected-resource`** — RFC 9728 bearer-token discovery for MCP clients.
- **`AGENTS.md`** — contributor guide that Claude Code / Cursor / Codex load automatically inside the repo.
- **`.claude/commands/eth-tools.md`** and **`.cursor/rules/eth-tools.mdc`** — project rules loaded by those tools' MCP-style discovery.

The dashboard's color theme is intentionally terminal-like (purple-on-black, monospace, scanlines) so it never reads as a human-targeted marketing page.

---

## Security

The wallet path is the most security-critical surface. Five hard rails apply to every signed transaction (see [§10 of the master plan](AGENTS.md)):

1. **Kill switch** — Vercel Edge Config `wallet_enabled=false` propagates in &lt; 15 s.
2. **Chain allowlist** — `chain_id` must equal 8453 (Base mainnet). Hard-coded.
3. **Recipient allowlist** — the 3 ERC-8004 registry addresses. Hard-coded in Rust, not config.
4. **Balance ceiling $5** — refuses to sign if over-funded; W8 queues a Safe sweep.
5. **Daily spend cap $1** — Upstash atomic `INCRBY`; over-cap denials write `denials` row.

Every deny is recorded in the `denials` table — survives Vercel's 1-day log retention. `EVM_PRIVATE_KEY` is a Vercel Sensitive env var (never readable after creation), never logged, never returned in any response. See [`AGENTS.md`](AGENTS.md#security-non-negotiables) for the contributor-side non-negotiables.

Found something? Open a [security advisory](https://github.com/jadenfix/eth-tools/security/advisories/new) (private). No bug bounty yet.

---

## Links

| | |
|---|---|
| Production | <https://eth-tools.dev> |
| Dashboard | <https://eth-tools.dev/dashboard> |
| Docs index | <https://eth-tools.dev/docs> |
| Status | <https://eth-tools.dev/dashboard/workers> |
| llms.txt | <https://eth-tools.dev/llms.txt> |
| OpenAPI | <https://eth-tools.dev/openapi.json> |
| ERC-8004 (draft) | <https://eips.ethereum.org/EIPS/eip-8004> |
| Reference contracts | <https://github.com/erc-8004/erc-8004-contracts> |
| Issues | <https://github.com/jadenfix/eth-tools/issues> |

---

## License

MIT. See [LICENSE](LICENSE) (if missing locally, the SPDX in `Cargo.toml`'s `[workspace.package]` is canonical).

Contributing: see [`AGENTS.md`](AGENTS.md) for the contributor / coding-agent guide, including the security non-negotiables that apply to any change in `crates/payments/` or `api/cron/wallet_*.rs`.

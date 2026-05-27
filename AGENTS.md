# AGENTS.md — How to work in this repo

This file is read by Claude Code, Cursor, OpenAI Codex, and any other AI coding agent that operates in this repo.

## What is this project?

**eth-tools** is a self-maintaining runtime for the ERC-8004 trustless-agent registry.
Three machine surfaces (HTTP API · MCP server · CLI) and a Next.js dashboard for humans.
Eight autonomous Rust worker agents keep the data plane fresh.

The master plan lives at `/Users/jadenfix/.claude/plans/let-s-do-8004-buzzing-bear.md` on the maintainer's machine. The repo follows that plan section-by-section; every PR cites a `§` reference.

## The hot path

- **Rust workspace** at `crates/*` — all application logic lives here. Most changes touch `crates/api` (HTTP handlers), `crates/mcp` (MCP tools), `crates/workers` (background jobs), `crates/db` (queries + migrations), or `crates/payments` (wallet).
- **Dashboard** at `app/` — Next.js 15 app router, Tailwind v4, Auth.js. The landing page is `app/page.tsx` with client islands in `app/_landing/*`.
- **Cron entrypoints** at `api/cron/*.rs` — thin Vercel function wrappers that call into `crates/workers`. New crons need both a file here AND a schedule entry in `vercel.json`.

## Repo map

| Path | Owner |
|---|---|
| `crates/core` | ERC-8004 types, chain registry, manifest schema, `DeniedReason` |
| `crates/db` | sqlx queries + migrations (canonical schema: `crates/db/migrations/0001_init.sql`) |
| `crates/rpc` | RotatingProvider with circuit breaker |
| `crates/workers` | Shared worker logic (8 worker entries in `api/cron/*.rs` wrap these) |
| `crates/api` | Axum router for the HTTP API |
| `crates/mcp` | rmcp tool definitions |
| `crates/payments` | x402 + env-var wallet with hard rails (plan §10) |
| `crates/cli` | `eth-tools` binary |
| `crates/dev-server` | Local-only Axum binary mounting prod handlers on :3000 |
| `crates/openapi-gen` | Build-time tool; emits `app/openapi.json` |
| `api/v1/index.rs` | Catch-all REST function (Vercel; `vercel.json` rewrites `/api/v1/*` → here) |
| `api/mcp/index.rs` | MCP server function (Vercel; `vercel.json` rewrites `/api/mcp/*` → here) |
| `api/cron/*.rs` | 8 cron worker functions (paths are static — Vercel cron requires it) |
| `app/` | Next.js 15 dashboard + Auth.js + .well-known + landing |

## Bootstrap

```bash
bash scripts/bootstrap.sh
# Terminal A:  cargo run -p eth-tools-dev-server
# Terminal B:  pnpm dev
```

## Conventions

- **Rust:** stable channel pinned via `rust-toolchain.toml`. `rustfmt` + `clippy -D warnings` on every PR.
- **Edition:** 2021. Workspace deps live in root `Cargo.toml [workspace.dependencies]`.
- **Cold-start budget:** every Rust function under 10 MB. Don't pull in features you don't need.
- **Errors:** typed `DeniedReason` for any "not allowed" path. Don't return opaque 5xx.
- **Logs:** `tracing` with JSON output. **Never** log `std::env::var(...)` results.
- **Tests:** mandatory per-crate. `payments` and `core` require ≥80% coverage before each phase completes.
- **Migrations:** every `0NNN_*.sql` ships with a paired `down.sql`.

## Security non-negotiables (plan §10.5)

1. `EVM_PRIVATE_KEY` never printed.
2. Wallet recipients hard-coded in `crates/payments::wallet::ALLOWED_RECIPIENTS_HEX`.
3. Chain allowlist hard-coded (`ALLOWED_CHAIN_ID = 8453`).
4. Balance ceiling, daily spend cap, kill switch — see `crates/payments`.
5. Idempotency on every mutating endpoint.

If you change anything in `crates/payments/` or `api/cron/wallet_*.rs`, ping `@jadenfix` (CODEOWNERS will enforce).

## How to add a new HTTP endpoint

1. Write the handler in `crates/api/src/handlers/<name>.rs` with a `utoipa` doc-attr.
2. Mount it in `crates/api/src/lib.rs` under the `Router` builder.
3. Register the schema in `crates/openapi-gen/src/main.rs` so it lands in `app/openapi.json`.
4. `pnpm gen:types` regenerates `app/lib/api-types.ts`.
5. Add a handler test under `crates/api/tests/` covering the happy path + one `DeniedReason`.

## How to add a new MCP tool

1. Drop a file in `crates/mcp/src/tools/<name>.rs` implementing `Tool` from `rmcp`.
2. Register it in `crates/mcp/src/lib.rs` `register_tools()`.
3. Read-only tools call into `crates/api` helpers; write tools must go through `crates/payments::wallet::sign_and_send` — never bypass.
4. Add an integration test under `crates/mcp/tests/` that drives the tool through the MCP transport.
5. Document the tool surface in `app/llms.txt/route.ts`.

## How to run tests

```bash
cargo test --workspace          # Rust unit + integration
pnpm typecheck                  # TypeScript
pnpm test                       # Dashboard smoke tests (when wired)
```

## How to ship a change

1. Branch off `main`: `git checkout -b feat/<scope>-<verb>`.
2. Code + tests + docs.
3. `cargo fmt && cargo clippy && cargo test && pnpm typecheck && pnpm gen:types`.
4. `gh pr create` using the template at `.github/pull_request_template.md`.
5. CI must be green. Migration PRs additionally require the production environment approval.

## Don't

- Don't add a new Vercel function without a `[[bin]]` entry in root `Cargo.toml`.
- Don't pull in heavy deps (full-feature `tokio`, OpenSSL, etc.) — see cold-start budget.
- Don't introduce Cloudflare, Sentry, Axiom, Datadog, or any third-party observability — Vercel built-in + Postgres audit only at MVP.
- Don't bypass the wallet rails. Every signed transaction goes through `crates/payments::wallet::sign_and_send`.
- Don't log secrets. `EVM_PRIVATE_KEY`, API keys, OAuth tokens — none of these belong in `tracing` events, even at `trace!` level.
- Don't add new top-level npm or `[workspace.dependencies]` entries without discussion. We're optimising for cold-start size and supply-chain surface; raise it in an issue first.

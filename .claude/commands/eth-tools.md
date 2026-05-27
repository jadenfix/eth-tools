---
description: Project context for eth-tools — load before any non-trivial edit.
---

# eth-tools — context for Claude Code

**eth-tools** is a self-maintaining runtime for the ERC-8004 trustless-agent registry. Three machine surfaces (HTTP API, MCP server, CLI) plus a Next.js dashboard, all backed by autonomous Rust workers that keep the on-chain data plane fresh. Read `AGENTS.md` at repo root for full conventions before starting work.

## Hot path

- **Rust workspace** at `crates/*` — `crates/api` (HTTP handlers), `crates/mcp` (MCP tools), `crates/workers` (background jobs), `crates/db` (queries + migrations), `crates/payments` (wallet rails).
- **Dashboard** at `app/` — Next.js 15 app router, Tailwind v4, Auth.js. Landing page is `app/page.tsx` with client islands in `app/_landing/*`. Don't touch `app/dashboard/*` unless that's the task.
- **Cron entrypoints** at `api/cron/*.rs` — thin Vercel function wrappers around `crates/workers`. Schedules live in `vercel.json`.

## How to add a new HTTP endpoint

1. Handler in `crates/api/src/handlers/<name>.rs` with a `utoipa` doc-attr.
2. Mount in `crates/api/src/lib.rs`.
3. Register schema in `crates/openapi-gen/src/main.rs`.
4. Run `pnpm gen:types` to refresh `app/lib/api-types.ts`.
5. Add a handler test in `crates/api/tests/`.

## How to add a new MCP tool

1. File in `crates/mcp/src/tools/<name>.rs` implementing `rmcp::Tool`.
2. Register in `crates/mcp/src/lib.rs` `register_tools()`.
3. Write tools MUST go through `crates/payments::wallet::sign_and_send`.
4. Integration test in `crates/mcp/tests/`.
5. Document in `app/llms.txt/route.ts`.

## How to run tests

```bash
cargo test --workspace
pnpm typecheck
pnpm test
```

## Don'ts

- Don't bypass the wallet rails (allowlist, chain allowlist, balance ceiling, daily cap, kill switch).
- Don't log secrets — no `EVM_PRIVATE_KEY`, API keys, or OAuth tokens in `tracing` events.
- Don't add new top-level npm or `[workspace.dependencies]` entries without discussion. Cold-start budget is real.
- Don't add Cloudflare / Sentry / Axiom / Datadog. Vercel built-in + Postgres audit only at MVP.

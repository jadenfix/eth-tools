---
description: Project context for eth-tools — load before any non-trivial edit.
---

# eth-tools — context for Claude Code

**eth-tools** is an agent-first, self-maintaining runtime for the ERC-8004 trustless-agent registry. Three machine surfaces (HTTP API, MCP server, CLI) plus a deliberately terminal-styled Next.js dashboard. Backed by eight autonomous Rust workers that keep the on-chain data plane fresh.

Read `AGENTS.md` at repo root for the full contributor guide before starting work. The master plan is at `/Users/jadenfix/.claude/plans/let-s-do-8004-buzzing-bear.md` on the maintainer's machine — every PR cites a `§` reference.

## Hot path

- **Rust workspace** at `crates/*` — `crates/api` (HTTP handlers), `crates/mcp` (MCP tools), `crates/workers` (background jobs), `crates/db` (queries + migrations), `crates/payments` (wallet rails), `crates/rpc` (`RotatingProvider`), `crates/core` (types + manifest schema + ssrf), `crates/cli`, `crates/dev-server`, `crates/openapi-gen`.
- **Dashboard** at `app/` — Next.js 15 App Router, Tailwind v4, Auth.js v5. Landing at `app/page.tsx`; islands in `app/_landing/*`. **Theme is terminal-style purple/black with monospace + scanlines — preserve it.** Reusable primitives in `app/globals.css`: `term-window`, `term-titlebar`, `term-prompt`, `term-prompt-bare`, `term-comment`, `term-cursor`, `term-pill[-ok|-warn|-err|-acc]`, `term-led[-ok|-warn|-err|-idle]`, `term-link`, `term-ascii`, `term-divider`.
- **Cron entrypoints** at `api/cron/*.rs` — thin Vercel function wrappers around `crates/workers`. Schedules live in `vercel.json`.
- **Catch-all functions** at `api/v1/index.rs` and `api/mcp/index.rs`. Total Vercel function count: 10.

## How to add a new HTTP endpoint

1. Handler in `crates/api/src/handlers/<name>.rs` with a `#[utoipa::path]` doc-attr.
2. Mount in `crates/api/src/lib.rs`.
3. Register schema in `crates/openapi-gen/src/main.rs`.
4. Run `pnpm gen:types` to refresh `app/lib/api-types.ts`.
5. Add a handler test in `crates/api/tests/` (happy path + one `DeniedReason`).
6. Mutating? Wire `Idempotency-Key` enforcement.

## How to add a new MCP tool

1. File in `crates/mcp/src/tools/<name>.rs` implementing `rmcp::Tool`.
2. Register in `crates/mcp/src/lib.rs` `register_tools()`.
3. Write tools MUST go through `crates/payments::wallet::sign_and_send` — never bypass.
4. Integration test in `crates/mcp/tests/`.
5. Document in `app/llms.txt/route.ts`.

## How to add a new cron worker

1. File in `api/cron/<name>.rs` — ~30-line wrapper calling `eth_tools_workers::serve_with_context(...)`.
2. Add `[[bin]]` entry in root `Cargo.toml`.
3. Add `maxDuration` in `vercel.json` `functions`.
4. Add `path + schedule` in `vercel.json` `crons`.
5. Implement body in `crates/workers/src/<name>.rs`. Honor all four invariants: advisory lock at entry · prod-only · cron-secret check · `worker_runs` telemetry.

## How to add a dashboard page

1. Create `app/dashboard/<slug>/page.tsx`. Server-render where possible.
2. Use the `term-*` utility classes from `app/globals.css`. Do NOT introduce a new color scheme or font; the terminal aesthetic is the spec.
3. Auth via `await auth()` from `@/lib/auth`; respect `oauthEnabledHost()` for PR previews.
4. Anything fetched from the Rust API uses the typed helpers in `app/lib/rust.ts`.

## How to run tests

```bash
cargo test --workspace          # Rust unit + integration (testcontainers Postgres)
pnpm typecheck                  # TypeScript across the dashboard
pnpm test                       # Dashboard smoke tests (node:test)
pnpm gen:check                  # Asserts no drift between handlers and api-types.ts
```

## Don'ts

- Don't bypass the wallet rails (kill switch · chain allowlist · recipient allowlist · balance ceiling · daily cap). All five enforced in `crates/payments::wallet::sign_and_send`.
- Don't log secrets — no `EVM_PRIVATE_KEY`, API keys, OAuth tokens, MCP bearers, or signed tx hex in `tracing` events at any level.
- Don't add new top-level npm or `[workspace.dependencies]` entries without discussion. Cold-start budget is real (< 10 MB per function).
- Don't add Cloudflare / Sentry / Axiom / Datadog. Vercel built-in + Postgres audit tables only at MVP.
- Don't change the dashboard's visual theme. Purple-on-black terminal aesthetic; if you need a new component, build it from the `term-*` primitives.
- Don't run `vercel dev` against the Rust functions — it's not supported. Use `cargo run -p eth-tools-dev-server`.
- Don't add a new Vercel function file without a `[[bin]]` entry in root `Cargo.toml`.
- Don't read `Authorization: x-vercel-cron-secret` — Vercel's cron contract is `Authorization: Bearer ${CRON_SECRET}`. Compare with `subtle` for constant-time.

## When you encounter an obstacle

- Look at existing handlers/tools in the relevant crate for the pattern; the codebase is consistent.
- Read the master plan (`§N.M` references in commit messages point you to the exact section).
- Migrations have paired `down.sql`. Don't rename or delete numbered migrations on a shipped branch — renumber by adding new ones.
- Cross-cutting changes (anything touching `crates/db/migrations/`, `crates/payments/`, or `api/cron/wallet_*.rs`) require coordination with the maintainer.

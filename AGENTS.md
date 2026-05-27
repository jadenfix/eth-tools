# AGENTS.md — How to work in this repo

This file is loaded by Claude Code, Cursor, OpenAI Codex, and any other AI coding agent that operates inside this repo. It is also the canonical contributor guide for humans.

The product itself is **agent-first**. The Next.js dashboard exists, but it's intentionally terminal-styled (purple-on-black, monospace, scanlines) and read-mostly. The hot path is Rust → HTTP / MCP / CLI. Treat the dashboard as one more machine surface, not the product.

---

## What is this project?

**eth-tools** is a self-maintaining runtime for the ERC-8004 trustless-agent registry. Three machine surfaces (HTTP API · MCP server · CLI) plus the dashboard, all backed by **eight autonomous Rust worker agents** that continuously scrape the on-chain Identity / Reputation / Validation registries, fetch + validate manifests, probe agent endpoints, roll up reputation, and recompute trust scores — without human intervention.

The master plan lives at `/Users/jadenfix/.claude/plans/let-s-do-8004-buzzing-bear.md` on the maintainer's machine. Repo follows it section-by-section; every PR cites a `§` reference.

---

## The hot path

| Layer | Where |
|---|---|
| **Rust workspace** | `crates/*` — all application logic lives here. |
| **Catch-all REST** | `api/v1/index.rs` — Vercel function; bridges `vercel_runtime::Request` ↔ Axum router from `crates/api`. |
| **Catch-all MCP** | `api/mcp/index.rs` — Vercel function; mounts `rmcp` Streamable HTTP from `crates/mcp`. |
| **Cron entrypoints** | `api/cron/*.rs` — 8 thin wrappers (Vercel cron paths must be static). |
| **Dashboard** | `app/` — Next.js 15 App Router, Tailwind v4, Auth.js v5. Landing at `app/page.tsx`; islands in `app/_landing/*`. **The visual theme is terminal-style — purple/black, monospace; never depart from it without coordinating.** |

---

## Repo map

| Path | Owner |
|---|---|
| `crates/core`        | ERC-8004 types · chain registry · manifest schema · `DeniedReason` envelope |
| `crates/db`          | sqlx queries + migrations (canonical: `crates/db/migrations/0001_init.sql`) |
| `crates/rpc`         | `RotatingProvider` with circuit breaker (Alchemy → QuickNode → public Base) |
| `crates/workers`     | Shared worker logic (8 worker entries in `api/cron/*.rs` wrap these) |
| `crates/api`         | Axum router for the HTTP API + `utoipa` annotations |
| `crates/mcp`         | `rmcp` tool definitions + Streamable HTTP transport |
| `crates/payments`    | x402 facilitator client + env-var wallet with 5 hard rails |
| `crates/cli`         | `eth-tools` binary (clap) |
| `crates/dev-server`  | Local-only Axum binary mounting prod handlers on `:3000` |
| `crates/openapi-gen` | Build-time tool emitting `app/openapi.json` |
| `api/v1/index.rs`    | Catch-all REST function |
| `api/mcp/index.rs`   | Catch-all MCP function |
| `api/cron/*.rs`      | 8 cron worker functions |
| `app/`               | Next.js dashboard + `.well-known` + `llms.txt` + landing |
| `app/_landing/*`     | Landing-page client islands (LiveCounters · TryIt · FeaturedAgents) |
| `app/dashboard/*`    | Authed dashboard pages (terminal-styled) |
| `app/docs/page.tsx`  | Terminal-style docs index |
| `fixtures/`          | Seed SQL + hand-curated `well-known-agents.json` |
| `vercel.json`        | Function maxDuration, cron schedules, route rewrites |

---

## Bootstrap

```bash
bash scripts/bootstrap.sh
# Terminal A:  cargo run -p eth-tools-dev-server      # Rust API + crons on :3000
# Terminal B:  pnpm dev                                # Next.js dashboard on :3001
```

`vercel dev` is **not** supported for the official Rust runtime. Use the dev-server.

---

## Conventions

- **Rust:** stable channel pinned via `rust-toolchain.toml`. `rustfmt` + `clippy -D warnings` on every PR.
- **Edition:** 2021. Workspace deps live in root `Cargo.toml [workspace.dependencies]`.
- **Cold-start budget:** every Rust function under 10 MB. Don't pull in features you don't need (default-features = false on `tokio`, `reqwest`, `alloy`, `axum`, `tower-http`).
- **Errors:** typed `DeniedReason` for any "not allowed" path. Don't return opaque 5xx.
- **Logs:** `tracing` with JSON output. **Never** log `std::env::var(...)` results, signed tx hex, signatures, or anything that looks like an API key.
- **Tests:** mandatory per-crate. `payments` and `core` require ≥80% coverage before each phase completes.
- **Migrations:** every `0NNN_*.sql` ships with a paired `down.sql`. Up→down→up tested in CI.
- **OpenAPI:** every Axum handler has a `#[utoipa::path]` doc-attr. CI fails on `pnpm gen:types && git diff --exit-code app/openapi.json app/lib/api-types.ts`.

---

## Security non-negotiables (plan §10.5)

1. `EVM_PRIVATE_KEY` is a Vercel Sensitive env var. **Never printed**, never returned in any response, never in `Debug` impls. Clippy denies `format!("{:?}", env::var(...))` patterns.
2. Wallet recipient list is hard-coded in `crates/payments::wallet::ALLOWED_RECIPIENTS_HEX` — the 3 ERC-8004 registry addresses. Adding to it requires CODEOWNERS approval.
3. Wallet chain allowlist hard-coded — `ALLOWED_CHAIN_ID = 8453` (Base mainnet).
4. Balance ceiling ($5), daily spend cap ($1 via Upstash), Edge Config kill switch — all enforced in `crates/payments::wallet::sign_and_send`. Never bypass.
5. Idempotency on every mutating endpoint (`Idempotency-Key` header, 24 h TTL).
6. Cron auth: `Authorization: Bearer ${CRON_SECRET}` (Vercel's documented contract — not `x-vercel-cron-secret`). Constant-time compare via `subtle`.
7. SSRF defense in `crates/core::ssrf` and `crates/mcp` — RFC1918 / loopback / link-local / ULA / IPv4-mapped IPv6 / 6to4 / Teredo all denied; `reqwest` `dns::Resolve` closes the TOCTOU window.

If you change anything in `crates/payments/` or `api/cron/wallet_*.rs`, ping `@jadenfix` (CODEOWNERS will enforce).

---

## How to add a new HTTP endpoint

1. Write the handler in `crates/api/src/handlers/<name>.rs` with a `#[utoipa::path(...)]` doc-attr.
2. Mount it in `crates/api/src/lib.rs` under the `Router` builder.
3. Register the schema in `crates/openapi-gen/src/main.rs` so it lands in `app/openapi.json`.
4. `pnpm gen:types` regenerates `app/lib/api-types.ts`.
5. Add a handler test under `crates/api/tests/` covering the happy path + one `DeniedReason`.
6. Public-read endpoint? It auto-inherits sliding-window rate-limit from the middleware. Mutating endpoint? Add `Idempotency-Key` enforcement.

## How to add a new MCP tool

1. Drop a file in `crates/mcp/src/tools/<name>.rs` implementing `Tool` from `rmcp`.
2. Register it in `crates/mcp/src/lib.rs` `register_tools()`.
3. Read-only tools call into `crates/api` helpers; **write tools must go through `crates/payments::wallet::sign_and_send` — never bypass**.
4. Add an integration test under `crates/mcp/tests/` that drives the tool through the MCP transport.
5. Document the tool name + one-line description in `app/llms.txt/route.ts`.

## How to add a new cron worker

1. Drop a file in `api/cron/<name>.rs` — a ~30-line wrapper that calls `eth_tools_workers::serve_with_context(...)`.
2. Add a `[[bin]]` entry in root `Cargo.toml` (name + path).
3. Add a `maxDuration` entry in `vercel.json` `functions`.
4. Add a `path + schedule` entry in `vercel.json` `crons`.
5. Implement the body in `crates/workers/src/<name>.rs`. Honor all four invariants: advisory lock at entry · prod-only · cron-secret check · `worker_runs` telemetry on completion.

## How to add a dashboard page

1. Page goes under `app/dashboard/<slug>/page.tsx`. **Use the existing terminal aesthetic** — `term-window`, `term-titlebar`, `term-prompt-bare`, `term-pill*`, `term-led*` utility classes from `app/globals.css`.
2. Server-render where possible; only the live-counter and copy-paste islands need `'use client'`.
3. If the page needs auth, gate via `await auth()` from `app/lib/auth.ts` and respect `oauthEnabledHost()` so PR previews show the banner instead of breaking.

---

## How to run tests

```bash
cargo test --workspace          # Rust unit + integration (uses testcontainers for db)
pnpm typecheck                  # TypeScript across the dashboard
pnpm test                       # Dashboard smoke tests (node:test)
pnpm gen:check                  # Asserts no drift between handlers and api-types.ts
```

## How to ship a change

1. Branch off `main`: `git checkout -b feat/<scope>-<verb>`.
2. Code + tests + docs.
3. `cargo fmt && cargo clippy && cargo test && pnpm typecheck && pnpm gen:check`.
4. `gh pr create` using `.github/pull_request_template.md`.
5. CI must be green. Migration PRs additionally require the `production` environment approval (manual gate in `.github/workflows/migrate-prod.yml`).

---

## Don't

- Don't add a new Vercel function without a `[[bin]]` entry in root `Cargo.toml`.
- Don't pull in heavy deps (full-feature `tokio`, OpenSSL, `native-tls`, etc.) — see cold-start budget.
- Don't introduce Cloudflare, Sentry, Axiom, Datadog, or any third-party observability — Vercel built-in + Postgres audit tables only at MVP.
- Don't bypass the wallet rails. Every signed transaction goes through `crates/payments::wallet::sign_and_send`.
- Don't log secrets. `EVM_PRIVATE_KEY`, API keys, OAuth tokens, MCP bearers — none belong in `tracing` events, even at `trace!` level.
- Don't add new top-level npm or `[workspace.dependencies]` entries without discussion. We optimise for cold-start size and supply-chain surface; raise an issue first.
- Don't change the dashboard's visual theme. Purple-on-black terminal aesthetic is the spec, not a default. If you need a new component, build it with the existing `term-*` primitives.
- Don't run `vercel dev` against the Rust functions — it's not supported. Use `cargo run -p eth-tools-dev-server`.

---

## Active branch map (snapshot)

The repo currently has the Phase 4–7 work split across worktree branches awaiting a merge train. If you're trying to understand the full surface area, the branches are:

| Branch | Brings |
|---|---|
| `main` | Phase 0 scaffolding only |
| `feat/phase-1-2-3` | core/db/rpc real impls + dashboard skeleton |
| `feat/phase-4-pr1-rpc-foundation` | `RotatingProvider`, circuit breaker, alloy sol! bindings |
| `feat/phase-4-pr2-worker-substrate` | `WorkerContext`, advisory lock helper, `worker_runs`, cron secret |
| `feat/phase-4-m4-vercel-runtime-2` | vercel_runtime 1.1 → 2.2 migration |
| `feat/phase-4-m5-utoipa` | utoipa-driven `app/openapi.json` |
| `feat/phase-4-w*` | individual worker implementations (W1–W8) |
| `feat/phase-4-http-api-expansion` | 15 missing REST routes + idempotency middleware |
| `feat/phase-5-mcp-server` | `rmcp` server with read-side tools + RFC 9728 |
| `feat/phase-5-api-keys` | OAuth → dashboard issuance, bcrypt middleware |
| `feat/phase-5-mcp-write-tools` | write-side MCP tools |
| `feat/phase-6-wallet-rails` | env-var wallet, 5 hard rails |
| `feat/phase-6.2-signing` | alloy `sign_and_send` + nonce serialization |
| `feat/phase-6-x402` | CDP facilitator client |
| `feat/phase-7-cli-scaffold` / `feat/phase-7-cli-completion` | CLI subcommands |
| `feat/phase-8-landing-content` | landing + docs + terminal skin (this branch) |

Merge order matters — the worker branches forked before the advisory-lock fix landed on `phase-4-pr2`. Coordinate with the maintainer before opening cross-cutting PRs.

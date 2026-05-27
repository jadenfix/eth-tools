// Landing page — terminal-style. eth-tools is agent-first; the dashboard
// is one more machine surface. This page reads as a terminal session, not
// a marketing site, on purpose. Three islands stay client components
// (LiveCounters, TryIt, FeaturedAgents server-only).

import { promises as fs } from 'node:fs';
import path from 'node:path';
import Link from 'next/link';
import LiveCounters from './_landing/LiveCounters';
import TryIt from './_landing/TryIt';
import FeaturedAgents, { type FeaturedAgent } from './_landing/FeaturedAgents';

async function loadFeaturedAgents(): Promise<FeaturedAgent[]> {
  try {
    const file = await fs.readFile(
      path.join(process.cwd(), 'fixtures/well-known-agents.json'),
      'utf8',
    );
    const raw = JSON.parse(file) as unknown;
    if (!Array.isArray(raw)) return [];
    return raw
      .filter((entry): entry is FeaturedAgent => {
        if (entry === null || typeof entry !== 'object') return false;
        const e = entry as Record<string, unknown>;
        return (
          typeof e.chain === 'string' &&
          typeof e.agentId === 'string' &&
          typeof e.label === 'string' &&
          typeof e.description === 'string' &&
          typeof e.manifestUrl === 'string'
        );
      })
      .slice(0, 12);
  } catch {
    return [];
  }
}

export default async function Page() {
  const featured = await loadFeaturedAgents();
  const year = new Date().getUTCFullYear();

  return (
    <main className="relative z-10 mx-auto max-w-5xl px-4 pb-24 pt-10 sm:px-6 sm:pt-16">
      {/* ── Top bar ─────────────────────────────────────────────────────── */}
      <header
        data-testid="top-bar"
        className="mb-8 flex flex-wrap items-center justify-between gap-3 text-xs"
        style={{ color: 'var(--term-muted)' }}
      >
        <div className="flex items-center gap-3">
          <span className="term-glow" style={{ color: 'var(--term-prompt)', fontWeight: 600 }}>
            eth-tools
          </span>
          <span style={{ color: 'var(--term-border-2)' }}>·</span>
          <span>ERC-8004 runtime</span>
          <span style={{ color: 'var(--term-border-2)' }}>·</span>
          <span>v0.0.1</span>
        </div>
        <nav className="flex flex-wrap items-center gap-x-4 gap-y-1">
          <Link href="/docs" className="term-link">docs</Link>
          <a href="/openapi.json" className="term-link">openapi</a>
          <a href="/llms.txt" className="term-link">llms.txt</a>
          <a href="/.well-known/eth-tools.json" className="term-link">.well-known</a>
          <Link href="/dashboard/workers" className="term-link">status</Link>
          <Link href="/dashboard" className="term-link">dashboard</Link>
          <a
            href="https://github.com/jadenfix/eth-tools"
            target="_blank"
            rel="noopener noreferrer"
            className="term-link"
          >
            github
          </a>
        </nav>
      </header>

      {/* ── Hero: terminal window with cat-like output ──────────────────── */}
      <section className="term-window" aria-labelledby="hero-headline">
        <div className="term-titlebar">
          <span className="term-dot term-dot-r" aria-hidden="true" />
          <span className="term-dot term-dot-y" aria-hidden="true" />
          <span className="term-dot term-dot-g" aria-hidden="true" />
          <span className="ml-3">~/eth-tools — agent@base — 80×24</span>
        </div>
        <div className="term-body">
          <p>
            <span className="term-prompt-bare">$</span>{' '}
            <span style={{ color: 'var(--term-fg)' }}>
              cat /etc/eth-tools/about
            </span>
            <span className="term-cursor" aria-hidden="true" />
          </p>

          <h1
            id="hero-headline"
            data-testid="hero-headline"
            className="mt-4 text-3xl font-semibold tracking-tight sm:text-4xl"
            style={{ color: 'var(--term-fg)' }}
          >
            <span style={{ color: 'var(--term-accent-2)' }}>eth-tools</span>
            <span style={{ color: 'var(--term-muted)' }}>(1)</span>
          </h1>
          <p
            data-testid="hero-subhead"
            className="mt-2 text-lg sm:text-xl"
            style={{ color: 'var(--term-fg-dim)' }}
          >
            The self-maintaining runtime for ERC-8004 agents.
          </p>
          <p className="mt-3 max-w-2xl text-sm sm:text-base" style={{ color: 'var(--term-fg-dim)' }}>
            Discoverable. Validated. Live. Paid. Maintained by other agents.
            Three machine surfaces (HTTP · MCP · CLI) and a thin dashboard for
            humans, all backed by autonomous Rust workers that keep the
            on-chain data plane fresh — without you.
          </p>

          <div className="mt-5 flex flex-wrap gap-2 text-xs">
            <span className="term-pill term-pill-acc">agent-first</span>
            <span className="term-pill">rust · axum · alloy</span>
            <span className="term-pill">vercel fluid · neon · upstash</span>
            <span className="term-pill term-pill-ok">ERC-8004 mainnet</span>
            <span className="term-pill">x402 paid-writes</span>
            <span className="term-pill">MIT</span>
          </div>

          <hr className="term-divider" />

          <p>
            <span className="term-comment">three surfaces — pick one (or all)</span>
          </p>
          <ul className="mt-2 space-y-1.5 text-sm">
            <li>
              <span className="term-prompt-bare">→</span>{' '}
              <span style={{ color: 'var(--term-accent-2)' }}>MCP</span>{' '}
              <span style={{ color: 'var(--term-muted)' }}>
                /api/mcp — 12 tools live · Streamable HTTP · RFC 9728 auth
              </span>
            </li>
            <li>
              <span className="term-prompt-bare">→</span>{' '}
              <span style={{ color: 'var(--term-accent-2)' }}>HTTP</span>{' '}
              <span style={{ color: 'var(--term-muted)' }}>
                /api/v1 — typed JSON, OpenAPI, idempotency, rate-limits
              </span>
            </li>
            <li>
              <span className="term-prompt-bare">→</span>{' '}
              <span style={{ color: 'var(--term-accent-2)' }}>CLI</span>{' '}
              <span style={{ color: 'var(--term-muted)' }}>
                npx eth-tools — 16 subcommands; mirrors MCP + workers
              </span>
            </li>
            <li>
              <span className="term-prompt-bare">→</span>{' '}
              <span style={{ color: 'var(--term-fg-dim)' }}>dashboard</span>{' '}
              <span style={{ color: 'var(--term-muted)' }}>
                /dashboard — keys · agents · workers · alerts · wallet (humans)
              </span>
            </li>
          </ul>
        </div>
      </section>

      {/* ── Live counters: streamed `tail -f` style ─────────────────────── */}
      <LiveCounters />

      {/* ── Try it: command tabs ────────────────────────────────────────── */}
      <TryIt />

      {/* ── What it is / What it isn't ──────────────────────────────────── */}
      <section
        aria-labelledby="what-heading"
        data-testid="what-it-is"
        className="mt-12 grid gap-4 sm:grid-cols-2"
      >
        <div className="term-window">
          <div className="term-titlebar">
            <span className="term-dot term-dot-r" aria-hidden="true" />
            <span className="term-dot term-dot-y" aria-hidden="true" />
            <span className="term-dot term-dot-g" aria-hidden="true" />
            <span className="ml-3">~/about/yes</span>
          </div>
          <div className="term-body text-sm">
            <p>
              <span className="term-prompt-bare">$</span> what is this
            </p>
            <h2 id="what-heading" className="sr-only">What it is</h2>
            <ul className="mt-3 space-y-2" style={{ color: 'var(--term-fg-dim)' }}>
              <li>
                <span style={{ color: 'var(--term-accent-2)' }}>▸ registry indexer</span> — Rust
                workers tail the on-chain Identity, Reputation, and Validation
                registries on each supported chain and write into Postgres.
                Cursor-based, restart-safe, idempotent.
              </li>
              <li>
                <span style={{ color: 'var(--term-accent-2)' }}>▸ MCP server</span> — 12 tools
                live (10 more planned) over the Model Context Protocol. Read agents,
                validate manifests, query reputation, submit signed writes — from
                inside Claude, Cursor, or any MCP client.
              </li>
              <li>
                <span style={{ color: 'var(--term-accent-2)' }}>▸ paid-write relay</span> —
                optional env-var wallet for clients without keys. Five hard
                rails: recipient allowlist · chain allowlist · balance ceiling
                · daily cap · kill switch.
              </li>
            </ul>
          </div>
        </div>

        <div className="term-window">
          <div className="term-titlebar">
            <span className="term-dot term-dot-r" aria-hidden="true" />
            <span className="term-dot term-dot-y" aria-hidden="true" />
            <span className="term-dot term-dot-g" aria-hidden="true" />
            <span className="ml-3">~/about/no</span>
          </div>
          <div className="term-body text-sm">
            <p>
              <span className="term-prompt-bare">$</span> what it isn&apos;t
            </p>
            <ul className="mt-3 space-y-2" style={{ color: 'var(--term-fg-dim)' }}>
              <li>
                <span style={{ color: 'var(--term-err)' }}>✗ new payment standard</span> —
                writes are standard EVM transactions, gated by x402.
              </li>
              <li>
                <span style={{ color: 'var(--term-err)' }}>✗ ui fork of an explorer</span> —
                the dashboard is thin and read-mostly; the data plane is the
                product.
              </li>
              <li>
                <span style={{ color: 'var(--term-err)' }}>✗ TEE / confidential compute</span> —
                the wallet runs in a plain Vercel function behind the hard
                rails above. Bounded blast radius, not zero.
              </li>
              <li>
                <span style={{ color: 'var(--term-err)' }}>✗ another reputation algorithm</span> —
                we surface the canonical events and a v0 composite score;
                bring your own scorer.
              </li>
            </ul>
          </div>
        </div>
      </section>

      {/* ── ASCII architecture diagram ──────────────────────────────────── */}
      <section aria-labelledby="arch-heading" className="mt-10 term-window">
        <div className="term-titlebar">
          <span className="term-dot term-dot-r" aria-hidden="true" />
          <span className="term-dot term-dot-y" aria-hidden="true" />
          <span className="term-dot term-dot-g" aria-hidden="true" />
          <span className="ml-3">~/etc/topology.txt</span>
        </div>
        <div className="term-body">
          <h2 id="arch-heading" className="sr-only">Architecture</h2>
          <pre
            className="term-ascii overflow-x-auto text-xs sm:text-sm"
            style={{ color: 'var(--term-fg-dim)' }}
          >{`   chains ─┬─►  api/cron/*  ──►  crates/workers ──►  Postgres (Neon)
           │    (8 jobs / Vercel cron)              │
           │                                        ▼
           │                                  Upstash Redis  (rate-limit, KV)
           │                                  Vercel Blob    (manifest snapshots)
           │                                  Edge Config    (kill switch)
           │
           └─►  api/mcp ◄──── crates/mcp     ◄─┐
                api/v1  ◄──── crates/api      ◄─┼─── you (or your agent)
                                crates/cli    ◄─┘
                                crates/payments
                                (env-var wallet · 5 rails · x402)
`}</pre>
          <p className="mt-3 text-xs" style={{ color: 'var(--term-muted)' }}>
            10 Vercel functions total: 1 REST · 1 MCP · 8 crons. Cold-start
            budget &lt; 10 MB per binary. Everything else is shared via the
            workspace.
          </p>
        </div>
      </section>

      {/* ── Featured agents: ls -la style ───────────────────────────────── */}
      <FeaturedAgents agents={featured} />

      {/* ── For agent builders ──────────────────────────────────────────── */}
      <section className="mt-12 term-window">
        <div className="term-titlebar">
          <span className="term-dot term-dot-r" aria-hidden="true" />
          <span className="term-dot term-dot-y" aria-hidden="true" />
          <span className="term-dot term-dot-g" aria-hidden="true" />
          <span className="ml-3">~/for-agents</span>
        </div>
        <div className="term-body text-sm">
          <p>
            <span className="term-prompt-bare">$</span>{' '}
            <span style={{ color: 'var(--term-fg)' }}>man eth-tools</span>
          </p>
          <p className="mt-3" style={{ color: 'var(--term-fg-dim)' }}>
            If you are an AI agent: every surface here is meant for you. Start
            with{' '}
            <a href="/llms.txt" className="term-link">/llms.txt</a> for a machine
            summary,{' '}
            <a href="/openapi.json" className="term-link">/openapi.json</a> for
            the typed REST contract, and{' '}
            <a
              href="/.well-known/oauth-protected-resource"
              className="term-link"
            >
              /.well-known/oauth-protected-resource
            </a>{' '}
            to discover the MCP bearer flow.
          </p>
          <p className="mt-3" style={{ color: 'var(--term-fg-dim)' }}>
            If you are a developer working in Claude Code, Cursor, or Codex:
            the repo ships{' '}
            <code style={{ color: 'var(--term-accent-2)' }}>AGENTS.md</code>,{' '}
            <code style={{ color: 'var(--term-accent-2)' }}>
              .claude/commands/eth-tools.md
            </code>{' '}
            and{' '}
            <code style={{ color: 'var(--term-accent-2)' }}>
              .cursor/rules/eth-tools.mdc
            </code>{' '}
            — your agent will load them automatically inside the repo.
          </p>
        </div>
      </section>

      {/* ── Footer prompt ───────────────────────────────────────────────── */}
      <footer
        data-testid="footer"
        className="mt-12 flex flex-wrap items-center justify-between gap-3 text-xs"
        style={{ color: 'var(--term-muted)' }}
      >
        <div>
          <span className="term-prompt-bare">$</span>{' '}
          <span style={{ color: 'var(--term-fg-dim)' }}>logout</span>
          <span className="term-cursor" aria-hidden="true" />
        </div>
        <div className="flex flex-wrap items-center gap-4">
          <span>© {year} eth-tools · MIT</span>
          <a
            href="https://github.com/jadenfix/eth-tools"
            target="_blank"
            rel="noopener noreferrer"
            className="term-link"
          >
            github
          </a>
          <Link href="/dashboard/workers" className="term-link">
            status
          </Link>
          <a
            href="https://twitter.com/eth_tools"
            target="_blank"
            rel="noopener noreferrer"
            className="term-link"
          >
            twitter
          </a>
        </div>
      </footer>
    </main>
  );
}

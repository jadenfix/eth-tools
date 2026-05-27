// Landing page (`eth-tools.dev`).
//
// Server-rendered shell. Three islands are client components:
//   - LiveCounters     — fetches /api/v1/health on mount.
//   - TryIt            — tab state + Add-to-Claude modal.
// FeaturedAgents is a server component that reads the curated JSON fixture
// at build/request time.
//
// No marketing superlatives ("the only", "the fastest"). Every claim points
// to an observable surface in the repo.

import { promises as fs } from 'node:fs';
import path from 'node:path';
import Link from 'next/link';
import LiveCounters from './_landing/LiveCounters';
import TryIt from './_landing/TryIt';
import FeaturedAgents, { type FeaturedAgent } from './_landing/FeaturedAgents';

async function loadFeaturedAgents(): Promise<FeaturedAgent[]> {
  // Source: `fixtures/well-known-agents.json`. Hand-curated; replaced before
  // public launch. Read at request time so editors don't need a redeploy
  // when adding/removing entries.
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

  return (
    <main className="min-h-screen">
      {/* ── Hero ────────────────────────────────────────────────────────── */}
      <section className="mx-auto max-w-5xl px-6 pb-20 pt-24 sm:pt-32">
        <p className="text-xs uppercase tracking-[0.2em] text-zinc-500">
          ERC-8004 · runtime
        </p>
        <h1
          data-testid="hero-headline"
          className="mt-3 text-4xl font-semibold tracking-tight text-zinc-50 sm:text-5xl"
        >
          The self-maintaining runtime for ERC-8004 agents
        </h1>
        <p
          data-testid="hero-subhead"
          className="mt-5 max-w-2xl text-lg text-zinc-300 sm:text-xl"
        >
          Discoverable, validated, live, paid — and maintained by other agents.
        </p>
        <div className="mt-8 flex flex-wrap gap-3">
          <Link
            href="/dashboard/keys"
            className="inline-flex items-center rounded-md bg-zinc-100 px-4 py-2 text-sm font-medium text-zinc-900 hover:bg-white"
          >
            Get an API key
          </Link>
          <Link
            href="/docs"
            className="inline-flex items-center rounded-md border border-zinc-700 px-4 py-2 text-sm font-medium text-zinc-200 hover:bg-zinc-900"
          >
            View the docs
          </Link>
        </div>
      </section>

      {/* ── Live counters strip ─────────────────────────────────────────── */}
      <LiveCounters />

      {/* ── Try it in 30 seconds ────────────────────────────────────────── */}
      <TryIt />

      {/* ── What it is (3 cards) ────────────────────────────────────────── */}
      <section
        aria-labelledby="what-heading"
        data-testid="what-it-is"
        className="mx-auto max-w-5xl px-6 py-16"
      >
        <h2 id="what-heading" className="text-2xl font-semibold tracking-tight">
          What it is
        </h2>
        <div className="mt-6 grid grid-cols-1 gap-4 sm:grid-cols-3">
          <Card
            title="Registry indexer"
            body="Rust workers tail the ERC-8004 Identity, Reputation, and Validation registries on each supported chain. Fresh data, no cron lag from a hosted indexer."
            badge="real-time"
          />
          <Card
            title="MCP server"
            body="20 tools exposed over the Model Context Protocol — read agents, validate manifests, query reputation, and submit signed writes from inside Claude, Cursor, or any MCP client."
            badge="20 tools"
          />
          <Card
            title="Paid-write relay"
            body="Optional env-var wallet relays signed transactions for clients without keys. Capped at $5/tx with five hard rails: recipient allowlist, chain allowlist, balance ceiling, daily cap, kill switch."
            badge="5 hard rails"
          />
        </div>
      </section>

      {/* ── Featured agents carousel ────────────────────────────────────── */}
      <FeaturedAgents agents={featured} />

      {/* ── Footer ──────────────────────────────────────────────────────── */}
      <footer
        data-testid="footer"
        className="border-t border-zinc-800 bg-zinc-950/60"
      >
        <div className="mx-auto flex max-w-5xl flex-col items-start gap-4 px-6 py-10 text-sm text-zinc-400 sm:flex-row sm:items-center sm:justify-between">
          <div>
            <span className="font-mono text-zinc-300">eth-tools</span>
            <span className="ml-2 text-xs text-zinc-500">
              MIT · ERC-8004 runtime
            </span>
          </div>
          <nav aria-label="Footer" className="flex flex-wrap gap-x-5 gap-y-2">
            <a
              href="https://github.com/jadenfix/eth-tools"
              target="_blank"
              rel="noopener noreferrer"
              className="hover:text-zinc-200"
            >
              GitHub
            </a>
            <Link href="/docs" className="hover:text-zinc-200">
              Docs
            </Link>
            <Link href="/dashboard/workers" className="hover:text-zinc-200">
              Status
            </Link>
            <a
              href="https://twitter.com/eth_tools"
              target="_blank"
              rel="noopener noreferrer"
              className="hover:text-zinc-200"
            >
              Twitter
            </a>
            <a
              href="https://discord.gg/eth-tools"
              target="_blank"
              rel="noopener noreferrer"
              className="hover:text-zinc-200"
            >
              Discord
            </a>
          </nav>
        </div>
      </footer>
    </main>
  );
}

function Card({ title, body, badge }: { title: string; body: string; badge: string }) {
  return (
    <div className="rounded-xl border border-zinc-800 bg-zinc-900/40 p-5">
      <div className="flex items-baseline justify-between">
        <h3 className="text-base font-medium text-zinc-100">{title}</h3>
        <span className="rounded bg-zinc-800 px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wider text-zinc-400">
          {badge}
        </span>
      </div>
      <p className="mt-2 text-sm leading-relaxed text-zinc-400">{body}</p>
    </div>
  );
}

// Terminal-style docs index. Real long-form docs live in the README, this
// page is the agent-discoverable directory listing.

import Link from 'next/link';

export const metadata = {
  title: 'docs · eth-tools',
  description: 'Documentation index for eth-tools — HTTP API, MCP server, CLI, dashboard.',
};

type Entry = { path: string; label: string; hint: string; external?: boolean };

const SURFACES: Entry[] = [
  { path: '/api/v1/health',           label: 'GET /api/v1/health',           hint: 'liveness · cursor lag per chain · version' },
  { path: '/api/v1/agents',           label: 'GET /api/v1/agents',           hint: 'paginated list (chain · agent_id · owner · uri)' },
  { path: '/api/v1/agents/base/1',    label: 'GET /api/v1/agents/{chain}/{id}', hint: 'single agent detail' },
  { path: '/api/mcp',                 label: 'POST /api/mcp',                hint: 'Streamable HTTP MCP transport — bearer auth' },
  { path: '/openapi.json',            label: '/openapi.json',                hint: 'utoipa-generated typed contract' },
  { path: '/llms.txt',                label: '/llms.txt',                    hint: 'machine summary for crawling agents' },
  { path: '/.well-known/eth-tools.json',         label: '/.well-known/eth-tools.json',         hint: 'self-describing kit metadata' },
  { path: '/.well-known/oauth-protected-resource', label: '/.well-known/oauth-protected-resource', hint: 'RFC 9728 — MCP bearer discovery' },
];

const EXTERNAL: Entry[] = [
  { path: 'https://github.com/jadenfix/eth-tools',         label: 'github.com/jadenfix/eth-tools',          hint: 'source · issues · PRs', external: true },
  { path: 'https://github.com/jadenfix/eth-tools/blob/main/README.md',     label: 'README.md',     hint: 'quickstart + architecture + deploy', external: true },
  { path: 'https://github.com/jadenfix/eth-tools/blob/main/AGENTS.md',     label: 'AGENTS.md',     hint: 'contributor + coding-agent guide', external: true },
  { path: 'https://eips.ethereum.org/EIPS/eip-8004',                       label: 'EIP-8004 (draft)', hint: 'the spec this runtime serves', external: true },
  { path: 'https://github.com/erc-8004/erc-8004-contracts',                label: 'erc-8004 contracts', hint: 'reference Solidity', external: true },
  { path: 'https://docs.cdp.coinbase.com/x402/welcome',                    label: 'x402 (Coinbase CDP)', hint: 'paid-write protocol used by /api/v1/invoke', external: true },
];

const TOPICS: Entry[] = [
  { path: '/docs#mcp',         label: '# MCP — adding eth-tools to Claude/Cursor', hint: 'config + auth + tool roster' },
  { path: '/docs#cli',         label: '# CLI — npx eth-tools …',                    hint: 'auth · find · inspect · manifest · invoke · workers · wallet · backfill' },
  { path: '/docs#workers',     label: '# Workers — 8 cron jobs',                    hint: 'cadences · invariants · advisory locks' },
  { path: '/docs#wallet',      label: '# Wallet — env-var with 5 hard rails',       hint: 'allowlists · caps · kill switch · audit' },
  { path: '/docs#x402',        label: '# x402 — paid writes',                       hint: '402 challenge + CDP facilitator' },
];

export default function DocsIndex() {
  return (
    <main className="relative z-10 mx-auto max-w-4xl px-4 py-10 sm:px-6">
      <p className="mb-6 flex items-center gap-3 text-xs" style={{ color: 'var(--term-muted)' }}>
        <Link href="/" className="term-link">eth-tools</Link>
        <span>/</span>
        <span style={{ color: 'var(--term-accent-2)' }}>docs</span>
      </p>

      <section className="term-window">
        <div className="term-titlebar">
          <span className="term-dot term-dot-r" aria-hidden="true" />
          <span className="term-dot term-dot-y" aria-hidden="true" />
          <span className="term-dot term-dot-g" aria-hidden="true" />
          <span className="ml-3">man eth-tools(1)</span>
        </div>
        <div className="term-body">
          <p>
            <span className="term-prompt-bare">$</span>{' '}
            <span style={{ color: 'var(--term-fg)' }}>man eth-tools</span>
          </p>
          <p className="mt-3 text-sm" style={{ color: 'var(--term-fg-dim)' }}>
            <strong style={{ color: 'var(--term-accent-2)' }}>NAME</strong> eth-tools — the
            self-maintaining runtime for ERC-8004 trustless agents.
          </p>
          <p className="mt-2 text-sm" style={{ color: 'var(--term-fg-dim)' }}>
            <strong style={{ color: 'var(--term-accent-2)' }}>SYNOPSIS</strong> three machine
            surfaces (HTTP · MCP · CLI) plus a thin dashboard, backed by eight Rust workers on
            Vercel Cron. The product is agent-first; the dashboard is one more surface.
          </p>

          <hr className="term-divider" />

          <Section title="MACHINE SURFACES" entries={SURFACES} />

          <hr className="term-divider" />

          <Section title="TOPICS" entries={TOPICS} />

          <hr className="term-divider" />

          <Section title="EXTERNAL" entries={EXTERNAL} />
        </div>
      </section>

      {/* ── deep-link anchors so the docs index hash-links land somewhere */}
      <Anchor id="mcp" title="MCP — adding eth-tools to Claude / Cursor">
        <p>
          The MCP server is mounted at <code>https://eth-tools.dev/api/mcp</code> with
          Streamable HTTP transport. Auth is a bearer token (your API key) advertised via{' '}
          <a href="/.well-known/oauth-protected-resource" className="term-link">
            /.well-known/oauth-protected-resource
          </a>{' '}
          per RFC 9728.
        </p>
        <p className="mt-2">
          For Claude Desktop, edit{' '}
          <code>~/Library/Application Support/Claude/claude_desktop_config.json</code>; for
          Cursor, <code>~/.cursor/mcp.json</code>. See the landing page modal for paste-ready
          JSON, or run <code>npx eth-tools mcp install</code> to write the config for you.
        </p>
      </Anchor>

      <Anchor id="cli" title="CLI — npx eth-tools …">
        <p>16 subcommands, mirrors the MCP roster plus operational bits:</p>
        <pre
          className="term-ascii mt-2 overflow-x-auto text-xs"
          style={{ color: 'var(--term-fg-dim)' }}
        >{`  eth-tools auth login          # opens dashboard, polls for issued key
  eth-tools find <query>        # registry search
  eth-tools inspect <chain>/<id>
  eth-tools manifest {validate|hash|generate} ./agent-card.json
  eth-tools register --interactive
  eth-tools invoke  <chain>/<id> --input ./input.json
  eth-tools watch   <chain>     # SSE tail of registry events
  eth-tools mcp install         # writes Claude / Cursor config
  eth-tools mcp from-card <url> # generates a runnable MCP server
  eth-tools workers status      # cron telemetry
  eth-tools wallet status       # balance · spend · rails
  eth-tools backfill --chain base --from-block <N>
`}</pre>
      </Anchor>

      <Anchor id="workers" title="Workers — 8 cron jobs">
        <p>
          Each worker is a thin <code>api/cron/*.rs</code> wrapper around shared logic in{' '}
          <code>crates/workers</code>. Cadences range from every minute (wallet-rotation
          watcher) to daily (balance keeper). All eight share four invariants: a Postgres
          advisory lock at entry, prod-only execution, constant-time cron-secret check, and a{' '}
          <code>worker_runs</code> telemetry row on completion.
        </p>
      </Anchor>

      <Anchor id="wallet" title="Wallet — env-var with five hard rails">
        <p>
          A single EOA on Base, key in the <code>EVM_PRIVATE_KEY</code> Vercel Sensitive env
          var. Every signed transaction passes through{' '}
          <code>crates/payments::wallet::sign_and_send</code>, which enforces:
        </p>
        <ol className="mt-2 list-decimal pl-6" style={{ color: 'var(--term-fg-dim)' }}>
          <li>Edge Config kill switch (read &lt; 15 ms).</li>
          <li>Chain allowlist (only chain_id = 8453).</li>
          <li>Recipient allowlist (the 3 ERC-8004 registries).</li>
          <li>Balance ceiling ($5) — refuses if accidentally over-funded.</li>
          <li>Daily spend cap ($1 via Upstash counter).</li>
        </ol>
        <p className="mt-2">
          Worst-case loss from a key leak is bounded under $5. Every deny writes a row to{' '}
          <code>denials</code> for audit.
        </p>
      </Anchor>

      <Anchor id="x402" title="x402 — paid writes">
        <p>
          Write endpoints respond <code>402 Payment Required</code> if no <code>X-PAYMENT</code>{' '}
          header is present. The client signs an EIP-3009{' '}
          <code>transferWithAuthorization</code> for USDC on Base and retries. Settlement goes
          through the Coinbase CDP x402 facilitator — eth-tools never custodies the receive
          wallet.
        </p>
      </Anchor>

      <p className="mt-10 text-xs" style={{ color: 'var(--term-muted)' }}>
        <span className="term-prompt-bare">$</span> exit
      </p>
    </main>
  );
}

function Section({ title, entries }: { title: string; entries: Entry[] }) {
  return (
    <div>
      <p className="text-xs uppercase tracking-[0.15em]" style={{ color: 'var(--term-prompt)' }}>
        {title}
      </p>
      <ul className="mt-3 space-y-1.5 text-sm">
        {entries.map((e) => (
          <li key={e.path} className="flex flex-wrap items-baseline gap-x-3">
            <span style={{ color: 'var(--term-muted)' }}>▸</span>
            {e.external ? (
              <a
                href={e.path}
                target="_blank"
                rel="noopener noreferrer"
                className="term-link"
              >
                {e.label}
              </a>
            ) : e.path.startsWith('#') || e.path.includes('#') ? (
              <a href={e.path} className="term-link">{e.label}</a>
            ) : (
              <Link href={e.path} className="term-link">{e.label}</Link>
            )}
            <span className="text-xs" style={{ color: 'var(--term-muted)' }}>
              — {e.hint}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function Anchor({ id, title, children }: { id: string; title: string; children: React.ReactNode }) {
  return (
    <section id={id} className="mt-10 term-window" style={{ scrollMarginTop: '2rem' }}>
      <div className="term-titlebar">
        <span className="term-dot term-dot-r" aria-hidden="true" />
        <span className="term-dot term-dot-y" aria-hidden="true" />
        <span className="term-dot term-dot-g" aria-hidden="true" />
        <span className="ml-3">~/docs/{id}.md</span>
      </div>
      <div className="term-body text-sm" style={{ color: 'var(--term-fg-dim)' }}>
        <p>
          <span className="term-prompt-bare">$</span>{' '}
          <span style={{ color: 'var(--term-fg)' }}>less ~/docs/{id}.md</span>
        </p>
        <h2 className="mt-3 text-lg font-semibold" style={{ color: 'var(--term-accent-2)' }}>
          {title}
        </h2>
        <div className="mt-2 space-y-2">{children}</div>
      </div>
    </section>
  );
}

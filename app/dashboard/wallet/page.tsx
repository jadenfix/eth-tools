// REMOVE-ON-MERGE: superseded by `feat/phase-6-wallet-rails` +
// `feat/phase-6.2-signing` + `feat/phase-6-x402`. Real wallet health
// view (balance, spend, rails, recent txs/denials) lives there.
// Placeholder so the dashboard home link resolves on this branch.

import Link from 'next/link';

export const metadata = { title: 'wallet · dashboard · eth-tools' };

export default function WalletPage() {
  return (
    <main className="relative z-10 mx-auto max-w-3xl px-4 py-10 sm:px-6">
      <p className="mb-4 flex items-center gap-3 text-xs" style={{ color: 'var(--term-muted)' }}>
        <Link href="/" className="term-link">eth-tools</Link>
        <span>/</span>
        <Link href="/dashboard" className="term-link">dashboard</Link>
        <span>/</span>
        <span style={{ color: 'var(--term-accent-2)' }}>wallet</span>
      </p>

      <section className="term-window">
        <div className="term-titlebar">
          <span className="term-dot term-dot-r" aria-hidden="true" />
          <span className="term-dot term-dot-y" aria-hidden="true" />
          <span className="term-dot term-dot-g" aria-hidden="true" />
          <span className="ml-3">~/dashboard/wallet — env-var EOA · base mainnet</span>
        </div>
        <div className="term-body text-sm">
          <p>
            <span className="term-prompt-bare">$</span>{' '}
            <span style={{ color: 'var(--term-fg)' }}>
              eth-tools wallet status --rails
            </span>
          </p>

          <p className="mt-3" style={{ color: 'var(--term-fg-dim)' }}>
            <span className="term-pill term-pill-warn">staged</span> Live balance + daily-spend
            telemetry ships on the next merge.
          </p>

          <hr className="term-divider" />

          <p>
            <span className="term-comment">five hard rails — enforced server-side</span>
          </p>
          <ul className="mt-2 space-y-1.5" style={{ color: 'var(--term-fg-dim)' }}>
            <li>
              <span className="term-pill term-pill-ok">1</span> Edge Config kill switch
              (read &lt; 15 ms · one toggle stops all writes)
            </li>
            <li>
              <span className="term-pill term-pill-ok">2</span> Chain allowlist —{' '}
              <code style={{ color: 'var(--term-accent-2)' }}>chain_id = 8453</code>
            </li>
            <li>
              <span className="term-pill term-pill-ok">3</span> Recipient allowlist — the 3
              ERC-8004 registry addresses, hard-coded in Rust
            </li>
            <li>
              <span className="term-pill term-pill-ok">4</span> Balance ceiling{' '}
              <code style={{ color: 'var(--term-accent-2)' }}>$5</code> — refuses if
              over-funded, queues Safe sweep
            </li>
            <li>
              <span className="term-pill term-pill-ok">5</span> Daily spend cap{' '}
              <code style={{ color: 'var(--term-accent-2)' }}>$1</code> via Upstash counter
            </li>
          </ul>

          <hr className="term-divider" />

          <p>
            <span className="term-comment">audit lives in postgres — survives vercel log TTL</span>
          </p>
          <ul className="mt-2 list-disc pl-6" style={{ color: 'var(--term-fg-dim)' }}>
            <li>
              <code style={{ color: 'var(--term-accent-2)' }}>wallet_txs</code> — every
              attempt: <em>queued · submitted · confirmed · reverted · rejected</em>
            </li>
            <li>
              <code style={{ color: 'var(--term-accent-2)' }}>denials</code> — every rail
              refusal: code · evaluator · api_key_id · request_path
            </li>
          </ul>
        </div>
      </section>
    </main>
  );
}

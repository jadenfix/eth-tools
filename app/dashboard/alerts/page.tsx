// REMOVE-ON-MERGE: superseded by `feat/phase-4-w6-w7-w8-bundle` +
// `feat/phase-4-http-api-expansion`. Real alerts ack queue lives there.
// Placeholder so the dashboard home link resolves on this branch.

import Link from 'next/link';

export const metadata = { title: 'alerts · dashboard · eth-tools' };

export default function AlertsPage() {
  return (
    <main className="relative z-10 mx-auto max-w-3xl px-4 py-10 sm:px-6">
      <p className="mb-4 flex items-center gap-3 text-xs" style={{ color: 'var(--term-muted)' }}>
        <Link href="/" className="term-link">eth-tools</Link>
        <span>/</span>
        <Link href="/dashboard" className="term-link">dashboard</Link>
        <span>/</span>
        <span style={{ color: 'var(--term-accent-2)' }}>alerts</span>
      </p>

      <section className="term-window">
        <div className="term-titlebar">
          <span className="term-dot term-dot-r" aria-hidden="true" />
          <span className="term-dot term-dot-y" aria-hidden="true" />
          <span className="term-dot term-dot-g" aria-hidden="true" />
          <span className="ml-3">~/dashboard/alerts — open queue</span>
        </div>
        <div className="term-body text-sm">
          <p>
            <span className="term-prompt-bare">$</span>{' '}
            <span style={{ color: 'var(--term-fg)' }}>
              eth-tools alerts open
            </span>
          </p>
          <p className="mt-3" style={{ color: 'var(--term-fg-dim)' }}>
            <span className="term-pill term-pill-warn">staged</span> The ack/audit table
            ships on the next merge. Until then, alerts are written to the{' '}
            <code style={{ color: 'var(--term-accent-2)' }}>alerts</code> Postgres table by
            the W8 watchdog with severities <code>info</code>, <code>warn</code>,{' '}
            <code>crit</code>.
          </p>
          <hr className="term-divider" />
          <p>
            <span className="term-comment">what raises an alert</span>
          </p>
          <ul className="mt-2 list-disc pl-6" style={{ color: 'var(--term-fg-dim)' }}>
            <li>Any worker with no <code>ok=true</code> row older than 3× its cadence.</li>
            <li>Cursor lag &gt; 100 blocks for any chain × event.</li>
            <li>Wallet balance &lt; $1 (refill warning) or &gt; $5 (over-fund crit).</li>
            <li>RPC primary failure rate &gt; 30% over 5 min.</li>
          </ul>
        </div>
      </section>
    </main>
  );
}

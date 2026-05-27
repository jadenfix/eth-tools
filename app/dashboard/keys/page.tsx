// REMOVE-ON-MERGE: superseded by `feat/phase-5-api-keys`.
// Placeholder — the live API-key issuance flow lives on that branch.
// Keeps the dashboard home from 404-ing while we ship the merge train.
// When merging, drop this file; the real implementation owns this path.

import Link from 'next/link';

export const metadata = { title: 'keys · dashboard · eth-tools' };

export default function KeysPage() {
  return (
    <main className="relative z-10 mx-auto max-w-3xl px-4 py-10 sm:px-6">
      <p className="mb-4 flex items-center gap-3 text-xs" style={{ color: 'var(--term-muted)' }}>
        <Link href="/" className="term-link">eth-tools</Link>
        <span>/</span>
        <Link href="/dashboard" className="term-link">dashboard</Link>
        <span>/</span>
        <span style={{ color: 'var(--term-accent-2)' }}>keys</span>
      </p>

      <section className="term-window">
        <div className="term-titlebar">
          <span className="term-dot term-dot-r" aria-hidden="true" />
          <span className="term-dot term-dot-y" aria-hidden="true" />
          <span className="term-dot term-dot-g" aria-hidden="true" />
          <span className="ml-3">~/dashboard/keys</span>
        </div>
        <div className="term-body text-sm">
          <p>
            <span className="term-prompt-bare">$</span>{' '}
            <span style={{ color: 'var(--term-fg)' }}>eth-tools keys list</span>
          </p>
          <p className="mt-3" style={{ color: 'var(--term-fg-dim)' }}>
            <span className="term-pill term-pill-warn">staged</span> Issuance, scoping, and
            revocation ship on the next merge.
          </p>
          <p className="mt-3" style={{ color: 'var(--term-fg-dim)' }}>
            Today: API keys are <code style={{ color: 'var(--term-accent-2)' }}>et_live_</code>{' '}
            (or <code style={{ color: 'var(--term-accent-2)' }}>et_test_</code> on preview),
            bcrypt-hashed, scoped to a GitHub user. The CLI&apos;s{' '}
            <code style={{ color: 'var(--term-accent-2)' }}>eth-tools auth login</code> opens
            this dashboard, polls until you click <em>Issue</em>, and stores the key in the
            OS keychain.
          </p>
          <hr className="term-divider" />
          <p>
            <span className="term-comment">in the meantime — write surfaces require:</span>
          </p>
          <ul className="mt-2 list-disc pl-6" style={{ color: 'var(--term-fg-dim)' }}>
            <li><code>Authorization: Bearer et_live_…</code></li>
            <li><code>Idempotency-Key: &lt;uuid&gt;</code> on every POST</li>
            <li>For paid writes — a settled x402 <code>X-PAYMENT</code> header</li>
          </ul>
          <p className="mt-3 text-xs" style={{ color: 'var(--term-muted)' }}>
            Track shipping in{' '}
            <a
              href="https://github.com/jadenfix/eth-tools/issues"
              className="term-link"
            >
              github issues
            </a>
            .
          </p>
        </div>
      </section>
    </main>
  );
}

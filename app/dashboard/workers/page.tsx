// REMOVE-ON-MERGE: superseded by the worker branches (`feat/phase-4-w*` +
// `feat/phase-4-pr2-worker-substrate`) once they merge in.
// Placeholder for the workers telemetry view — the live `worker_runs`
// fetch + cursor-lag widgets ship from those branches; this stub keeps
// the landing footer's "status" link from 404-ing in the meantime.

import Link from 'next/link';

export const metadata = { title: 'workers · dashboard · eth-tools' };

type Cron = { name: string; cadence: string; budgetMs: number; doc: string };

const CRONS: Cron[] = [
  { name: 'wallet_rotation_watcher',  cadence: '*/1 * * * *',  budgetMs: 60_000,  doc: 'detect agent-wallet rotations · invalidate caches' },
  { name: 'registry_scraper',         cadence: '*/2 * * * *',  budgetMs: 120_000, doc: 'tail Registered / URIUpdated / Transfer logs' },
  { name: 'reputation_aggregator',    cadence: '*/5 * * * *',  budgetMs: 120_000, doc: 'roll up NewFeedback / FeedbackRevoked' },
  { name: 'validation_aggregator',    cadence: '*/5 * * * *',  budgetMs: 120_000, doc: 'track ValidationRequest / Response' },
  { name: 'manifest_fetcher',         cadence: '*/15 * * * *', budgetMs: 300_000, doc: 'fetch · validate · snapshot agentURI' },
  { name: 'endpoint_prober',          cadence: '*/30 * * * *', budgetMs: 300_000, doc: 'HEAD/health every services[].endpoint' },
  { name: 'trust_score_recompute',    cadence: '0 * * * *',    budgetMs: 300_000, doc: 'composite v0 score per agent' },
  { name: 'wallet_balance_keeper',    cadence: '0 0 * * *',    budgetMs: 120_000, doc: 'daily sweep + stale-worker watchdog' },
];

export default function WorkersPage() {
  return (
    <main className="relative z-10 mx-auto max-w-5xl px-4 py-10 sm:px-6">
      <p className="mb-4 flex items-center gap-3 text-xs" style={{ color: 'var(--term-muted)' }}>
        <Link href="/" className="term-link">eth-tools</Link>
        <span>/</span>
        <Link href="/dashboard" className="term-link">dashboard</Link>
        <span>/</span>
        <span style={{ color: 'var(--term-accent-2)' }}>workers</span>
      </p>

      <section className="term-window">
        <div className="term-titlebar">
          <span className="term-dot term-dot-r" aria-hidden="true" />
          <span className="term-dot term-dot-y" aria-hidden="true" />
          <span className="term-dot term-dot-g" aria-hidden="true" />
          <span className="ml-3">~/dashboard/workers — cron telemetry</span>
        </div>
        <div className="term-body text-sm">
          <p>
            <span className="term-prompt-bare">$</span>{' '}
            <span style={{ color: 'var(--term-fg)' }}>
              eth-tools workers status --json
            </span>
          </p>
          <p className="mt-3" style={{ color: 'var(--term-fg-dim)' }}>
            <span className="term-pill term-pill-warn">staged</span> Live <code>worker_runs</code>{' '}
            telemetry + cursor-lag wires up on the next merge.
          </p>

          <div className="mt-4 overflow-x-auto">
            <table className="min-w-full text-sm" style={{ borderCollapse: 'collapse' }}>
              <thead>
                <tr
                  className="text-left text-xs uppercase tracking-[0.1em]"
                  style={{ color: 'var(--term-muted)' }}
                >
                  <th className="px-3 py-2">worker</th>
                  <th className="px-3 py-2">cadence</th>
                  <th className="px-3 py-2">maxDuration</th>
                  <th className="px-3 py-2">does</th>
                </tr>
              </thead>
              <tbody>
                {CRONS.map((c) => (
                  <tr
                    key={c.name}
                    style={{ borderTop: '1px dashed var(--term-border)' }}
                  >
                    <td className="px-3 py-2" style={{ color: 'var(--term-accent-2)' }}>
                      {c.name}
                    </td>
                    <td className="px-3 py-2" style={{ color: 'var(--term-fg-dim)' }}>
                      <code>{c.cadence}</code>
                    </td>
                    <td className="px-3 py-2" style={{ color: 'var(--term-fg-dim)' }}>
                      {(c.budgetMs / 1000).toFixed(0)}s
                    </td>
                    <td className="px-3 py-2" style={{ color: 'var(--term-muted)' }}>
                      {c.doc}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <hr className="term-divider" />
          <p className="text-xs" style={{ color: 'var(--term-muted)' }}>
            # invariants — all 8 workers: advisory lock at entry · prod-only · cron-secret
            (constant-time) · worker_runs telemetry row · dryrun=1 support
          </p>
        </div>
      </section>
    </main>
  );
}

'use client';

// Live counters — terminal "ps aux"-style status table fed by /api/v1/health.
// agents_indexed + chains are real; the two later metrics are tbd and render
// as em-dash with a "tbd" pill so we don't fabricate.

import { useEffect, useState } from 'react';

type ChainHealth = {
  chain_id: number;
  name: string;
  is_testnet: boolean;
  agents_indexed: number;
};

type Health = {
  status: string;
  version: string;
  agents_indexed: number;
  chains: ChainHealth[];
};

type State =
  | { kind: 'loading' }
  | { kind: 'ok'; health: Health }
  | { kind: 'error' };

export default function LiveCounters() {
  const [state, setState] = useState<State>({ kind: 'loading' });

  useEffect(() => {
    let cancelled = false;
    fetch('/api/v1/health', { headers: { accept: 'application/json' } })
      .then((r) => {
        if (!r.ok) throw new Error(`status ${r.status}`);
        return r.json() as Promise<Health>;
      })
      .then((h) => {
        if (!cancelled) setState({ kind: 'ok', health: h });
      })
      .catch(() => {
        if (!cancelled) setState({ kind: 'error' });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const ledClass =
    state.kind === 'ok'
      ? 'term-led term-led-ok'
      : state.kind === 'loading'
        ? 'term-led term-led-idle'
        : 'term-led term-led-err';

  const statusText =
    state.kind === 'ok' ? `ok · v${state.health.version}` : state.kind === 'loading' ? 'querying…' : 'unreachable';

  const agentsText =
    state.kind === 'ok' ? state.health.agents_indexed.toLocaleString() : state.kind === 'loading' ? '…' : '—';

  const chains =
    state.kind === 'ok'
      ? state.health.chains
          .filter((c) => c.agents_indexed > 0)
          .map((c) => `${c.name}(${c.agents_indexed})`)
          .join(' · ')
      : '—';

  const chainsCount =
    state.kind === 'ok' ? state.health.chains.filter((c) => c.agents_indexed > 0).length : 0;

  return (
    <section
      aria-label="Live runtime counters"
      data-testid="live-counters"
      className="mt-8 term-window"
    >
      <div className="term-titlebar">
        <span className="term-dot term-dot-r" aria-hidden="true" />
        <span className="term-dot term-dot-y" aria-hidden="true" />
        <span className="term-dot term-dot-g" aria-hidden="true" />
        <span className="ml-3">tail -f /var/log/eth-tools/runtime.log</span>
      </div>
      <div className="term-body text-sm">
        <p>
          <span className="term-prompt-bare">$</span>{' '}
          <span style={{ color: 'var(--term-fg)' }}>
            curl -s eth-tools.dev/api/v1/health | jq
          </span>
        </p>
        <div className="mt-3 grid grid-cols-1 gap-x-6 gap-y-2 sm:grid-cols-2">
          <Row label="STATUS">
            <span className={ledClass} aria-hidden="true" />
            <span style={{ color: 'var(--term-fg)' }}>{statusText}</span>
          </Row>
          <Row label="AGENTS">
            <span style={{ color: 'var(--term-accent-2)' }}>{agentsText}</span>{' '}
            <span style={{ color: 'var(--term-muted)' }}>
              indexed{state.kind === 'ok' ? ` · ${chainsCount} chain${chainsCount === 1 ? '' : 's'}` : ''}
            </span>
          </Row>
          <Row label="CHAINS">
            <span style={{ color: 'var(--term-fg-dim)' }}>{chains}</span>
          </Row>
          <Row label="WORKERS">
            <span className="term-pill" title="exposed at /dashboard/workers">
              8 cron · see /dashboard/workers
            </span>
          </Row>
          <Row label="MANIFESTS/24h">
            <span style={{ color: 'var(--term-muted)' }}>—</span>{' '}
            <span className="term-pill">tbd</span>
          </Row>
          <Row label="LAST EVENT">
            <span style={{ color: 'var(--term-muted)' }}>—</span>{' '}
            <span className="term-pill">tbd</span>
          </Row>
        </div>
      </div>
    </section>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-baseline gap-3">
      <span
        className="inline-block w-32 shrink-0 text-xs uppercase tracking-[0.12em]"
        style={{ color: 'var(--term-muted)' }}
      >
        {label}
      </span>
      <span className="text-sm">{children}</span>
    </div>
  );
}

'use client';

// Live counters strip — fetches `/api/v1/health` on the client.
//
// The health endpoint currently returns `{status, version, agents_indexed, chains[]}`.
// It does NOT yet surface "manifests validated in last 24h" or
// "seconds since last on-chain event" — those metrics are queued for a later
// phase. Until they land we render an em-dash with a `tbd` label so we don't
// fabricate numbers on the landing page.

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

  const agentsText =
    state.kind === 'ok' ? state.health.agents_indexed.toLocaleString() : state.kind === 'loading' ? '…' : '—';
  const chainsText =
    state.kind === 'ok'
      ? state.health.chains.filter((c) => c.agents_indexed > 0).length.toString()
      : state.kind === 'loading'
        ? '…'
        : '—';

  return (
    <section
      aria-label="Live runtime counters"
      data-testid="live-counters"
      className="border-y border-zinc-800/80 bg-zinc-950/40"
    >
      <div className="mx-auto grid max-w-5xl grid-cols-1 gap-6 px-6 py-8 sm:grid-cols-3">
        <Counter
          value={agentsText}
          label={`${
            state.kind === 'ok' ? `agents indexed across ${chainsText} chains` : 'agents indexed'
          }`}
        />
        <Counter
          value="—"
          label="manifests validated in last 24h"
          hint="tbd"
        />
        <Counter
          value="—"
          label="seconds since last on-chain event"
          hint="tbd"
        />
      </div>
    </section>
  );
}

function Counter({ value, label, hint }: { value: string; label: string; hint?: string }) {
  return (
    <div className="text-center sm:text-left">
      <div className="font-mono text-3xl font-semibold tracking-tight text-zinc-50">
        {value}
      </div>
      <div className="mt-1 text-xs uppercase tracking-wider text-zinc-500">
        {label}
        {hint ? <span className="ml-2 rounded bg-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-400">{hint}</span> : null}
      </div>
    </div>
  );
}

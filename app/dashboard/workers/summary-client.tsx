'use client';

// Client island that refreshes the worker summary table every 30s.
//
// We re-render only the rows here (server fetches the initial snapshot),
// so the first paint is immediate and the polling is purely overlay.

import { useEffect, useState } from 'react';
import type { WorkerSummaryRow } from '@/lib/db';
import { classifyWorker, WORKER_CADENCE_SEC, type WorkerStatusRow } from './shared';

const POLL_MS = 30_000;

function statusBadge(status: WorkerStatusRow['status']) {
  const cls: Record<WorkerStatusRow['status'], string> = {
    OK: 'bg-emerald-700/40 text-emerald-200',
    STALE: 'bg-amber-700/40 text-amber-200',
    CRIT: 'bg-red-700/40 text-red-200',
    NEVER: 'bg-zinc-700/40 text-zinc-300',
  };
  return (
    <span
      className={`inline-flex rounded px-2 py-0.5 text-xs font-medium ${cls[status]}`}
    >
      {status}
    </span>
  );
}

function fmtAge(sec: number | null): string {
  if (sec === null) return '—';
  if (sec < 60) return `${sec}s`;
  if (sec < 3600) return `${Math.floor(sec / 60)}m`;
  return `${Math.floor(sec / 3600)}h`;
}

function fmtCadence(sec: number): string {
  if (sec < 60) return `${sec}s`;
  if (sec < 3600) return `${Math.floor(sec / 60)}m`;
  return `${Math.floor(sec / 3600)}h`;
}

export function WorkerSummaryClient({
  initial,
}: {
  initial: WorkerSummaryRow[];
}) {
  const [rows, setRows] = useState<WorkerSummaryRow[]>(initial);
  // `nowMs` is the timestamp used to compute "age" on the current row
  // snapshot. We refresh it whenever a poll lands so badges flip from
  // OK→STALE→CRIT without us needing an extra render tick.
  const [nowMs, setNowMs] = useState<number>(() => Date.now());
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let cancelled = false;
    const id = setInterval(() => {
      void (async () => {
        try {
          const res = await fetch('/api/v1/workers/status', {
            method: 'GET',
            headers: { accept: 'application/json' },
            cache: 'no-store',
          });
          if (!res.ok) return;
          const data: unknown = await res.json();
          if (cancelled) return;
          if (
            typeof data === 'object' &&
            data !== null &&
            'rows' in data &&
            Array.isArray((data as { rows: unknown }).rows)
          ) {
            setRows((data as { rows: WorkerSummaryRow[] }).rows);
          }
          setNowMs(Date.now());
          setTick((t) => t + 1);
        } catch {
          // Swallow — next tick will retry.
        }
      })();
    }, POLL_MS);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, []);

  const classified = rows.map((r) => classifyWorker(r, nowMs));

  return (
    <section className="mt-6">
      <div className="flex items-baseline justify-between">
        <h2 className="text-lg font-medium">Workers</h2>
        <p className="text-xs text-zinc-500" data-testid="worker-poll-tick">
          auto-refresh every 30s · ticks={tick}
        </p>
      </div>
      <div className="mt-2 overflow-x-auto rounded-lg border border-zinc-800">
        <table className="min-w-full divide-y divide-zinc-800 text-sm">
          <thead className="bg-zinc-900/60 text-left text-xs uppercase tracking-wider text-zinc-400">
            <tr>
              <th className="px-4 py-2">name</th>
              <th className="px-4 py-2">cadence</th>
              <th className="px-4 py-2">last_ok_at</th>
              <th className="px-4 py-2">age</th>
              <th className="px-4 py-2">status</th>
              <th className="px-4 py-2">rows in/out</th>
              <th className="px-4 py-2">last_error</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-zinc-800/80">
            {classified.map((row) => (
              <tr key={row.worker_name} className="hover:bg-zinc-900/40">
                <td className="px-4 py-2 font-mono">{row.worker_name}</td>
                <td className="px-4 py-2 text-zinc-400">
                  {fmtCadence(WORKER_CADENCE_SEC[row.worker_name] ?? row.cadence_sec)}
                </td>
                <td className="px-4 py-2 text-zinc-400">
                  {row.last_ok_at ?? '—'}
                </td>
                <td className="px-4 py-2 font-mono">{fmtAge(row.age_sec)}</td>
                <td className="px-4 py-2">{statusBadge(row.status)}</td>
                <td className="px-4 py-2 font-mono text-zinc-300">
                  {(row.rows_in ?? '—') + ' / ' + (row.rows_out ?? '—')}
                </td>
                <td className="px-4 py-2 text-xs text-red-300">
                  {row.last_error ? row.last_error.slice(0, 80) : ''}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}

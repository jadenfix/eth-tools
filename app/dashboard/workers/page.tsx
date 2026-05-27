// /dashboard/workers — SLO compliance + cursor lag live view.
//
// Server component renders the initial snapshot; a small client island
// polls /api/v1/workers/status every 30s and replaces the summary table.
// We keep cursor-lag and recent-runs server-rendered (less critical).

import Link from 'next/link';
import { redirect } from 'next/navigation';
import { Suspense } from 'react';
import { auth } from '@/lib/auth';
import {
  cursorLag,
  lastWorkerRuns,
  workerSummaries,
  type CursorRow,
  type WorkerRunRow,
  type WorkerSummaryRow,
} from '@/lib/db';
import { WorkerSummaryClient } from './summary-client';
import { WORKER_CADENCE_SEC } from './shared';

function cursorLagBucket(lag: number): {
  label: string;
  className: string;
} {
  if (lag <= 15) return { label: 'green', className: 'bg-emerald-700/40 text-emerald-200' };
  if (lag <= 100) return { label: 'yellow', className: 'bg-amber-700/40 text-amber-200' };
  return { label: 'red', className: 'bg-red-700/40 text-red-200' };
}

function Skeleton({ label }: { label: string }) {
  return (
    <div
      role="status"
      aria-label={`Loading ${label}`}
      className="mt-4 h-32 animate-pulse rounded-lg border border-zinc-800 bg-zinc-900/30"
    />
  );
}

async function WorkerSummarySection() {
  let summaries: WorkerSummaryRow[] = [];
  let dbError: string | null = null;
  try {
    summaries = await workerSummaries();
  } catch (err) {
    dbError = err instanceof Error ? err.message : String(err);
  }

  // Ensure every cadence-known worker shows up, even if it has zero runs.
  const known = new Set(summaries.map((s) => s.worker_name));
  for (const name of Object.keys(WORKER_CADENCE_SEC)) {
    if (!known.has(name)) {
      summaries.push({
        worker_name: name,
        last_ok_at: null,
        last_run_at: null,
        rows_in: null,
        rows_out: null,
        last_error: null,
      });
    }
  }
  summaries.sort((a, b) => a.worker_name.localeCompare(b.worker_name));

  if (dbError) {
    return (
      <section className="mt-6">
        <h2 className="text-lg font-medium">Workers</h2>
        <p className="mt-2 text-sm text-red-400">DB error: {dbError}</p>
      </section>
    );
  }

  // Hand to the client island, which polls /api/v1/workers/status and
  // replaces the rows in-place. The server-rendered table is the initial
  // paint so first-load is fast.
  return <WorkerSummaryClient initial={summaries} />;
}

interface CursorRowWithLag extends CursorRow {
  lag_blocks: number;
}

async function loadCursorRows(): Promise<{
  rows: CursorRowWithLag[];
  error: string | null;
}> {
  try {
    const raw = await cursorLag();
    // Read the clock once, in this side-effecty data loader, so the
    // JSX render below stays pure (no Date.now()/Math.random()).
    const renderedAt = Date.now();
    const rows: CursorRowWithLag[] = raw.map((r) => {
      const ageSec = Math.max(
        0,
        Math.floor((renderedAt - Date.parse(r.updated_at)) / 1000),
      );
      return { ...r, lag_blocks: Math.floor(ageSec / 2) };
    });
    return { rows, error: null };
  } catch (err) {
    return {
      rows: [],
      error: err instanceof Error ? err.message : String(err),
    };
  }
}

async function CursorLagSection() {
  const { rows, error: dbError } = await loadCursorRows();

  return (
    <section className="mt-10">
      <h2 className="text-lg font-medium">Cursor lag</h2>
      <p className="mt-1 text-xs text-zinc-500">
        Snapshot at render time. Head-block estimate is not yet wired —
        block deltas will appear once the rpc head-tracker ships.
      </p>
      {dbError ? (
        <p className="mt-2 text-sm text-red-400">DB error: {dbError}</p>
      ) : rows.length === 0 ? (
        <p className="mt-2 rounded-lg border border-dashed border-zinc-800 p-6 text-center text-sm text-zinc-500">
          No cursors recorded yet.
        </p>
      ) : (
        <div className="mt-2 overflow-x-auto rounded-lg border border-zinc-800">
          <table className="min-w-full divide-y divide-zinc-800 text-sm">
            <thead className="bg-zinc-900/60 text-left text-xs uppercase tracking-wider text-zinc-400">
              <tr>
                <th className="px-4 py-2">cursor</th>
                <th className="px-4 py-2">last block</th>
                <th className="px-4 py-2">log idx</th>
                <th className="px-4 py-2">updated_at</th>
                <th className="px-4 py-2">bucket</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-zinc-800/80">
              {rows.map((row) => {
                // Lag computed in the data loader; here we only render.
                const lagBlocks = row.lag_blocks;
                const bucket = cursorLagBucket(lagBlocks);
                return (
                  <tr key={row.cursor_key} className="hover:bg-zinc-900/40">
                    <td className="px-4 py-2 font-mono">{row.cursor_key}</td>
                    <td className="px-4 py-2 font-mono">{row.last_block}</td>
                    <td className="px-4 py-2 font-mono">
                      {row.last_log_index}
                    </td>
                    <td className="px-4 py-2 text-zinc-400">
                      {row.updated_at}
                    </td>
                    <td className="px-4 py-2">
                      <span
                        className={`inline-flex rounded px-2 py-0.5 text-xs ${bucket.className}`}
                      >
                        ~{lagBlocks} blocks ({bucket.label})
                      </span>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

async function RecentRunsSection() {
  let rows: WorkerRunRow[] = [];
  let dbError: string | null = null;
  try {
    rows = await lastWorkerRuns(50);
  } catch (err) {
    dbError = err instanceof Error ? err.message : String(err);
  }

  return (
    <section className="mt-10">
      <h2 className="text-lg font-medium">Recent runs (last 50)</h2>
      {dbError ? (
        <p className="mt-2 text-sm text-red-400">DB error: {dbError}</p>
      ) : rows.length === 0 ? (
        <p className="mt-2 rounded-lg border border-dashed border-zinc-800 p-6 text-center text-sm text-zinc-500">
          No runs recorded yet.
        </p>
      ) : (
        <div className="mt-2 overflow-x-auto rounded-lg border border-zinc-800">
          <table className="min-w-full divide-y divide-zinc-800 text-sm">
            <thead className="bg-zinc-900/60 text-left text-xs uppercase tracking-wider text-zinc-400">
              <tr>
                <th className="px-4 py-2">worker</th>
                <th className="px-4 py-2">env</th>
                <th className="px-4 py-2">started_at</th>
                <th className="px-4 py-2">ok</th>
                <th className="px-4 py-2">rows in/out</th>
                <th className="px-4 py-2">error</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-zinc-800/80">
              {rows.map((row) => (
                <tr key={row.id} className="hover:bg-zinc-900/40">
                  <td className="px-4 py-2 font-mono">{row.worker_name}</td>
                  <td className="px-4 py-2 font-mono text-zinc-400">
                    {row.vercel_env}
                  </td>
                  <td className="px-4 py-2 text-zinc-400">{row.started_at}</td>
                  <td className="px-4 py-2">
                    {row.ok === true ? (
                      <span className="text-emerald-400">ok</span>
                    ) : row.ok === false ? (
                      <span className="text-red-400">fail</span>
                    ) : (
                      <span className="text-zinc-500">—</span>
                    )}
                  </td>
                  <td className="px-4 py-2 font-mono text-zinc-300">
                    {(row.rows_in ?? '—') + ' / ' + (row.rows_out ?? '—')}
                  </td>
                  <td className="px-4 py-2 text-xs text-red-300">
                    {row.error ? row.error.slice(0, 120) : ''}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

export default async function WorkersPage() {
  const session = await auth();
  if (!session?.user) {
    redirect('/api/auth/signin?callbackUrl=%2Fdashboard%2Fworkers');
  }

  return (
    <main className="mx-auto max-w-6xl px-6 py-10">
      <div className="mb-6 flex items-baseline justify-between">
        <h1 className="text-2xl font-semibold tracking-tight">Workers</h1>
        <Link
          href="/dashboard"
          className="text-sm text-zinc-400 underline-offset-2 hover:underline"
        >
          ← Dashboard
        </Link>
      </div>

      <Suspense fallback={<Skeleton label="worker summary" />}>
        <WorkerSummarySection />
      </Suspense>
      <Suspense fallback={<Skeleton label="cursor lag" />}>
        <CursorLagSection />
      </Suspense>
      <Suspense fallback={<Skeleton label="recent runs" />}>
        <RecentRunsSection />
      </Suspense>
    </main>
  );
}

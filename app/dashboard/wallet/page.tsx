// /dashboard/wallet — read-only operator view of the agent wallet.
//
// Plan §9.4 calls for: current balance, daily spend vs cap, kill-switch
// state, recent tx + denial tables. All writes (toggling the kill switch,
// adjusting the cap) happen via the Vercel CLI on the Edge Config slot.
//
// Server component: queries Postgres directly via @neondatabase/serverless.
// The manual refresh button lives in a tiny client subcomponent.

import Link from 'next/link';
import { redirect } from 'next/navigation';
import { Suspense } from 'react';
import { auth } from '@/lib/auth';
import {
  lastDenials,
  lastWalletTxs,
  spendToday,
  type DenialRow,
  type WalletTxRow,
} from '@/lib/db';
import { readEdgeConfigBoolean } from '@/lib/edge-config';
import { RefreshButton } from './refresh-button';

// Daily cap (USD) — mirrors crates/payments/src/wallet/daily_cap.rs default.
// Pulled from env so ops can bump it without a code change.
const DAILY_CAP_USDC = Number.parseFloat(
  process.env.WALLET_DAILY_CAP_USDC ?? '1',
);

const SELECTOR_LABELS: Record<string, string> = {
  '0xa9059cbb': 'transfer',
  '0x095ea7b3': 'approve',
  '0x23b872dd': 'transferFrom',
};

function shortHex(h: string | null | undefined, head = 6, tail = 4): string {
  if (!h) return '—';
  if (h.length <= head + tail + 1) return h;
  return `${h.slice(0, head)}…${h.slice(-tail)}`;
}

function basescanTx(hash: string): string {
  return `https://basescan.org/tx/${hash}`;
}

function basescanAddr(addr: string): string {
  return `https://basescan.org/address/${addr}`;
}

function selectorLabel(sel: string): string {
  return SELECTOR_LABELS[sel.toLowerCase()] ?? sel;
}

function statusBadge(status: WalletTxRow['status']) {
  const styles: Record<WalletTxRow['status'], string> = {
    queued: 'bg-zinc-700/40 text-zinc-200',
    submitted: 'bg-blue-700/40 text-blue-200',
    confirmed: 'bg-emerald-700/40 text-emerald-200',
    reverted: 'bg-red-700/40 text-red-200',
    rejected: 'bg-amber-700/40 text-amber-200',
  };
  return (
    <span
      className={`inline-flex rounded px-2 py-0.5 text-xs font-medium ${styles[status]}`}
    >
      {status}
    </span>
  );
}

async function WalletTiles() {
  // Fan out the four read queries in parallel — they're independent.
  const [spend, killSwitch] = await Promise.all([
    spendToday(),
    readEdgeConfigBoolean('wallet_enabled'),
  ]);

  const spentUsdc = Number.parseFloat(spend.spend_usdc || '0');
  const capUsdc = Number.isFinite(DAILY_CAP_USDC) ? DAILY_CAP_USDC : 1;
  const pctRaw = capUsdc > 0 ? (spentUsdc / capUsdc) * 100 : 0;
  const pct = Math.min(100, Math.max(0, pctRaw));
  const overCap = spentUsdc > capUsdc;

  return (
    <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
      <Tile title="Balance (USDC)">
        <p className="font-mono text-2xl">
          {process.env.WALLET_BALANCE_USDC ?? '—'}
        </p>
        <p className="mt-1 text-xs text-zinc-500">
          On-chain balance is refreshed by the{' '}
          <span className="font-mono">wallet_balance_keeper</span> cron;
          dashboard does not poll RPC directly.
        </p>
        <div className="mt-3">
          <RefreshButton />
        </div>
      </Tile>

      <Tile title="Spend today">
        <p className="font-mono text-2xl">
          ${spentUsdc.toFixed(4)}{' '}
          <span className="text-sm text-zinc-500">/ ${capUsdc.toFixed(2)}</span>
        </p>
        <div className="mt-2 h-2 w-full rounded bg-zinc-800">
          <div
            className={`h-2 rounded ${overCap ? 'bg-red-500' : 'bg-emerald-500'}`}
            style={{ width: `${pct}%` }}
            aria-label={`Spend ${pct.toFixed(1)}% of cap`}
          />
        </div>
        <p className="mt-1 text-xs text-zinc-500">
          {spend.tx_count} tx today
        </p>
      </Tile>

      <Tile title="Kill switch">
        <KillSwitchBadge value={killSwitch} />
        <p className="mt-3 text-xs text-zinc-500">
          Read-only here. Toggle via{' '}
          <span className="font-mono">vercel edge-config</span>.
        </p>
      </Tile>
    </div>
  );
}

function Tile({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="rounded-lg border border-zinc-800 bg-zinc-900/30 p-4">
      <h2 className="text-xs uppercase tracking-wider text-zinc-400">{title}</h2>
      <div className="mt-2">{children}</div>
    </div>
  );
}

function KillSwitchBadge({ value }: { value: boolean | null }) {
  if (value === null) {
    return (
      <span className="inline-flex rounded bg-zinc-700/50 px-2 py-1 text-sm font-medium text-zinc-200">
        unknown
      </span>
    );
  }
  if (value === true) {
    return (
      <span className="inline-flex rounded bg-emerald-700/40 px-2 py-1 text-sm font-medium text-emerald-200">
        enabled
      </span>
    );
  }
  return (
    <span className="inline-flex rounded bg-red-700/40 px-2 py-1 text-sm font-medium text-red-200">
      disabled
    </span>
  );
}

async function TxTable() {
  let rows: WalletTxRow[] = [];
  let dbError: string | null = null;
  try {
    rows = await lastWalletTxs(50);
  } catch (err) {
    dbError = err instanceof Error ? err.message : String(err);
  }

  return (
    <section className="mt-10">
      <h2 className="text-lg font-medium">Recent wallet transactions</h2>
      {dbError ? (
        <p className="mt-2 text-sm text-red-400">DB error: {dbError}</p>
      ) : rows.length === 0 ? (
        <EmptyState text="No tx yet — make a paid call to populate." />
      ) : (
        <div className="mt-2 overflow-x-auto rounded-lg border border-zinc-800">
          <table className="min-w-full divide-y divide-zinc-800 text-sm">
            <thead className="bg-zinc-900/60 text-left text-xs uppercase tracking-wider text-zinc-400">
              <tr>
                <th className="px-4 py-2">status</th>
                <th className="px-4 py-2">to</th>
                <th className="px-4 py-2">selector</th>
                <th className="px-4 py-2">fee (USDC)</th>
                <th className="px-4 py-2">tx hash</th>
                <th className="px-4 py-2">created_at</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-zinc-800/80">
              {rows.map((row) => (
                <tr key={row.id} className="hover:bg-zinc-900/40">
                  <td className="px-4 py-2">{statusBadge(row.status)}</td>
                  <td className="px-4 py-2 font-mono">
                    <a
                      href={basescanAddr(row.to_addr)}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="underline-offset-2 hover:underline"
                    >
                      {shortHex(row.to_addr)}
                    </a>
                  </td>
                  <td className="px-4 py-2 font-mono">
                    {selectorLabel(row.selector)}
                  </td>
                  <td className="px-4 py-2 font-mono">{row.fee_usdc ?? '—'}</td>
                  <td className="px-4 py-2 font-mono">
                    {row.tx_hash ? (
                      <a
                        href={basescanTx(row.tx_hash)}
                        target="_blank"
                        rel="noopener noreferrer"
                        className="underline-offset-2 hover:underline"
                      >
                        {shortHex(row.tx_hash)}
                      </a>
                    ) : (
                      '—'
                    )}
                  </td>
                  <td className="px-4 py-2 text-zinc-400">{row.created_at}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

async function DenialsTable() {
  let rows: DenialRow[] = [];
  let dbError: string | null = null;
  try {
    rows = await lastDenials(50);
  } catch (err) {
    dbError = err instanceof Error ? err.message : String(err);
  }

  return (
    <section className="mt-10">
      <h2 className="text-lg font-medium">Recent rail denials</h2>
      {dbError ? (
        <p className="mt-2 text-sm text-red-400">DB error: {dbError}</p>
      ) : rows.length === 0 ? (
        <EmptyState text="No denials — rails happy." />
      ) : (
        <div className="mt-2 overflow-x-auto rounded-lg border border-zinc-800">
          <table className="min-w-full divide-y divide-zinc-800 text-sm">
            <thead className="bg-zinc-900/60 text-left text-xs uppercase tracking-wider text-zinc-400">
              <tr>
                <th className="px-4 py-2">code</th>
                <th className="px-4 py-2">evaluator</th>
                <th className="px-4 py-2">request_path</th>
                <th className="px-4 py-2">occurred_at</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-zinc-800/80">
              {rows.map((row) => (
                <tr key={row.id} className="hover:bg-zinc-900/40">
                  <td className="px-4 py-2 font-mono">{row.code}</td>
                  <td className="px-4 py-2 font-mono text-zinc-300">
                    {row.evaluator}
                  </td>
                  <td className="px-4 py-2 font-mono text-zinc-300">
                    {row.request_path}
                  </td>
                  <td className="px-4 py-2 text-zinc-400">{row.occurred_at}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

function EmptyState({ text }: { text: string }) {
  return (
    <p className="mt-2 rounded-lg border border-dashed border-zinc-800 p-6 text-center text-sm text-zinc-500">
      {text}
    </p>
  );
}

function Skeleton({ label }: { label: string }) {
  return (
    <div
      role="status"
      aria-label={`Loading ${label}`}
      className="mt-4 h-24 animate-pulse rounded-lg border border-zinc-800 bg-zinc-900/30"
    />
  );
}

export default async function WalletPage() {
  const session = await auth();
  if (!session?.user) {
    // Middleware already protects /dashboard/*, but belt-and-suspenders
    // (e.g. if middleware matcher changes) — redirect to login.
    redirect('/api/auth/signin?callbackUrl=%2Fdashboard%2Fwallet');
  }

  return (
    <main className="mx-auto max-w-6xl px-6 py-10">
      <div className="mb-6 flex items-baseline justify-between">
        <h1 className="text-2xl font-semibold tracking-tight">Wallet</h1>
        <Link
          href="/dashboard"
          className="text-sm text-zinc-400 underline-offset-2 hover:underline"
        >
          ← Dashboard
        </Link>
      </div>

      <Suspense fallback={<Skeleton label="wallet tiles" />}>
        <WalletTiles />
      </Suspense>
      <Suspense fallback={<Skeleton label="recent transactions" />}>
        <TxTable />
      </Suspense>
      <Suspense fallback={<Skeleton label="recent denials" />}>
        <DenialsTable />
      </Suspense>
    </main>
  );
}

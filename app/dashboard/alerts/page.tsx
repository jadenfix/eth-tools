// /dashboard/alerts — operational triage view. Tabs: Open | All.
//
// Server fetches the relevant rows; client island renders the table and
// owns the acknowledge interaction (POST /api/v1/alerts/:id/ack, then
// router.refresh()).

import Link from 'next/link';
import { redirect } from 'next/navigation';
import { Suspense } from 'react';
import { auth } from '@/lib/auth';
import { listAlerts, type AlertRow } from '@/lib/db';
import { AlertsTable } from './alerts-table';

type SearchParams = Promise<{
  tab?: string | string[];
  severity?: string | string[];
  source?: string | string[];
}>;

function pick(v: string | string[] | undefined): string | undefined {
  if (Array.isArray(v)) return v[0];
  return v;
}

function Skeleton() {
  return (
    <div
      role="status"
      aria-label="Loading alerts"
      className="mt-4 h-48 animate-pulse rounded-lg border border-zinc-800 bg-zinc-900/30"
    />
  );
}

async function AlertsSection({
  tab,
  severity,
  source,
}: {
  tab: 'open' | 'all';
  severity: string | undefined;
  source: string | undefined;
}) {
  let rows: AlertRow[] = [];
  let dbError: string | null = null;
  try {
    rows = await listAlerts({ onlyOpen: tab === 'open', limit: 100 });
  } catch (err) {
    dbError = err instanceof Error ? err.message : String(err);
  }

  // Filter client-side after the LIMITed fetch — keeps the SQL paths
  // small and the surface easy to reason about. A row that's filtered
  // out here is still bounded by the SELECT LIMIT.
  const filtered = rows.filter((r) => {
    if (severity && r.severity !== severity) return false;
    if (source && r.source !== source) return false;
    return true;
  });

  // Compute the unique severity + source options from the unfiltered set
  // so the dropdowns stay populated even when the current filter empties
  // the list.
  const severityOptions = Array.from(new Set(rows.map((r) => r.severity))).sort();
  const sourceOptions = Array.from(new Set(rows.map((r) => r.source))).sort();

  if (dbError) {
    return <p className="mt-4 text-sm text-red-400">DB error: {dbError}</p>;
  }

  return (
    <AlertsTable
      rows={filtered}
      tab={tab}
      severity={severity}
      source={source}
      severityOptions={severityOptions}
      sourceOptions={sourceOptions}
    />
  );
}

export default async function AlertsPage({
  searchParams,
}: {
  searchParams: SearchParams;
}) {
  const session = await auth();
  if (!session?.user) {
    redirect('/api/auth/signin?callbackUrl=%2Fdashboard%2Falerts');
  }

  const sp = await searchParams;
  const rawTab = pick(sp.tab) ?? 'open';
  const tab: 'open' | 'all' = rawTab === 'all' ? 'all' : 'open';
  const severity = pick(sp.severity);
  const source = pick(sp.source);

  return (
    <main className="mx-auto max-w-6xl px-6 py-10">
      <div className="mb-6 flex items-baseline justify-between">
        <h1 className="text-2xl font-semibold tracking-tight">Alerts</h1>
        <Link
          href="/dashboard"
          className="text-sm text-zinc-400 underline-offset-2 hover:underline"
        >
          ← Dashboard
        </Link>
      </div>

      <div className="mb-4 flex gap-2 border-b border-zinc-800 text-sm">
        <TabLink href={buildHref('open', severity, source)} active={tab === 'open'}>
          Open
        </TabLink>
        <TabLink href={buildHref('all', severity, source)} active={tab === 'all'}>
          All
        </TabLink>
      </div>

      <Suspense fallback={<Skeleton />}>
        <AlertsSection tab={tab} severity={severity} source={source} />
      </Suspense>
    </main>
  );
}

function buildHref(
  tab: 'open' | 'all',
  severity: string | undefined,
  source: string | undefined,
): string {
  const qs = new URLSearchParams();
  qs.set('tab', tab);
  if (severity) qs.set('severity', severity);
  if (source) qs.set('source', source);
  return `/dashboard/alerts?${qs.toString()}`;
}

function TabLink({
  href,
  active,
  children,
}: {
  href: string;
  active: boolean;
  children: React.ReactNode;
}) {
  return (
    <Link
      href={href}
      className={`-mb-px border-b-2 px-3 py-2 ${
        active
          ? 'border-zinc-200 text-zinc-100'
          : 'border-transparent text-zinc-500 hover:text-zinc-300'
      }`}
    >
      {children}
    </Link>
  );
}

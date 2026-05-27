'use client';

// Client island for the alerts table. Owns:
//   - Filter dropdowns (severity, source) → updates URL search params.
//   - Body row expand/collapse (pretty-printed JSON).
//   - Acknowledge button → POST /api/v1/alerts/:id/ack, then refresh.

import { useRouter } from 'next/navigation';
import { useState, useTransition } from 'react';
import type { AlertRow } from '@/lib/db';

function severityClasses(sev: AlertRow['severity']): string {
  switch (sev) {
    case 'crit':
      return 'bg-red-700/40 text-red-200';
    case 'warn':
      return 'bg-amber-700/40 text-amber-200';
    case 'info':
    default:
      return 'bg-blue-700/40 text-blue-200';
  }
}

function bodyPreview(body: unknown, max = 120): string {
  try {
    const s = JSON.stringify(body);
    if (!s) return '';
    return s.length > max ? `${s.slice(0, max)}…` : s;
  } catch {
    return '<unserializable>';
  }
}

export function AlertsTable({
  rows,
  tab,
  severity,
  source,
  severityOptions,
  sourceOptions,
}: {
  rows: AlertRow[];
  tab: 'open' | 'all';
  severity: string | undefined;
  source: string | undefined;
  severityOptions: string[];
  sourceOptions: string[];
}) {
  const router = useRouter();
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [pending, startTransition] = useTransition();
  const [ackError, setAckError] = useState<string | null>(null);

  function toggleExpand(id: string) {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  function setFilter(key: 'severity' | 'source', value: string) {
    const qs = new URLSearchParams();
    qs.set('tab', tab);
    if (key === 'severity') {
      if (value) qs.set('severity', value);
      if (source) qs.set('source', source);
    } else {
      if (severity) qs.set('severity', severity);
      if (value) qs.set('source', value);
    }
    router.push(`/dashboard/alerts?${qs.toString()}`);
  }

  async function acknowledge(id: string) {
    setAckError(null);
    try {
      const res = await fetch(
        `/api/v1/alerts/${encodeURIComponent(id)}/ack`,
        {
          method: 'POST',
          headers: { accept: 'application/json' },
        },
      );
      if (!res.ok) {
        const txt = await res.text().catch(() => '');
        setAckError(`ack failed (${res.status}): ${txt.slice(0, 120)}`);
        return;
      }
      startTransition(() => {
        router.refresh();
      });
    } catch (err) {
      setAckError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <div>
      <div className="mb-4 flex flex-wrap items-center gap-3 text-sm">
        <label className="flex items-center gap-2 text-zinc-400">
          severity
          <select
            value={severity ?? ''}
            onChange={(e) => setFilter('severity', e.target.value)}
            className="rounded border border-zinc-700 bg-zinc-900 px-2 py-1 text-zinc-100"
          >
            <option value="">(any)</option>
            {severityOptions.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </select>
        </label>
        <label className="flex items-center gap-2 text-zinc-400">
          source
          <select
            value={source ?? ''}
            onChange={(e) => setFilter('source', e.target.value)}
            className="rounded border border-zinc-700 bg-zinc-900 px-2 py-1 text-zinc-100"
          >
            <option value="">(any)</option>
            {sourceOptions.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </select>
        </label>
        {ackError ? (
          <span className="ml-2 text-xs text-red-400">{ackError}</span>
        ) : null}
      </div>

      {rows.length === 0 ? (
        <p className="rounded-lg border border-dashed border-zinc-800 p-6 text-center text-sm text-zinc-500">
          {tab === 'open'
            ? 'No alerts — workers happy.'
            : 'No alerts match the current filter.'}
        </p>
      ) : (
        <div className="overflow-x-auto rounded-lg border border-zinc-800">
          <table className="min-w-full divide-y divide-zinc-800 text-sm">
            <thead className="bg-zinc-900/60 text-left text-xs uppercase tracking-wider text-zinc-400">
              <tr>
                <th className="px-4 py-2">severity</th>
                <th className="px-4 py-2">source</th>
                <th className="px-4 py-2">raised_at</th>
                <th className="px-4 py-2">body</th>
                <th className="px-4 py-2">acked</th>
                <th className="px-4 py-2">actions</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-zinc-800/80">
              {rows.map((row) => {
                const isOpen = expanded.has(row.id);
                return (
                  <tr
                    key={row.id}
                    data-testid={`alert-row-${row.id}`}
                    className="hover:bg-zinc-900/40"
                  >
                    <td className="px-4 py-2">
                      <span
                        className={`inline-flex rounded px-2 py-0.5 text-xs font-medium ${severityClasses(row.severity)}`}
                      >
                        {row.severity}
                      </span>
                    </td>
                    <td className="px-4 py-2 font-mono text-zinc-300">
                      {row.source}
                    </td>
                    <td className="px-4 py-2 text-zinc-400">{row.raised_at}</td>
                    <td className="px-4 py-2 align-top">
                      <button
                        type="button"
                        onClick={() => toggleExpand(row.id)}
                        className="text-left text-zinc-300 underline-offset-2 hover:underline"
                        aria-expanded={isOpen}
                      >
                        {isOpen ? '▾ collapse' : '▸ expand'}
                      </button>
                      {isOpen ? (
                        <pre className="mt-2 max-w-xl overflow-x-auto rounded bg-black/40 p-2 text-xs text-zinc-300">
                          {JSON.stringify(row.body, null, 2)}
                        </pre>
                      ) : (
                        <p className="mt-1 font-mono text-xs text-zinc-500">
                          {bodyPreview(row.body)}
                        </p>
                      )}
                    </td>
                    <td className="px-4 py-2 text-xs text-zinc-400">
                      {row.acknowledged ? (
                        <span>
                          yes · {row.ack_by ?? '?'} ·{' '}
                          <span className="text-zinc-500">{row.ack_at}</span>
                        </span>
                      ) : (
                        <span className="text-zinc-500">no</span>
                      )}
                    </td>
                    <td className="px-4 py-2">
                      {row.acknowledged ? (
                        <span className="text-zinc-600">—</span>
                      ) : (
                        <button
                          type="button"
                          disabled={pending}
                          onClick={() => acknowledge(row.id)}
                          className="rounded-md border border-zinc-700 px-2 py-1 text-xs text-zinc-100 hover:bg-zinc-900 disabled:opacity-50"
                        >
                          {pending ? '…' : 'acknowledge'}
                        </button>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

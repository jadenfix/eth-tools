// Pure helpers shared between the server page and the client polling
// island. Keep this file framework-free (no next/, no react) so it can
// be imported from either boundary without dragging server-only code
// into the client bundle.

import type { WorkerSummaryRow } from '@/lib/db';

// Worker cadences (seconds). Pulled from plan §6.x.
export const WORKER_CADENCE_SEC: Record<string, number> = {
  registry_scraper: 60,
  manifest_fetcher: 300,
  endpoint_prober: 300,
  feedback_aggregator: 300,
  validation_aggregator: 300,
  trust_scorer: 900,
  wallet_balance_keeper: 60,
  wallet_rotation_watcher: 3600,
};

export type WorkerStatus = 'OK' | 'STALE' | 'CRIT' | 'NEVER';

export interface WorkerStatusRow extends WorkerSummaryRow {
  cadence_sec: number;
  age_sec: number | null;
  status: WorkerStatus;
}

export function classifyWorker(
  row: WorkerSummaryRow,
  nowMs: number,
): WorkerStatusRow {
  const cadence = WORKER_CADENCE_SEC[row.worker_name] ?? 300;
  if (!row.last_ok_at) {
    return { ...row, cadence_sec: cadence, age_sec: null, status: 'NEVER' };
  }
  const lastMs = Date.parse(row.last_ok_at);
  const ageSec = Math.max(0, Math.floor((nowMs - lastMs) / 1000));
  let status: WorkerStatus = 'OK';
  if (ageSec > cadence * 3) status = 'CRIT';
  else if (ageSec > cadence * 1.5) status = 'STALE';
  return { ...row, cadence_sec: cadence, age_sec: ageSec, status };
}

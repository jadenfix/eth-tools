// Read-only Neon Postgres client for dashboard server components.
//
// We deliberately keep this isolated from the Rust API path. The Rust API
// (crates/api, crates/db) owns all writes and uses sqlx pooling. Dashboard
// pages read from Neon directly over the serverless HTTP driver — that
// avoids cold-start pool penalties on Vercel functions and keeps the
// dashboard render path on a single round-trip per query.
//
// All exported helpers MUST include `LIMIT` in their queries so a runaway
// table cannot DoS the page.

import { neon, type NeonQueryFunction } from '@neondatabase/serverless';

/**
 * Lazily-initialised Neon client. We avoid creating it at module-eval time so
 * builds (which import this transitively via page bundles) don't blow up when
 * `DATABASE_URL` is unset. Pages that hit DB queries during render at build
 * time would still fail there, but on Vercel pages render per-request, so
 * `DATABASE_URL` is always present in the request scope.
 */
let _sql: NeonQueryFunction<false, false> | null = null;

export function sql(): NeonQueryFunction<false, false> {
  if (_sql) return _sql;
  const url = process.env.DATABASE_URL;
  if (!url) {
    throw new Error(
      'DATABASE_URL is not set — dashboard read queries cannot run. Set it in the Vercel project settings.',
    );
  }
  _sql = neon(url);
  return _sql;
}

// ────────────────────────────────────────────────────────────────────────────
// Row shapes. These mirror the Postgres tables in crates/db/migrations/0001.
// All BYTEA columns are returned as hex strings (we convert in SELECT).
// ────────────────────────────────────────────────────────────────────────────

export interface WalletTxRow {
  id: string;
  chain_id: string;
  nonce: number | null;
  to_addr: string; // 0x-hex
  selector: string; // 0x-hex (4 bytes)
  fee_usdc: string | null;
  tx_hash: string | null; // 0x-hex
  status: 'queued' | 'submitted' | 'confirmed' | 'reverted' | 'rejected';
  created_at: string;
}

export interface DenialRow {
  id: string;
  occurred_at: string;
  code: string;
  evaluator: string;
  request_path: string;
}

export interface AlertRow {
  id: string;
  raised_at: string;
  severity: 'info' | 'warn' | 'crit';
  source: string;
  body: unknown;
  acknowledged: boolean;
  ack_by: string | null;
  ack_at: string | null;
}

export interface WorkerRunRow {
  id: string;
  worker_name: string;
  vercel_env: string;
  started_at: string;
  finished_at: string | null;
  rows_in: number | null;
  rows_out: number | null;
  ok: boolean | null;
  error: string | null;
}

export interface WorkerSummaryRow {
  worker_name: string;
  last_ok_at: string | null;
  last_run_at: string | null;
  rows_in: number | null;
  rows_out: number | null;
  last_error: string | null;
}

export interface CursorRow {
  cursor_key: string;
  last_block: string;
  last_log_index: number;
  updated_at: string;
}

export interface WalletSpendTodayRow {
  spend_usdc: string;
  tx_count: number;
}

export interface WalletBalanceRow {
  balance_usdc: string;
  observed_at: string;
}

// ────────────────────────────────────────────────────────────────────────────
// Wallet
// ────────────────────────────────────────────────────────────────────────────

export async function lastWalletTxs(limit = 50): Promise<WalletTxRow[]> {
  const rows = await sql()`
    SELECT
      id::text                        AS id,
      chain_id::text                  AS chain_id,
      nonce,
      '0x' || encode(to_addr, 'hex')  AS to_addr,
      '0x' || encode(selector, 'hex') AS selector,
      fee_usdc::text                  AS fee_usdc,
      CASE WHEN tx_hash IS NULL THEN NULL ELSE '0x' || encode(tx_hash, 'hex') END AS tx_hash,
      status,
      to_char(created_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at
    FROM wallet_txs
    ORDER BY created_at DESC
    LIMIT ${Math.min(limit, 200)}
  `;
  return rows as unknown as WalletTxRow[];
}

export async function lastDenials(limit = 50): Promise<DenialRow[]> {
  const rows = await sql()`
    SELECT
      id::text AS id,
      to_char(occurred_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS occurred_at,
      code,
      evaluator,
      request_path
    FROM denials
    ORDER BY occurred_at DESC
    LIMIT ${Math.min(limit, 200)}
  `;
  return rows as unknown as DenialRow[];
}

export async function spendToday(): Promise<WalletSpendTodayRow> {
  // Sum confirmed-and-submitted fees in the current UTC calendar day.
  // Rejected/reverted entries don't burn USDC, so they're excluded.
  const rows = await sql()`
    SELECT
      COALESCE(SUM(fee_usdc), 0)::text AS spend_usdc,
      COUNT(*)::int                    AS tx_count
    FROM wallet_txs
    WHERE created_at >= date_trunc('day', NOW())
      AND status IN ('submitted', 'confirmed')
    LIMIT 1
  `;
  const r = (rows as unknown as WalletSpendTodayRow[])[0];
  return r ?? { spend_usdc: '0', tx_count: 0 };
}

// ────────────────────────────────────────────────────────────────────────────
// Workers
// ────────────────────────────────────────────────────────────────────────────

export async function lastWorkerRuns(limit = 50): Promise<WorkerRunRow[]> {
  const rows = await sql()`
    SELECT
      id::text         AS id,
      worker_name,
      vercel_env,
      to_char(started_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS started_at,
      CASE WHEN finished_at IS NULL THEN NULL
           ELSE to_char(finished_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') END AS finished_at,
      rows_in,
      rows_out,
      ok,
      error
    FROM worker_runs
    ORDER BY started_at DESC
    LIMIT ${Math.min(limit, 200)}
  `;
  return rows as unknown as WorkerRunRow[];
}

/**
 * One summary row per worker: most recent successful run plus most recent
 * error message. Used for the SLO table on /dashboard/workers.
 */
export async function workerSummaries(): Promise<WorkerSummaryRow[]> {
  const rows = await sql()`
    SELECT
      worker_name,
      MAX(CASE WHEN ok IS TRUE  THEN finished_at END) AS last_ok_at,
      MAX(started_at)                                  AS last_run_at,
      (ARRAY_AGG(rows_in  ORDER BY started_at DESC))[1] AS rows_in,
      (ARRAY_AGG(rows_out ORDER BY started_at DESC))[1] AS rows_out,
      (ARRAY_AGG(error    ORDER BY started_at DESC) FILTER (WHERE error IS NOT NULL))[1] AS last_error
    FROM worker_runs
    GROUP BY worker_name
    ORDER BY worker_name
    LIMIT 50
  `;
  return rows as unknown as WorkerSummaryRow[];
}

export async function cursorLag(): Promise<CursorRow[]> {
  const rows = await sql()`
    SELECT
      cursor_key,
      last_block::text AS last_block,
      last_log_index,
      to_char(updated_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS updated_at
    FROM cursors
    ORDER BY cursor_key
    LIMIT 100
  `;
  return rows as unknown as CursorRow[];
}

// ────────────────────────────────────────────────────────────────────────────
// Alerts
// ────────────────────────────────────────────────────────────────────────────

export async function listAlerts(
  opts: { onlyOpen?: boolean; limit?: number } = {},
): Promise<AlertRow[]> {
  const limit = Math.min(opts.limit ?? 100, 200);
  // We can't conditionally compose a parameterised query string with Neon's
  // tagged-template helper, so branch the two cases explicitly.
  const rows = opts.onlyOpen
    ? await sql()`
        SELECT
          id::text AS id,
          to_char(raised_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS raised_at,
          severity,
          source,
          body,
          acknowledged,
          ack_by,
          CASE WHEN ack_at IS NULL THEN NULL
               ELSE to_char(ack_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') END AS ack_at
        FROM alerts
        WHERE acknowledged = FALSE
        ORDER BY raised_at DESC
        LIMIT ${limit}
      `
    : await sql()`
        SELECT
          id::text AS id,
          to_char(raised_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS raised_at,
          severity,
          source,
          body,
          acknowledged,
          ack_by,
          CASE WHEN ack_at IS NULL THEN NULL
               ELSE to_char(ack_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') END AS ack_at
        FROM alerts
        ORDER BY raised_at DESC
        LIMIT ${limit}
      `;
  return rows as unknown as AlertRow[];
}

export async function acknowledgeAlert(
  id: number,
  ackBy: string,
): Promise<{ acknowledged: boolean }> {
  const rows = await sql()`
    UPDATE alerts
    SET acknowledged = TRUE,
        ack_by       = ${ackBy},
        ack_at       = NOW()
    WHERE id = ${id}
      AND acknowledged = FALSE
    RETURNING id
  `;
  return { acknowledged: (rows as unknown[]).length > 0 };
}

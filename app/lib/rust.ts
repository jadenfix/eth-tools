// Typed fetcher to the Rust API.
//
// - In local dev (NODE_ENV !== 'production'): hits `INTERNAL_API_URL`
//   (defaults to http://127.0.0.1:3000 — the `crates/dev-server` Axum bin).
// - On Vercel: same-host routing, so we use relative `/api/v1/...` URLs.
//   Same-project internal traffic is unbilled.
//
// Helpers wrap the three endpoints currently exposed in `app/lib/api-types.ts`.

import type { paths } from './api-types';

export type ApiPaths = paths;

type AgentList =
  paths['/api/v1/agents']['get']['responses']['200']['content']['application/json'];
type AgentDetail =
  paths['/api/v1/agents/{chain}/{agent_id}']['get']['responses']['200']['content']['application/json'];
type Health =
  paths['/api/v1/health']['get']['responses']['200']['content']['application/json'];

export type { AgentList, AgentDetail, Health };
export type Agent = AgentList['data'][number];

/** Marker class for non-2xx responses so callers can branch on status. */
export class ApiError extends Error {
  constructor(
    public readonly status: number,
    public readonly path: string,
    public readonly body: string,
  ) {
    super(`API ${status} on ${path}: ${body.slice(0, 200)}`);
    this.name = 'ApiError';
  }
}

function baseUrl(): string {
  if (process.env.NODE_ENV !== 'production') {
    return process.env.INTERNAL_API_URL ?? 'http://127.0.0.1:3000';
  }
  // In Vercel prod, same-host. Caller must pass a path starting with '/'.
  return '';
}

/**
 * Typed GET helper for any path documented in `paths`.
 *
 * The `path` parameter is the OpenAPI template string (e.g.
 * `'/api/v1/agents/{chain}/{agent_id}'`); we pass it through verbatim, so
 * callers either substitute themselves or use the dedicated helpers below.
 */
export async function getJson<P extends keyof paths>(
  path: P & string,
  init?: RequestInit,
): Promise<unknown> {
  const url = `${baseUrl()}${path}`;
  const res = await fetch(url, {
    ...init,
    method: 'GET',
    headers: {
      accept: 'application/json',
      ...(init?.headers ?? {}),
    },
    cache: 'no-store',
  });
  if (!res.ok) {
    const body = await res.text().catch(() => '');
    throw new ApiError(res.status, url, body);
  }
  return res.json();
}

export interface ListAgentsParams {
  limit?: number;
  chain?: string;
  cursor?: string;
}

export async function listAgents(params: ListAgentsParams = {}): Promise<AgentList> {
  const qs = new URLSearchParams();
  if (params.limit !== undefined) qs.set('limit', String(params.limit));
  if (params.chain) qs.set('chain', params.chain);
  if (params.cursor) qs.set('cursor', params.cursor);
  const suffix = qs.toString();
  const url = `${baseUrl()}/api/v1/agents${suffix ? `?${suffix}` : ''}`;
  const res = await fetch(url, {
    method: 'GET',
    headers: { accept: 'application/json' },
    cache: 'no-store',
  });
  if (!res.ok) {
    const body = await res.text().catch(() => '');
    throw new ApiError(res.status, url, body);
  }
  return (await res.json()) as AgentList;
}

export async function getAgent(
  chain: string,
  agentId: string,
): Promise<AgentDetail> {
  const url = `${baseUrl()}/api/v1/agents/${encodeURIComponent(chain)}/${encodeURIComponent(agentId)}`;
  const res = await fetch(url, {
    method: 'GET',
    headers: { accept: 'application/json' },
    cache: 'no-store',
  });
  if (!res.ok) {
    const body = await res.text().catch(() => '');
    throw new ApiError(res.status, url, body);
  }
  return (await res.json()) as AgentDetail;
}

export async function health(): Promise<Health> {
  const url = `${baseUrl()}/api/v1/health`;
  const res = await fetch(url, {
    method: 'GET',
    headers: { accept: 'application/json' },
    cache: 'no-store',
  });
  if (!res.ok) {
    const body = await res.text().catch(() => '');
    throw new ApiError(res.status, url, body);
  }
  return (await res.json()) as Health;
}

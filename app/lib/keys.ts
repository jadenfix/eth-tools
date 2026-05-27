// Server-side helpers for talking to the Rust keys admin endpoints.
//
// Auth model: Auth.js v5 sessions are JWE-encrypted (A256GCM) by default.
// Re-implementing that decrypt path in Rust would be ~200 LOC of crypto for
// no business benefit, so we keep the session check in Next.js (where
// `auth()` is a one-liner) and forward to Rust with:
//   - `x-internal-secret`: shared secret comparable in constant time on the
//     Rust side. Set via `INTERNAL_API_SECRET` env var on both Next.js and
//     Rust deployments.
//   - `x-github-user-id`: numeric GitHub user id from `session.user.id`.
//   - `x-github-login`: GitHub login from `session.user.githubLogin`.
//
// Without `INTERNAL_API_SECRET` set in production, the Rust middleware
// returns 500 (fail-closed). In local dev the secret is optional so curl
// against `crates/dev-server` works.

// Server-side absolute base URL for the Rust admin endpoints. `fetch()` in
// Node requires absolute URLs (no relative paths), so we resolve here at
// import time:
//   - dev / preview: explicit `INTERNAL_API_URL` (defaults to the
//     `crates/dev-server` Axum bin on :3000).
//   - Vercel prod: `https://<VERCEL_URL>` (per-deployment hostname Vercel
//     injects into every function). Same-host internal traffic is unbilled.
//
// Rationale for ALSO using `_internal` in the path: the Next.js Route Handler
// at `app/api/v1/keys/route.ts` would otherwise loop back to itself, because
// Next.js file-system routes take precedence over Vercel rewrites. The Rust
// router mounts these at `/api/v1/_internal/keys*` and Vercel's
// `/api/v1/(.*)` rewrite delivers them to the Rust function.
const RUST_BASE = (() => {
  if (process.env.INTERNAL_API_URL) return process.env.INTERNAL_API_URL;
  if (process.env.VERCEL_URL) return `https://${process.env.VERCEL_URL}`;
  return 'http://127.0.0.1:3000';
})();

const RUST_KEYS_PATH = '/api/v1/_internal/keys';

export interface SessionIdentity {
  userId: string; // session.user.id (GitHub user id, stringified)
  login: string;  // session.user.githubLogin
}

function commonHeaders(identity: SessionIdentity): HeadersInit {
  const h: Record<string, string> = {
    'content-type': 'application/json',
    accept: 'application/json',
    'x-github-user-id': identity.userId,
    'x-github-login': identity.login,
  };
  const secret = process.env.INTERNAL_API_SECRET;
  if (secret && secret.length > 0) {
    h['x-internal-secret'] = secret;
  }
  return h;
}

export interface KeyView {
  id: string;
  prefix: string;
  name: string;
  scopes: string[];
  created_at: string;
  last_used_at: string | null;
}

export interface IssuedKey {
  id: string;
  prefix: string;
  /** Plaintext bearer — surface to the user ONCE then discard. */
  plaintext: string;
  name: string;
  scopes: string[];
}

export async function listKeys(identity: SessionIdentity): Promise<KeyView[]> {
  const res = await fetch(`${RUST_BASE}${RUST_KEYS_PATH}`, {
    method: 'GET',
    headers: commonHeaders(identity),
    cache: 'no-store',
  });
  if (!res.ok) {
    throw new Error(`list keys: ${res.status} ${await res.text()}`);
  }
  const body = (await res.json()) as { data: KeyView[] };
  return body.data;
}

export async function createKey(
  identity: SessionIdentity,
  body: { name: string; scopes: string[] },
): Promise<IssuedKey> {
  const res = await fetch(`${RUST_BASE}${RUST_KEYS_PATH}`, {
    method: 'POST',
    headers: commonHeaders(identity),
    body: JSON.stringify(body),
    cache: 'no-store',
  });
  if (!res.ok) {
    throw new Error(`create key: ${res.status} ${await res.text()}`);
  }
  return (await res.json()) as IssuedKey;
}

export async function revokeKey(identity: SessionIdentity, id: string): Promise<void> {
  const res = await fetch(`${RUST_BASE}${RUST_KEYS_PATH}/${encodeURIComponent(id)}`, {
    method: 'DELETE',
    headers: commonHeaders(identity),
    cache: 'no-store',
  });
  if (res.status === 204) return;
  if (res.status === 404) {
    throw new Error('key not found');
  }
  throw new Error(`revoke key: ${res.status} ${await res.text()}`);
}

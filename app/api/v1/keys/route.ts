// Next.js Route Handler for /api/v1/keys.
//
// Verifies the Auth.js v5 session, then forwards to the Rust admin endpoint
// with shared-secret + identity headers. We do session verification here
// because Auth.js v5 cookies are JWE-encrypted (A256GCM) and reimplementing
// that decrypt path in Rust would be ~200 LOC of crypto for no business
// benefit. See `app/lib/keys.ts` for the forwarding helpers.
//
// In a Vercel prod deployment this route lives at `<host>/api/v1/keys` and
// rewrites in `vercel.json` send `/api/v1/(.*)` to the Rust function. Next.js
// Route Handlers take priority over rewrites for paths that have an actual
// app/api file, so this handler runs and the Rust call is server-side fetch
// to `INTERNAL_API_URL` (preferred for local) or relative path (prod).

import { NextResponse, type NextRequest } from 'next/server';
import { auth } from '@/lib/auth';
import { createKey, listKeys } from '@/lib/keys';

function unauthorized() {
  return NextResponse.json(
    {
      error: {
        code: 'UNAUTHENTICATED',
        policy_version: 'v1',
        evaluator: 'app.auth',
        override_hint: 'sign in with GitHub at /api/auth/signin',
      },
    },
    { status: 401 },
  );
}

function identityFromSession(session: { user?: { id?: string; githubLogin?: string } } | null) {
  if (!session?.user?.id || !session.user.githubLogin) return null;
  return { userId: session.user.id, login: session.user.githubLogin };
}

export async function GET() {
  const session = await auth();
  const identity = identityFromSession(session);
  if (!identity) return unauthorized();
  try {
    const data = await listKeys(identity);
    return NextResponse.json({ data });
  } catch (err) {
    return NextResponse.json(
      {
        error: {
          code: 'INTERNAL',
          policy_version: 'v1',
          evaluator: 'app.keys',
          override_hint: 'retry; if persistent, file an issue',
          detail: err instanceof Error ? err.message : String(err),
        },
      },
      { status: 502 },
    );
  }
}

export async function POST(req: NextRequest) {
  const session = await auth();
  const identity = identityFromSession(session);
  if (!identity) return unauthorized();

  let body: unknown;
  try {
    body = await req.json();
  } catch {
    return NextResponse.json(
      {
        error: {
          code: 'INVALID_BODY',
          policy_version: 'v1',
          evaluator: 'app.keys',
          override_hint: 'body must be JSON: { name: string, scopes?: string[] }',
        },
      },
      { status: 400 },
    );
  }

  const parsed = parseCreateBody(body);
  if ('error' in parsed) return parsed.error;

  try {
    const issued = await createKey(identity, parsed.value);
    return NextResponse.json(issued, { status: 201 });
  } catch (err) {
    // The Rust layer already shapes 400s as JSON envelopes; we surface its
    // raw error text as a 502 fallback. A nicer pass-through would parse the
    // upstream JSON and re-emit it — defer until a real bad-request path
    // exists.
    return NextResponse.json(
      {
        error: {
          code: 'UPSTREAM',
          policy_version: 'v1',
          evaluator: 'app.keys',
          override_hint: 'check name length and scope spelling; retry',
          detail: err instanceof Error ? err.message : String(err),
        },
      },
      { status: 502 },
    );
  }
}

type CreateInput = { name: string; scopes: string[] };

function parseCreateBody(
  raw: unknown,
): { value: CreateInput } | { error: NextResponse } {
  if (typeof raw !== 'object' || raw === null) {
    return { error: badBody('body must be an object') };
  }
  const obj = raw as Record<string, unknown>;
  const name = obj.name;
  if (typeof name !== 'string' || name.trim().length === 0 || name.length > 128) {
    return { error: badBody('name must be a 1-128 char string') };
  }
  let scopes: string[] = ['read'];
  if (Array.isArray(obj.scopes)) {
    if (!obj.scopes.every((s): s is string => typeof s === 'string')) {
      return { error: badBody('scopes must be string[]') };
    }
    if (obj.scopes.length > 0) scopes = obj.scopes;
  } else if (obj.scopes !== undefined) {
    return { error: badBody('scopes must be string[]') };
  }
  return { value: { name: name.trim(), scopes } };
}

function badBody(hint: string) {
  return NextResponse.json(
    {
      error: {
        code: 'INVALID_BODY',
        policy_version: 'v1',
        evaluator: 'app.keys',
        override_hint: hint,
      },
    },
    { status: 400 },
  );
}

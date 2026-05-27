// DELETE /api/v1/keys/:id — revoke. Session-verified here, then forwarded to
// Rust (`crates/api::handlers::keys::delete`) with the shared-secret +
// identity headers. The Rust layer owner-checks in SQL, so a different user
// hitting this with a guessed UUID still gets 404, not 200.

import { NextResponse } from 'next/server';
import { auth } from '@/lib/auth';
import { revokeKey } from '@/lib/keys';

export async function DELETE(
  _req: Request,
  { params }: { params: Promise<{ id: string }> },
) {
  const session = await auth();
  const userId = session?.user?.id;
  const login = session?.user?.githubLogin;
  if (!userId || !login) {
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

  const { id } = await params;
  if (!/^[0-9a-fA-F-]{36}$/.test(id)) {
    return NextResponse.json(
      {
        error: {
          code: 'INVALID_ID',
          policy_version: 'v1',
          evaluator: 'app.keys',
          override_hint: 'id must be a UUID',
        },
      },
      { status: 400 },
    );
  }

  try {
    await revokeKey({ userId, login }, id);
    return new NextResponse(null, { status: 204 });
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    if (msg.includes('not found')) {
      return NextResponse.json(
        {
          error: {
            code: 'KEY_NOT_FOUND',
            policy_version: 'v1',
            evaluator: 'app.keys',
            override_hint: 'key is unknown, already revoked, or not yours',
          },
        },
        { status: 404 },
      );
    }
    return NextResponse.json(
      {
        error: {
          code: 'INTERNAL',
          policy_version: 'v1',
          evaluator: 'app.keys',
          override_hint: 'retry; if persistent, file an issue',
          detail: msg,
        },
      },
      { status: 502 },
    );
  }
}

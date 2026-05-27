// POST /api/v1/alerts/:id/ack — flip `acknowledged=TRUE` for one alert.
//
// Spec gap: plan §9.4 specifies the dashboard interaction but not the
// transport. We expose it as a Next.js route (Node runtime, same Neon
// client as the read paths) so we can ship without a Rust round-trip.
// Promote into the Rust API (with rate-limit + audit) once the operator
// surface stabilises.
//
// Auth: requires a GitHub session. `ack_by` is the GitHub login pulled
// from the session — never trusted from the client.

import { NextResponse } from 'next/server';
import { auth } from '@/lib/auth';
import { acknowledgeAlert } from '@/lib/db';

export const runtime = 'nodejs';
export const dynamic = 'force-dynamic';

export async function POST(
  _req: Request,
  ctx: { params: Promise<{ id: string }> },
) {
  const session = await auth();
  if (!session?.user) {
    return NextResponse.json({ error: 'unauthenticated' }, { status: 401 });
  }

  const { id: rawId } = await ctx.params;
  const id = Number.parseInt(rawId, 10);
  if (!Number.isFinite(id) || id <= 0) {
    return NextResponse.json({ error: 'bad_id' }, { status: 400 });
  }

  const ackBy =
    session.user.githubLogin ?? session.user.name ?? session.user.email ?? 'unknown';

  try {
    const result = await acknowledgeAlert(id, ackBy);
    if (!result.acknowledged) {
      // The row was already acknowledged or doesn't exist. Treat as 200
      // so the dashboard can refresh idempotently.
      return NextResponse.json(
        { acknowledged: false, reason: 'already_acked_or_not_found' },
        { status: 200 },
      );
    }
    return NextResponse.json(
      { acknowledged: true, ack_by: ackBy },
      { status: 200 },
    );
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    return NextResponse.json(
      { error: 'db_error', message: msg.slice(0, 200) },
      { status: 500 },
    );
  }
}

// Internal Next.js route handler that backs the /dashboard/workers polling
// island. Returns one summary row per worker (latest run, last ok, error).
//
// Auth: session-gated. We deliberately do NOT expose this on the public
// Rust API surface yet — the dashboard is the only consumer, and the
// shape may change as we add per-worker metrics. Promote to the Rust API
// (and OpenAPI) once the shape stabilises.

import { NextResponse } from 'next/server';
import { auth } from '@/lib/auth';
import { workerSummaries } from '@/lib/db';

export const runtime = 'nodejs';
export const dynamic = 'force-dynamic';

export async function GET() {
  const session = await auth();
  if (!session?.user) {
    return NextResponse.json({ error: 'unauthenticated' }, { status: 401 });
  }
  try {
    const rows = await workerSummaries();
    return NextResponse.json({ rows }, { status: 200 });
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    return NextResponse.json(
      { error: 'db_error', message: msg.slice(0, 200) },
      { status: 500 },
    );
  }
}

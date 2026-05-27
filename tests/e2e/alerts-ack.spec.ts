import { expect, test } from '@playwright/test';

// The acknowledge POST endpoint must reject unauthenticated requests.
// We can't easily mock an Auth.js v5 JWT cookie inside Playwright
// without forging it, so the positive ack flow is left for the
// follow-up PR (#alerts-ack-e2e) once we wire a test-only session
// stub. Asserting the 401 path still locks the auth contract.

test('unauthed POST /api/v1/alerts/:id/ack → 401', async ({ request }) => {
  const res = await request.post('/api/v1/alerts/1/ack');
  expect(res.status()).toBe(401);
  const body = (await res.json()) as { error?: string };
  expect(body.error).toBe('unauthenticated');
});

test('unauthed GET /api/v1/workers/status → 401', async ({ request }) => {
  const res = await request.get('/api/v1/workers/status');
  expect(res.status()).toBe(401);
  const body = (await res.json()) as { error?: string };
  expect(body.error).toBe('unauthenticated');
});

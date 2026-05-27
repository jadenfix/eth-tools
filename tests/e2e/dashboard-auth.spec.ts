import { expect, test } from '@playwright/test';

// Protected pages should redirect unauthenticated visitors to the GitHub
// signin endpoint. We don't follow the redirect to GitHub itself — the
// assertion is that middleware fires.

const PROTECTED = [
  '/dashboard/wallet',
  '/dashboard/workers',
  '/dashboard/alerts',
];

test.describe('dashboard auth gating (unauthenticated)', () => {
  for (const path of PROTECTED) {
    test(`${path} redirects to /api/auth/signin`, async ({ page }) => {
      // The signin endpoint further redirects to GitHub (which may fail
      // in our test env because the OAuth app id is a placeholder) — we
      // only care that middleware fires before any page renders. Wait
      // for the URL to leave `/dashboard/*` and land on the signin
      // handler. We swallow `goto`'s rejection because the downstream
      // GitHub fetch can error out as soon as DNS hits a non-existent
      // OAuth client.
      await page.goto(path, { waitUntil: 'commit' }).catch(() => undefined);
      await page.waitForURL(/\/api\/auth\/signin/, { timeout: 10_000 });
      expect(page.url()).toMatch(/\/api\/auth\/signin/);
      // Sanity: the redirect carries our intended destination.
      expect(page.url()).toContain('callbackUrl');
    });
  }
});

// Playwright config for dashboard smoke tests.
//
// The tests cover only what's cheap to assert in a headless browser:
//   - Public pages render (/, /docs, /docs/<slug>).
//   - Protected pages redirect to /api/auth/signin when unauthenticated.
//   - Authenticated flows (session-mocked) load and basic interactions
//     work (acknowledge, polling).
//
// We deliberately spin up the production build (`pnpm start`) rather
// than dev mode — dev mode HMR makes assertions flaky and our tests
// don't exercise hot reload.

import { defineConfig, devices } from '@playwright/test';

const PORT = process.env.PORT ? Number(process.env.PORT) : 3030;
const BASE_URL = `http://127.0.0.1:${PORT}`;

export default defineConfig({
  testDir: './tests/e2e',
  timeout: 30_000,
  fullyParallel: false,
  retries: 0,
  reporter: 'list',
  use: {
    baseURL: BASE_URL,
    trace: 'retain-on-failure',
  },
  webServer: {
    command: `pnpm start -p ${PORT}`,
    url: BASE_URL,
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
    env: {
      // Auth.js requires NEXTAUTH_SECRET in production builds; we set a
      // throwaway secret here so the runtime boots. The OAuth flow itself
      // is never actually exercised — tests stub the session cookie.
      AUTH_SECRET: process.env.AUTH_SECRET ?? 'pw-test-secret-do-not-use-in-prod',
      // GitHub provider needs client id/secret to construct. Throwaways.
      AUTH_GITHUB_ID: process.env.AUTH_GITHUB_ID ?? 'pw-test-id',
      AUTH_GITHUB_SECRET: process.env.AUTH_GITHUB_SECRET ?? 'pw-test-secret',
      // Auth.js v5 demands an explicit Host allowlist outside Vercel.
      AUTH_TRUST_HOST: process.env.AUTH_TRUST_HOST ?? 'true',
      // Disable workers/wallet DB queries — tests render the empty path.
      // Pages catch DB errors and render an inline error string; assertions
      // tolerate either the empty-state or the inline-error path.
      DATABASE_URL: process.env.DATABASE_URL ?? '',
    },
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
});

import { expect, test } from '@playwright/test';

// /docs is public; no auth flow needed.

test.describe('/docs', () => {
  test('index lists all docs', async ({ page }) => {
    await page.goto('/docs');
    await expect(page.getByRole('heading', { name: 'Docs' })).toBeVisible();
    for (const slug of ['quickstart', 'auth', 'mcp', 'wallet', 'workers']) {
      await expect(page.getByRole('link', { name: new RegExp(slug, 'i') })).toBeVisible();
    }
  });

  test('renders a single doc page', async ({ page }) => {
    await page.goto('/docs/quickstart');
    // The layout renders an H1 with the doc title; the markdown body
    // also contains an H1 ("# Quickstart"). Both should be present —
    // assert at least one of each is visible rather than picking by
    // role (strict mode rejects multi-match).
    await expect(
      page.locator('h1', { hasText: 'Quickstart' }).first(),
    ).toBeVisible();
    // Markdown code block should be rendered as a <pre><code>.
    await expect(page.locator('pre code').first()).toContainText('curl');
  });

  test('unknown slug 404s', async ({ page }) => {
    const res = await page.goto('/docs/does-not-exist');
    expect(res?.status()).toBe(404);
  });
});

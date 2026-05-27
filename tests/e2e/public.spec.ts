import { expect, test } from '@playwright/test';

// Quick sanity check that public endpoints we touched still load.

test('landing / loads', async ({ page }) => {
  await page.goto('/');
  expect(await page.title()).not.toBe('');
});

test('llms.txt is served', async ({ request }) => {
  const res = await request.get('/llms.txt');
  expect(res.status()).toBe(200);
});

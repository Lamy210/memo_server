import { expect, test } from '@playwright/test';

test('creates a memo through the protected browser BFF', async ({ page }) => {
  await page.goto('/memos/new');

  await page.getByLabel('タイトル').fill('Browser E2E memo');
  await page.getByLabel('本文').fill('The browser API helper must satisfy the BFF CSRF boundary.');
  await page.getByLabel('タグ').fill('e2e, csrf');

  const requestPromise = page.waitForRequest(
    (request) => request.method() === 'POST' && request.url().endsWith('/api/v1/memos')
  );
  const responsePromise = page.waitForResponse(
    (response) => response.request().method() === 'POST' && response.url().endsWith('/api/v1/memos')
  );

  await page.getByRole('button', { name: /保存/ }).click();

  const [request, response] = await Promise.all([requestPromise, responsePromise]);
  expect(response.status()).toBe(201);
  const requestHeaders = await request.allHeaders();
  expect(requestHeaders['x-schnee-memo-request']).toBe('1');
  expect(requestHeaders.origin).toBe('http://127.0.0.1:4173');

  await expect(page).toHaveURL(/\/memos\/018f0c7a-8b7d-7f25-b239-36e6d9f9b001\/edit$/);
  await expect(page.getByRole('heading', { name: 'メモを編集' })).toBeVisible();
});

test('rejects a same-origin raw mutation that bypasses the shared API helper', async ({ page }) => {
  await page.goto('/memos');

  const status = await page.evaluate(async () => {
    const response = await fetch('/api/v1/memos', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        title: 'Bypass attempt',
        content: 'This request intentionally omits the BFF marker.',
        tags: ['e2e']
      })
    });

    return response.status;
  });

  expect(status).toBe(403);
});


test('serves the frontend security header and CSP baseline', async ({ page }) => {
  const response = await page.goto('/memos');
  expect(response).not.toBeNull();

  const headers = response ? await response.allHeaders() : {};
  expect(headers['x-content-type-options']).toBe('nosniff');
  expect(headers['x-frame-options']).toBe('DENY');
  expect(headers['referrer-policy']).toBe('strict-origin-when-cross-origin');
  expect(headers['cross-origin-opener-policy']).toBe('same-origin');
  expect(headers['cross-origin-resource-policy']).toBe('same-origin');
  expect(headers['permissions-policy']).toContain('camera=()');

  const csp = headers['content-security-policy'];
  expect(csp).toBeDefined();
  expect(csp).toContain("default-src 'self'");
  expect(csp).toContain("script-src 'self'");
  expect(csp).toContain("object-src 'none'");
  expect(csp).toContain("frame-ancestors 'none'");
  expect(csp).toContain("form-action 'self'");
});

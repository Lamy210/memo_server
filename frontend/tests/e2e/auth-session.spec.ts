import { expect, test } from '@playwright/test';

const MEMO_BACKEND_FIXTURE = 'http://127.0.0.1:18080';

test('runs provider-neutral login, refresh and logout through the BFF', async ({ page }) => {
  const initial = await page.context().request.get('/auth/session');
  expect(initial.status()).toBe(200);
  expect(await initial.json()).toEqual({ authenticated: false });

  await page.goto('/auth/login?return_to=/memos');
  await expect(page).toHaveURL(/\/memos$/);
  await expect(page.getByText('UI regression baseline')).toBeVisible();

  const authenticated = await page.context().request.get('/auth/session');
  expect(await authenticated.json()).toEqual({ authenticated: true });

  const browserCookies = await page.context().cookies();
  const session = browserCookies.find((cookie) => cookie.name === '__Host-schnee_session');
  expect(session).toBeTruthy();
  expect(session?.httpOnly).toBe(true);
  expect(session?.secure).toBe(true);
  expect(session?.sameSite).toBe('Lax');
  expect(browserCookies.some((cookie) => cookie.name === 'upstream_internal')).toBe(false);

  const firstBackendAuth = await page.context().request.get(
    `${MEMO_BACKEND_FIXTURE}/__fixture__/last-authorization`
  );
  expect(await firstBackendAuth.json()).toEqual({ authorization: 'Bearer access-fixture-1' });

  await page.waitForTimeout(6_000);
  await page.reload();
  await expect(page.getByText('UI regression baseline')).toBeVisible();

  const refreshedBackendAuth = await page.context().request.get(
    `${MEMO_BACKEND_FIXTURE}/__fixture__/last-authorization`
  );
  expect(await refreshedBackendAuth.json()).toEqual({ authorization: 'Bearer access-fixture-2' });

  await page.getByRole('button', { name: 'ログアウト' }).click();
  await expect(page).toHaveURL(/\/memos$/);
  await expect(page.getByRole('link', { name: 'ログイン' })).toBeVisible();

  const loggedOut = await page.context().request.get('/auth/session');
  expect(await loggedOut.json()).toEqual({ authenticated: false });
});

test('keeps return targets internal and normalizes invalid callbacks', async ({ page }) => {
  await page.goto('/auth/login?return_to=https://evil.example/steal');
  await expect(page).toHaveURL(/\/memos$/);
  expect(new URL(page.url()).origin).toBe('http://127.0.0.1:4173');

  await page.getByRole('button', { name: 'ログアウト' }).click();

  const invalid = await page.context().request.get('/auth/callback?code=forged&state=forged', {
    maxRedirects: 0
  });
  expect(invalid.status()).toBe(400);
  expect(await invalid.json()).toEqual({ error: 'invalid_callback' });
  expect(invalid.headers()['x-fixture-internal']).toBeUndefined();
});

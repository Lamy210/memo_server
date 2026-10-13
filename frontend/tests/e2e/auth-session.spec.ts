import { expect, test, type Page } from '@playwright/test';

const MEMO_BACKEND_FIXTURE = 'http://127.0.0.1:18080';
const BROWSER_CREDENTIAL_CANARIES = [
  'access-fixture-1',
  'access-fixture-2',
  'refresh-fixture-1',
  'refresh-fixture-2',
  'fixture-code-'
] as const;

async function browserSessionStatus(page: Page): Promise<{ authenticated: boolean }> {
  return page.evaluate(async () => {
    const response = await fetch('/auth/session', { cache: 'no-store' });
    return response.json() as Promise<{ authenticated: boolean }>;
  });
}

async function browserReadableIdentitySurface(page: Page): Promise<string> {
  return page.evaluate(() => {
    const dumpStorage = (storage: Storage): string => {
      const entries: string[] = [];
      for (let index = 0; index < storage.length; index += 1) {
        const key = storage.key(index);
        if (!key) continue;
        entries.push(`${key}=${storage.getItem(key) ?? ''}`);
      }
      return entries.join('\n');
    };

    return [
      window.location.href,
      document.cookie,
      dumpStorage(window.localStorage),
      dumpStorage(window.sessionStorage),
      document.documentElement.outerHTML
    ].join('\n');
  });
}

async function expectBrowserCredentialIsolation(
  page: Page,
  expectedAuthenticated: boolean
): Promise<void> {
  const readableSurface = await browserReadableIdentitySurface(page);

  expect(readableSurface).not.toContain('__Host-schnee_session=');
  expect(readableSurface).not.toContain('__Host-schnee_login=');
  for (const canary of BROWSER_CREDENTIAL_CANARIES) {
    expect(readableSurface).not.toContain(canary);
  }

  const response = await page.evaluate(async () => {
    const sessionResponse = await fetch('/auth/session', { cache: 'no-store' });
    return {
      status: sessionResponse.status,
      cacheControl: sessionResponse.headers.get('cache-control'),
      payload: (await sessionResponse.json()) as Record<string, unknown>
    };
  });

  expect(response.status).toBe(200);
  expect(response.cacheControl).toBe('no-store');
  expect(response.payload).toEqual({ authenticated: expectedAuthenticated });
  expect(Object.keys(response.payload)).toEqual(['authenticated']);
}

test('runs provider-neutral login, refresh and logout through the BFF', async ({ page }) => {
  const initial = await page.context().request.get('/auth/session');
  expect(initial.status()).toBe(200);
  expect(await initial.json()).toEqual({ authenticated: false });

  await page.goto('/auth/login?return_to=/memos');
  await expect(page).toHaveURL(/\/memos$/);
  await expect(page.getByText('UI regression baseline')).toBeVisible();
  expect(new URL(page.url()).searchParams.has('code')).toBe(false);
  expect(new URL(page.url()).searchParams.has('state')).toBe(false);

  expect(await browserSessionStatus(page)).toEqual({ authenticated: true });
  await expectBrowserCredentialIsolation(page, true);

  const browserCookies = await page.context().cookies();
  const session = browserCookies.find((cookie) => cookie.name === '__Host-schnee_session');
  expect(session).toBeTruthy();
  expect(session?.httpOnly).toBe(true);
  expect(session?.secure).toBe(true);
  expect(session?.sameSite).toBe('Lax');
  expect(browserCookies.some((cookie) => cookie.name === '__Host-schnee_login')).toBe(false);
  expect(browserCookies.some((cookie) => cookie.name === 'upstream_internal')).toBe(false);
  for (const canary of BROWSER_CREDENTIAL_CANARIES) {
    expect(session?.value).not.toContain(canary);
  }

  const firstBackendAuth = await page.context().request.get(
    `${MEMO_BACKEND_FIXTURE}/__fixture__/last-authorization`
  );
  expect(await firstBackendAuth.json()).toEqual({ authorization: 'Bearer access-fixture-1' });

  await page.waitForTimeout(6_000);
  await page.reload();
  await expect(page.getByText('UI regression baseline')).toBeVisible();
  await expectBrowserCredentialIsolation(page, true);

  const refreshedBackendAuth = await page.context().request.get(
    `${MEMO_BACKEND_FIXTURE}/__fixture__/last-authorization`
  );
  expect(await refreshedBackendAuth.json()).toEqual({ authorization: 'Bearer access-fixture-2' });

  await page.getByRole('button', { name: 'ログアウト' }).click();
  await expect(page).toHaveURL(/\/memos$/);
  await expect(page.getByRole('link', { name: 'ログイン' })).toBeVisible();
  expect(await browserSessionStatus(page)).toEqual({ authenticated: false });
  await expectBrowserCredentialIsolation(page, false);
});

test('keeps return targets internal and normalizes invalid callbacks', async ({ page }) => {
  await page.goto('/auth/login?return_to=https://evil.example/steal');
  await expect(page).toHaveURL(/\/memos$/);
  expect(new URL(page.url()).origin).toBe('http://127.0.0.1:4173');
  expect(new URL(page.url()).searchParams.has('code')).toBe(false);
  expect(new URL(page.url()).searchParams.has('state')).toBe(false);
  await expectBrowserCredentialIsolation(page, true);

  await page.getByRole('button', { name: 'ログアウト' }).click();

  const invalid = await page.context().request.get('/auth/callback?code=forged&state=forged', {
    maxRedirects: 0
  });
  expect(invalid.status()).toBe(400);
  expect(await invalid.json()).toEqual({ error: 'invalid_callback' });
  expect(invalid.headers()['x-fixture-internal']).toBeUndefined();
});

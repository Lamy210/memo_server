import { Buffer } from 'node:buffer';

import type { Cookies } from '@sveltejs/kit';
import { expect, it, vi } from 'vitest';

import { completeLogin } from '$lib/server/identity/browserAuth';
import {
  LOGIN_TRANSACTION_COOKIE_NAME,
  sealLoginTransaction,
  SESSION_COOKIE_NAME
} from '$lib/server/identity/session';

const KEY_BYTES = new Uint8Array(32).fill(41);
const KEY = Buffer.from(KEY_BYTES).toString('base64');
const NOW = 1_900_000_000_000;

type DeleteCookieOptions = Parameters<Cookies['delete']>[1];

it('maps a state-bound OAuth access_denied callback to the stable public error', async () => {
  const transaction = await sealLoginTransaction(
    {
      state: 'state-1',
      codeVerifier: 'v'.repeat(43),
      returnTarget: '/memos',
      expiresAt: NOW + 60_000
    },
    KEY_BYTES
  );
  const values = new Map([[LOGIN_TRANSACTION_COOKIE_NAME, transaction]]);
  const deleted: Array<{ name: string; options: DeleteCookieOptions }> = [];
  const cookies: Pick<Cookies, 'get' | 'set' | 'delete'> = {
    get(name) {
      return values.get(name);
    },
    set(name, value) {
      values.set(name, value);
    },
    delete(name, options) {
      values.delete(name);
      deleted.push({ name, options });
    }
  };
  const fetchFn = vi.fn() as unknown as typeof fetch;

  const response = await completeLogin({
    url: new URL('https://memo.example.test/auth/callback?error=access_denied&state=state-1'),
    cookies,
    fetchFn,
    identityEnv: {
      IDENTITY_ISSUER: 'https://id.example.test',
      IDENTITY_CLIENT_ID: 'memo-web',
      IDENTITY_CLIENT_SECRET: 'client-secret',
      IDENTITY_SESSION_KEY: KEY
    },
    now: NOW
  });

  expect(response.status).toBe(401);
  await expect(response.json()).resolves.toEqual({ error: 'access_denied' });
  expect(response.headers.get('cache-control')).toBe('no-store');
  expect(values.has(LOGIN_TRANSACTION_COOKIE_NAME)).toBe(false);
  expect(values.has(SESSION_COOKIE_NAME)).toBe(false);
  expect(fetchFn).not.toHaveBeenCalled();
  expect(deleted).toContainEqual({ name: LOGIN_TRANSACTION_COOKIE_NAME, options: { path: '/' } });
});

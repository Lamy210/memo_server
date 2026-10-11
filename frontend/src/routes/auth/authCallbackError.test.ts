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

function identityEnv() {
  return {
    IDENTITY_ISSUER: 'https://id.example.test',
    IDENTITY_CLIENT_ID: 'memo-web',
    IDENTITY_CLIENT_SECRET: 'client-secret',
    IDENTITY_SESSION_KEY: KEY
  };
}

async function transactionCookie(state = 'state-1') {
  return sealLoginTransaction(
    {
      state,
      codeVerifier: 'v'.repeat(43),
      returnTarget: '/memos',
      expiresAt: NOW + 60_000
    },
    KEY_BYTES
  );
}

function cookieJar(transaction: string) {
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
  return { cookies, values, deleted };
}

it('maps a state-bound OAuth access_denied callback to the stable public error', async () => {
  const { cookies, values, deleted } = cookieJar(await transactionCookie());
  const fetchFn = vi.fn() as unknown as typeof fetch;

  const response = await completeLogin({
    url: new URL('https://memo.example.test/auth/callback?error=access_denied&state=state-1'),
    cookies,
    fetchFn,
    identityEnv: identityEnv(),
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

it('maps token-endpoint access denial to the same stable public error', async () => {
  const { cookies, values } = cookieJar(await transactionCookie());
  const fetchFn = vi.fn(async (request: RequestInfo | URL) => {
    const url = String(request);
    if (url.endsWith('/.well-known/openid-configuration')) {
      return Response.json({
        issuer: 'https://id.example.test',
        authorization_endpoint: 'https://id.example.test/authorize',
        token_endpoint: 'https://id.example.test/token',
        code_challenge_methods_supported: ['S256'],
        token_endpoint_auth_methods_supported: ['client_secret_basic']
      });
    }
    return new Response(JSON.stringify({ error: 'invalid_grant', error_description: 'private detail' }), {
      status: 400,
      headers: { 'Content-Type': 'application/json' }
    });
  }) as unknown as typeof fetch;

  const response = await completeLogin({
    url: new URL('https://memo.example.test/auth/callback?code=code-1&state=state-1'),
    cookies,
    fetchFn,
    identityEnv: identityEnv(),
    now: NOW
  });

  expect(response.status).toBe(401);
  await expect(response.json()).resolves.toEqual({ error: 'access_denied' });
  expect(await response.clone().text()).not.toContain('private detail');
  expect(values.has(SESSION_COOKIE_NAME)).toBe(false);
});

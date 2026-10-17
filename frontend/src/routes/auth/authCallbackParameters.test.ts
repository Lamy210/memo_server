import { Buffer } from 'node:buffer';

import type { Cookies } from '@sveltejs/kit';
import { describe, expect, it, vi } from 'vitest';

import { completeLogin } from '$lib/server/identity/browserAuth';
import {
  LOGIN_TRANSACTION_COOKIE_NAME,
  sealLoginTransaction,
  SESSION_COOKIE_NAME
} from '$lib/server/identity/session';

const SESSION_KEY_BYTES = new Uint8Array(32).fill(23);
const SESSION_KEY = Buffer.from(SESSION_KEY_BYTES).toString('base64');
const NOW = 1_900_000_000_000;

type SetCookieOptions = Parameters<Cookies['set']>[2];
type DeleteCookieOptions = Parameters<Cookies['delete']>[1];

function identityEnv() {
  return {
    IDENTITY_ISSUER: 'https://id.example.test',
    IDENTITY_CLIENT_ID: 'memo-web',
    IDENTITY_CLIENT_SECRET: 'client-secret',
    IDENTITY_SESSION_KEY: SESSION_KEY
  };
}

function fakeCookies(initial: Record<string, string> = {}) {
  const values = new Map(Object.entries(initial));
  const api: Pick<Cookies, 'get' | 'set' | 'delete'> = {
    get(name: string) {
      return values.get(name);
    },
    set(name: string, value: string, _options: SetCookieOptions) {
      values.set(name, value);
    },
    delete(name: string, _options: DeleteCookieOptions) {
      values.delete(name);
    }
  };

  return { api, values };
}

async function loginTransaction() {
  return sealLoginTransaction(
    {
      state: 'state-1',
      codeVerifier: 'v'.repeat(43),
      returnTarget: '/memos',
      expiresAt: NOW + 60_000
    },
    SESSION_KEY_BYTES
  );
}

describe('OAuth callback parameter shape', () => {
  it.each([
    [
      'duplicate state',
      'https://memo.example.test/auth/callback?code=code-1&state=state-1&state=other-state'
    ],
    [
      'duplicate code',
      'https://memo.example.test/auth/callback?code=code-1&code=code-2&state=state-1'
    ],
    [
      'duplicate error',
      'https://memo.example.test/auth/callback?error=access_denied&error=server_error&state=state-1'
    ],
    [
      'code and error together',
      'https://memo.example.test/auth/callback?code=code-1&error=access_denied&state=state-1'
    ]
  ])('rejects %s before contacting the identity issuer', async (_caseName, callbackUrl) => {
    const transaction = await loginTransaction();
    const cookies = fakeCookies({ [LOGIN_TRANSACTION_COOKIE_NAME]: transaction });
    const fetchFn = vi.fn() as unknown as typeof fetch;

    const response = await completeLogin({
      url: new URL(callbackUrl),
      cookies: cookies.api,
      fetchFn,
      identityEnv: identityEnv(),
      now: NOW
    });

    expect(response.status).toBe(400);
    await expect(response.json()).resolves.toEqual({ error: 'invalid_callback' });
    expect(cookies.values.has(LOGIN_TRANSACTION_COOKIE_NAME)).toBe(false);
    expect(cookies.values.has(SESSION_COOKIE_NAME)).toBe(false);
    expect(fetchFn).not.toHaveBeenCalled();
  });
});

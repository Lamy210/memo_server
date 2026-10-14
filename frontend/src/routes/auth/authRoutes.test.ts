import { Buffer } from 'node:buffer';

import type { Cookies } from '@sveltejs/kit';
import { describe, expect, it, vi } from 'vitest';

import {
  completeLogin,
  getSessionStatus,
  logoutSession,
  startLogin
} from '$lib/server/identity/browserAuth';
import {
  LOGIN_TRANSACTION_COOKIE_NAME,
  openSession,
  sealLoginTransaction,
  SESSION_COOKIE_NAME
} from '$lib/server/identity/session';

const SESSION_KEY_BYTES = new Uint8Array(32).fill(23);
const SESSION_KEY = Buffer.from(SESSION_KEY_BYTES).toString('base64');
const NOW = 1_900_000_000_000;

type SetCookieOptions = Parameters<Cookies['set']>[2];
type DeleteCookieOptions = Parameters<Cookies['delete']>[1];

function identityEnv(overrides: Record<string, string | undefined> = {}) {
  return {
    IDENTITY_ISSUER: 'https://id.example.test',
    IDENTITY_CLIENT_ID: 'memo-web',
    IDENTITY_CLIENT_SECRET: 'client-secret',
    IDENTITY_SESSION_KEY: SESSION_KEY,
    ...overrides
  };
}

function fakeCookies(initial: Record<string, string> = {}) {
  const values = new Map(Object.entries(initial));
  const setCalls: Array<{ name: string; value: string; options: SetCookieOptions }> = [];
  const deleteCalls: Array<{ name: string; options: DeleteCookieOptions }> = [];

  const api: Pick<Cookies, 'get' | 'set' | 'delete'> = {
    get(name: string) {
      return values.get(name);
    },
    set(name: string, value: string, options: SetCookieOptions) {
      values.set(name, value);
      setCalls.push({ name, value, options });
    },
    delete(name: string, options: DeleteCookieOptions) {
      values.delete(name);
      deleteCalls.push({ name, options });
    }
  };

  return { api, values, setCalls, deleteCalls };
}

function discoveryPayload() {
  return {
    issuer: 'https://id.example.test',
    authorization_endpoint: 'https://id.example.test/authorize',
    token_endpoint: 'https://id.example.test/token',
    end_session_endpoint: 'https://id.example.test/logout',
    code_challenge_methods_supported: ['S256'],
    token_endpoint_auth_methods_supported: ['client_secret_basic']
  };
}

function authFetch(input: { refreshToken?: string | null } = {}) {
  return vi.fn(async (request: RequestInfo | URL, init?: RequestInit) => {
    const url = String(request);
    if (url.endsWith('/.well-known/openid-configuration')) {
      return Response.json(discoveryPayload());
    }

    expect(url).toBe('https://id.example.test/token');
    const body = new URLSearchParams(String(init?.body));
    expect(body.get('grant_type')).toBe('authorization_code');
    expect(body.get('code')).toBe('code-1');
    expect(body.get('code_verifier')).toBe('v'.repeat(43));
    expect(body.get('redirect_uri')).toBe('https://memo.example.test/auth/callback');

    return Response.json({
      access_token: 'access-1',
      token_type: 'Bearer',
      expires_in: 300,
      ...(input.refreshToken === null
        ? {}
        : { refresh_token: input.refreshToken ?? 'refresh-1' })
    });
  }) as unknown as typeof fetch;
}

describe('provider-neutral browser auth routes', () => {
  it('starts login with a sealed one-time transaction, safe return target and PKCE S256', async () => {
    const cookies = fakeCookies();
    const randomValues = [new Uint8Array(32).fill(1), new Uint8Array(32).fill(2)];
    const response = await startLogin({
      url: new URL('https://memo.example.test/auth/login?return_to=https://evil.example/steal'),
      cookies: cookies.api,
      fetchFn: authFetch(),
      identityEnv: identityEnv(),
      now: NOW,
      randomBytes: () => randomValues.shift()!
    });

    expect(response.status).toBe(303);
    const location = new URL(response.headers.get('location')!);
    expect(location.origin + location.pathname).toBe('https://id.example.test/authorize');
    expect(location.searchParams.get('response_type')).toBe('code');
    expect(location.searchParams.get('scope')).toBe('openid offline_access');
    expect(location.searchParams.get('code_challenge_method')).toBe('S256');
    expect(location.searchParams.get('state')).toBeTruthy();
    expect(location.searchParams.get('code_challenge')).toBeTruthy();

    expect(cookies.setCalls).toHaveLength(1);
    expect(cookies.setCalls[0].name).toBe(LOGIN_TRANSACTION_COOKIE_NAME);
    expect(cookies.setCalls[0].options).toMatchObject({
      path: '/',
      httpOnly: true,
      secure: true,
      sameSite: 'lax',
      maxAge: 600
    });
    expect(cookies.setCalls[0].value).not.toContain('/memos');
    expect(cookies.setCalls[0].value).not.toContain('evil.example');
  });

  it('completes login once, uses the original verifier, and redirects only to stored target', async () => {
    const transaction = await sealLoginTransaction(
      {
        state: 'state-1',
        codeVerifier: 'v'.repeat(43),
        returnTarget: '/memos?from=login',
        expiresAt: NOW + 60_000
      },
      SESSION_KEY_BYTES
    );
    const cookies = fakeCookies({ [LOGIN_TRANSACTION_COOKIE_NAME]: transaction });

    const response = await completeLogin({
      url: new URL('https://memo.example.test/auth/callback?code=code-1&state=state-1'),
      cookies: cookies.api,
      fetchFn: authFetch(),
      identityEnv: identityEnv(),
      now: NOW
    });

    expect(response.status).toBe(303);
    expect(response.headers.get('location')).toBe('/memos?from=login');
    expect(cookies.deleteCalls).toContainEqual({
      name: LOGIN_TRANSACTION_COOKIE_NAME,
      options: { path: '/' }
    });
    const sessionSet = cookies.setCalls.find((call) => call.name === SESSION_COOKIE_NAME);
    expect(sessionSet?.options).toMatchObject({ path: '/', httpOnly: true, secure: true, sameSite: 'lax' });
    await expect(openSession(sessionSet!.value, SESSION_KEY_BYTES)).resolves.toEqual({
      accessToken: 'access-1',
      accessTokenExpiresAt: NOW + 300_000,
      refreshToken: 'refresh-1'
    });

    const replay = await completeLogin({
      url: new URL('https://memo.example.test/auth/callback?code=code-1&state=state-1'),
      cookies: cookies.api,
      fetchFn: authFetch(),
      identityEnv: identityEnv(),
      now: NOW
    });
    expect(replay.status).toBe(400);
  });

  it('creates a bounded v2 session when refresh coordination is configured', async () => {
    const transaction = await sealLoginTransaction(
      {
        state: 'state-1',
        codeVerifier: 'v'.repeat(43),
        returnTarget: '/memos',
        expiresAt: NOW + 60_000
      },
      SESSION_KEY_BYTES
    );
    const cookies = fakeCookies({ [LOGIN_TRANSACTION_COOKIE_NAME]: transaction });

    const response = await completeLogin({
      url: new URL('https://memo.example.test/auth/callback?code=code-1&state=state-1'),
      cookies: cookies.api,
      fetchFn: authFetch(),
      identityEnv: identityEnv({
        IDENTITY_REFRESH_COORDINATION_URL: 'valkey://valkey:6379',
        IDENTITY_BROWSER_SESSION_MAX_AGE_SECONDS: '3600'
      }),
      now: NOW
    });

    expect(response.status).toBe(303);
    const sessionSet = cookies.setCalls.find((call) => call.name === SESSION_COOKIE_NAME);
    await expect(openSession(sessionSet!.value, SESSION_KEY_BYTES, NOW)).resolves.toEqual({
      schemaVersion: 2,
      accessToken: 'access-1',
      accessTokenExpiresAt: NOW + 300_000,
      refreshToken: 'refresh-1',
      sessionExpiresAt: NOW + 3_600_000
    });
  });

  it.each([
    'https://memo.example.test/auth/callback?code=code-1',
    'https://memo.example.test/auth/callback?state=state-1',
    'https://memo.example.test/auth/callback?code=code-1&state=wrong-state'
  ])('rejects invalid callback input and consumes the transaction: %s', async (callbackUrl) => {
    const transaction = await sealLoginTransaction(
      {
        state: 'state-1',
        codeVerifier: 'v'.repeat(43),
        returnTarget: '/memos',
        expiresAt: NOW + 60_000
      },
      SESSION_KEY_BYTES
    );
    const cookies = fakeCookies({ [LOGIN_TRANSACTION_COOKIE_NAME]: transaction });
    const fetchFn = authFetch();

    const response = await completeLogin({
      url: new URL(callbackUrl),
      cookies: cookies.api,
      fetchFn,
      identityEnv: identityEnv(),
      now: NOW
    });

    expect(response.status).toBe(400);
    expect(cookies.values.has(LOGIN_TRANSACTION_COOKIE_NAME)).toBe(false);
    expect(cookies.values.has(SESSION_COOKIE_NAME)).toBe(false);
  });

  it('does not create a session when the initial token response lacks a refresh token', async () => {
    const transaction = await sealLoginTransaction(
      {
        state: 'state-1',
        codeVerifier: 'v'.repeat(43),
        returnTarget: '/memos',
        expiresAt: NOW + 60_000
      },
      SESSION_KEY_BYTES
    );
    const cookies = fakeCookies({ [LOGIN_TRANSACTION_COOKIE_NAME]: transaction });

    const response = await completeLogin({
      url: new URL('https://memo.example.test/auth/callback?code=code-1&state=state-1'),
      cookies: cookies.api,
      fetchFn: authFetch({ refreshToken: null }),
      identityEnv: identityEnv(),
      now: NOW
    });

    expect(response.status).toBe(502);
    expect(cookies.values.has(LOGIN_TRANSACTION_COOKIE_NAME)).toBe(false);
    expect(cookies.values.has(SESSION_COOKIE_NAME)).toBe(false);
    expect(await response.text()).not.toContain('refresh');
  });

  it('clears the local session before redirecting on logout', () => {
    const cookies = fakeCookies({ [SESSION_COOKIE_NAME]: 'opaque-session' });
    const response = logoutSession({
      url: new URL('https://memo.example.test/auth/logout?return_to=//evil.example'),
      cookies: cookies.api
    });

    expect(response.status).toBe(303);
    expect(response.headers.get('location')).toBe('/memos');
    expect(cookies.values.has(SESSION_COOKIE_NAME)).toBe(false);
    expect(cookies.deleteCalls[0]).toEqual({ name: SESSION_COOKIE_NAME, options: { path: '/' } });
  });

  it('returns only authenticated state from the session endpoint with no-store', async () => {
    const response = getSessionStatus(true);
    expect(response.status).toBe(200);
    expect(response.headers.get('cache-control')).toBe('no-store');
    await expect(response.json()).resolves.toEqual({ authenticated: true });
  });
});

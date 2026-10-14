import { Buffer } from 'node:buffer';

import type { Cookies } from '@sveltejs/kit';
import { describe, expect, it, vi } from 'vitest';

import { restoreIdentitySession } from './hooks.server';
import type {
  RefreshCoordinationDecision,
  RefreshCoordinator
} from '$lib/server/identity/refreshCoordinator';
import { openSession, sealSession, SESSION_COOKIE_NAME } from '$lib/server/identity/session';

const SESSION_KEY_BYTES = new Uint8Array(32).fill(17);
const SESSION_KEY = Buffer.from(SESSION_KEY_BYTES).toString('base64');
const NOW = 1_900_000_000_000;
const SESSION_EXPIRES_AT = NOW + 3_600_000;

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

function coordinatedEnv() {
  return identityEnv({
    IDENTITY_REFRESH_COORDINATION_URL: 'valkey://valkey:6379',
    IDENTITY_BROWSER_SESSION_MAX_AGE_SECONDS: '3600'
  });
}

function fakeCookies(initial?: string) {
  let current = initial;
  const setCalls: Array<{ name: string; value: string; options: SetCookieOptions }> = [];
  const deleteCalls: Array<{ name: string; options: DeleteCookieOptions }> = [];

  const api: Pick<Cookies, 'get' | 'set' | 'delete'> = {
    get(name: string) {
      return name === SESSION_COOKIE_NAME ? current : undefined;
    },
    set(name: string, value: string, options: SetCookieOptions) {
      if (name === SESSION_COOKIE_NAME) current = value;
      setCalls.push({ name, value, options });
    },
    delete(name: string, options: DeleteCookieOptions) {
      if (name === SESSION_COOKIE_NAME) current = undefined;
      deleteCalls.push({ name, options });
    }
  };

  return {
    api,
    setCalls,
    deleteCalls,
    current: () => current
  };
}

function discoveryAndRefreshFetch(input: {
  accessToken?: string;
  expiresIn?: number;
  refreshToken?: string | null;
  failRefresh?: boolean;
} = {}) {
  return vi.fn(async (request: RequestInfo | URL, init?: RequestInit) => {
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

    expect(url).toBe('https://id.example.test/token');
    expect(init?.redirect).toBe('error');
    const body = new URLSearchParams(String(init?.body));
    expect(body.get('grant_type')).toBe('refresh_token');
    expect(body.get('refresh_token')).toBe('refresh-old');

    if (input.failRefresh) return new Response('raw upstream failure', { status: 503 });
    return Response.json({
      access_token: input.accessToken ?? 'access-new',
      token_type: 'Bearer',
      expires_in: input.expiresIn ?? 120,
      ...(input.refreshToken === null
        ? {}
        : { refresh_token: input.refreshToken ?? 'refresh-new' })
    });
  }) as unknown as typeof fetch;
}

function fakeCoordinator(
  decision: RefreshCoordinationDecision,
  publishResult = true
): {
  coordinator: RefreshCoordinator;
  acquire: ReturnType<typeof vi.fn>;
  publishSuccess: ReturnType<typeof vi.fn>;
  completeFailure: ReturnType<typeof vi.fn>;
} {
  const acquire = vi.fn(async () => decision);
  const publishSuccess = vi.fn(async () => publishResult);
  const completeFailure = vi.fn(async () => {});
  return {
    coordinator: {
      acquire,
      publishSuccess,
      completeFailure,
      close: vi.fn(async () => {})
    },
    acquire,
    publishSuccess,
    completeFailure
  };
}

async function v2Session(accessTokenExpiresAt: number): Promise<string> {
  return sealSession(
    {
      schemaVersion: 2,
      accessToken: 'access-old',
      accessTokenExpiresAt,
      refreshToken: 'refresh-old',
      sessionExpiresAt: SESSION_EXPIRES_AT
    },
    SESSION_KEY_BYTES
  );
}

describe('restoreIdentitySession', () => {
  it('restores a live access token without contacting the identity service', async () => {
    const sealed = await sealSession(
      {
        accessToken: 'access-live',
        accessTokenExpiresAt: NOW + 120_000,
        refreshToken: 'refresh-old'
      },
      SESSION_KEY_BYTES
    );
    const cookies = fakeCookies(sealed);
    const fetchFn = vi.fn() as unknown as typeof fetch;
    const locals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({ cookies: cookies.api, locals, env: identityEnv(), fetchFn, now: NOW });

    expect(locals).toEqual({ authenticated: true, accessToken: 'access-live' });
    expect(fetchFn).not.toHaveBeenCalled();
    expect(cookies.setCalls).toHaveLength(0);
  });

  it('refreshes an expiring access token and rotates the refresh credential', async () => {
    const sealed = await sealSession(
      {
        accessToken: 'access-old',
        accessTokenExpiresAt: NOW + 10_000,
        refreshToken: 'refresh-old'
      },
      SESSION_KEY_BYTES
    );
    const cookies = fakeCookies(sealed);
    const locals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: identityEnv(),
      fetchFn: discoveryAndRefreshFetch({ refreshToken: 'refresh-new' }),
      now: NOW
    });

    expect(locals).toEqual({ authenticated: true, accessToken: 'access-new' });
    expect(cookies.setCalls).toHaveLength(1);
    expect(cookies.setCalls[0].name).toBe(SESSION_COOKIE_NAME);
    expect(cookies.setCalls[0].options).toMatchObject({
      path: '/',
      httpOnly: true,
      secure: true,
      sameSite: 'lax'
    });
    await expect(openSession(cookies.setCalls[0].value, SESSION_KEY_BYTES)).resolves.toEqual({
      accessToken: 'access-new',
      accessTokenExpiresAt: NOW + 120_000,
      refreshToken: 'refresh-new'
    });
  });

  it('retains the prior refresh credential when refresh omits a replacement', async () => {
    const sealed = await sealSession(
      { accessToken: 'access-old', accessTokenExpiresAt: NOW, refreshToken: 'refresh-old' },
      SESSION_KEY_BYTES
    );
    const cookies = fakeCookies(sealed);
    const locals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: identityEnv(),
      fetchFn: discoveryAndRefreshFetch({ refreshToken: null }),
      now: NOW
    });

    const rotated = await openSession(cookies.setCalls[0].value, SESSION_KEY_BYTES);
    expect(rotated?.refreshToken).toBe('refresh-old');
  });

  it('clears the session and remains unauthenticated when refresh fails', async () => {
    const sealed = await sealSession(
      { accessToken: 'access-old', accessTokenExpiresAt: NOW, refreshToken: 'refresh-old' },
      SESSION_KEY_BYTES
    );
    const cookies = fakeCookies(sealed);
    const locals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: identityEnv(),
      fetchFn: discoveryAndRefreshFetch({ failRefresh: true }),
      now: NOW
    });

    expect(locals).toEqual({ authenticated: false });
    expect(cookies.deleteCalls).toContainEqual({ name: SESSION_COOKIE_NAME, options: { path: '/' } });
  });

  it('clears a tampered cookie without deriving identity from request-controlled input', async () => {
    const cookies = fakeCookies('v1.invalid.invalid');
    const locals: { authenticated?: boolean; accessToken?: string } = {};
    const fetchFn = vi.fn() as unknown as typeof fetch;

    await restoreIdentitySession({ cookies: cookies.api, locals, env: identityEnv(), fetchFn, now: NOW });

    expect(locals).toEqual({ authenticated: false });
    expect(fetchFn).not.toHaveBeenCalled();
    expect(cookies.deleteCalls).toHaveLength(1);
  });

  it('does not require identity configuration when no production session exists', async () => {
    const cookies = fakeCookies();
    const locals: { authenticated?: boolean; accessToken?: string } = {
      authenticated: true,
      accessToken: 'browser-controlled-value-that-must-be-overwritten'
    };

    await restoreIdentitySession({ cookies: cookies.api, locals, env: {}, fetchFn: vi.fn() as never, now: NOW });

    expect(locals).toEqual({ authenticated: false });
  });

  it('clears a legacy v1 session before coordination or upstream refresh is attempted', async () => {
    const sealed = await sealSession(
      { accessToken: 'old', accessTokenExpiresAt: NOW, refreshToken: 'refresh-old' },
      SESSION_KEY_BYTES
    );
    const cookies = fakeCookies(sealed);
    const coordination = fakeCoordinator({ kind: 'leader', owner: 'owner-a' });
    const fetchFn = vi.fn() as unknown as typeof fetch;
    const locals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: coordinatedEnv(),
      fetchFn,
      now: NOW,
      coordinator: coordination.coordinator
    });

    expect(locals).toEqual({ authenticated: false });
    expect(coordination.acquire).not.toHaveBeenCalled();
    expect(fetchFn).not.toHaveBeenCalled();
    expect(cookies.deleteCalls).toHaveLength(1);
  });

  it('keeps a live v2 access token outside the refresh window without touching coordination', async () => {
    const sealed = await v2Session(NOW + 120_000);
    const cookies = fakeCookies(sealed);
    const coordination = fakeCoordinator({ kind: 'leader', owner: 'owner-a' });
    const fetchFn = vi.fn() as unknown as typeof fetch;
    const locals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: coordinatedEnv(),
      fetchFn,
      now: NOW,
      coordinator: coordination.coordinator
    });

    expect(locals).toEqual({ authenticated: true, accessToken: 'access-old' });
    expect(coordination.acquire).not.toHaveBeenCalled();
    expect(fetchFn).not.toHaveBeenCalled();
  });

  it('lets only the elected leader refresh and preserves the fixed v2 session expiry', async () => {
    const sealed = await v2Session(NOW + 10_000);
    const cookies = fakeCookies(sealed);
    const coordination = fakeCoordinator({ kind: 'leader', owner: 'owner-a' });
    const fetchFn = discoveryAndRefreshFetch({ refreshToken: 'refresh-new' });
    const locals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: coordinatedEnv(),
      fetchFn,
      now: NOW,
      coordinator: coordination.coordinator
    });

    expect(fetchFn).toHaveBeenCalledTimes(2);
    expect(coordination.publishSuccess).toHaveBeenCalledTimes(1);
    expect(locals).toEqual({ authenticated: true, accessToken: 'access-new' });
    await expect(openSession(cookies.setCalls[0].value, SESSION_KEY_BYTES, NOW)).resolves.toEqual({
      schemaVersion: 2,
      accessToken: 'access-new',
      accessTokenExpiresAt: NOW + 120_000,
      refreshToken: 'refresh-new',
      sessionExpiresAt: SESSION_EXPIRES_AT
    });
  });

  it('adopts a coordinated follower result without contacting the identity service', async () => {
    const sealed = await v2Session(NOW + 10_000);
    const sealedResult = await sealSession(
      {
        schemaVersion: 2,
        accessToken: 'access-new',
        accessTokenExpiresAt: NOW + 120_000,
        refreshToken: 'refresh-new',
        sessionExpiresAt: SESSION_EXPIRES_AT
      },
      SESSION_KEY_BYTES
    );
    const cookies = fakeCookies(sealed);
    const coordination = fakeCoordinator({ kind: 'result', sealedSession: sealedResult });
    const fetchFn = vi.fn() as unknown as typeof fetch;
    const locals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: coordinatedEnv(),
      fetchFn,
      now: NOW,
      coordinator: coordination.coordinator
    });

    expect(fetchFn).not.toHaveBeenCalled();
    expect(cookies.setCalls[0].value).toBe(sealedResult);
    expect(locals).toEqual({ authenticated: true, accessToken: 'access-new' });
  });

  it('fails closed when the leader cannot publish a successful upstream refresh', async () => {
    const sealed = await v2Session(NOW + 10_000);
    const cookies = fakeCookies(sealed);
    const coordination = fakeCoordinator({ kind: 'leader', owner: 'owner-a' }, false);
    const locals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: coordinatedEnv(),
      fetchFn: discoveryAndRefreshFetch(),
      now: NOW,
      coordinator: coordination.coordinator
    });

    expect(locals).toEqual({ authenticated: false });
    expect(coordination.completeFailure).toHaveBeenCalledWith({ sealedSession: sealed, owner: 'owner-a' });
    expect(cookies.deleteCalls).toHaveLength(1);
  });

  it('records leader failure and clears the session when upstream refresh fails', async () => {
    const sealed = await v2Session(NOW);
    const cookies = fakeCookies(sealed);
    const coordination = fakeCoordinator({ kind: 'leader', owner: 'owner-a' });
    const locals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: coordinatedEnv(),
      fetchFn: discoveryAndRefreshFetch({ failRefresh: true }),
      now: NOW,
      coordinator: coordination.coordinator
    });

    expect(coordination.completeFailure).toHaveBeenCalledWith({ sealedSession: sealed, owner: 'owner-a' });
    expect(locals).toEqual({ authenticated: false });
    expect(cookies.deleteCalls).toHaveLength(1);
  });

  it('uses a still-valid token for the current request when coordination is unavailable', async () => {
    const sealed = await v2Session(NOW + 1);
    const cookies = fakeCookies(sealed);
    const coordination = fakeCoordinator({ kind: 'unavailable' });
    const fetchFn = vi.fn() as unknown as typeof fetch;
    const locals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: coordinatedEnv(),
      fetchFn,
      now: NOW,
      coordinator: coordination.coordinator
    });

    expect(locals).toEqual({ authenticated: true, accessToken: 'access-old' });
    expect(fetchFn).not.toHaveBeenCalled();
    expect(cookies.setCalls).toHaveLength(0);
    expect(cookies.deleteCalls).toHaveLength(0);
  });

  it('clears an expired token when coordination is unavailable instead of refreshing without coordination', async () => {
    const sealed = await v2Session(NOW);
    const cookies = fakeCookies(sealed);
    const coordination = fakeCoordinator({ kind: 'unavailable' });
    const fetchFn = vi.fn() as unknown as typeof fetch;
    const locals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: coordinatedEnv(),
      fetchFn,
      now: NOW,
      coordinator: coordination.coordinator
    });

    expect(locals).toEqual({ authenticated: false });
    expect(fetchFn).not.toHaveBeenCalled();
    expect(cookies.deleteCalls).toHaveLength(1);
  });
});

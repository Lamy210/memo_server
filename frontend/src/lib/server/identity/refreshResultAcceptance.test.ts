import { Buffer } from 'node:buffer';

import type { Cookies } from '@sveltejs/kit';
import { describe, expect, it, vi } from 'vitest';

import { restoreIdentitySession } from '../../../hooks.server';
import type { RefreshCoordinator } from './refreshCoordinator';
import { sealSession, SESSION_COOKIE_NAME } from './session';

const SESSION_KEY_BYTES = new Uint8Array(32).fill(83);
const SESSION_KEY = Buffer.from(SESSION_KEY_BYTES).toString('base64');
const NOW = 1_900_000_000_000;
const SESSION_EXPIRES_AT = NOW + 3_600_000;

type SetCookieOptions = Parameters<Cookies['set']>[2];
type DeleteCookieOptions = Parameters<Cookies['delete']>[1];

function cookieJar(initial: string) {
  let current: string | undefined = initial;
  const setCalls: string[] = [];
  const deleteCalls: string[] = [];
  const api: Pick<Cookies, 'get' | 'set' | 'delete'> = {
    get(name: string) {
      return name === SESSION_COOKIE_NAME ? current : undefined;
    },
    set(name: string, value: string, _options: SetCookieOptions) {
      if (name === SESSION_COOKIE_NAME) current = value;
      setCalls.push(name);
    },
    delete(name: string, _options: DeleteCookieOptions) {
      if (name === SESSION_COOKIE_NAME) current = undefined;
      deleteCalls.push(name);
    }
  };
  return { api, setCalls, deleteCalls };
}

function coordinatedEnv() {
  return {
    IDENTITY_ISSUER: 'https://id.example.test',
    IDENTITY_CLIENT_ID: 'memo-web',
    IDENTITY_CLIENT_SECRET: 'client-secret',
    IDENTITY_SESSION_KEY: SESSION_KEY,
    IDENTITY_REFRESH_COORDINATION_URL: 'valkey://unused.test:6379',
    IDENTITY_BROWSER_SESSION_MAX_AGE_SECONDS: '3600'
  };
}

function resultCoordinator(sealedSession: string): RefreshCoordinator {
  return {
    acquire: vi.fn(async () => ({ kind: 'result' as const, sealedSession })),
    publishSuccess: vi.fn(async () => true),
    completeFailure: vi.fn(async () => {}),
    close: vi.fn(async () => {})
  };
}

describe('coordinated refresh result acceptance', () => {
  it('rejects an authenticated handoff whose access token is already expired', async () => {
    const oldSealed = await sealSession(
      {
        schemaVersion: 2,
        accessToken: 'access-old',
        accessTokenExpiresAt: NOW,
        refreshToken: 'refresh-old',
        sessionExpiresAt: SESSION_EXPIRES_AT
      },
      SESSION_KEY_BYTES
    );
    const staleResult = await sealSession(
      {
        schemaVersion: 2,
        accessToken: 'access-stale',
        accessTokenExpiresAt: NOW,
        refreshToken: 'refresh-new',
        sessionExpiresAt: SESSION_EXPIRES_AT
      },
      SESSION_KEY_BYTES
    );
    const cookies = cookieJar(oldSealed);
    const locals: { authenticated?: boolean; accessToken?: string } = {};
    const fetchFn = vi.fn() as unknown as typeof fetch;

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: coordinatedEnv(),
      fetchFn,
      now: NOW,
      coordinator: resultCoordinator(staleResult)
    });

    expect(fetchFn).not.toHaveBeenCalled();
    expect(locals).toEqual({ authenticated: false });
    expect(cookies.setCalls).toHaveLength(0);
    expect(cookies.deleteCalls).toEqual([SESSION_COOKIE_NAME]);
  });

  it('uses wall-clock time after coordination before admitting a follower result', async () => {
    const wallNow = Date.now();
    const requestStartedAt = wallNow - 60_000;
    const sessionExpiresAt = wallNow + 3_600_000;
    const oldSealed = await sealSession(
      {
        schemaVersion: 2,
        accessToken: 'access-old',
        accessTokenExpiresAt: requestStartedAt,
        refreshToken: 'refresh-old',
        sessionExpiresAt
      },
      SESSION_KEY_BYTES
    );
    const expiredDuringWait = await sealSession(
      {
        schemaVersion: 2,
        accessToken: 'access-expired-during-wait',
        accessTokenExpiresAt: wallNow - 1,
        refreshToken: 'refresh-new',
        sessionExpiresAt
      },
      SESSION_KEY_BYTES
    );
    const cookies = cookieJar(oldSealed);
    const locals: { authenticated?: boolean; accessToken?: string } = {};
    const fetchFn = vi.fn() as unknown as typeof fetch;

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: coordinatedEnv(),
      fetchFn,
      now: requestStartedAt,
      coordinator: resultCoordinator(expiredDuringWait)
    });

    expect(fetchFn).not.toHaveBeenCalled();
    expect(locals).toEqual({ authenticated: false });
    expect(cookies.setCalls).toHaveLength(0);
    expect(cookies.deleteCalls).toEqual([SESSION_COOKIE_NAME]);
  });

  it('does not authenticate on coordination outage after the fixed v2 session lifetime expires', async () => {
    let wallNow = NOW;
    const dateNow = vi.spyOn(Date, 'now').mockImplementation(() => wallNow);
    const oldSealed = await sealSession(
      {
        schemaVersion: 2,
        accessToken: 'access-still-valid',
        accessTokenExpiresAt: NOW + 60_000,
        refreshToken: 'refresh-old',
        sessionExpiresAt: NOW + 1_000
      },
      SESSION_KEY_BYTES
    );
    const coordinator: RefreshCoordinator = {
      acquire: vi.fn(async () => {
        wallNow = NOW + 1_001;
        return { kind: 'unavailable' as const };
      }),
      publishSuccess: vi.fn(async () => true),
      completeFailure: vi.fn(async () => {}),
      close: vi.fn(async () => {})
    };
    const cookies = cookieJar(oldSealed);
    const locals: { authenticated?: boolean; accessToken?: string } = {};
    const fetchFn = vi.fn() as unknown as typeof fetch;

    try {
      await restoreIdentitySession({
        cookies: cookies.api,
        locals,
        env: coordinatedEnv(),
        fetchFn,
        now: NOW,
        coordinator
      });
    } finally {
      dateNow.mockRestore();
    }

    expect(fetchFn).not.toHaveBeenCalled();
    expect(locals).toEqual({ authenticated: false });
    expect(cookies.setCalls).toHaveLength(0);
    expect(cookies.deleteCalls).toEqual([SESSION_COOKIE_NAME]);
  });
});

import { Buffer } from 'node:buffer';

import type { Cookies } from '@sveltejs/kit';
import { describe, expect, it, vi } from 'vitest';

import { restoreIdentitySession } from '../../../hooks.server';
import type { RefreshCoordinator } from './refreshCoordinator';
import { sealSession, SESSION_COOKIE_NAME } from './session';

const KEY_BYTES = new Uint8Array(32).fill(83);
const KEY = Buffer.from(KEY_BYTES).toString('base64');
const NOW = 1_900_000_000_000;
const SESSION_EXPIRES_AT = NOW + 3_600_000;

type SetOptions = Parameters<Cookies['set']>[2];
type DeleteOptions = Parameters<Cookies['delete']>[1];

function cookieJar(initial: string) {
  let current: string | undefined = initial;
  const setCalls: string[] = [];
  const deleteCalls: string[] = [];
  const api: Pick<Cookies, 'get' | 'set' | 'delete'> = {
    get(name: string) {
      return name === SESSION_COOKIE_NAME ? current : undefined;
    },
    set(name: string, value: string, _options: SetOptions) {
      if (name === SESSION_COOKIE_NAME) current = value;
      setCalls.push(name);
    },
    delete(name: string, _options: DeleteOptions) {
      if (name === SESSION_COOKIE_NAME) current = undefined;
      deleteCalls.push(name);
    }
  };
  return { api, current: () => current, setCalls, deleteCalls };
}

function coordinatedEnv() {
  return {
    IDENTITY_ISSUER: 'https://id.example.test',
    IDENTITY_CLIENT_ID: 'memo-web',
    IDENTITY_CLIENT_SECRET: 'test-only-value',
    IDENTITY_SESSION_KEY: KEY,
    IDENTITY_REFRESH_COORDINATION_URL: 'valkey://valkey:6379',
    IDENTITY_BROWSER_SESSION_MAX_AGE_SECONDS: '3600'
  };
}

describe('coordinated refresh result admission', () => {
  it('fails closed instead of adopting a handoff whose access token is already expired', async () => {
    const s0 = await sealSession(
      {
        schemaVersion: 2,
        accessToken: 'old-access',
        accessTokenExpiresAt: NOW,
        refreshToken: 'old-refresh',
        sessionExpiresAt: SESSION_EXPIRES_AT
      },
      KEY_BYTES
    );
    const staleS1 = await sealSession(
      {
        schemaVersion: 2,
        accessToken: 'stale-access',
        accessTokenExpiresAt: NOW,
        refreshToken: 'new-refresh',
        sessionExpiresAt: SESSION_EXPIRES_AT
      },
      KEY_BYTES
    );
    const coordinator: RefreshCoordinator = {
      acquire: vi.fn(async () => ({ kind: 'result' as const, sealedSession: staleS1 })),
      publishSuccess: vi.fn(async () => true),
      completeFailure: vi.fn(async () => {}),
      close: vi.fn(async () => {})
    };
    const cookies = cookieJar(s0);
    const fetchFn = vi.fn() as unknown as typeof fetch;
    const locals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: coordinatedEnv(),
      fetchFn,
      now: NOW,
      coordinator
    });

    expect(fetchFn).not.toHaveBeenCalled();
    expect(locals).toEqual({ authenticated: false });
    expect(cookies.setCalls).toHaveLength(0);
    expect(cookies.deleteCalls).toEqual([SESSION_COOKIE_NAME]);
    expect(cookies.current()).toBeUndefined();
  });
});

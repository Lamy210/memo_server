import { Buffer } from 'node:buffer';

import type { Cookies } from '@sveltejs/kit';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { restoreIdentitySession } from '../../../hooks.server';
import {
  createRefreshResultEnvelope,
  deriveCoordinationId
} from './refreshCoordinationCrypto';
import { createRefreshCoordinator } from './refreshCoordinator';
import { openSession, sealSession, SESSION_COOKIE_NAME } from './session';
import {
  createValkeyRefreshCoordinationStore,
  type RefreshCoordinationStore
} from './valkeyRefreshCoordinationStore';

const TEST_URL = process.env.VALKEY_TEST_URL;
const describeWithValkey = TEST_URL ? describe : describe.skip;
const SESSION_KEY_BYTES = new Uint8Array(32).fill(73);
const SESSION_KEY = Buffer.from(SESSION_KEY_BYTES).toString('base64');
const NOW = 1_900_000_000_000;
const SESSION_EXPIRES_AT = NOW + 3_600_000;
const stores: RefreshCoordinationStore[] = [];

type SetCookieOptions = Parameters<Cookies['set']>[2];
type DeleteCookieOptions = Parameters<Cookies['delete']>[1];

function coordinatedEnv() {
  return {
    IDENTITY_ISSUER: 'https://id.example.test',
    IDENTITY_CLIENT_ID: 'memo-web',
    IDENTITY_CLIENT_SECRET: 'client-secret',
    IDENTITY_SESSION_KEY: SESSION_KEY,
    IDENTITY_REFRESH_COORDINATION_URL: TEST_URL,
    IDENTITY_BROWSER_SESSION_MAX_AGE_SECONDS: '3600'
  };
}

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
  return { api, current: () => current, setCalls, deleteCalls };
}

function realStore(): RefreshCoordinationStore {
  const store = createValkeyRefreshCoordinationStore(new URL(TEST_URL!));
  stores.push(store);
  return store;
}

function strictSingleUseRefresh() {
  let redemptionCount = 0;
  let discoveryCount = 0;
  const fetchFn = vi.fn(async (request: RequestInfo | URL, init?: RequestInit) => {
    const url = String(request);
    if (url.endsWith('/.well-known/openid-configuration')) {
      discoveryCount += 1;
      return Response.json({
        issuer: 'https://id.example.test',
        authorization_endpoint: 'https://id.example.test/authorize',
        token_endpoint: 'https://id.example.test/token',
        code_challenge_methods_supported: ['S256'],
        token_endpoint_auth_methods_supported: ['client_secret_basic']
      });
    }

    const body = new URLSearchParams(String(init?.body));
    expect(body.get('refresh_token')).toBe('refresh-old');
    redemptionCount += 1;
    if (redemptionCount > 1) {
      return Response.json({ error: 'invalid_grant' }, { status: 400 });
    }

    await new Promise((resolve) => setTimeout(resolve, 25));
    return Response.json({
      access_token: 'access-new',
      token_type: 'Bearer',
      expires_in: 120,
      refresh_token: 'refresh-new'
    });
  }) as unknown as typeof fetch;

  return {
    fetchFn,
    redemptionCount: () => redemptionCount,
    discoveryCount: () => discoveryCount
  };
}

async function sealedV2(input: { expiresAt?: number; label?: string } = {}): Promise<string> {
  return sealSession(
    {
      schemaVersion: 2,
      accessToken: input.label ? `access-old-${input.label}` : 'access-old',
      accessTokenExpiresAt: input.expiresAt ?? NOW,
      refreshToken: 'refresh-old',
      sessionExpiresAt: SESSION_EXPIRES_AT
    },
    SESSION_KEY_BYTES
  );
}

afterEach(async () => {
  await Promise.all(stores.splice(0).map((store) => store.close()));
});

describeWithValkey('multi-replica refresh coordination', () => {
  it('redeems one strict rotating refresh token exactly once across two independent replicas', async () => {
    const sealed = await sealedV2();
    const firstCookies = cookieJar(sealed);
    const secondCookies = cookieJar(sealed);
    const firstCoordinator = createRefreshCoordinator({ store: realStore(), sessionKey: SESSION_KEY_BYTES });
    const secondCoordinator = createRefreshCoordinator({ store: realStore(), sessionKey: SESSION_KEY_BYTES });
    const upstream = strictSingleUseRefresh();
    const firstLocals: { authenticated?: boolean; accessToken?: string } = {};
    const secondLocals: { authenticated?: boolean; accessToken?: string } = {};

    await Promise.all([
      restoreIdentitySession({
        cookies: firstCookies.api,
        locals: firstLocals,
        env: coordinatedEnv(),
        fetchFn: upstream.fetchFn,
        now: NOW,
        coordinator: firstCoordinator
      }),
      restoreIdentitySession({
        cookies: secondCookies.api,
        locals: secondLocals,
        env: coordinatedEnv(),
        fetchFn: upstream.fetchFn,
        now: NOW,
        coordinator: secondCoordinator
      })
    ]);

    expect(upstream.redemptionCount()).toBe(1);
    expect(upstream.discoveryCount()).toBe(1);
    expect(firstLocals).toEqual({ authenticated: true, accessToken: 'access-new' });
    expect(secondLocals).toEqual({ authenticated: true, accessToken: 'access-new' });

    const firstSession = await openSession(firstCookies.current()!, SESSION_KEY_BYTES, NOW);
    const secondSession = await openSession(secondCookies.current()!, SESSION_KEY_BYTES, NOW);
    expect(firstSession).toEqual({
      schemaVersion: 2,
      accessToken: 'access-new',
      accessTokenExpiresAt: NOW + 120_000,
      refreshToken: 'refresh-new',
      sessionExpiresAt: SESSION_EXPIRES_AT
    });
    expect(secondSession).toEqual(firstSession);
  });

  it('never redeems S0 after an attempt loses its lease without a result', async () => {
    const sealed = await sealedV2();
    const coordinationId = await deriveCoordinationId(sealed, SESSION_KEY_BYTES);
    const firstStore = realStore();
    await firstStore.acquire({
      coordinationId,
      owner: 'owner-lost',
      attemptTtlMs: 5_000,
      leaseTtlMs: 5_000
    });
    await firstStore.completeFailure({ coordinationId, owner: 'owner-lost' });

    const cookies = cookieJar(sealed);
    const coordinator = createRefreshCoordinator({ store: realStore(), sessionKey: SESSION_KEY_BYTES });
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
    expect(cookies.deleteCalls).toEqual([SESSION_COOKIE_NAME]);
  });

  it('does not allow a later replica to redeem S0 after upstream success but publish failure', async () => {
    const sealed = await sealedV2();
    const delegate = realStore();
    const publishFailingStore: RefreshCoordinationStore = {
      acquire: (input) => delegate.acquire(input),
      publishSuccess: async () => false,
      completeFailure: (input) => delegate.completeFailure(input),
      close: async () => {}
    };
    const firstCoordinator = createRefreshCoordinator({
      store: publishFailingStore,
      sessionKey: SESSION_KEY_BYTES
    });
    const upstream = strictSingleUseRefresh();
    const firstCookies = cookieJar(sealed);
    const firstLocals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({
      cookies: firstCookies.api,
      locals: firstLocals,
      env: coordinatedEnv(),
      fetchFn: upstream.fetchFn,
      now: NOW,
      coordinator: firstCoordinator
    });
    expect(upstream.redemptionCount()).toBe(1);
    expect(firstLocals).toEqual({ authenticated: false });

    const secondCookies = cookieJar(sealed);
    const secondCoordinator = createRefreshCoordinator({ store: realStore(), sessionKey: SESSION_KEY_BYTES });
    const secondFetch = vi.fn() as unknown as typeof fetch;
    const secondLocals: { authenticated?: boolean; accessToken?: string } = {};
    await restoreIdentitySession({
      cookies: secondCookies.api,
      locals: secondLocals,
      env: coordinatedEnv(),
      fetchFn: secondFetch,
      now: NOW,
      coordinator: secondCoordinator
    });

    expect(secondFetch).not.toHaveBeenCalled();
    expect(upstream.redemptionCount()).toBe(1);
    expect(secondLocals).toEqual({ authenticated: false });
  });

  it('rejects a valid result envelope copied into another coordination generation', async () => {
    const source = await sealedV2({ label: 'source' });
    const target = await sealedV2({ label: 'target' });
    const sourceId = await deriveCoordinationId(source, SESSION_KEY_BYTES);
    const targetId = await deriveCoordinationId(target, SESSION_KEY_BYTES);
    const refreshed = await sealedV2({ expiresAt: NOW + 120_000, label: 'refreshed' });
    const sourceEnvelope = await createRefreshResultEnvelope(sourceId, refreshed, SESSION_KEY_BYTES);
    const store = realStore();

    await store.acquire({ coordinationId: targetId, owner: 'target-owner', attemptTtlMs: 5_000, leaseTtlMs: 5_000 });
    await expect(
      store.publishSuccess({
        coordinationId: targetId,
        owner: 'target-owner',
        envelope: sourceEnvelope,
        resultTtlMs: 5_000
      })
    ).resolves.toBe(true);

    const coordinator = createRefreshCoordinator({ store: realStore(), sessionKey: SESSION_KEY_BYTES });
    await expect(coordinator.acquire({ sealedSession: target, replayGuardMs: 5_000 })).resolves.toEqual({
      kind: 'indeterminate',
      reason: 'invalid_result'
    });
  });

  it('does not elect a new leader after a result expires while the attempt marker remains', async () => {
    const sealed = await sealedV2();
    const coordinationId = await deriveCoordinationId(sealed, SESSION_KEY_BYTES);
    const store = realStore();
    const envelope = await createRefreshResultEnvelope(coordinationId, 'sealed-S1', SESSION_KEY_BYTES);

    await store.acquire({ coordinationId, owner: 'owner-a', attemptTtlMs: 5_000, leaseTtlMs: 5_000 });
    await store.publishSuccess({ coordinationId, owner: 'owner-a', envelope, resultTtlMs: 25 });
    await new Promise((resolve) => setTimeout(resolve, 60));

    const coordinator = createRefreshCoordinator({ store: realStore(), sessionKey: SESSION_KEY_BYTES });
    await expect(coordinator.acquire({ sealedSession: sealed, replayGuardMs: 5_000 })).resolves.toEqual({
      kind: 'indeterminate',
      reason: 'leader_lost'
    });
  });

  it('rejects an expired v2 session before Valkey or the upstream identity service is contacted', async () => {
    const expired = await sealSession(
      {
        schemaVersion: 2,
        accessToken: 'access-old',
        accessTokenExpiresAt: NOW,
        refreshToken: 'refresh-old',
        sessionExpiresAt: NOW
      },
      SESSION_KEY_BYTES
    );
    const delegate = realStore();
    let acquireCalls = 0;
    const countingStore: RefreshCoordinationStore = {
      acquire: (input) => {
        acquireCalls += 1;
        return delegate.acquire(input);
      },
      publishSuccess: (input) => delegate.publishSuccess(input),
      completeFailure: (input) => delegate.completeFailure(input),
      close: async () => {}
    };
    const coordinator = createRefreshCoordinator({ store: countingStore, sessionKey: SESSION_KEY_BYTES });
    const cookies = cookieJar(expired);
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

    expect(acquireCalls).toBe(0);
    expect(fetchFn).not.toHaveBeenCalled();
    expect(locals).toEqual({ authenticated: false });
  });
});

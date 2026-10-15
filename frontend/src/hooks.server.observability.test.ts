import { Buffer } from 'node:buffer';

import type { Cookies } from '@sveltejs/kit';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { restoreIdentitySession } from './hooks.server';
import type {
  RefreshCoordinationDecision,
  RefreshCoordinator
} from '$lib/server/identity/refreshCoordinator';
import { sealSession, SESSION_COOKIE_NAME } from '$lib/server/identity/session';

const SESSION_KEY_BYTES = new Uint8Array(32).fill(23);
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

  return { api, setCalls, deleteCalls, current: () => current };
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

async function v2Session(input: {
  accessTokenExpiresAt?: number;
  sessionExpiresAt?: number;
  accessToken?: string;
  refreshToken?: string;
} = {}): Promise<string> {
  return sealSession(
    {
      schemaVersion: 2,
      accessToken: input.accessToken ?? 'access-old',
      accessTokenExpiresAt: input.accessTokenExpiresAt ?? NOW + 10_000,
      refreshToken: input.refreshToken ?? 'refresh-old',
      sessionExpiresAt: input.sessionExpiresAt ?? SESSION_EXPIRES_AT
    },
    SESSION_KEY_BYTES
  );
}

function discoveryAndRefreshFetch(input: {
  failRefresh?: boolean;
  afterRefreshResponse?: () => void;
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
    input.afterRefreshResponse?.();
    if (input.failRefresh) return new Response('raw upstream failure', { status: 503 });
    return Response.json({
      access_token: 'access-new',
      token_type: 'Bearer',
      expires_in: 120,
      refresh_token: 'refresh-new'
    });
  }) as unknown as typeof fetch;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('shared refresh coordination observability', () => {
  it('records leader success exactly once', async () => {
    const sealed = await v2Session();
    const cookies = fakeCookies(sealed);
    const coordination = fakeCoordinator({ kind: 'leader', owner: 'owner-a' });
    const recordOutcome = vi.fn();
    const locals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: coordinatedEnv(),
      fetchFn: discoveryAndRefreshFetch(),
      now: NOW,
      coordinator: coordination.coordinator,
      refreshOutcomeRecorder: recordOutcome
    });

    expect(recordOutcome).toHaveBeenCalledTimes(1);
    expect(recordOutcome).toHaveBeenCalledWith('leader_success');
    expect(locals).toEqual({ authenticated: true, accessToken: 'access-new' });
  });

  it('records follower success exactly once', async () => {
    const sealed = await v2Session();
    const sealedResult = await v2Session({
      accessToken: 'access-new',
      refreshToken: 'refresh-new',
      accessTokenExpiresAt: NOW + 120_000
    });
    const cookies = fakeCookies(sealed);
    const coordination = fakeCoordinator({ kind: 'result', sealedSession: sealedResult });
    const recordOutcome = vi.fn();
    const locals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: coordinatedEnv(),
      fetchFn: vi.fn() as unknown as typeof fetch,
      now: NOW,
      coordinator: coordination.coordinator,
      refreshOutcomeRecorder: recordOutcome
    });

    expect(recordOutcome).toHaveBeenCalledTimes(1);
    expect(recordOutcome).toHaveBeenCalledWith('follower_success');
    expect(locals).toEqual({ authenticated: true, accessToken: 'access-new' });
  });

  it('records unavailable continuation when both session and token remain valid', async () => {
    const sealed = await v2Session({ accessTokenExpiresAt: NOW + 1 });
    const cookies = fakeCookies(sealed);
    const coordination = fakeCoordinator({ kind: 'unavailable' });
    const recordOutcome = vi.fn();
    const locals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: coordinatedEnv(),
      fetchFn: vi.fn() as unknown as typeof fetch,
      now: NOW,
      coordinator: coordination.coordinator,
      refreshOutcomeRecorder: recordOutcome
    });

    expect(recordOutcome).toHaveBeenCalledTimes(1);
    expect(recordOutcome).toHaveBeenCalledWith('unavailable_continue');
    expect(locals).toEqual({ authenticated: true, accessToken: 'access-old' });
  });

  it('records unavailable fail-closed when only the access token is expired', async () => {
    const sealed = await v2Session({ accessTokenExpiresAt: NOW });
    const cookies = fakeCookies(sealed);
    const coordination = fakeCoordinator({ kind: 'unavailable' });
    const recordOutcome = vi.fn();
    const locals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: coordinatedEnv(),
      fetchFn: vi.fn() as unknown as typeof fetch,
      now: NOW,
      coordinator: coordination.coordinator,
      refreshOutcomeRecorder: recordOutcome
    });

    expect(recordOutcome).toHaveBeenCalledTimes(1);
    expect(recordOutcome).toHaveBeenCalledWith('unavailable_fail_closed');
    expect(locals).toEqual({ authenticated: false });
    expect(cookies.deleteCalls).toHaveLength(1);
  });

  it.each(['leader_lost', 'wait_timeout', 'invalid_result'] as const)(
    'records indeterminate reason %s directly',
    async (reason) => {
      const sealed = await v2Session();
      const cookies = fakeCookies(sealed);
      const coordination = fakeCoordinator({ kind: 'indeterminate', reason });
      const recordOutcome = vi.fn();
      const locals: { authenticated?: boolean; accessToken?: string } = {};

      await restoreIdentitySession({
        cookies: cookies.api,
        locals,
        env: coordinatedEnv(),
        fetchFn: vi.fn() as unknown as typeof fetch,
        now: NOW,
        coordinator: coordination.coordinator,
        refreshOutcomeRecorder: recordOutcome
      });

      expect(recordOutcome).toHaveBeenCalledTimes(1);
      expect(recordOutcome).toHaveBeenCalledWith(reason);
      expect(locals).toEqual({ authenticated: false });
      expect(cookies.deleteCalls).toHaveLength(1);
    }
  );

  it('records invalid_result for a decryptable follower result that changes fixed session expiry', async () => {
    const sealed = await v2Session();
    const invalidResult = await v2Session({
      accessToken: 'access-new',
      refreshToken: 'refresh-new',
      accessTokenExpiresAt: NOW + 120_000,
      sessionExpiresAt: SESSION_EXPIRES_AT + 1
    });
    const cookies = fakeCookies(sealed);
    const coordination = fakeCoordinator({ kind: 'result', sealedSession: invalidResult });
    const recordOutcome = vi.fn();
    const locals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: coordinatedEnv(),
      fetchFn: vi.fn() as unknown as typeof fetch,
      now: NOW,
      coordinator: coordination.coordinator,
      refreshOutcomeRecorder: recordOutcome
    });

    expect(recordOutcome).toHaveBeenCalledTimes(1);
    expect(recordOutcome).toHaveBeenCalledWith('invalid_result');
    expect(locals).toEqual({ authenticated: false });
  });

  it('prioritizes session_expired at the exact unavailable decision boundary', async () => {
    const decisionBoundary = NOW + 1_000;
    const sealed = await v2Session({
      sessionExpiresAt: decisionBoundary,
      accessTokenExpiresAt: NOW + 2_000
    });
    const cookies = fakeCookies(sealed);
    const coordination = fakeCoordinator({ kind: 'unavailable' });
    coordination.acquire.mockImplementation(async () => {
      vi.spyOn(Date, 'now').mockReturnValue(decisionBoundary);
      return { kind: 'unavailable' } as const;
    });
    const recordOutcome = vi.fn();
    const locals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: coordinatedEnv(),
      fetchFn: vi.fn() as unknown as typeof fetch,
      now: NOW,
      coordinator: coordination.coordinator,
      refreshOutcomeRecorder: recordOutcome
    });

    expect(coordination.acquire).toHaveBeenCalledTimes(1);
    expect(recordOutcome).toHaveBeenCalledTimes(1);
    expect(recordOutcome).toHaveBeenCalledWith('session_expired');
    expect(locals).toEqual({ authenticated: false });
  });

  it('prioritizes original session expiry over an invalid follower result', async () => {
    const sealed = await v2Session();
    const cookies = fakeCookies(sealed);
    const coordination = fakeCoordinator({ kind: 'result', sealedSession: 'v1.invalid.invalid' });
    coordination.acquire.mockImplementation(async () => {
      vi.spyOn(Date, 'now').mockReturnValue(SESSION_EXPIRES_AT);
      return { kind: 'result', sealedSession: 'v1.invalid.invalid' } as const;
    });
    const recordOutcome = vi.fn();
    const locals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: coordinatedEnv(),
      fetchFn: vi.fn() as unknown as typeof fetch,
      now: NOW,
      coordinator: coordination.coordinator,
      refreshOutcomeRecorder: recordOutcome
    });

    expect(recordOutcome).toHaveBeenCalledTimes(1);
    expect(recordOutcome).toHaveBeenCalledWith('session_expired');
    expect(locals).toEqual({ authenticated: false });
  });

  it('records session_expired when successful leader work finishes after the fixed lifetime', async () => {
    const sealed = await v2Session();
    const cookies = fakeCookies(sealed);
    const coordination = fakeCoordinator({ kind: 'leader', owner: 'owner-a' });
    const recordOutcome = vi.fn();
    const locals: { authenticated?: boolean; accessToken?: string } = {};
    const fetchFn = discoveryAndRefreshFetch({
      afterRefreshResponse: () => vi.spyOn(Date, 'now').mockReturnValue(SESSION_EXPIRES_AT)
    });

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: coordinatedEnv(),
      fetchFn,
      now: NOW,
      coordinator: coordination.coordinator,
      refreshOutcomeRecorder: recordOutcome
    });

    expect(recordOutcome).toHaveBeenCalledTimes(1);
    expect(recordOutcome).toHaveBeenCalledWith('session_expired');
    expect(coordination.completeFailure).toHaveBeenCalledTimes(1);
    expect(locals).toEqual({ authenticated: false });
  });

  it('keeps upstream_failure precedence when refresh fails after the fixed lifetime elapses', async () => {
    const sealed = await v2Session();
    const cookies = fakeCookies(sealed);
    const coordination = fakeCoordinator({ kind: 'leader', owner: 'owner-a' });
    const recordOutcome = vi.fn();
    const locals: { authenticated?: boolean; accessToken?: string } = {};
    const fetchFn = discoveryAndRefreshFetch({
      failRefresh: true,
      afterRefreshResponse: () => vi.spyOn(Date, 'now').mockReturnValue(SESSION_EXPIRES_AT)
    });

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: coordinatedEnv(),
      fetchFn,
      now: NOW,
      coordinator: coordination.coordinator,
      refreshOutcomeRecorder: recordOutcome
    });

    expect(recordOutcome).toHaveBeenCalledTimes(1);
    expect(recordOutcome).toHaveBeenCalledWith('upstream_failure');
    expect(coordination.completeFailure).toHaveBeenCalledTimes(1);
    expect(locals).toEqual({ authenticated: false });
  });

  it('records publish_failure exactly once', async () => {
    const sealed = await v2Session();
    const cookies = fakeCookies(sealed);
    const coordination = fakeCoordinator({ kind: 'leader', owner: 'owner-a' }, false);
    const recordOutcome = vi.fn();
    const locals: { authenticated?: boolean; accessToken?: string } = {};

    await restoreIdentitySession({
      cookies: cookies.api,
      locals,
      env: coordinatedEnv(),
      fetchFn: discoveryAndRefreshFetch(),
      now: NOW,
      coordinator: coordination.coordinator,
      refreshOutcomeRecorder: recordOutcome
    });

    expect(recordOutcome).toHaveBeenCalledTimes(1);
    expect(recordOutcome).toHaveBeenCalledWith('publish_failure');
    expect(coordination.completeFailure).toHaveBeenCalledTimes(1);
    expect(locals).toEqual({ authenticated: false });
  });

  it('contains recorder failure without changing leader authentication success', async () => {
    const sealed = await v2Session();
    const cookies = fakeCookies(sealed);
    const coordination = fakeCoordinator({ kind: 'leader', owner: 'owner-a' });
    const locals: { authenticated?: boolean; accessToken?: string } = {};
    const recordOutcome = vi.fn(() => {
      throw new Error('telemetry sink failed');
    });

    await expect(
      restoreIdentitySession({
        cookies: cookies.api,
        locals,
        env: coordinatedEnv(),
        fetchFn: discoveryAndRefreshFetch(),
        now: NOW,
        coordinator: coordination.coordinator,
        refreshOutcomeRecorder: recordOutcome
      })
    ).resolves.toBeUndefined();

    expect(recordOutcome).toHaveBeenCalledTimes(1);
    expect(locals).toEqual({ authenticated: true, accessToken: 'access-new' });
    expect(cookies.setCalls).toHaveLength(1);
  });

  it('does not record outside shared coordination paths', async () => {
    const recordLive = vi.fn();
    const liveSealed = await v2Session({ accessTokenExpiresAt: NOW + 120_000 });
    await restoreIdentitySession({
      cookies: fakeCookies(liveSealed).api,
      locals: {},
      env: coordinatedEnv(),
      fetchFn: vi.fn() as unknown as typeof fetch,
      now: NOW,
      coordinator: fakeCoordinator({ kind: 'leader', owner: 'owner-a' }).coordinator,
      refreshOutcomeRecorder: recordLive
    });
    expect(recordLive).not.toHaveBeenCalled();

    const recordLegacy = vi.fn();
    const legacySealed = await sealSession(
      { accessToken: 'old', accessTokenExpiresAt: NOW, refreshToken: 'refresh-old' },
      SESSION_KEY_BYTES
    );
    await restoreIdentitySession({
      cookies: fakeCookies(legacySealed).api,
      locals: {},
      env: coordinatedEnv(),
      fetchFn: vi.fn() as unknown as typeof fetch,
      now: NOW,
      coordinator: fakeCoordinator({ kind: 'leader', owner: 'owner-a' }).coordinator,
      refreshOutcomeRecorder: recordLegacy
    });
    expect(recordLegacy).not.toHaveBeenCalled();

    const recordLocal = vi.fn();
    const localSealed = await sealSession(
      { accessToken: 'old', accessTokenExpiresAt: NOW, refreshToken: 'refresh-old' },
      SESSION_KEY_BYTES
    );
    await restoreIdentitySession({
      cookies: fakeCookies(localSealed).api,
      locals: {},
      env: identityEnv(),
      fetchFn: discoveryAndRefreshFetch(),
      now: NOW,
      refreshOutcomeRecorder: recordLocal
    });
    expect(recordLocal).not.toHaveBeenCalled();
  });
});

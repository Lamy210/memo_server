import { dev } from '$app/environment';
import { env } from '$env/dynamic/private';
import type { Cookies, Handle } from '@sveltejs/kit';

import {
  readIdentityConfig,
  type IdentityConfig
} from '$lib/server/identity/config';
import {
  createRefreshCoordinator,
  type RefreshCoordinator
} from '$lib/server/identity/refreshCoordinator';
import { discoverIdentity, refreshAccessToken } from '$lib/server/identity/oidcClient';
import {
  isIdentitySessionV2,
  openSession,
  sealSession,
  SESSION_COOKIE_NAME,
  type IdentitySession
} from '$lib/server/identity/session';
import { createValkeyRefreshCoordinationStore } from '$lib/server/identity/valkeyRefreshCoordinationStore';
import { applyFrontendSecurityHeaders } from '$lib/server/securityHeaders';

const ACCESS_TOKEN_REFRESH_WINDOW_MS = 30_000;
const ATTEMPT_CLEANUP_MARGIN_MS = 60_000;
const MIN_ATTEMPT_REPLAY_GUARD_MS = 1_000;

type RefreshedIdentitySession = {
  session: IdentitySession;
  sealed: string;
};

const refreshFlights = new Map<string, Promise<RefreshedIdentitySession>>();

let sharedRefreshCoordinator:
  | {
      url: string;
      sessionKey: Uint8Array;
      coordinator: RefreshCoordinator;
    }
  | undefined;

export type IdentityLocals = {
  authenticated?: boolean;
  accessToken?: string;
};

type IdentitySessionRestoreInput = {
  cookies: Pick<Cookies, 'get' | 'set' | 'delete'>;
  locals: IdentityLocals;
  env: Record<string, string | undefined>;
  fetchFn: typeof fetch;
  now?: number;
  coordinator?: RefreshCoordinator;
};

function clearIdentityLocals(locals: IdentityLocals): void {
  locals.authenticated = false;
  delete locals.accessToken;
}

function authenticate(locals: IdentityLocals, accessToken: string): void {
  locals.authenticated = true;
  locals.accessToken = accessToken;
}

function deleteSessionCookie(cookies: Pick<Cookies, 'delete'>): void {
  cookies.delete(SESSION_COOKIE_NAME, { path: '/' });
}

function setSessionCookie(cookies: Pick<Cookies, 'set'>, value: string): void {
  cookies.set(SESSION_COOKIE_NAME, value, {
    path: '/',
    httpOnly: true,
    secure: true,
    sameSite: 'lax'
  });
}

function sameBytes(left: Uint8Array, right: Uint8Array): boolean {
  if (left.length !== right.length) return false;
  let difference = 0;
  for (let index = 0; index < left.length; index += 1) {
    difference |= left[index] ^ right[index];
  }
  return difference === 0;
}

function productionRefreshCoordinator(config: IdentityConfig): RefreshCoordinator {
  if (!config.refreshCoordinationUrl) {
    throw new Error('Refresh coordination is not configured');
  }

  const url = config.refreshCoordinationUrl.toString();
  const existing = sharedRefreshCoordinator;
  if (existing && existing.url === url && sameBytes(existing.sessionKey, config.sessionKey)) {
    return existing.coordinator;
  }

  const store = createValkeyRefreshCoordinationStore(config.refreshCoordinationUrl);
  const coordinator = createRefreshCoordinator({ store, sessionKey: config.sessionKey });
  sharedRefreshCoordinator = {
    url,
    sessionKey: config.sessionKey.slice(),
    coordinator
  };

  if (existing) {
    void existing.coordinator.close().catch(() => undefined);
  }

  return coordinator;
}

function refreshedSession(
  session: IdentitySession,
  accessToken: string,
  accessTokenExpiresAt: number,
  refreshToken: string
): IdentitySession {
  if (isIdentitySessionV2(session)) {
    return {
      schemaVersion: 2,
      accessToken,
      accessTokenExpiresAt,
      refreshToken,
      sessionExpiresAt: session.sessionExpiresAt
    };
  }

  return { accessToken, accessTokenExpiresAt, refreshToken };
}

async function refreshIdentitySession(
  sealed: string,
  session: IdentitySession,
  config: IdentityConfig,
  fetchFn: typeof fetch,
  now: number
): Promise<RefreshedIdentitySession> {
  const existing = refreshFlights.get(sealed);
  if (existing) return existing;

  const flight = (async () => {
    const metadata = await discoverIdentity(config, fetchFn);
    const refreshed = await refreshAccessToken(metadata, config, fetchFn, {
      refreshToken: session.refreshToken
    });
    const nextSession = refreshedSession(
      session,
      refreshed.accessToken,
      now + Math.floor(refreshed.expiresIn * 1000),
      refreshed.refreshToken ?? session.refreshToken
    );

    return {
      session: nextSession,
      sealed: await sealSession(nextSession, config.sessionKey)
    };
  })();

  refreshFlights.set(sealed, flight);
  try {
    return await flight;
  } finally {
    if (refreshFlights.get(sealed) === flight) {
      refreshFlights.delete(sealed);
    }
  }
}

async function restoreWithSharedCoordination(input: {
  sealed: string;
  session: IdentitySession;
  config: IdentityConfig;
  cookies: Pick<Cookies, 'set' | 'delete'>;
  locals: IdentityLocals;
  fetchFn: typeof fetch;
  now: number;
  coordinator?: RefreshCoordinator;
}): Promise<void> {
  const { sealed, session, config, cookies, locals, fetchFn, now } = input;
  if (!isIdentitySessionV2(session)) {
    deleteSessionCookie(cookies);
    return;
  }

  const coordinator = input.coordinator ?? productionRefreshCoordinator(config);
  const replayGuardMs =
    Math.max(MIN_ATTEMPT_REPLAY_GUARD_MS, session.sessionExpiresAt - now) +
    ATTEMPT_CLEANUP_MARGIN_MS;
  const decision = await coordinator.acquire({ sealedSession: sealed, replayGuardMs });
  const decisionNow = Math.max(now, Date.now());

  if (decision.kind === 'unavailable') {
    if (session.accessTokenExpiresAt > decisionNow) {
      authenticate(locals, session.accessToken);
    } else {
      deleteSessionCookie(cookies);
    }
    return;
  }

  if (decision.kind === 'indeterminate') {
    deleteSessionCookie(cookies);
    return;
  }

  if (decision.kind === 'result') {
    const resultSession = await openSession(decision.sealedSession, config.sessionKey, decisionNow);
    if (
      !resultSession ||
      !isIdentitySessionV2(resultSession) ||
      resultSession.sessionExpiresAt !== session.sessionExpiresAt ||
      resultSession.accessTokenExpiresAt <= decisionNow
    ) {
      deleteSessionCookie(cookies);
      return;
    }

    setSessionCookie(cookies, decision.sealedSession);
    authenticate(locals, resultSession.accessToken);
    return;
  }

  try {
    const metadata = await discoverIdentity(config, fetchFn);
    const refreshed = await refreshAccessToken(metadata, config, fetchFn, {
      refreshToken: session.refreshToken
    });
    const refreshedAt = Math.max(now, Date.now());
    if (session.sessionExpiresAt <= refreshedAt) {
      await coordinator.completeFailure({ sealedSession: sealed, owner: decision.owner });
      deleteSessionCookie(cookies);
      return;
    }

    const nextSession = refreshedSession(
      session,
      refreshed.accessToken,
      refreshedAt + Math.floor(refreshed.expiresIn * 1000),
      refreshed.refreshToken ?? session.refreshToken
    );
    const nextSealed = await sealSession(nextSession, config.sessionKey);
    const published = await coordinator.publishSuccess({
      sealedSession: sealed,
      owner: decision.owner,
      refreshedSealedSession: nextSealed
    });
    if (!published) {
      await coordinator.completeFailure({ sealedSession: sealed, owner: decision.owner });
      deleteSessionCookie(cookies);
      return;
    }

    setSessionCookie(cookies, nextSealed);
    authenticate(locals, nextSession.accessToken);
  } catch {
    await coordinator.completeFailure({ sealedSession: sealed, owner: decision.owner });
    deleteSessionCookie(cookies);
    clearIdentityLocals(locals);
  }
}

export async function restoreIdentitySession({
  cookies,
  locals,
  env: identityEnv,
  fetchFn,
  now = Date.now(),
  coordinator
}: IdentitySessionRestoreInput): Promise<void> {
  clearIdentityLocals(locals);

  const sealed = cookies.get(SESSION_COOKIE_NAME);
  if (!sealed) return;

  let config;
  try {
    config = readIdentityConfig(identityEnv);
  } catch {
    deleteSessionCookie(cookies);
    return;
  }

  const session = await openSession(sealed, config.sessionKey, now);
  if (!session) {
    deleteSessionCookie(cookies);
    return;
  }

  if (config.refreshCoordinationUrl && !isIdentitySessionV2(session)) {
    deleteSessionCookie(cookies);
    return;
  }

  if (session.accessTokenExpiresAt > now + ACCESS_TOKEN_REFRESH_WINDOW_MS) {
    authenticate(locals, session.accessToken);
    return;
  }

  if (config.refreshCoordinationUrl) {
    await restoreWithSharedCoordination({
      sealed,
      session,
      config,
      cookies,
      locals,
      fetchFn,
      now,
      coordinator
    });
    return;
  }

  try {
    const refreshed = await refreshIdentitySession(sealed, session, config, fetchFn, now);
    setSessionCookie(cookies, refreshed.sealed);
    authenticate(locals, refreshed.session.accessToken);
  } catch {
    deleteSessionCookie(cookies);
    clearIdentityLocals(locals);
  }
}

export const handle: Handle = async ({ event, resolve }) => {
  clearIdentityLocals(event.locals);

  if (!dev) {
    await restoreIdentitySession({
      cookies: event.cookies,
      locals: event.locals,
      env,
      fetchFn: event.fetch
    });
  }

  const response = await resolve(event);
  applyFrontendSecurityHeaders(response.headers);
  return response;
};

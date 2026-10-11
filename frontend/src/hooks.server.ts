import { dev } from '$app/environment';
import { env } from '$env/dynamic/private';
import type { Cookies, Handle } from '@sveltejs/kit';

import {
  readIdentityConfig,
  type IdentityConfig
} from '$lib/server/identity/config';
import { discoverIdentity, refreshAccessToken } from '$lib/server/identity/oidcClient';
import {
  openSession,
  sealSession,
  SESSION_COOKIE_NAME,
  type IdentitySession
} from '$lib/server/identity/session';
import { applyFrontendSecurityHeaders } from '$lib/server/securityHeaders';

const ACCESS_TOKEN_REFRESH_WINDOW_MS = 30_000;

type RefreshedIdentitySession = {
  session: IdentitySession;
  sealed: string;
};

const refreshFlights = new Map<string, Promise<RefreshedIdentitySession>>();

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
};

function clearIdentityLocals(locals: IdentityLocals): void {
  locals.authenticated = false;
  delete locals.accessToken;
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
    const nextSession: IdentitySession = {
      accessToken: refreshed.accessToken,
      accessTokenExpiresAt: now + Math.floor(refreshed.expiresIn * 1000),
      refreshToken: refreshed.refreshToken ?? session.refreshToken
    };

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

export async function restoreIdentitySession({
  cookies,
  locals,
  env: identityEnv,
  fetchFn,
  now = Date.now()
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

  const session = await openSession(sealed, config.sessionKey);
  if (!session) {
    deleteSessionCookie(cookies);
    return;
  }

  if (session.accessTokenExpiresAt > now + ACCESS_TOKEN_REFRESH_WINDOW_MS) {
    locals.authenticated = true;
    locals.accessToken = session.accessToken;
    return;
  }

  try {
    const refreshed = await refreshIdentitySession(sealed, session, config, fetchFn, now);
    setSessionCookie(cookies, refreshed.sealed);
    locals.authenticated = true;
    locals.accessToken = refreshed.session.accessToken;
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

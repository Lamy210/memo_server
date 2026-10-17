import { Buffer } from 'node:buffer';

import type { Cookies } from '@sveltejs/kit';

import { resolveSafeReturnTarget } from '$lib/security/returnTarget';
import { readIdentityConfig } from './config';
import {
  createAuthorizationRequest,
  createPkceChallenge,
  discoverIdentity,
  exchangeAuthorizationCode,
  IdentityClientError
} from './oidcClient';
import {
  LOGIN_TRANSACTION_COOKIE_NAME,
  openLoginTransaction,
  sealLoginTransaction,
  sealSession,
  SESSION_COOKIE_NAME,
  type IdentitySession
} from './session';

const LOGIN_TRANSACTION_TTL_MS = 10 * 60 * 1000;
const LOGIN_TRANSACTION_MAX_AGE_SECONDS = 600;

type StartLoginInput = {
  url: URL;
  cookies: Pick<Cookies, 'set'>;
  fetchFn: typeof fetch;
  identityEnv: Record<string, string | undefined>;
  now?: number;
  randomBytes?: (size: number) => Uint8Array;
};

type CompleteLoginInput = {
  url: URL;
  cookies: Pick<Cookies, 'get' | 'set' | 'delete'>;
  fetchFn: typeof fetch;
  identityEnv: Record<string, string | undefined>;
  now?: number;
};

type LogoutInput = {
  url: URL;
  cookies: Pick<Cookies, 'delete'>;
};

type CallbackErrorCode = 'invalid_callback' | 'access_denied' | 'authentication_failed';

type OAuthCallbackParameters = {
  state: string;
  code: string | null;
  authorizationError: string | null;
};

function randomBase64Url(size: number, randomBytes: (size: number) => Uint8Array): string {
  return Buffer.from(randomBytes(size))
    .toString('base64')
    .replace(/=/g, '')
    .replace(/\+/g, '-')
    .replace(/\//g, '_');
}

function secureRandomBytes(size: number): Uint8Array {
  return crypto.getRandomValues(new Uint8Array(size));
}

function callbackFailure(status: number, error: CallbackErrorCode): Response {
  return Response.json(
    { error },
    { status, headers: { 'Cache-Control': 'no-store' } }
  );
}

function readOAuthCallbackParameters(searchParams: URLSearchParams): OAuthCallbackParameters | null {
  const states = searchParams.getAll('state');
  const codes = searchParams.getAll('code');
  const errors = searchParams.getAll('error');

  if (states.length !== 1 || !states[0]) return null;

  if (codes.length === 1 && errors.length === 0 && codes[0]) {
    return { state: states[0], code: codes[0], authorizationError: null };
  }

  if (errors.length === 1 && codes.length === 0 && errors[0]) {
    return { state: states[0], code: null, authorizationError: errors[0] };
  }

  return null;
}

export async function startLogin({
  url,
  cookies,
  fetchFn,
  identityEnv,
  now = Date.now(),
  randomBytes = secureRandomBytes
}: StartLoginInput): Promise<Response> {
  try {
    const config = readIdentityConfig(identityEnv);
    const metadata = await discoverIdentity(config, fetchFn);
    const state = randomBase64Url(32, randomBytes);
    const codeVerifier = randomBase64Url(32, randomBytes);
    const codeChallenge = await createPkceChallenge(codeVerifier);
    const returnTarget = resolveSafeReturnTarget(url.searchParams.get('return_to'));
    const redirectUri = new URL('/auth/callback', url.origin).toString();
    const transaction = await sealLoginTransaction(
      {
        state,
        codeVerifier,
        returnTarget,
        expiresAt: now + LOGIN_TRANSACTION_TTL_MS
      },
      config.sessionKey
    );

    cookies.set(LOGIN_TRANSACTION_COOKIE_NAME, transaction, {
      path: '/',
      httpOnly: true,
      secure: true,
      sameSite: 'lax',
      maxAge: LOGIN_TRANSACTION_MAX_AGE_SECONDS
    });

    const authorization = createAuthorizationRequest(metadata, {
      clientId: config.clientId,
      redirectUri,
      state,
      codeChallenge
    });

    return new Response(null, {
      status: 303,
      headers: {
        Location: authorization.url.toString(),
        'Cache-Control': 'no-store'
      }
    });
  } catch {
    return Response.json(
      { error: 'temporarily_unavailable' },
      { status: 502, headers: { 'Cache-Control': 'no-store' } }
    );
  }
}

export async function completeLogin({
  url,
  cookies,
  fetchFn,
  identityEnv,
  now = Date.now()
}: CompleteLoginInput): Promise<Response> {
  const sealedTransaction = cookies.get(LOGIN_TRANSACTION_COOKIE_NAME);
  if (sealedTransaction) {
    cookies.delete(LOGIN_TRANSACTION_COOKIE_NAME, { path: '/' });
  }
  if (!sealedTransaction) return callbackFailure(400, 'invalid_callback');

  let config;
  try {
    config = readIdentityConfig(identityEnv);
  } catch {
    return callbackFailure(502, 'authentication_failed');
  }

  const transaction = await openLoginTransaction(sealedTransaction, config.sessionKey, now);
  const callback = readOAuthCallbackParameters(url.searchParams);
  if (!transaction || !callback || callback.state !== transaction.state) {
    return callbackFailure(400, 'invalid_callback');
  }

  if (callback.authorizationError) {
    return callback.authorizationError === 'access_denied'
      ? callbackFailure(401, 'access_denied')
      : callbackFailure(502, 'authentication_failed');
  }

  const code = callback.code;
  if (!code) {
    return callbackFailure(400, 'invalid_callback');
  }

  try {
    const metadata = await discoverIdentity(config, fetchFn);
    const tokenSet = await exchangeAuthorizationCode(metadata, config, fetchFn, {
      code,
      codeVerifier: transaction.codeVerifier,
      redirectUri: new URL('/auth/callback', url.origin).toString()
    });
    const baseSession = {
      accessToken: tokenSet.accessToken,
      accessTokenExpiresAt: now + Math.floor(tokenSet.expiresIn * 1000),
      refreshToken: tokenSet.refreshToken!
    };
    const session: IdentitySession = config.browserSessionMaxAgeSeconds
      ? {
          schemaVersion: 2,
          ...baseSession,
          sessionExpiresAt: now + config.browserSessionMaxAgeSeconds * 1000
        }
      : baseSession;
    const sealedSession = await sealSession(session, config.sessionKey);

    cookies.set(SESSION_COOKIE_NAME, sealedSession, {
      path: '/',
      httpOnly: true,
      secure: true,
      sameSite: 'lax'
    });

    return new Response(null, {
      status: 303,
      headers: { Location: transaction.returnTarget, 'Cache-Control': 'no-store' }
    });
  } catch (error) {
    cookies.delete(SESSION_COOKIE_NAME, { path: '/' });
    if (error instanceof IdentityClientError && error.kind === 'access_denied') {
      return callbackFailure(401, 'access_denied');
    }
    return callbackFailure(502, 'authentication_failed');
  }
}

export function logoutSession({ url, cookies }: LogoutInput): Response {
  cookies.delete(SESSION_COOKIE_NAME, { path: '/' });
  const returnTarget = resolveSafeReturnTarget(url.searchParams.get('return_to'));
  return new Response(null, {
    status: 303,
    headers: { Location: returnTarget, 'Cache-Control': 'no-store' }
  });
}

export function getSessionStatus(authenticated: boolean): Response {
  return Response.json(
    { authenticated },
    { status: 200, headers: { 'Cache-Control': 'no-store' } }
  );
}

import { env } from '$env/dynamic/private';
import type { Cookies, RequestHandler } from '@sveltejs/kit';

import { readIdentityConfig } from '$lib/server/identity/config';
import {
  discoverIdentity,
  exchangeAuthorizationCode,
  IdentityClientError
} from '$lib/server/identity/oidcClient';
import {
  LOGIN_TRANSACTION_COOKIE_NAME,
  openLoginTransaction,
  sealSession,
  SESSION_COOKIE_NAME
} from '$lib/server/identity/session';

type CompleteLoginInput = {
  url: URL;
  cookies: Pick<Cookies, 'get' | 'set' | 'delete'>;
  fetchFn: typeof fetch;
  identityEnv: Record<string, string | undefined>;
  now?: number;
};

function callbackFailure(status: number): Response {
  return Response.json(
    { error: status === 400 ? 'invalid_callback' : 'authentication_failed' },
    { status, headers: { 'Cache-Control': 'no-store' } }
  );
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
  if (!sealedTransaction) return callbackFailure(400);

  let config;
  try {
    config = readIdentityConfig(identityEnv);
  } catch {
    return callbackFailure(502);
  }

  const transaction = await openLoginTransaction(sealedTransaction, config.sessionKey, now);
  const code = url.searchParams.get('code');
  const state = url.searchParams.get('state');
  if (!transaction || !code || !state || state !== transaction.state) {
    return callbackFailure(400);
  }

  try {
    const metadata = await discoverIdentity(config, fetchFn);
    const tokenSet = await exchangeAuthorizationCode(metadata, config, fetchFn, {
      code,
      codeVerifier: transaction.codeVerifier,
      redirectUri: new URL('/auth/callback', url.origin).toString()
    });
    const sealedSession = await sealSession(
      {
        accessToken: tokenSet.accessToken,
        accessTokenExpiresAt: now + Math.floor(tokenSet.expiresIn * 1000),
        refreshToken: tokenSet.refreshToken!
      },
      config.sessionKey
    );

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
      return callbackFailure(401);
    }
    return callbackFailure(502);
  }
}

export const GET: RequestHandler = async ({ url, cookies, fetch }) =>
  completeLogin({ url, cookies, fetchFn: fetch, identityEnv: env });

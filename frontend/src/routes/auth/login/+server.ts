import { Buffer } from 'node:buffer';

import { env } from '$env/dynamic/private';
import type { Cookies, RequestHandler } from '@sveltejs/kit';

import { resolveSafeReturnTarget } from '$lib/security/returnTarget';
import { readIdentityConfig } from '$lib/server/identity/config';
import {
  createAuthorizationRequest,
  createPkceChallenge,
  discoverIdentity
} from '$lib/server/identity/oidcClient';
import {
  LOGIN_TRANSACTION_COOKIE_NAME,
  sealLoginTransaction
} from '$lib/server/identity/session';

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

export const GET: RequestHandler = async ({ url, cookies, fetch }) =>
  startLogin({ url, cookies, fetchFn: fetch, identityEnv: env });

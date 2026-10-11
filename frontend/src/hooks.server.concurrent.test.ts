import { Buffer } from 'node:buffer';

import type { Cookies } from '@sveltejs/kit';
import { describe, expect, it, vi } from 'vitest';

import { restoreIdentitySession } from './hooks.server';
import { sealSession, SESSION_COOKIE_NAME } from '$lib/server/identity/session';

const KEY_BYTES = new Uint8Array(32).fill(31);
const KEY = Buffer.from(KEY_BYTES).toString('base64');
const NOW = 1_900_000_000_000;

type SetCookieOptions = Parameters<Cookies['set']>[2];
type DeleteCookieOptions = Parameters<Cookies['delete']>[1];

function cookiesWith(value: string): Pick<Cookies, 'get' | 'set' | 'delete'> {
  let current: string | undefined = value;
  return {
    get(name: string) {
      return name === SESSION_COOKIE_NAME ? current : undefined;
    },
    set(name: string, next: string, _options: SetCookieOptions) {
      if (name === SESSION_COOKIE_NAME) current = next;
    },
    delete(name: string, _options: DeleteCookieOptions) {
      if (name === SESSION_COOKIE_NAME) current = undefined;
    }
  };
}

const env = {
  IDENTITY_ISSUER: 'https://id.example.test',
  IDENTITY_CLIENT_ID: 'memo-web',
  IDENTITY_CLIENT_SECRET: 'client-secret',
  IDENTITY_SESSION_KEY: KEY
};

describe('concurrent identity refresh', () => {
  it('singleflights requests carrying the same sealed refresh session', async () => {
    const sealed = await sealSession(
      { accessToken: 'old', accessTokenExpiresAt: NOW, refreshToken: 'refresh-old' },
      KEY_BYTES
    );
    const fetchFn = vi.fn(async (request: RequestInfo | URL) => {
      const url = String(request);
      if (url.endsWith('/.well-known/openid-configuration')) {
        await Promise.resolve();
        return Response.json({
          issuer: 'https://id.example.test',
          authorization_endpoint: 'https://id.example.test/authorize',
          token_endpoint: 'https://id.example.test/token',
          code_challenge_methods_supported: ['S256'],
          token_endpoint_auth_methods_supported: ['client_secret_basic']
        });
      }
      await Promise.resolve();
      return Response.json({
        access_token: 'new',
        token_type: 'Bearer',
        expires_in: 120,
        refresh_token: 'refresh-new'
      });
    }) as unknown as typeof fetch;

    const firstLocals: { authenticated?: boolean; accessToken?: string } = {};
    const secondLocals: { authenticated?: boolean; accessToken?: string } = {};

    await Promise.all([
      restoreIdentitySession({
        cookies: cookiesWith(sealed),
        locals: firstLocals,
        env,
        fetchFn,
        now: NOW
      }),
      restoreIdentitySession({
        cookies: cookiesWith(sealed),
        locals: secondLocals,
        env,
        fetchFn,
        now: NOW
      })
    ]);

    expect(fetchFn).toHaveBeenCalledTimes(2);
    expect(firstLocals).toEqual({ authenticated: true, accessToken: 'new' });
    expect(secondLocals).toEqual({ authenticated: true, accessToken: 'new' });
  });
});
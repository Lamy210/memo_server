import { Buffer } from 'node:buffer';

import { describe, expect, it, vi } from 'vitest';

import type { IdentityConfig } from './config';
import {
  createAuthorizationRequest,
  createPkceChallenge,
  discoverIdentity,
  exchangeAuthorizationCode,
  IdentityClientError,
  refreshAccessToken
} from './oidcClient';

const config: IdentityConfig = {
  issuer: new URL('https://id.example.test'),
  clientId: 'memo-web',
  clientSecret: 'client-secret',
  sessionKey: new Uint8Array(32)
};

function discovery(overrides: Record<string, unknown> = {}) {
  return {
    issuer: 'https://id.example.test/',
    authorization_endpoint: 'https://id.example.test/authorize',
    token_endpoint: 'https://id.example.test/token',
    end_session_endpoint: 'https://id.example.test/logout',
    code_challenge_methods_supported: ['S256'],
    token_endpoint_auth_methods_supported: ['client_secret_basic'],
    ...overrides
  };
}

describe('discoverIdentity', () => {
  it('loads exact issuer metadata without following redirects', async () => {
    const fetchFn = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      expect(String(input)).toBe('https://id.example.test/.well-known/openid-configuration');
      expect(init?.redirect).toBe('error');
      return Response.json(discovery());
    }) as unknown as typeof fetch;

    const metadata = await discoverIdentity(config, fetchFn);

    expect(metadata.issuer).toBe('https://id.example.test/');
    expect(metadata.authorizationEndpoint.toString()).toBe('https://id.example.test/authorize');
    expect(metadata.tokenEndpoint.toString()).toBe('https://id.example.test/token');
  });

  it('rejects issuer mismatch, insecure endpoints, or missing S256 support', async () => {
    for (const payload of [
      discovery({ issuer: 'https://other.example.test/' }),
      discovery({ token_endpoint: 'http://id.example.test/token' }),
      discovery({ code_challenge_methods_supported: ['plain'] })
    ]) {
      const fetchFn = vi.fn(async () => Response.json(payload)) as unknown as typeof fetch;
      await expect(discoverIdentity(config, fetchFn)).rejects.toBeInstanceOf(IdentityClientError);
    }
  });
});

describe('PKCE authorization request', () => {
  it('uses S256 and the RFC 7636 challenge derivation', async () => {
    const verifier = 'dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk';
    const challenge = await createPkceChallenge(verifier);
    expect(challenge).toBe('E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM');

    const metadata = await discoverIdentity(
      config,
      vi.fn(async () => Response.json(discovery())) as unknown as typeof fetch
    );
    const request = createAuthorizationRequest(metadata, {
      clientId: 'memo-web',
      redirectUri: 'https://memo.example.test/auth/callback',
      state: 'state-123',
      codeChallenge: challenge
    });

    expect(request.url.origin + request.url.pathname).toBe('https://id.example.test/authorize');
    expect(request.url.searchParams.get('response_type')).toBe('code');
    expect(request.url.searchParams.get('scope')).toBe('openid offline_access');
    expect(request.url.searchParams.get('client_id')).toBe('memo-web');
    expect(request.url.searchParams.get('redirect_uri')).toBe(
      'https://memo.example.test/auth/callback'
    );
    expect(request.url.searchParams.get('state')).toBe('state-123');
    expect(request.url.searchParams.get('code_challenge')).toBe(challenge);
    expect(request.url.searchParams.get('code_challenge_method')).toBe('S256');
  });
});

describe('token endpoint', () => {
  it('exchanges a code with PKCE and requires an initial refresh token', async () => {
    const metadata = await discoverIdentity(
      config,
      vi.fn(async () => Response.json(discovery())) as unknown as typeof fetch
    );
    const fetchFn = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      expect(String(input)).toBe('https://id.example.test/token');
      expect(init?.method).toBe('POST');
      expect(init?.redirect).toBe('error');
      expect(new Headers(init?.headers).get('authorization')).toBe(
        `Basic ${Buffer.from('memo-web:client-secret').toString('base64')}`
      );
      const body = new URLSearchParams(String(init?.body));
      expect(body.get('grant_type')).toBe('authorization_code');
      expect(body.get('code')).toBe('code-1');
      expect(body.get('code_verifier')).toBe('verifier-1');
      expect(body.get('redirect_uri')).toBe('https://memo.example.test/auth/callback');
      return Response.json({
        access_token: 'access-1',
        token_type: 'Bearer',
        expires_in: 300,
        refresh_token: 'refresh-1',
        id_token: 'ignored-for-memo-authorization'
      });
    }) as unknown as typeof fetch;

    await expect(
      exchangeAuthorizationCode(metadata, config, fetchFn, {
        code: 'code-1',
        codeVerifier: 'verifier-1',
        redirectUri: 'https://memo.example.test/auth/callback'
      })
    ).resolves.toEqual({ accessToken: 'access-1', expiresIn: 300, refreshToken: 'refresh-1' });

    const missingRefresh = vi.fn(async () =>
      Response.json({ access_token: 'access-1', token_type: 'Bearer', expires_in: 300 })
    ) as unknown as typeof fetch;
    await expect(
      exchangeAuthorizationCode(metadata, config, missingRefresh, {
        code: 'code-1',
        codeVerifier: 'verifier-1',
        redirectUri: 'https://memo.example.test/auth/callback'
      })
    ).rejects.toBeInstanceOf(IdentityClientError);
  });

  it('supports refresh-token rotation and absence of a replacement token', async () => {
    const metadata = await discoverIdentity(
      config,
      vi.fn(async () => Response.json(discovery())) as unknown as typeof fetch
    );

    const rotated = await refreshAccessToken(
      metadata,
      config,
      vi.fn(async () =>
        Response.json({
          access_token: 'access-2',
          token_type: 'Bearer',
          expires_in: 120,
          refresh_token: 'refresh-2'
        })
      ) as unknown as typeof fetch,
      { refreshToken: 'refresh-1' }
    );
    expect(rotated).toEqual({ accessToken: 'access-2', expiresIn: 120, refreshToken: 'refresh-2' });

    const retained = await refreshAccessToken(
      metadata,
      config,
      vi.fn(async () =>
        Response.json({ access_token: 'access-3', token_type: 'Bearer', expires_in: 120 })
      ) as unknown as typeof fetch,
      { refreshToken: 'refresh-2' }
    );
    expect(retained).toEqual({ accessToken: 'access-3', expiresIn: 120 });
  });

  it('does not expose raw upstream error bodies', async () => {
    const metadata = await discoverIdentity(
      config,
      vi.fn(async () => Response.json(discovery())) as unknown as typeof fetch
    );
    const fetchFn = vi.fn(async () => new Response('super-secret-upstream-error', { status: 500 })) as unknown as typeof fetch;

    try {
      await refreshAccessToken(metadata, config, fetchFn, { refreshToken: 'refresh-1' });
      throw new Error('expected refresh to fail');
    } catch (error) {
      expect(error).toBeInstanceOf(IdentityClientError);
      expect(String(error)).not.toContain('super-secret-upstream-error');
    }
  });
});

import { Buffer } from 'node:buffer';

import { expect, it, vi } from 'vitest';

import type { IdentityConfig } from './config';
import { exchangeAuthorizationCode, type OidcMetadata } from './oidcClient';

it('form-encodes client credentials before client_secret_basic authentication', async () => {
  const config: IdentityConfig = {
    issuer: new URL('https://id.example.test'),
    issuerIdentifier: 'https://id.example.test/',
    clientId: 'client id!',
    clientSecret: 's ecret!',
    sessionKey: new Uint8Array(32)
  };
  const metadata: OidcMetadata = {
    issuer: 'https://id.example.test/',
    authorizationEndpoint: new URL('https://id.example.test/authorize'),
    tokenEndpoint: new URL('https://id.example.test/token'),
    tokenEndpointAuthMethodsSupported: ['client_secret_basic']
  };
  const fetchFn = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
    expect(new Headers(init?.headers).get('authorization')).toBe(
      `Basic ${Buffer.from('client+id%21:s+ecret%21').toString('base64')}`
    );
    return Response.json({
      access_token: 'access-1',
      token_type: 'Bearer',
      expires_in: 300,
      refresh_token: 'refresh-1'
    });
  }) as unknown as typeof fetch;

  await exchangeAuthorizationCode(metadata, config, fetchFn, {
    code: 'code-1',
    codeVerifier: 'verifier-1',
    redirectUri: 'https://memo.example.test/auth/callback'
  });
});

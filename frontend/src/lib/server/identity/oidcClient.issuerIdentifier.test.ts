import { Buffer } from 'node:buffer';

import { expect, it, vi } from 'vitest';

import { readIdentityConfig } from './config';
import { discoverIdentity } from './oidcClient';

it('compares discovery issuer against the exact configured issuer identifier', async () => {
  const config = readIdentityConfig({
    IDENTITY_ISSUER: 'https://id.example.test',
    IDENTITY_CLIENT_ID: 'memo-web',
    IDENTITY_CLIENT_SECRET: 'client-secret',
    IDENTITY_SESSION_KEY: Buffer.alloc(32, 9).toString('base64')
  });
  const fetchFn = vi.fn(async () =>
    Response.json({
      issuer: 'https://id.example.test',
      authorization_endpoint: 'https://id.example.test/authorize',
      token_endpoint: 'https://id.example.test/token',
      code_challenge_methods_supported: ['S256'],
      token_endpoint_auth_methods_supported: ['client_secret_basic']
    })
  ) as unknown as typeof fetch;

  const metadata = await discoverIdentity(config, fetchFn);

  expect(metadata.issuer).toBe('https://id.example.test');
});

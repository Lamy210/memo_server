import { Buffer } from 'node:buffer';

import { describe, expect, it } from 'vitest';

import { readIdentityConfig } from './config';

const SESSION_KEY = Buffer.alloc(32, 7).toString('base64');

function validEnv(overrides: Record<string, string | undefined> = {}) {
  return {
    IDENTITY_ISSUER: 'https://id.example.test',
    IDENTITY_CLIENT_ID: 'memo-web',
    IDENTITY_CLIENT_SECRET: 'client-secret',
    IDENTITY_SESSION_KEY: SESSION_KEY,
    ...overrides
  };
}

describe('readIdentityConfig', () => {
  it('accepts a provider-neutral HTTPS issuer and exact 32-byte session key', () => {
    const config = readIdentityConfig(validEnv());

    expect(config.issuer.toString()).toBe('https://id.example.test/');
    expect(config.clientId).toBe('memo-web');
    expect(config.clientSecret).toBe('client-secret');
    expect(config.sessionKey).toHaveLength(32);
  });

  it.each([
    'http://id.example.test',
    'http://127.0.0.1:19090',
    'http://localhost:19090',
    'http://[::1]:19090',
    'ftp://id.example.test',
    'https://user:secret@id.example.test',
    'https://id.example.test?tenant=a',
    'https://id.example.test#fragment'
  ])('rejects non-HTTPS or unsafe issuer %s', (issuer) => {
    expect(() => readIdentityConfig(validEnv({ IDENTITY_ISSUER: issuer }))).toThrow();
  });

  it.each([
    ['IDENTITY_CLIENT_ID', ''],
    ['IDENTITY_CLIENT_ID', '   '],
    ['IDENTITY_CLIENT_SECRET', ''],
    ['IDENTITY_CLIENT_SECRET', '   ']
  ] as const)('rejects blank %s', (name, value) => {
    expect(() => readIdentityConfig(validEnv({ [name]: value }))).toThrow();
  });

  it('preserves non-blank client credentials exactly', () => {
    const config = readIdentityConfig(
      validEnv({
        IDENTITY_CLIENT_ID: ' memo-web ',
        IDENTITY_CLIENT_SECRET: ' client-secret '
      })
    );

    expect(config.clientId).toBe(' memo-web ');
    expect(config.clientSecret).toBe(' client-secret ');
  });

  it.each(['not-base64!', Buffer.alloc(31).toString('base64'), Buffer.alloc(33).toString('base64')])(
    'rejects malformed or non-32-byte session key',
    (value) => {
      expect(() => readIdentityConfig(validEnv({ IDENTITY_SESSION_KEY: value }))).toThrow();
    }
  );
});

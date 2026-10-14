import { describe, expect, it } from 'vitest';

import {
  LOGIN_TRANSACTION_COOKIE_NAME,
  MAX_SEALED_COOKIE_BYTES,
  openLoginTransaction,
  openSession,
  sealLoginTransaction,
  sealSession,
  SESSION_COOKIE_NAME,
  IdentitySessionTooLargeError
} from './session';

const KEY = new Uint8Array(32).fill(9);
const OTHER_KEY = new Uint8Array(32).fill(10);

const session = {
  accessToken: 'access-token',
  accessTokenExpiresAt: 2_000_000_000_000,
  refreshToken: 'refresh-token'
};

describe('sealed identity session', () => {
  it('round trips through AES-GCM and uses a random nonce per seal', async () => {
    const first = await sealSession(session, KEY);
    const second = await sealSession(session, KEY);

    expect(first).not.toBe(second);
    await expect(openSession(first, KEY)).resolves.toEqual(session);
    await expect(openSession(second, KEY)).resolves.toEqual(session);
  });

  it('rejects tampering and the wrong key without exposing plaintext', async () => {
    const sealed = await sealSession(session, KEY);
    const replacement = sealed.endsWith('A') ? 'B' : 'A';
    const tampered = `${sealed.slice(0, -1)}${replacement}`;

    await expect(openSession(tampered, KEY)).resolves.toBeNull();
    await expect(openSession(sealed, OTHER_KEY)).resolves.toBeNull();
    expect(sealed).not.toContain('refresh-token');
    expect(sealed).not.toContain('access-token');
  });

  it('rejects a sealed payload that would exceed the single-cookie budget', async () => {
    const oversized = { ...session, accessToken: 'x'.repeat(MAX_SEALED_COOKIE_BYTES) };
    await expect(sealSession(oversized, KEY)).rejects.toBeInstanceOf(IdentitySessionTooLargeError);
  });

  it('round-trips a bounded v2 identity session before expiry', async () => {
    const now = 1_900_000_000_000;
    const bounded = {
      schemaVersion: 2 as const,
      accessToken: 'access-v2',
      accessTokenExpiresAt: now + 120_000,
      refreshToken: 'refresh-v2',
      sessionExpiresAt: now + 3_600_000
    };

    const sealed = await sealSession(bounded, KEY);
    await expect(openSession(sealed, KEY, now)).resolves.toEqual(bounded);
  });

  it('rejects a v2 identity session at sessionExpiresAt', async () => {
    const now = 1_900_000_000_000;
    const bounded = {
      schemaVersion: 2 as const,
      accessToken: 'access-v2',
      accessTokenExpiresAt: now + 120_000,
      refreshToken: 'refresh-v2',
      sessionExpiresAt: now
    };

    const sealed = await sealSession(bounded, KEY);
    await expect(openSession(sealed, KEY, now)).resolves.toBeNull();
  });

  it('continues to open legacy v1 sessions', async () => {
    const sealed = await sealSession(session, KEY);
    await expect(openSession(sealed, KEY, 1_900_000_000_000)).resolves.toEqual(session);
  });

  it.each([Number.NaN, Number.POSITIVE_INFINITY, -1, 'tomorrow'])(
    'rejects malformed v2 lifetime metadata %s',
    async (sessionExpiresAt) => {
      const malformed = {
        schemaVersion: 2 as const,
        accessToken: 'access-v2',
        accessTokenExpiresAt: 1_900_000_120_000,
        refreshToken: 'refresh-v2',
        sessionExpiresAt
      };

      await expect(sealSession(malformed as never, KEY)).rejects.toThrow('Invalid identity session');
    }
  );
});

describe('sealed login transaction', () => {
  it('round trips a live transaction and rejects an expired transaction', async () => {
    const now = 1_900_000_000_000;
    const live = {
      state: 'state-1',
      codeVerifier: 'v'.repeat(43),
      returnTarget: '/memos',
      expiresAt: now + 60_000
    };
    const expired = { ...live, expiresAt: now - 1 };

    await expect(openLoginTransaction(await sealLoginTransaction(live, KEY), KEY, now)).resolves.toEqual(
      live
    );
    await expect(
      openLoginTransaction(await sealLoginTransaction(expired, KEY), KEY, now)
    ).resolves.toBeNull();
  });
});

describe('production cookie contract', () => {
  it('uses __Host names and a bounded single-cookie payload', () => {
    expect(SESSION_COOKIE_NAME).toBe('__Host-schnee_session');
    expect(LOGIN_TRANSACTION_COOKIE_NAME).toBe('__Host-schnee_login');
    expect(MAX_SEALED_COOKIE_BYTES).toBeLessThanOrEqual(3800);
  });
});

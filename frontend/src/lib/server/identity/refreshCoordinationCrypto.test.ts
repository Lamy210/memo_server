import { describe, expect, it } from 'vitest';

import {
  createRefreshResultEnvelope,
  deriveCoordinationId,
  verifyRefreshResultEnvelope
} from './refreshCoordinationCrypto';

const encoder = new TextEncoder();
const SESSION_KEY = new Uint8Array(32).fill(41);
const KEY_ID_INFO = 'memo_server/identity-refresh/v1/key-id';
const RESULT_MAC_INFO = 'memo_server/identity-refresh/v1/result-mac';

function hex(bytes: ArrayBuffer): string {
  return Array.from(new Uint8Array(bytes), (byte) => byte.toString(16).padStart(2, '0')).join('');
}

async function deriveReferenceHmacKey(info: string): Promise<CryptoKey> {
  const root = await crypto.subtle.importKey('raw', SESSION_KEY, 'HKDF', false, ['deriveKey']);
  return crypto.subtle.deriveKey(
    {
      name: 'HKDF',
      hash: 'SHA-256',
      salt: new Uint8Array(0),
      info: encoder.encode(info)
    },
    root,
    { name: 'HMAC', hash: 'SHA-256', length: 256 },
    false,
    ['sign', 'verify']
  );
}

async function referenceHmac(info: string, message: string): Promise<string> {
  const key = await deriveReferenceHmacKey(info);
  return hex(await crypto.subtle.sign('HMAC', key, encoder.encode(message)));
}

describe('refresh coordination cryptographic boundary', () => {
  it('derives a keyed opaque coordination id with the dedicated key-id domain', async () => {
    const sealed = 'v1.opaque-session-generation-zero';
    const id = await deriveCoordinationId(sealed, SESSION_KEY);

    expect(id).toMatch(/^[0-9a-f]{64}$/);
    expect(id).not.toContain(sealed);
    expect(id).toBe(await referenceHmac(KEY_ID_INFO, sealed));
    expect(id).not.toBe(await referenceHmac(RESULT_MAC_INFO, sealed));
  });

  it('changes the coordination id when the sealed session generation changes', async () => {
    await expect(deriveCoordinationId('S0', SESSION_KEY)).resolves.not.toBe(
      await deriveCoordinationId('S1', SESSION_KEY)
    );
  });

  it('authenticates a sealed refresh result with the dedicated result-mac domain', async () => {
    const coordinationId = await deriveCoordinationId('S0', SESSION_KEY);
    const envelope = await createRefreshResultEnvelope(coordinationId, 'S1', SESSION_KEY);
    const parsed = JSON.parse(envelope) as { version: number; sealedSession: string; mac: string };

    expect(parsed).toMatchObject({ version: 1, sealedSession: 'S1' });
    expect(parsed.mac).toBe(
      await referenceHmac(RESULT_MAC_INFO, `v1\0${coordinationId}\0S1`)
    );
    expect(parsed.mac).not.toBe(
      await referenceHmac(KEY_ID_INFO, `v1\0${coordinationId}\0S1`)
    );
    await expect(verifyRefreshResultEnvelope(coordinationId, envelope, SESSION_KEY)).resolves.toBe(
      'S1'
    );
  });

  it('rejects a valid result envelope copied to another coordination generation', async () => {
    const id0 = await deriveCoordinationId('S0', SESSION_KEY);
    const id1 = await deriveCoordinationId('other-S0', SESSION_KEY);
    const envelope = await createRefreshResultEnvelope(id0, 'S1', SESSION_KEY);

    await expect(verifyRefreshResultEnvelope(id1, envelope, SESSION_KEY)).resolves.toBeNull();
  });

  it('rejects malformed or tampered result envelopes without throwing', async () => {
    const id = await deriveCoordinationId('S0', SESSION_KEY);
    const envelope = await createRefreshResultEnvelope(id, 'S1', SESSION_KEY);
    const parsed = JSON.parse(envelope) as { version: number; sealedSession: string; mac: string };

    await expect(
      verifyRefreshResultEnvelope(
        id,
        JSON.stringify({ ...parsed, sealedSession: 'attacker-session' }),
        SESSION_KEY
      )
    ).resolves.toBeNull();
    await expect(verifyRefreshResultEnvelope(id, '{not-json', SESSION_KEY)).resolves.toBeNull();
    await expect(
      verifyRefreshResultEnvelope(id, JSON.stringify({ ...parsed, version: 2 }), SESSION_KEY)
    ).resolves.toBeNull();
  });
});

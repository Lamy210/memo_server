import { Buffer } from 'node:buffer';

import { describe, expect, it } from 'vitest';

import {
  createRefreshResultEnvelope,
  deriveCoordinationId,
  verifyRefreshResultEnvelope
} from './refreshCoordinationCrypto';

const encoder = new TextEncoder();
const decoder = new TextDecoder();
const SESSION_KEY = new Uint8Array(32).fill(41);
const KEY_ID_INFO = 'memo_server/identity-refresh/v1/key-id';
const RESULT_ENCRYPTION_INFO = 'memo_server/identity-refresh/v1/result-encryption';
const RESULT_MAC_INFO = 'memo_server/identity-refresh/v1/result-mac';

function hex(bytes: ArrayBuffer): string {
  return Array.from(new Uint8Array(bytes), (byte) => byte.toString(16).padStart(2, '0')).join('');
}

function base64UrlDecode(value: string): Uint8Array {
  const padding = '='.repeat((4 - (value.length % 4)) % 4);
  return new Uint8Array(
    Buffer.from(value.replace(/-/g, '+').replace(/_/g, '/') + padding, 'base64')
  );
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

async function deriveReferenceEncryptionKey(): Promise<CryptoKey> {
  const root = await crypto.subtle.importKey('raw', SESSION_KEY, 'HKDF', false, ['deriveKey']);
  return crypto.subtle.deriveKey(
    {
      name: 'HKDF',
      hash: 'SHA-256',
      salt: new Uint8Array(0),
      info: encoder.encode(RESULT_ENCRYPTION_INFO)
    },
    root,
    { name: 'AES-GCM', length: 256 },
    false,
    ['decrypt']
  );
}

async function referenceHmac(info: string, message: string): Promise<string> {
  const key = await deriveReferenceHmacKey(info);
  return hex(await crypto.subtle.sign('HMAC', key, encoder.encode(message)));
}

type ParsedEnvelope = {
  version: number;
  nonce: string;
  ciphertext: string;
  mac: string;
};

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

  it('encrypts the browser cookie before storing a result handoff and authenticates its ciphertext', async () => {
    const coordinationId = await deriveCoordinationId('S0', SESSION_KEY);
    const sealedSession = 'v1.browser-cookie-value-that-must-not-be-stored-verbatim';
    const envelope = await createRefreshResultEnvelope(coordinationId, sealedSession, SESSION_KEY);
    const parsed = JSON.parse(envelope) as ParsedEnvelope;

    expect(parsed.version).toBe(1);
    expect(parsed.nonce).toMatch(/^[A-Za-z0-9_-]+$/);
    expect(parsed.ciphertext).toMatch(/^[A-Za-z0-9_-]+$/);
    expect(envelope).not.toContain(sealedSession);
    expect(envelope).not.toContain('sealedSession');
    expect(parsed.mac).toBe(
      await referenceHmac(
        RESULT_MAC_INFO,
        `v1\0${coordinationId}\0${parsed.nonce}\0${parsed.ciphertext}`
      )
    );

    const decryptionKey = await deriveReferenceEncryptionKey();
    const plaintext = await crypto.subtle.decrypt(
      {
        name: 'AES-GCM',
        iv: base64UrlDecode(parsed.nonce),
        additionalData: encoder.encode(`v1\0${coordinationId}`)
      },
      decryptionKey,
      base64UrlDecode(parsed.ciphertext)
    );
    expect(decoder.decode(plaintext)).toBe(sealedSession);
    await expect(verifyRefreshResultEnvelope(coordinationId, envelope, SESSION_KEY)).resolves.toBe(
      sealedSession
    );
  });

  it('uses fresh encryption nonces while preserving the same verified result', async () => {
    const coordinationId = await deriveCoordinationId('S0', SESSION_KEY);
    const first = await createRefreshResultEnvelope(coordinationId, 'S1', SESSION_KEY);
    const second = await createRefreshResultEnvelope(coordinationId, 'S1', SESSION_KEY);

    expect(first).not.toBe(second);
    await expect(verifyRefreshResultEnvelope(coordinationId, first, SESSION_KEY)).resolves.toBe('S1');
    await expect(verifyRefreshResultEnvelope(coordinationId, second, SESSION_KEY)).resolves.toBe('S1');
  });

  it('rejects a valid result envelope copied to another coordination generation', async () => {
    const id0 = await deriveCoordinationId('S0', SESSION_KEY);
    const id1 = await deriveCoordinationId('other-S0', SESSION_KEY);
    const envelope = await createRefreshResultEnvelope(id0, 'S1', SESSION_KEY);

    await expect(verifyRefreshResultEnvelope(id1, envelope, SESSION_KEY)).resolves.toBeNull();
  });

  it('rejects malformed or tampered encrypted result envelopes without throwing', async () => {
    const id = await deriveCoordinationId('S0', SESSION_KEY);
    const envelope = await createRefreshResultEnvelope(id, 'S1', SESSION_KEY);
    const parsed = JSON.parse(envelope) as ParsedEnvelope;

    const replacement = parsed.ciphertext.endsWith('A') ? 'B' : 'A';
    await expect(
      verifyRefreshResultEnvelope(
        id,
        JSON.stringify({ ...parsed, ciphertext: `${parsed.ciphertext.slice(0, -1)}${replacement}` }),
        SESSION_KEY
      )
    ).resolves.toBeNull();
    await expect(verifyRefreshResultEnvelope(id, '{not-json', SESSION_KEY)).resolves.toBeNull();
    await expect(
      verifyRefreshResultEnvelope(id, JSON.stringify({ ...parsed, version: 2 }), SESSION_KEY)
    ).resolves.toBeNull();
  });
});

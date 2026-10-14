import { Buffer } from 'node:buffer';

const encoder = new TextEncoder();
const decoder = new TextDecoder('utf-8', { fatal: true });

const KEY_ID_INFO = 'memo_server/identity-refresh/v1/key-id';
const RESULT_ENCRYPTION_INFO = 'memo_server/identity-refresh/v1/result-encryption';
const RESULT_MAC_INFO = 'memo_server/identity-refresh/v1/result-mac';
const RESULT_VERSION = 1;
const RESULT_NONCE_BYTES = 12;

type RefreshResultEnvelope = {
  version: 1;
  nonce: string;
  ciphertext: string;
  mac: string;
};

function bytesToHex(bytes: ArrayBuffer): string {
  return Array.from(new Uint8Array(bytes), (byte) => byte.toString(16).padStart(2, '0')).join('');
}

function hexToBytes(value: string): Uint8Array | null {
  if (!/^[0-9a-f]{64}$/.test(value)) return null;
  const bytes = new Uint8Array(value.length / 2);
  for (let index = 0; index < value.length; index += 2) {
    bytes[index / 2] = Number.parseInt(value.slice(index, index + 2), 16);
  }
  return bytes;
}

function base64UrlEncode(bytes: Uint8Array): string {
  return Buffer.from(bytes)
    .toString('base64')
    .replace(/=/g, '')
    .replace(/\+/g, '-')
    .replace(/\//g, '_');
}

function base64UrlDecode(value: string): Uint8Array | null {
  if (!/^[A-Za-z0-9_-]+$/.test(value)) return null;
  const padding = '='.repeat((4 - (value.length % 4)) % 4);
  try {
    const bytes = new Uint8Array(
      Buffer.from(value.replace(/-/g, '+').replace(/_/g, '/') + padding, 'base64')
    );
    if (bytes.length === 0 || base64UrlEncode(bytes) !== value) return null;
    return bytes;
  } catch {
    return null;
  }
}

async function importHkdfRoot(sessionKey: Uint8Array): Promise<CryptoKey> {
  if (sessionKey.length !== 32) throw new Error('Invalid identity session key');
  return crypto.subtle.importKey('raw', sessionKey, 'HKDF', false, ['deriveKey']);
}

async function deriveHmacKey(sessionKey: Uint8Array, info: string): Promise<CryptoKey> {
  const root = await importHkdfRoot(sessionKey);
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

async function deriveResultEncryptionKey(
  sessionKey: Uint8Array,
  usages: KeyUsage[]
): Promise<CryptoKey> {
  const root = await importHkdfRoot(sessionKey);
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
    usages
  );
}

async function signHex(key: CryptoKey, message: string): Promise<string> {
  return bytesToHex(await crypto.subtle.sign('HMAC', key, encoder.encode(message)));
}

function resultAad(coordinationId: string): Uint8Array {
  return encoder.encode(`v1\0${coordinationId}`);
}

function resultMacInput(coordinationId: string, nonce: string, ciphertext: string): string {
  return `v1\0${coordinationId}\0${nonce}\0${ciphertext}`;
}

export async function deriveCoordinationId(
  sealedSession: string,
  sessionKey: Uint8Array
): Promise<string> {
  const key = await deriveHmacKey(sessionKey, KEY_ID_INFO);
  return signHex(key, sealedSession);
}

export async function createRefreshResultEnvelope(
  coordinationId: string,
  sealedSession: string,
  sessionKey: Uint8Array
): Promise<string> {
  const nonceBytes = crypto.getRandomValues(new Uint8Array(RESULT_NONCE_BYTES));
  const encryptionKey = await deriveResultEncryptionKey(sessionKey, ['encrypt']);
  const ciphertextBytes = new Uint8Array(
    await crypto.subtle.encrypt(
      {
        name: 'AES-GCM',
        iv: nonceBytes,
        additionalData: resultAad(coordinationId)
      },
      encryptionKey,
      encoder.encode(sealedSession)
    )
  );
  const nonce = base64UrlEncode(nonceBytes);
  const ciphertext = base64UrlEncode(ciphertextBytes);
  const macKey = await deriveHmacKey(sessionKey, RESULT_MAC_INFO);
  const mac = await signHex(macKey, resultMacInput(coordinationId, nonce, ciphertext));
  const envelope: RefreshResultEnvelope = {
    version: RESULT_VERSION,
    nonce,
    ciphertext,
    mac
  };
  return JSON.stringify(envelope);
}

export async function verifyRefreshResultEnvelope(
  coordinationId: string,
  envelope: string,
  sessionKey: Uint8Array
): Promise<string | null> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(envelope);
  } catch {
    return null;
  }

  if (!parsed || typeof parsed !== 'object') return null;
  const candidate = parsed as Record<string, unknown>;
  if (
    candidate.version !== RESULT_VERSION ||
    typeof candidate.nonce !== 'string' ||
    typeof candidate.ciphertext !== 'string' ||
    typeof candidate.mac !== 'string'
  ) {
    return null;
  }

  const nonce = base64UrlDecode(candidate.nonce);
  const ciphertext = base64UrlDecode(candidate.ciphertext);
  const signature = hexToBytes(candidate.mac);
  if (!nonce || nonce.length !== RESULT_NONCE_BYTES || !ciphertext || !signature) return null;

  try {
    const macKey = await deriveHmacKey(sessionKey, RESULT_MAC_INFO);
    const valid = await crypto.subtle.verify(
      'HMAC',
      macKey,
      signature,
      encoder.encode(resultMacInput(coordinationId, candidate.nonce, candidate.ciphertext))
    );
    if (!valid) return null;

    const encryptionKey = await deriveResultEncryptionKey(sessionKey, ['decrypt']);
    const plaintext = await crypto.subtle.decrypt(
      {
        name: 'AES-GCM',
        iv: nonce,
        additionalData: resultAad(coordinationId)
      },
      encryptionKey,
      ciphertext
    );
    const sealedSession = decoder.decode(plaintext);
    return sealedSession.length > 0 ? sealedSession : null;
  } catch {
    return null;
  }
}

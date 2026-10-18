import { Buffer } from 'node:buffer';

export const SESSION_COOKIE_NAME = '__Host-schnee_session';
export const LOGIN_TRANSACTION_COOKIE_NAME = '__Host-schnee_login';
export const MAX_SEALED_COOKIE_BYTES = 3800;

const VERSION = 'v1';
const IV_BYTES = 12;
const encoder = new TextEncoder();
const decoder = new TextDecoder();

export type IdentitySessionV1 = {
  accessToken: string;
  accessTokenExpiresAt: number;
  refreshToken: string;
};

export type IdentitySessionV2 = {
  schemaVersion: 2;
  accessToken: string;
  accessTokenExpiresAt: number;
  refreshToken: string;
  sessionExpiresAt: number;
};

export type IdentitySession = IdentitySessionV1 | IdentitySessionV2;

export type LoginTransaction = {
  state: string;
  codeVerifier: string;
  returnTarget: string;
  expiresAt: number;
};

export class IdentitySessionTooLargeError extends Error {
  constructor() {
    super('Identity session exceeds cookie budget');
    this.name = 'IdentitySessionTooLargeError';
  }
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
    const bytes = Buffer.from(value.replace(/-/g, '+').replace(/_/g, '/') + padding, 'base64');
    if (bytes.length === 0 || base64UrlEncode(bytes) !== value) return null;
    return new Uint8Array(bytes);
  } catch {
    return null;
  }
}

async function importAesKey(key: Uint8Array, usage: KeyUsage): Promise<CryptoKey> {
  if (key.length !== 32) throw new Error('Invalid identity session key');
  return crypto.subtle.importKey('raw', key, { name: 'AES-GCM' }, false, [usage]);
}

async function seal(value: unknown, key: Uint8Array): Promise<string> {
  const iv = crypto.getRandomValues(new Uint8Array(IV_BYTES));
  const cryptoKey = await importAesKey(key, 'encrypt');
  const plaintext = encoder.encode(JSON.stringify(value));
  const ciphertext = new Uint8Array(
    await crypto.subtle.encrypt(
      { name: 'AES-GCM', iv, additionalData: encoder.encode(VERSION) },
      cryptoKey,
      plaintext
    )
  );
  const encoded = `${VERSION}.${base64UrlEncode(iv)}.${base64UrlEncode(ciphertext)}`;

  if (Buffer.byteLength(encoded, 'utf8') > MAX_SEALED_COOKIE_BYTES) {
    throw new IdentitySessionTooLargeError();
  }

  return encoded;
}

async function open(value: string, key: Uint8Array): Promise<unknown | null> {
  if (Buffer.byteLength(value, 'utf8') > MAX_SEALED_COOKIE_BYTES) return null;

  const parts = value.split('.');
  if (parts.length !== 3 || parts[0] !== VERSION) return null;

  const iv = base64UrlDecode(parts[1]);
  const ciphertext = base64UrlDecode(parts[2]);
  if (!iv || iv.length !== IV_BYTES || !ciphertext) return null;

  try {
    const cryptoKey = await importAesKey(key, 'decrypt');
    const plaintext = await crypto.subtle.decrypt(
      { name: 'AES-GCM', iv, additionalData: encoder.encode(VERSION) },
      cryptoKey,
      ciphertext
    );
    return JSON.parse(decoder.decode(plaintext));
  } catch {
    return null;
  }
}

function hasBaseIdentitySessionFields(candidate: Record<string, unknown>): boolean {
  return (
    typeof candidate.accessToken === 'string' &&
    candidate.accessToken.length > 0 &&
    typeof candidate.accessTokenExpiresAt === 'number' &&
    Number.isFinite(candidate.accessTokenExpiresAt) &&
    typeof candidate.refreshToken === 'string' &&
    candidate.refreshToken.length > 0
  );
}

export function isIdentitySessionV2(session: IdentitySession): session is IdentitySessionV2 {
  return 'schemaVersion' in session && session.schemaVersion === 2;
}

function isIdentitySession(value: unknown): value is IdentitySession {
  if (!value || typeof value !== 'object') return false;
  const candidate = value as Record<string, unknown>;
  if (!hasBaseIdentitySessionFields(candidate)) return false;

  if (candidate.schemaVersion === undefined) return true;
  if (candidate.schemaVersion !== 2) return false;

  return (
    typeof candidate.sessionExpiresAt === 'number' &&
    Number.isFinite(candidate.sessionExpiresAt) &&
    candidate.sessionExpiresAt >= 0
  );
}

function isLoginTransaction(value: unknown): value is LoginTransaction {
  if (!value || typeof value !== 'object') return false;
  const candidate = value as Record<string, unknown>;
  return (
    typeof candidate.state === 'string' &&
    candidate.state.length > 0 &&
    typeof candidate.codeVerifier === 'string' &&
    /^[A-Za-z0-9._~-]{43,128}$/.test(candidate.codeVerifier) &&
    typeof candidate.returnTarget === 'string' &&
    candidate.returnTarget.startsWith('/') &&
    typeof candidate.expiresAt === 'number' &&
    Number.isFinite(candidate.expiresAt)
  );
}

export async function sealSession(value: IdentitySession, key: Uint8Array): Promise<string> {
  if (!isIdentitySession(value)) throw new Error('Invalid identity session');
  return seal(value, key);
}

export async function openSession(
  value: string,
  key: Uint8Array,
  now = Date.now()
): Promise<IdentitySession | null> {
  const decoded = await open(value, key);
  if (!isIdentitySession(decoded)) return null;
  if (isIdentitySessionV2(decoded) && decoded.sessionExpiresAt <= now) return null;
  return decoded;
}

export async function sealLoginTransaction(
  value: LoginTransaction,
  key: Uint8Array
): Promise<string> {
  if (!isLoginTransaction(value)) throw new Error('Invalid login transaction');
  return seal(value, key);
}

export async function openLoginTransaction(
  value: string,
  key: Uint8Array,
  now = Date.now()
): Promise<LoginTransaction | null> {
  const decoded = await open(value, key);
  if (!isLoginTransaction(decoded) || decoded.expiresAt <= now) return null;
  return decoded;
}

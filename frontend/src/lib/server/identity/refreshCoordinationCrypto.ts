const encoder = new TextEncoder();

const KEY_ID_INFO = 'memo_server/identity-refresh/v1/key-id';
const RESULT_MAC_INFO = 'memo_server/identity-refresh/v1/result-mac';
const RESULT_VERSION = 1;

type RefreshResultEnvelope = {
  version: 1;
  sealedSession: string;
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

async function deriveHmacKey(sessionKey: Uint8Array, info: string): Promise<CryptoKey> {
  if (sessionKey.length !== 32) throw new Error('Invalid identity session key');
  const root = await crypto.subtle.importKey('raw', sessionKey, 'HKDF', false, ['deriveKey']);
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

async function signHex(key: CryptoKey, message: string): Promise<string> {
  return bytesToHex(await crypto.subtle.sign('HMAC', key, encoder.encode(message)));
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
  const key = await deriveHmacKey(sessionKey, RESULT_MAC_INFO);
  const mac = await signHex(key, `v1\0${coordinationId}\0${sealedSession}`);
  const envelope: RefreshResultEnvelope = {
    version: RESULT_VERSION,
    sealedSession,
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
    typeof candidate.sealedSession !== 'string' ||
    candidate.sealedSession.length === 0 ||
    typeof candidate.mac !== 'string'
  ) {
    return null;
  }

  const signature = hexToBytes(candidate.mac);
  if (!signature) return null;

  const key = await deriveHmacKey(sessionKey, RESULT_MAC_INFO);
  const valid = await crypto.subtle.verify(
    'HMAC',
    key,
    signature,
    encoder.encode(`v1\0${coordinationId}\0${candidate.sealedSession}`)
  );
  return valid ? candidate.sealedSession : null;
}

import {
  isMemoMutationMethod,
  MEMO_BFF_MUTATION_HEADER,
  MEMO_BFF_MUTATION_VALUE
} from '$lib/security/memoBff';

const REQUEST_HEADERS_TO_STRIP = [
  'accept-encoding',
  'authorization',
  'connection',
  'content-length',
  'cookie',
  'host',
  'keep-alive',
  'proxy-authenticate',
  'proxy-authorization',
  'te',
  'trailer',
  'transfer-encoding',
  'upgrade',
  'x-development-user-id',
  'x-schnee-memo-request'
] as const;

const RESPONSE_HEADERS_TO_STRIP = [
  'connection',
  'content-length',
  'keep-alive',
  'proxy-authenticate',
  'proxy-authorization',
  'set-cookie',
  'te',
  'trailer',
  'transfer-encoding',
  'upgrade'
] as const;

const INVALID_PROXY_PATH_CHARACTER = /[\\\u0000-\u001f\u007f]/;

export const MEMO_BFF_MAX_REQUEST_BODY_BYTES = 512 * 1024;

export interface BackendAuthContext {
  developmentUserId?: string;
  bearerToken?: string;
}

export class InvalidBackendUrlError extends Error {
  constructor() {
    super('Invalid memo backend URL');
    this.name = 'InvalidBackendUrlError';
  }
}

export class InvalidProxyPathError extends Error {
  constructor() {
    super('Invalid memo API proxy path');
    this.name = 'InvalidProxyPathError';
  }
}

export class MemoProxyBodyTooLargeError extends Error {
  constructor() {
    super('Memo API proxy request body exceeds the supported size');
    this.name = 'MemoProxyBodyTooLargeError';
  }
}

function declaredBodyExceedsLimit(headers: Headers): boolean {
  const rawLength = headers.get('content-length');
  if (rawLength === null || !/^\d+$/.test(rawLength)) {
    return false;
  }

  const declaredLength = Number(rawLength);
  return (
    !Number.isSafeInteger(declaredLength) || declaredLength > MEMO_BFF_MAX_REQUEST_BODY_BYTES
  );
}

export async function readMemoProxyRequestBody(request: Request): Promise<ArrayBuffer | undefined> {
  if (declaredBodyExceedsLimit(request.headers)) {
    throw new MemoProxyBodyTooLargeError();
  }

  if (!request.body) {
    return undefined;
  }

  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let totalBytes = 0;

  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) {
        break;
      }
      if (value.byteLength === 0) {
        continue;
      }
      if (value.byteLength > MEMO_BFF_MAX_REQUEST_BODY_BYTES - totalBytes) {
        await reader.cancel().catch(() => undefined);
        throw new MemoProxyBodyTooLargeError();
      }

      totalBytes += value.byteLength;
      chunks.push(value);
    }
  } finally {
    reader.releaseLock();
  }

  if (totalBytes === 0) {
    return undefined;
  }

  const body = new Uint8Array(totalBytes);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }

  return body.buffer;
}

export function buildBackendRequestHeaders(
  source: Headers,
  auth: BackendAuthContext
): Headers {
  const headers = new Headers(source);

  for (const name of REQUEST_HEADERS_TO_STRIP) {
    headers.delete(name);
  }

  if (auth.bearerToken) {
    headers.set('Authorization', `Bearer ${auth.bearerToken}`);
  } else if (auth.developmentUserId) {
    headers.set('X-Development-User-Id', auth.developmentUserId);
  }

  return headers;
}

export function buildFrontendResponseHeaders(source: Headers): Headers {
  const headers = new Headers(source);
  for (const name of RESPONSE_HEADERS_TO_STRIP) {
    headers.delete(name);
  }
  return headers;
}

function hasOriginOnlyRawShape(value: string): boolean {
  const schemeEnd = value.indexOf('://');
  if (schemeEnd < 0) {
    return false;
  }

  const remainder = value.slice(schemeEnd + 3);
  const authorityEnd = remainder.search(/[/?#]/);
  const splitAt = authorityEnd < 0 ? remainder.length : authorityEnd;
  const authority = remainder.slice(0, splitAt);
  const suffix = remainder.slice(splitAt);

  return authority.length > 0 && !authority.includes('@') && (suffix === '' || suffix === '/');
}

function parseBackendOrigin(value: string): URL {
  let target: URL;
  try {
    target = new URL(value);
  } catch {
    throw new InvalidBackendUrlError();
  }

  const valid =
    (target.protocol === 'http:' || target.protocol === 'https:') &&
    target.hostname.length > 0 &&
    hasOriginOnlyRawShape(value) &&
    target.username === '' &&
    target.password === '' &&
    target.pathname === '/' &&
    target.search === '' &&
    target.hash === '';

  if (!valid) {
    throw new InvalidBackendUrlError();
  }

  return target;
}

function encodeProxyPath(path: string | undefined): string {
  if (!path) return '';

  const segments = path.split('/');
  if (
    segments.some(
      (segment) =>
        segment.length === 0 ||
        segment === '.' ||
        segment === '..' ||
        INVALID_PROXY_PATH_CHARACTER.test(segment)
    )
  ) {
    throw new InvalidProxyPathError();
  }

  return segments.map((segment) => encodeURIComponent(segment)).join('/');
}

export function buildBackendUrl(backendUrl: string, path: string | undefined, search: string): URL {
  const target = parseBackendOrigin(backendUrl);
  target.pathname = `/api/v1/${encodeProxyPath(path)}`;
  target.search = search;
  return target;
}


export function isTrustedMemoProxyRequest(
  method: string,
  headers: Headers,
  requestUrl: URL
): boolean {
  if (!isMemoMutationMethod(method)) return true;

  if (headers.get(MEMO_BFF_MUTATION_HEADER) !== MEMO_BFF_MUTATION_VALUE) {
    return false;
  }

  const origin = headers.get('origin');
  if (!origin) return false;

  try {
    const parsedOrigin = new URL(origin);
    return parsedOrigin.origin === origin && parsedOrigin.origin === requestUrl.origin;
  } catch {
    return false;
  }
}

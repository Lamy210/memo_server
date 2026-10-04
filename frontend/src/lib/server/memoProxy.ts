import {
  isMemoMutationMethod,
  MEMO_BFF_MUTATION_HEADER,
  MEMO_BFF_MUTATION_VALUE
} from '$lib/security/memoBff';
import { validateMemoProxyQuery } from '$lib/server/memoProxyQuery';

const REQUEST_HEADERS_TO_STRIP = [
  'accept-encoding',
  'authorization',
  'cf-connecting-ip',
  'client-ip',
  'connection',
  'content-length',
  'cookie',
  'fastly-client-ip',
  'forwarded',
  'host',
  'keep-alive',
  'origin',
  'proxy-authenticate',
  'proxy-authorization',
  'proxy-connection',
  'referer',
  'te',
  'trailer',
  'transfer-encoding',
  'true-client-ip',
  'upgrade',
  'via',
  'x-development-user-id',
  'x-http-method',
  'x-http-method-override',
  'x-method-override',
  'x-original-url',
  'x-real-ip',
  'x-rewrite-url',
  'x-schnee-memo-request'
] as const;

const REQUEST_HEADER_PREFIXES_TO_STRIP = ['sec-fetch-', 'x-forwarded-'] as const;

const RESPONSE_HEADERS_TO_STRIP = [
  'alt-svc',
  'clear-site-data',
  'connection',
  'content-encoding',
  'content-length',
  'content-security-policy',
  'content-security-policy-report-only',
  'cross-origin-embedder-policy',
  'keep-alive',
  'location',
  'nel',
  'origin-agent-cluster',
  'proxy-authenticate',
  'proxy-authorization',
  'proxy-connection',
  'refresh',
  'report-to',
  'reporting-endpoints',
  'set-cookie',
  'strict-transport-security',
  'te',
  'trailer',
  'transfer-encoding',
  'upgrade'
] as const;

const RESPONSE_HEADER_PREFIXES_TO_STRIP = ['access-control-', 'x-accel-'] as const;

const INVALID_PROXY_PATH_CHARACTER = /[\\\u0000-\u001f\u007f]/;
const INVALID_BACKEND_URL_RAW_CHARACTER = /[\\\u0000-\u0020\u007f]/;
const CONNECTION_OPTION_TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
const MEMO_COLLECTION_METHODS: readonly string[] = ['GET', 'HEAD', 'POST'];
const MEMO_SEARCH_METHODS: readonly string[] = ['GET', 'HEAD'];
const MEMO_ITEM_METHODS: readonly string[] = ['GET', 'HEAD', 'PATCH', 'DELETE'];

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

function stripConnectionOptionHeaders(headers: Headers, connectionValue: string | null): void {
  if (!connectionValue) {
    return;
  }

  for (const option of connectionValue.split(',')) {
    const headerName = option.trim();
    if (CONNECTION_OPTION_TOKEN.test(headerName)) {
      headers.delete(headerName);
    }
  }
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
  const connectionValue = headers.get('connection');

  for (const name of REQUEST_HEADERS_TO_STRIP) {
    headers.delete(name);
  }
  stripConnectionOptionHeaders(headers, connectionValue);

  const sourceHeaderNames: string[] = [];
  headers.forEach((_value, name) => sourceHeaderNames.push(name));
  for (const name of sourceHeaderNames) {
    if (REQUEST_HEADER_PREFIXES_TO_STRIP.some((prefix) => name.startsWith(prefix))) {
      headers.delete(name);
    }
  }

  // Node fetch adds compression negotiation automatically when this header is absent.
  // Keep the trusted backend hop representation-stable so a decoded body cannot be
  // paired with stale Content-Encoding metadata when it is proxied to the browser.
  headers.set('Accept-Encoding', 'identity');

  if (auth.bearerToken) {
    headers.set('Authorization', `Bearer ${auth.bearerToken}`);
  } else if (auth.developmentUserId) {
    headers.set('X-Development-User-Id', auth.developmentUserId);
  }

  return headers;
}

export function buildFrontendResponseHeaders(source: Headers): Headers {
  const headers = new Headers(source);
  const connectionValue = headers.get('connection');

  for (const name of RESPONSE_HEADERS_TO_STRIP) {
    headers.delete(name);
  }
  stripConnectionOptionHeaders(headers, connectionValue);

  const sourceHeaderNames: string[] = [];
  headers.forEach((_value, name) => sourceHeaderNames.push(name));
  for (const name of sourceHeaderNames) {
    if (RESPONSE_HEADER_PREFIXES_TO_STRIP.some((prefix) => name.startsWith(prefix))) {
      headers.delete(name);
    }
  }

  // Memo API responses can contain authenticated/private data. Never allow
  // backend cache policy to make the browser-facing BFF response storable.
  headers.set('Cache-Control', 'no-store');

  return headers;
}

function hasOriginOnlyRawShape(value: string): boolean {
  if (INVALID_BACKEND_URL_RAW_CHARACTER.test(value)) {
    return false;
  }

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

function parseProxyPath(path: string | undefined): string[] {
  if (!path) {
    throw new InvalidProxyPathError();
  }

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

  if (segments[0] !== 'memos' || segments.length > 2) {
    throw new InvalidProxyPathError();
  }

  return segments;
}

function encodeProxyPath(path: string | undefined): string {
  return parseProxyPath(path)
    .map((segment) => encodeURIComponent(segment))
    .join('/');
}

export function allowedMemoProxyMethods(path: string | undefined): readonly string[] {
  const segments = parseProxyPath(path);
  if (segments.length === 1) {
    return MEMO_COLLECTION_METHODS;
  }
  if (segments[1] === 'search') {
    return MEMO_SEARCH_METHODS;
  }
  return MEMO_ITEM_METHODS;
}

export function buildBackendUrl(backendUrl: string, path: string | undefined, search: string): URL {
  const target = parseBackendOrigin(backendUrl);
  target.pathname = `/api/v1/${encodeProxyPath(path)}`;
  validateMemoProxyQuery(path, search);
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

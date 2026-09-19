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
  'x-development-user-id'
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

export interface BackendAuthContext {
  developmentUserId?: string;
  bearerToken?: string;
}

export class InvalidProxyPathError extends Error {
  constructor() {
    super('Invalid memo API proxy path');
    this.name = 'InvalidProxyPathError';
  }
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
  const target = new URL(backendUrl);
  target.pathname = `/api/v1/${encodeProxyPath(path)}`;
  target.search = search;
  target.hash = '';
  return target;
}

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

export interface BackendAuthContext {
  developmentUserId?: string;
  bearerToken?: string;
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

export function buildBackendUrl(backendUrl: string, path: string | undefined, search: string): URL {
  const base = backendUrl.endsWith('/') ? backendUrl : `${backendUrl}/`;
  const target = new URL(`api/v1/${path ?? ''}`, base);
  target.search = search;
  return target;
}

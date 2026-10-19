import { Buffer } from 'node:buffer';

import type { IdentityConfig } from './config';

const IDENTITY_HTTP_TIMEOUT_MS = 10_000;
const IDENTITY_JSON_MAX_BYTES = 64 * 1024;

export type IdentityClientErrorKind =
  | 'configuration'
  | 'temporarily_unavailable'
  | 'access_denied'
  | 'invalid_response';

export class IdentityClientError extends Error {
  readonly kind: IdentityClientErrorKind;

  constructor(kind: IdentityClientErrorKind) {
    super(`Identity request failed: ${kind}`);
    this.name = 'IdentityClientError';
    this.kind = kind;
  }
}

export type OidcMetadata = {
  issuer: string;
  authorizationEndpoint: URL;
  tokenEndpoint: URL;
  endSessionEndpoint?: URL;
  tokenEndpointAuthMethodsSupported: readonly string[];
};

export type AuthorizationRequestInput = {
  clientId: string;
  redirectUri: string;
  state: string;
  codeChallenge: string;
};

export type AuthorizationRequest = {
  url: URL;
};

export type AuthorizationCodeExchangeInput = {
  code: string;
  codeVerifier: string;
  redirectUri: string;
};

export type RefreshAccessTokenInput = {
  refreshToken: string;
};

export type TokenSet = {
  accessToken: string;
  expiresIn: number;
  refreshToken?: string;
};

type DiscoveryDocument = {
  issuer?: unknown;
  authorization_endpoint?: unknown;
  token_endpoint?: unknown;
  end_session_endpoint?: unknown;
  code_challenge_methods_supported?: unknown;
  token_endpoint_auth_methods_supported?: unknown;
};

type TokenDocument = {
  access_token?: unknown;
  token_type?: unknown;
  expires_in?: unknown;
  refresh_token?: unknown;
};

function parseSecureEndpoint(value: unknown): URL {
  if (typeof value !== 'string' || value.length === 0) {
    throw new IdentityClientError('invalid_response');
  }

  let endpoint: URL;
  try {
    endpoint = new URL(value);
  } catch {
    throw new IdentityClientError('invalid_response');
  }

  if (
    endpoint.protocol !== 'https:' ||
    !endpoint.hostname ||
    endpoint.username ||
    endpoint.password ||
    endpoint.hash
  ) {
    throw new IdentityClientError('invalid_response');
  }

  return endpoint;
}

function discoveryUrl(issuer: URL): URL {
  const base = issuer.toString().replace(/\/$/, '');
  return new URL(`${base}/.well-known/openid-configuration`);
}

async function readJson(response: Response): Promise<unknown> {
  const contentLength = response.headers.get('content-length');
  if (contentLength !== null) {
    const declaredLength = Number(contentLength);
    if (Number.isFinite(declaredLength) && declaredLength > IDENTITY_JSON_MAX_BYTES) {
      try {
        await response.body?.cancel();
      } catch {
        // Best-effort release only; the public error remains stable.
      }
      throw new IdentityClientError('invalid_response');
    }
  }

  if (!response.body) {
    throw new IdentityClientError('invalid_response');
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let totalBytes = 0;
  let text = '';

  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      if (!value) continue;

      totalBytes += value.byteLength;
      if (totalBytes > IDENTITY_JSON_MAX_BYTES) {
        try {
          await reader.cancel();
        } catch {
          // Best-effort release only; the public error remains stable.
        }
        throw new IdentityClientError('invalid_response');
      }

      text += decoder.decode(value, { stream: true });
    }

    text += decoder.decode();
    return JSON.parse(text);
  } catch (error) {
    if (error instanceof IdentityClientError) throw error;
    throw new IdentityClientError('invalid_response');
  } finally {
    reader.releaseLock();
  }
}

export async function discoverIdentity(
  config: IdentityConfig,
  fetchFn: typeof fetch
): Promise<OidcMetadata> {
  let response: Response;
  try {
    response = await fetchFn(discoveryUrl(config.issuer), {
      method: 'GET',
      redirect: 'error',
      signal: AbortSignal.timeout(IDENTITY_HTTP_TIMEOUT_MS),
      headers: { Accept: 'application/json' }
    });
  } catch {
    throw new IdentityClientError('temporarily_unavailable');
  }

  if (!response.ok) {
    throw new IdentityClientError(response.status >= 500 ? 'temporarily_unavailable' : 'invalid_response');
  }

  const payload = (await readJson(response)) as DiscoveryDocument | null;
  if (!payload || typeof payload !== 'object' || Array.isArray(payload)) {
    throw new IdentityClientError('invalid_response');
  }
  if (payload.issuer !== config.issuerIdentifier) {
    throw new IdentityClientError('invalid_response');
  }

  if (
    !Array.isArray(payload.code_challenge_methods_supported) ||
    !payload.code_challenge_methods_supported.includes('S256')
  ) {
    throw new IdentityClientError('configuration');
  }

  const authorizationEndpoint = parseSecureEndpoint(payload.authorization_endpoint);
  const tokenEndpoint = parseSecureEndpoint(payload.token_endpoint);
  const endSessionEndpoint =
    payload.end_session_endpoint === undefined
      ? undefined
      : parseSecureEndpoint(payload.end_session_endpoint);

  const authMethods = Array.isArray(payload.token_endpoint_auth_methods_supported)
    ? payload.token_endpoint_auth_methods_supported.filter(
        (value): value is string => typeof value === 'string'
      )
    : ['client_secret_basic'];

  if (!authMethods.includes('client_secret_basic')) {
    throw new IdentityClientError('configuration');
  }

  return {
    issuer: config.issuerIdentifier,
    authorizationEndpoint,
    tokenEndpoint,
    endSessionEndpoint,
    tokenEndpointAuthMethodsSupported: authMethods
  };
}

function base64Url(bytes: Uint8Array): string {
  return Buffer.from(bytes)
    .toString('base64')
    .replace(/=/g, '')
    .replace(/\+/g, '-')
    .replace(/\//g, '_');
}

export async function createPkceChallenge(codeVerifier: string): Promise<string> {
  if (!/^[A-Za-z0-9._~-]{43,128}$/.test(codeVerifier)) {
    throw new IdentityClientError('configuration');
  }

  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(codeVerifier));
  return base64Url(new Uint8Array(digest));
}

export function createAuthorizationRequest(
  metadata: OidcMetadata,
  input: AuthorizationRequestInput
): AuthorizationRequest {
  const url = new URL(metadata.authorizationEndpoint);
  url.searchParams.set('response_type', 'code');
  url.searchParams.set('scope', 'openid offline_access');
  url.searchParams.set('prompt', 'consent');
  url.searchParams.set('client_id', input.clientId);
  url.searchParams.set('redirect_uri', input.redirectUri);
  url.searchParams.set('state', input.state);
  url.searchParams.set('code_challenge', input.codeChallenge);
  url.searchParams.set('code_challenge_method', 'S256');
  return { url };
}

function formUrlEncode(value: string): string {
  const encoded = new URLSearchParams({ value }).toString();
  return encoded.slice('value='.length);
}

function basicClientAuthorization(config: IdentityConfig): string {
  const clientId = formUrlEncode(config.clientId);
  const clientSecret = formUrlEncode(config.clientSecret);
  return `Basic ${Buffer.from(`${clientId}:${clientSecret}`).toString('base64')}`;
}

function validateTokenDocument(payload: unknown): TokenSet {
  if (!payload || typeof payload !== 'object') {
    throw new IdentityClientError('invalid_response');
  }

  const token = payload as TokenDocument;
  if (
    typeof token.access_token !== 'string' ||
    token.access_token.length === 0 ||
    typeof token.token_type !== 'string' ||
    token.token_type.toLowerCase() !== 'bearer' ||
    typeof token.expires_in !== 'number' ||
    !Number.isSafeInteger(token.expires_in) ||
    !Number.isSafeInteger(token.expires_in * 1000) ||
    token.expires_in <= 0
  ) {
    throw new IdentityClientError('invalid_response');
  }

  if (
    token.refresh_token !== undefined &&
    (typeof token.refresh_token !== 'string' || token.refresh_token.length === 0)
  ) {
    throw new IdentityClientError('invalid_response');
  }

  return {
    accessToken: token.access_token,
    expiresIn: token.expires_in,
    ...(typeof token.refresh_token === 'string' ? { refreshToken: token.refresh_token } : {})
  };
}

async function requestToken(
  metadata: OidcMetadata,
  config: IdentityConfig,
  fetchFn: typeof fetch,
  body: URLSearchParams
): Promise<TokenSet> {
  let response: Response;
  try {
    response = await fetchFn(metadata.tokenEndpoint, {
      method: 'POST',
      redirect: 'error',
      signal: AbortSignal.timeout(IDENTITY_HTTP_TIMEOUT_MS),
      headers: {
        Accept: 'application/json',
        Authorization: basicClientAuthorization(config),
        'Content-Type': 'application/x-www-form-urlencoded'
      },
      body: body.toString()
    });
  } catch {
    throw new IdentityClientError('temporarily_unavailable');
  }

  if (!response.ok) {
    throw new IdentityClientError(
      response.status === 400 || response.status === 401
        ? 'access_denied'
        : response.status >= 500
          ? 'temporarily_unavailable'
          : 'invalid_response'
    );
  }

  return validateTokenDocument(await readJson(response));
}

export async function exchangeAuthorizationCode(
  metadata: OidcMetadata,
  config: IdentityConfig,
  fetchFn: typeof fetch,
  input: AuthorizationCodeExchangeInput
): Promise<TokenSet> {
  const body = new URLSearchParams({
    grant_type: 'authorization_code',
    code: input.code,
    redirect_uri: input.redirectUri,
    code_verifier: input.codeVerifier
  });
  const tokenSet = await requestToken(metadata, config, fetchFn, body);
  if (!tokenSet.refreshToken) {
    throw new IdentityClientError('invalid_response');
  }
  return tokenSet;
}

export async function refreshAccessToken(
  metadata: OidcMetadata,
  config: IdentityConfig,
  fetchFn: typeof fetch,
  input: RefreshAccessTokenInput
): Promise<TokenSet> {
  return requestToken(
    metadata,
    config,
    fetchFn,
    new URLSearchParams({ grant_type: 'refresh_token', refresh_token: input.refreshToken })
  );
}

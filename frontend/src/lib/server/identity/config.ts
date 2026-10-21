import { Buffer } from 'node:buffer';

export type IdentityConfig = {
  issuer: URL;
  issuerIdentifier: string;
  clientId: string;
  clientSecret: string;
  sessionKey: Uint8Array;
  refreshCoordinationUrl?: URL;
  browserSessionMaxAgeSeconds?: number;
};

export class IdentityConfigurationError extends Error {
  constructor() {
    super('Invalid identity configuration');
    this.name = 'IdentityConfigurationError';
  }
}

function requireNonBlank(value: string | undefined): string {
  if (!value || !value.trim()) throw new IdentityConfigurationError();
  return value;
}

function parseIssuer(value: string): URL {
  // WHATWG URL parsing normalizes raw tabs/newlines and treats backslashes as
  // separators in HTTPS URLs. Never reinterpret the configured OIDC issuer.
  if (value !== value.trim() || /[\\\u0000-\u0020\u007f]/u.test(value)) {
    throw new IdentityConfigurationError();
  }

  let issuer: URL;
  try {
    issuer = new URL(value);
  } catch {
    throw new IdentityConfigurationError();
  }

  if (
    issuer.protocol !== 'https:' ||
    !issuer.hostname ||
    issuer.username ||
    issuer.password ||
    issuer.search ||
    issuer.hash
  ) {
    throw new IdentityConfigurationError();
  }

  return issuer;
}

function parseSessionKey(value: string): Uint8Array {
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(value) || value.length % 4 !== 0) {
    throw new IdentityConfigurationError();
  }

  const decoded = Buffer.from(value, 'base64');
  if (decoded.length !== 32 || decoded.toString('base64') !== value) {
    throw new IdentityConfigurationError();
  }

  return new Uint8Array(decoded);
}

function parseRefreshCoordinationUrl(value: string | undefined): URL | undefined {
  if (value === undefined) return undefined;
  if (!value || value !== value.trim()) throw new IdentityConfigurationError();

  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new IdentityConfigurationError();
  }

  if (!['valkey:', 'valkeys:', 'redis:', 'rediss:'].includes(url.protocol) || !url.hostname) {
    throw new IdentityConfigurationError();
  }

  return url;
}

function parseBrowserSessionMaxAgeSeconds(value: string | undefined): number | undefined {
  if (value === undefined) return undefined;
  if (!/^\d+$/.test(value)) throw new IdentityConfigurationError();

  const seconds = Number(value);
  if (!Number.isSafeInteger(seconds) || seconds < 900 || seconds > 86_400) {
    throw new IdentityConfigurationError();
  }

  return seconds;
}

export function readIdentityConfig(env: Record<string, string | undefined>): IdentityConfig {
  const issuerIdentifier = requireNonBlank(env.IDENTITY_ISSUER);
  const issuer = parseIssuer(issuerIdentifier);
  const clientId = requireNonBlank(env.IDENTITY_CLIENT_ID);
  const clientSecret = requireNonBlank(env.IDENTITY_CLIENT_SECRET);
  const sessionKey = parseSessionKey(requireNonBlank(env.IDENTITY_SESSION_KEY));
  const refreshCoordinationUrl = parseRefreshCoordinationUrl(env.IDENTITY_REFRESH_COORDINATION_URL);
  const browserSessionMaxAgeSeconds = parseBrowserSessionMaxAgeSeconds(
    env.IDENTITY_BROWSER_SESSION_MAX_AGE_SECONDS
  );

  if (refreshCoordinationUrl && browserSessionMaxAgeSeconds === undefined) {
    throw new IdentityConfigurationError();
  }

  return {
    issuer,
    issuerIdentifier,
    clientId,
    clientSecret,
    sessionKey,
    refreshCoordinationUrl,
    browserSessionMaxAgeSeconds
  };
}

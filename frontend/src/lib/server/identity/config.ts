import { Buffer } from 'node:buffer';

export type IdentityConfig = {
  issuer: URL;
  issuerIdentifier: string;
  clientId: string;
  clientSecret: string;
  sessionKey: Uint8Array;
};

export class IdentityConfigurationError extends Error {
  constructor() {
    super('Invalid identity configuration');
    this.name = 'IdentityConfigurationError';
  }
}

function requireNonBlank(value: string | undefined): string {
  const normalized = value?.trim();
  if (!normalized) throw new IdentityConfigurationError();
  return normalized;
}

function parseIssuer(value: string): URL {
  let issuer: URL;
  try {
    issuer = new URL(value);
  } catch {
    throw new IdentityConfigurationError();
  }

  const loopbackHttp =
    issuer.protocol === 'http:' &&
    (issuer.hostname === '127.0.0.1' || issuer.hostname === 'localhost' || issuer.hostname === '[::1]');

  if (
    (issuer.protocol !== 'https:' && !loopbackHttp) ||
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

export function readIdentityConfig(env: Record<string, string | undefined>): IdentityConfig {
  const issuerIdentifier = requireNonBlank(env.IDENTITY_ISSUER);
  const issuer = parseIssuer(issuerIdentifier);
  const clientId = requireNonBlank(env.IDENTITY_CLIENT_ID);
  const clientSecret = requireNonBlank(env.IDENTITY_CLIENT_SECRET);
  const sessionKey = parseSessionKey(requireNonBlank(env.IDENTITY_SESSION_KEY));

  return { issuer, issuerIdentifier, clientId, clientSecret, sessionKey };
}

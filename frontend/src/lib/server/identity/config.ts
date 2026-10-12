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
  if (!value || !value.trim()) throw new IdentityConfigurationError();
  return value;
}

function parseIssuer(value: string): URL {
  if (value !== value.trim()) {
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

export function readIdentityConfig(env: Record<string, string | undefined>): IdentityConfig {
  const issuerIdentifier = requireNonBlank(env.IDENTITY_ISSUER);
  const issuer = parseIssuer(issuerIdentifier);
  const clientId = requireNonBlank(env.IDENTITY_CLIENT_ID);
  const clientSecret = requireNonBlank(env.IDENTITY_CLIENT_SECRET);
  const sessionKey = parseSessionKey(requireNonBlank(env.IDENTITY_SESSION_KEY));

  return { issuer, issuerIdentifier, clientId, clientSecret, sessionKey };
}

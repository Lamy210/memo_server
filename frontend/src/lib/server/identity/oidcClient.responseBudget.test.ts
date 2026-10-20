import { describe, expect, it, vi } from 'vitest';

import type { IdentityConfig } from './config';
import {
  discoverIdentity,
  IdentityClientError,
  refreshAccessToken
} from './oidcClient';

const MAX_IDENTITY_JSON_BYTES = 64 * 1024;

const config: IdentityConfig = {
  issuer: new URL('https://id.example.test'),
  issuerIdentifier: 'https://id.example.test/',
  clientId: 'memo-web',
  clientSecret: 'client-secret',
  sessionKey: new Uint8Array(32)
};

function discovery(overrides: Record<string, unknown> = {}) {
  return {
    issuer: 'https://id.example.test/',
    authorization_endpoint: 'https://id.example.test/authorize',
    token_endpoint: 'https://id.example.test/token',
    code_challenge_methods_supported: ['S256'],
    token_endpoint_auth_methods_supported: ['client_secret_basic'],
    ...overrides
  };
}

function tokenDocument() {
  return {
    access_token: 'access-1',
    token_type: 'Bearer',
    expires_in: 300,
    refresh_token: 'refresh-2'
  };
}

function oversizedJson(payload: Record<string, unknown>): string {
  return JSON.stringify({ ...payload, padding: 'x'.repeat(MAX_IDENTITY_JSON_BYTES) });
}

function responseWithOversizedDeclaredLength(body: string): Response {
  return new Response(body, {
    headers: {
      'Content-Type': 'application/json',
      'Content-Length': String(MAX_IDENTITY_JSON_BYTES + 1)
    }
  });
}

function chunkedResponse(body: string, splitAt?: number): Response {
  const bytes = new TextEncoder().encode(body);
  const midpoint = splitAt ?? Math.floor(bytes.byteLength / 2);
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(bytes.slice(0, midpoint));
      controller.enqueue(bytes.slice(midpoint));
      controller.close();
    }
  });

  return new Response(stream, {
    headers: { 'Content-Type': 'application/json' }
  });
}

function responseWithMalformedUtf8(json: string, marker: string): Response {
  const markerOffset = json.indexOf(marker);
  if (markerOffset < 0) throw new Error('UTF-8 fixture marker is missing');

  // The fixture is ASCII up to the marker, so its character offset equals its byte offset.
  const bytes = new TextEncoder().encode(json);
  bytes[markerOffset] = 0xff;

  return new Response(bytes, { headers: { 'Content-Type': 'application/json' } });
}

async function metadata() {
  return discoverIdentity(
    config,
    vi.fn(async () => Response.json(discovery())) as unknown as typeof fetch
  );
}

async function expectInvalidResponse(promise: Promise<unknown>) {
  try {
    await promise;
    throw new Error('expected identity response to be rejected');
  } catch (error) {
    expect(error).toBeInstanceOf(IdentityClientError);
    expect(error).toMatchObject({ kind: 'invalid_response' });
  }
}

describe('identity JSON response budget', () => {
  it('accepts discovery with a valid declared Content-Length', async () => {
    const body = JSON.stringify(discovery());
    const fetchFn = vi.fn(async () => new Response(body, {
      headers: { 'Content-Length': String(new TextEncoder().encode(body).byteLength) }
    })) as unknown as typeof fetch;

    await expect(discoverIdentity(config, fetchFn)).resolves.toMatchObject({
      issuer: config.issuerIdentifier
    });
  });

  it('accepts token JSON with a valid zero-padded Content-Length', async () => {
    const oidcMetadata = await metadata();
    const body = JSON.stringify(tokenDocument());
    const length = new TextEncoder().encode(body).byteLength;
    const fetchFn = vi.fn(async () => new Response(body, {
      headers: { 'Content-Length': String(length).padStart(5, '0') }
    })) as unknown as typeof fetch;

    await expect(
      refreshAccessToken(oidcMetadata, config, fetchFn, { refreshToken: 'refresh-1' })
    ).resolves.toMatchObject({ accessToken: 'access-1', refreshToken: 'refresh-2' });
  });

  it.each(['invalid', '-1', '1e2', '0x10', ''])(
    'rejects discovery with malformed Content-Length %j',
    async (contentLength) => {
      const fetchFn = vi.fn(async () => new Response(JSON.stringify(discovery()), {
        headers: { 'Content-Type': 'application/json', 'Content-Length': contentLength }
      })) as unknown as typeof fetch;

      await expectInvalidResponse(discoverIdentity(config, fetchFn));
    }
  );

  it.each(['invalid', '-1', '1e2', '0x10', ''])(
    'rejects token response with malformed Content-Length %j',
    async (contentLength) => {
      const oidcMetadata = await metadata();
      const fetchFn = vi.fn(async () => new Response(JSON.stringify(tokenDocument()), {
        headers: { 'Content-Type': 'application/json', 'Content-Length': contentLength }
      })) as unknown as typeof fetch;

      await expectInvalidResponse(
        refreshAccessToken(oidcMetadata, config, fetchFn, { refreshToken: 'refresh-1' })
      );
    }
  );

  it('rejects discovery responses whose declared Content-Length exceeds the budget', async () => {
    const body = JSON.stringify(discovery());
    const fetchFn = vi.fn(
      async () => responseWithOversizedDeclaredLength(body)
    ) as unknown as typeof fetch;

    await expectInvalidResponse(discoverIdentity(config, fetchFn));
  });

  it('rejects oversized chunked discovery responses', async () => {
    const body = oversizedJson(discovery());
    const fetchFn = vi.fn(async () => chunkedResponse(body)) as unknown as typeof fetch;

    await expectInvalidResponse(discoverIdentity(config, fetchFn));
  });

  it('rejects token responses whose declared Content-Length exceeds the budget', async () => {
    const oidcMetadata = await metadata();
    const body = JSON.stringify(tokenDocument());
    const fetchFn = vi.fn(
      async () => responseWithOversizedDeclaredLength(body)
    ) as unknown as typeof fetch;

    await expectInvalidResponse(
      refreshAccessToken(oidcMetadata, config, fetchFn, { refreshToken: 'refresh-1' })
    );
  });

  it('rejects oversized chunked token responses', async () => {
    const oidcMetadata = await metadata();
    const body = oversizedJson(tokenDocument());
    const fetchFn = vi.fn(async () => chunkedResponse(body)) as unknown as typeof fetch;

    await expectInvalidResponse(
      refreshAccessToken(oidcMetadata, config, fetchFn, { refreshToken: 'refresh-1' })
    );
  });
});

describe('identity JSON UTF-8 decoding', () => {
  it('rejects malformed UTF-8 in an otherwise valid discovery document', async () => {
    const body = JSON.stringify(discovery({ ignored: 'invalid-byte' }));
    const fetchFn = vi.fn(async () =>
      responseWithMalformedUtf8(body, 'invalid-byte')
    ) as unknown as typeof fetch;

    await expectInvalidResponse(discoverIdentity(config, fetchFn));
  });

  it('rejects malformed UTF-8 in access tokens instead of silently replacing bytes', async () => {
    const oidcMetadata = await metadata();
    const body = JSON.stringify({ ...tokenDocument(), access_token: 'invalid-token' });
    const fetchFn = vi.fn(async () =>
      responseWithMalformedUtf8(body, 'invalid-token')
    ) as unknown as typeof fetch;

    await expectInvalidResponse(
      refreshAccessToken(oidcMetadata, config, fetchFn, { refreshToken: 'refresh-1' })
    );
  });

  it('accepts valid UTF-8 code points split across response chunks', async () => {
    const body = JSON.stringify(discovery({ ignored: '雪' }));
    const markerOffset = new TextEncoder().encode(body.slice(0, body.indexOf('雪'))).byteLength;
    const fetchFn = vi.fn(async () =>
      chunkedResponse(body, markerOffset + 1)
    ) as unknown as typeof fetch;

    await expect(discoverIdentity(config, fetchFn)).resolves.toMatchObject({
      issuer: config.issuerIdentifier
    });
  });

  it('cancels an unfinished response stream after encountering malformed UTF-8', async () => {
    const json = JSON.stringify(discovery({ ignored: 'invalid-byte' }));
    const bytes = new TextEncoder().encode(json);
    bytes[json.indexOf('invalid-byte')] = 0xff;

    let cancelled = false;
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(bytes);
        // Keep the stream open to detect abandoned response bodies.
      },
      cancel() {
        cancelled = true;
      }
    });
    const fetchFn = vi.fn(async () =>
      new Response(body, { headers: { 'Content-Type': 'application/json' } })
    ) as unknown as typeof fetch;

    await expectInvalidResponse(discoverIdentity(config, fetchFn));
    expect(cancelled).toBe(true);
  });
});

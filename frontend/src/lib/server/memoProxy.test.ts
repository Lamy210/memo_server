import { describe, expect, it } from 'vitest';

import {
  buildBackendRequestHeaders,
  buildBackendUrl,
  buildFrontendResponseHeaders,
  InvalidBackendUrlError,
  InvalidProxyPathError,
  isTrustedMemoProxyRequest,
  MEMO_BFF_MAX_REQUEST_BODY_BYTES,
  MemoProxyBodyTooLargeError,
  readMemoProxyRequestBody
} from './memoProxy';

const AUTH_CONTEXT = {
  developmentUserId: '12345678-1234-1234-1234-123456789012'
};

function createStreamingRequest(totalBytes: number, chunkBytes: number): Request {
  let remaining = totalBytes;
  return new Request('https://memo.example/api/v1/memos', {
    method: 'POST',
    headers: { 'Content-Type': 'application/octet-stream' },
    body: new ReadableStream<Uint8Array>({
      pull(controller) {
        if (remaining === 0) {
          controller.close();
          return;
        }
        const next = Math.min(remaining, chunkBytes);
        remaining -= next;
        controller.enqueue(new Uint8Array(next));
      }
    }),
    duplex: 'half'
  } as RequestInit & { duplex: 'half' }) as Request;
}

describe('buildBackendRequestHeaders', () => {
  it('replaces browser-controlled auth, cookies and hop-by-hop headers', () => {
    const headers = buildBackendRequestHeaders(
      new Headers({
        'Accept-Encoding': 'gzip, deflate',
        Authorization: 'Bearer browser-token',
        'CF-Connecting-IP': '203.0.113.20',
        'Client-IP': '203.0.113.21',
        Connection: 'keep-alive',
        Cookie: 'session=browser-secret',
        'Content-Type': 'application/json',
        'Fastly-Client-IP': '203.0.113.22',
        Forwarded: 'for=203.0.113.23;proto=https',
        'True-Client-IP': '203.0.113.24',
        Via: '1.1 attacker.example',
        'X-Development-User-Id': '87654321-4321-4321-4321-210987654321',
        'X-Forwarded-For': '203.0.113.25',
        'X-Forwarded-Host': 'attacker.example',
        'X-Forwarded-Proto': 'http',
        'X-Forwarded-Untrusted': 'browser-controlled',
        'X-Real-IP': '203.0.113.26',
        'X-Request-Id': 'browser-request-id',
        'X-Schnee-Memo-Request': '1'
      }),
      AUTH_CONTEXT
    );

    expect(headers.get('authorization')).toBeNull();
    expect(headers.get('connection')).toBeNull();
    expect(headers.get('cf-connecting-ip')).toBeNull();
    expect(headers.get('client-ip')).toBeNull();
    expect(headers.get('fastly-client-ip')).toBeNull();
    expect(headers.get('forwarded')).toBeNull();
    expect(headers.get('true-client-ip')).toBeNull();
    expect(headers.get('via')).toBeNull();
    expect(headers.get('x-forwarded-for')).toBeNull();
    expect(headers.get('x-forwarded-host')).toBeNull();
    expect(headers.get('x-forwarded-proto')).toBeNull();
    expect(headers.get('x-forwarded-untrusted')).toBeNull();
    expect(headers.get('x-real-ip')).toBeNull();
    expect(headers.get('content-type')).toBe('application/json');
    expect(headers.get('x-request-id')).toBe('browser-request-id');
    expect(headers.get('x-development-user-id')).toBe(
      '12345678-1234-1234-1234-123456789012'
    );
    expect(headers.get('x-schnee-memo-request')).toBeNull();
    expect(headers.get('accept-encoding')).toBe('identity');
  });

  it('uses server bearer auth when provided', () => {
    const headers = buildBackendRequestHeaders(new Headers(), {
      developmentUserId: '12345678-1234-1234-1234-123456789012',
      bearerToken: 'trusted-access-token'
    });

    expect(headers.get('accept-encoding')).toBe('identity');
    expect(headers.get('authorization')).toBe('Bearer trusted-access-token');
    expect(headers.get('x-development-user-id')).toBeNull();
  });

  it('forwards no identity when the server has no trusted auth context', () => {
    const headers = buildBackendRequestHeaders(
      new Headers({
        Authorization: 'Bearer browser-token',
        'X-Development-User-Id': '87654321-4321-4321-4321-210987654321'
      }),
      {}
    );

    expect(headers.get('authorization')).toBeNull();
    expect(headers.get('x-development-user-id')).toBeNull();
  });
});

describe('buildBackendUrl', () => {
  it('preserves the API namespace and encoded path', () => {
    expect(buildBackendUrl('http://backend:8080', 'memos/folder name', '').toString()).toBe(
      'http://backend:8080/api/v1/memos/folder%20name'
    );
  });

  it('accepts HTTP and HTTPS backend origins only', () => {
    expect(buildBackendUrl('https://backend.example.com/', 'memos', '').toString()).toBe(
      'https://backend.example.com/api/v1/memos'
    );
  });

  it.each([
    'ftp://backend:8080',
    'file:///tmp/memo',
    'backend:8080',
    'http://user:secret@backend:8080',
    'http://@backend:8080',
    'http://backend:8080/internal',
    'http://backend:8080/internal/..',
    'http://backend:8080\\internal\\..',
    'http://back\tend:8080',
    'http://backend:8080/?',
    'http://backend:8080/#',
    'http://backend:8080?tenant=a',
    'http://backend:8080#memo'
  ])('rejects unsafe or ambiguous backend URL %s', (backendUrl) => {
    expect(() => buildBackendUrl(backendUrl, 'memos', '')).toThrow(InvalidBackendUrlError);
  });

  it.each(['../admin', 'memos/../health', 'memos//admin', 'memos\\admin'])(
    'rejects ambiguous proxy path %s',
    (path) => {
      expect(() => buildBackendUrl('http://backend:8080', path, '')).toThrow(InvalidProxyPathError);
    }
  );
});

describe('readMemoProxyRequestBody', () => {
  it('accepts a body exactly at the BFF limit', async () => {
    const request = createStreamingRequest(MEMO_BFF_MAX_REQUEST_BODY_BYTES, 64 * 1024);
    const body = await readMemoProxyRequestBody(request);

    expect(body?.byteLength).toBe(MEMO_BFF_MAX_REQUEST_BODY_BYTES);
  });

  it('rejects an oversized body declared by Content-Length before reading it', async () => {
    const body = new ReadableStream<Uint8Array>({
      start() {
        throw new Error('body stream must not be consumed');
      }
    });
    const request = new Request('https://memo.example/api/v1/memos', {
      method: 'POST',
      headers: { 'Content-Length': String(MEMO_BFF_MAX_REQUEST_BODY_BYTES + 1) },
      body,
      duplex: 'half'
    } as RequestInit & { duplex: 'half' });

    await expect(readMemoProxyRequestBody(request)).rejects.toBeInstanceOf(
      MemoProxyBodyTooLargeError
    );
  });

  it('rejects an oversized streamed body even without a declared length', async () => {
    const request = createStreamingRequest(MEMO_BFF_MAX_REQUEST_BODY_BYTES + 1, 64 * 1024);

    await expect(readMemoProxyRequestBody(request)).rejects.toBeInstanceOf(
      MemoProxyBodyTooLargeError
    );
  });

  it('returns undefined for an empty body', async () => {
    const request = new Request('https://memo.example/api/v1/memos', {
      method: 'POST',
      body: new Uint8Array(),
      duplex: 'half'
    } as RequestInit & { duplex: 'half' });

    await expect(readMemoProxyRequestBody(request)).resolves.toBeUndefined();
  });
});

describe('buildFrontendResponseHeaders', () => {
  it('removes hop-by-hop response headers and disables caching', () => {
    const headers = buildFrontendResponseHeaders(
      new Headers({
        'Cache-Control': 'public, max-age=3600',
        Connection: 'keep-alive',
        'Content-Type': 'application/json',
        'Set-Cookie': 'memo=should-not-cross-boundary',
        'Transfer-Encoding': 'chunked'
      })
    );

    expect(headers.get('cache-control')).toBe('no-store');
    expect(headers.get('connection')).toBeNull();
    expect(headers.get('set-cookie')).toBeNull();
    expect(headers.get('transfer-encoding')).toBeNull();
    expect(headers.get('content-type')).toBe('application/json');
  });
});

describe('isTrustedMemoProxyRequest', () => {
  const requestUrl = new URL('https://memo.example/api/v1/memos');

  it('allows safe methods without a mutation marker', () => {
    expect(isTrustedMemoProxyRequest('GET', new Headers(), requestUrl)).toBe(true);
    expect(isTrustedMemoProxyRequest('HEAD', new Headers(), requestUrl)).toBe(true);
  });

  it('requires marker and exact same-origin for mutations', () => {
    const headers = new Headers({
      Origin: 'https://memo.example',
      'X-Schnee-Memo-Request': '1'
    });

    expect(isTrustedMemoProxyRequest('POST', headers, requestUrl)).toBe(true);
  });

  it('rejects missing or incorrect marker values', () => {
    expect(
      isTrustedMemoProxyRequest(
        'POST',
        new Headers({ Origin: 'https://memo.example' }),
        requestUrl
      )
    ).toBe(false);
    expect(
      isTrustedMemoProxyRequest(
        'POST',
        new Headers({ Origin: 'https://memo.example', 'X-Schnee-Memo-Request': '0' }),
        requestUrl
      )
    ).toBe(false);
  });

  it('rejects cross-origin and malformed Origin values', () => {
    expect(
      isTrustedMemoProxyRequest(
        'POST',
        new Headers({ Origin: 'https://attacker.example', 'X-Schnee-Memo-Request': '1' }),
        requestUrl
      )
    ).toBe(false);
    expect(
      isTrustedMemoProxyRequest(
        'POST',
        new Headers({ Origin: 'not a url', 'X-Schnee-Memo-Request': '1' }),
        requestUrl
      )
    ).toBe(false);
  });
});

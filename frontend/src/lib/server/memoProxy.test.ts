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

describe('buildBackendRequestHeaders', () => {
  it('strips browser credentials and injects the server-controlled development identity', () => {
    const source = new Headers({
      'Accept-Encoding': 'gzip',
      Authorization: 'Bearer attacker-token',
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
    });

    const headers = buildBackendRequestHeaders(source, {
      developmentUserId: '12345678-1234-1234-1234-123456789012'
    });

    expect(headers.get('accept-encoding')).toBe('identity');
    expect(headers.get('authorization')).toBeNull();
    expect(headers.get('cookie')).toBeNull();
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
  });

  it('prefers a server-provided bearer token over development identity', () => {
    const headers = buildBackendRequestHeaders(new Headers(), {
      bearerToken: 'trusted-access-token',
      developmentUserId: '12345678-1234-1234-1234-123456789012'
    });

    expect(headers.get('accept-encoding')).toBe('identity');
    expect(headers.get('authorization')).toBe('Bearer trusted-access-token');
    expect(headers.get('x-development-user-id')).toBeNull();
  });

  it('forwards no authentication when the server has no auth context', () => {
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
  it('preserves the API namespace, encoded path and query string', () => {
    expect(
      buildBackendUrl(
        'http://backend:8080',
        'memos/folder name',
        '?query=hello&page=2'
      ).toString()
    ).toBe('http://backend:8080/api/v1/memos/folder%20name?query=hello&page=2');
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
    'rejects path traversal or ambiguous path %s',
    (path) => {
      expect(() => buildBackendUrl('http://backend:8080', path, '')).toThrow(
        InvalidProxyPathError
      );
    }
  );
});

describe('readMemoProxyRequestBody', () => {
  it('preserves bounded request bodies and treats an empty body as absent', async () => {
    const body = await readMemoProxyRequestBody(
      new Request('https://memo.example.com/api/v1/memos', {
        method: 'POST',
        body: 'memo payload'
      })
    );

    expect(new TextDecoder().decode(body)).toBe('memo payload');

    const empty = await readMemoProxyRequestBody(
      new Request('https://memo.example.com/api/v1/memos', {
        method: 'POST'
      })
    );
    expect(empty).toBeUndefined();
  });

  it('accepts exactly the shared 512 KiB request-body budget', async () => {
    const body = await readMemoProxyRequestBody(
      new Request('https://memo.example.com/api/v1/memos', {
        method: 'POST',
        body: 'x'.repeat(MEMO_BFF_MAX_REQUEST_BODY_BYTES)
      })
    );

    expect(body?.byteLength).toBe(MEMO_BFF_MAX_REQUEST_BODY_BYTES);
  });

  it('rejects an actual streamed body that exceeds the limit', async () => {
    const request = new Request('https://memo.example.com/api/v1/memos', {
      method: 'POST',
      body: 'x'.repeat(MEMO_BFF_MAX_REQUEST_BODY_BYTES + 1)
    });

    await expect(readMemoProxyRequestBody(request)).rejects.toBeInstanceOf(
      MemoProxyBodyTooLargeError
    );
  });

  it('rejects an oversized declared Content-Length before consuming the body', async () => {
    const request = new Request('https://memo.example.com/api/v1/memos', {
      method: 'POST',
      headers: {
        'Content-Length': String(MEMO_BFF_MAX_REQUEST_BODY_BYTES + 1)
      },
      body: 'small'
    });

    await expect(readMemoProxyRequestBody(request)).rejects.toBeInstanceOf(
      MemoProxyBodyTooLargeError
    );
    expect(request.bodyUsed).toBe(false);
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
  const requestUrl = new URL('https://memo.example.com/api/v1/memos');

  it('allows safe methods without CSRF metadata', () => {
    expect(isTrustedMemoProxyRequest('GET', new Headers(), requestUrl)).toBe(true);
    expect(isTrustedMemoProxyRequest('HEAD', new Headers(), requestUrl)).toBe(true);
  });

  it('allows same-origin mutations with the BFF marker', () => {
    const headers = new Headers({
      Origin: 'https://memo.example.com',
      'X-Schnee-Memo-Request': '1'
    });

    expect(isTrustedMemoProxyRequest('POST', headers, requestUrl)).toBe(true);
  });

  it('rejects mutations without the BFF marker', () => {
    const headers = new Headers({ Origin: 'https://memo.example.com' });
    expect(isTrustedMemoProxyRequest('PATCH', headers, requestUrl)).toBe(false);
  });

  it('rejects missing, opaque, invalid, and cross-site origins', () => {
    expect(
      isTrustedMemoProxyRequest(
        'DELETE',
        new Headers({ 'X-Schnee-Memo-Request': '1' }),
        requestUrl
      )
    ).toBe(false);

    expect(
      isTrustedMemoProxyRequest(
        'DELETE',
        new Headers({ Origin: 'null', 'X-Schnee-Memo-Request': '1' }),
        requestUrl
      )
    ).toBe(false);

    expect(
      isTrustedMemoProxyRequest(
        'DELETE',
        new Headers({ Origin: 'not a url', 'X-Schnee-Memo-Request': '1' }),
        requestUrl
      )
    ).toBe(false);

    expect(
      isTrustedMemoProxyRequest(
        'DELETE',
        new Headers({
          Origin: 'https://attacker.example',
          'X-Schnee-Memo-Request': '1'
        }),
        requestUrl
      )
    ).toBe(false);
  });

  it('requires the serialized origin rather than an origin with a path', () => {
    const headers = new Headers({
      Origin: 'https://memo.example.com/path',
      'X-Schnee-Memo-Request': '1'
    });

    expect(isTrustedMemoProxyRequest('PUT', headers, requestUrl)).toBe(false);
  });
});

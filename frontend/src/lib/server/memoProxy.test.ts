import { describe, expect, it } from 'vitest';

import {
  buildBackendRequestHeaders,
  buildBackendUrl,
  buildFrontendResponseHeaders,
  InvalidProxyPathError
} from './memoProxy';

describe('buildBackendRequestHeaders', () => {
  it('strips browser credentials and injects the server-controlled development identity', () => {
    const source = new Headers({
      'Accept-Encoding': 'gzip',
      Authorization: 'Bearer attacker-token',
      Connection: 'keep-alive',
      Cookie: 'session=browser-secret',
      'Content-Type': 'application/json',
      'X-Development-User-Id': '87654321-4321-4321-4321-210987654321'
    });

    const headers = buildBackendRequestHeaders(source, {
      developmentUserId: '12345678-1234-1234-1234-123456789012'
    });

    expect(headers.get('accept-encoding')).toBeNull();
    expect(headers.get('authorization')).toBeNull();
    expect(headers.get('cookie')).toBeNull();
    expect(headers.get('connection')).toBeNull();
    expect(headers.get('content-type')).toBe('application/json');
    expect(headers.get('x-development-user-id')).toBe(
      '12345678-1234-1234-1234-123456789012'
    );
  });

  it('prefers a server-provided bearer token over development identity', () => {
    const headers = buildBackendRequestHeaders(new Headers(), {
      bearerToken: 'trusted-access-token',
      developmentUserId: '12345678-1234-1234-1234-123456789012'
    });

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

  it.each(['../admin', 'memos/../health', 'memos//admin', 'memos\\admin'])(
    'rejects path traversal or ambiguous path %s',
    (path) => {
      expect(() => buildBackendUrl('http://backend:8080', path, '')).toThrow(
        InvalidProxyPathError
      );
    }
  );
});

describe('buildFrontendResponseHeaders', () => {
  it('removes hop-by-hop response headers', () => {
    const headers = buildFrontendResponseHeaders(
      new Headers({
        Connection: 'keep-alive',
        'Content-Type': 'application/json',
        'Set-Cookie': 'memo=should-not-cross-boundary',
        'Transfer-Encoding': 'chunked'
      })
    );

    expect(headers.get('connection')).toBeNull();
    expect(headers.get('set-cookie')).toBeNull();
    expect(headers.get('transfer-encoding')).toBeNull();
    expect(headers.get('content-type')).toBe('application/json');
  });
});

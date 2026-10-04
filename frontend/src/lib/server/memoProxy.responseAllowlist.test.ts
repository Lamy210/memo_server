import { describe, expect, it } from 'vitest';

import { buildFrontendResponseHeaders } from './memoProxy';

describe('buildFrontendResponseHeaders explicit response allowlist', () => {
  it('preserves only the memo API response metadata owned by the public BFF contract', () => {
    const headers = buildFrontendResponseHeaders(
      new Headers({
        Allow: 'GET, HEAD',
        'Cache-Control': 'public, max-age=3600',
        'Content-Language': 'ja',
        'Content-Type': 'application/json',
        'Retry-After': '30',
        Server: 'memo-server-internal',
        'X-Backend-Debug': 'internal-detail',
        'X-Powered-By': 'internal-stack',
        'X-Request-Id': 'request-123'
      })
    );

    expect(headers.get('allow')).toBe('GET, HEAD');
    expect(headers.get('cache-control')).toBe('no-store');
    expect(headers.get('content-language')).toBe('ja');
    expect(headers.get('content-type')).toBe('application/json');
    expect(headers.get('retry-after')).toBe('30');
    expect(headers.get('x-request-id')).toBe('request-123');

    expect(headers.get('server')).toBeNull();
    expect(headers.get('x-backend-debug')).toBeNull();
    expect(headers.get('x-powered-by')).toBeNull();
  });
});

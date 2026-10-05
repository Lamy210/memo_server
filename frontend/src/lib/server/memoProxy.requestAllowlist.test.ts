import { describe, expect, it } from 'vitest';

import { buildBackendRequestHeaders } from './memoProxy';

describe('buildBackendRequestHeaders explicit request allowlist', () => {
  it('forwards only memo API request metadata owned by the BFF contract', () => {
    const headers = buildBackendRequestHeaders(
      new Headers({
        Accept: 'text/html',
        'Accept-Language': 'ja-JP',
        'Content-Type': 'application/json',
        'If-None-Match': '"browser-cache-tag"',
        Range: 'bytes=0-99',
        'User-Agent': 'browser-agent',
        'X-Client-Debug': 'browser-controlled',
        'X-Request-Id': 'request-123'
      }),
      { bearerToken: 'trusted-access-token' }
    );

    expect(headers.get('accept')).toBe('application/json');
    expect(headers.get('accept-encoding')).toBe('identity');
    expect(headers.get('authorization')).toBe('Bearer trusted-access-token');
    expect(headers.get('content-type')).toBe('application/json');
    expect(headers.get('x-request-id')).toBe('request-123');

    expect(headers.get('accept-language')).toBeNull();
    expect(headers.get('if-none-match')).toBeNull();
    expect(headers.get('range')).toBeNull();
    expect(headers.get('user-agent')).toBeNull();
    expect(headers.get('x-client-debug')).toBeNull();
  });
});

import { describe, expect, it } from 'vitest';

import { buildFrontendResponseHeaders } from './memoProxy';

describe('buildFrontendResponseHeaders CORS boundary', () => {
  it('does not expose backend CORS policy through the same-origin memo BFF', () => {
    const headers = buildFrontendResponseHeaders(
      new Headers({
        'Access-Control-Allow-Credentials': 'true',
        'Access-Control-Allow-Headers': '*',
        'Access-Control-Allow-Methods': 'GET, POST, PATCH, DELETE',
        'Access-Control-Allow-Origin': '*',
        'Access-Control-Expose-Headers': '*',
        'Access-Control-Max-Age': '86400',
        'Content-Type': 'application/json'
      })
    );

    expect(headers.get('access-control-allow-credentials')).toBeNull();
    expect(headers.get('access-control-allow-headers')).toBeNull();
    expect(headers.get('access-control-allow-methods')).toBeNull();
    expect(headers.get('access-control-allow-origin')).toBeNull();
    expect(headers.get('access-control-expose-headers')).toBeNull();
    expect(headers.get('access-control-max-age')).toBeNull();
    expect(headers.get('content-type')).toBe('application/json');
    expect(headers.get('cache-control')).toBe('no-store');
  });
});

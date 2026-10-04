import { describe, expect, it } from 'vitest';

import { buildBackendRequestHeaders } from './memoProxy';

describe('buildBackendRequestHeaders request-control boundary', () => {
  it('strips browser method and URL override headers before calling memo_server', () => {
    const headers = buildBackendRequestHeaders(
      new Headers({
        'Content-Type': 'application/json',
        'X-HTTP-Method': 'DELETE',
        'X-HTTP-Method-Override': 'DELETE',
        'X-Method-Override': 'DELETE',
        'X-Original-URL': '/api/v1/admin',
        'X-Rewrite-URL': '/api/v1/admin',
        'X-Request-Id': 'browser-request-id'
      }),
      {}
    );

    expect(headers.get('x-http-method')).toBeNull();
    expect(headers.get('x-http-method-override')).toBeNull();
    expect(headers.get('x-method-override')).toBeNull();
    expect(headers.get('x-original-url')).toBeNull();
    expect(headers.get('x-rewrite-url')).toBeNull();
    expect(headers.get('content-type')).toBe('application/json');
    expect(headers.get('x-request-id')).toBe('browser-request-id');
  });
});

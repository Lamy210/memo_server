import { describe, expect, it } from 'vitest';

import { buildBackendRequestHeaders, buildFrontendResponseHeaders } from './memoProxy';

describe('memo proxy Connection header semantics', () => {
  it('strips request headers named by Connection options before forwarding', () => {
    const headers = buildBackendRequestHeaders(
      new Headers({
        Connection: 'X-Hop-Only, X-Second-Hop',
        'Proxy-Connection': 'keep-alive',
        'X-Hop-Only': 'request-secret',
        'X-Second-Hop': 'request-secret-2',
        'X-End-To-End': 'preserve-me'
      }),
      {}
    );

    expect(headers.get('connection')).toBeNull();
    expect(headers.get('proxy-connection')).toBeNull();
    expect(headers.get('x-hop-only')).toBeNull();
    expect(headers.get('x-second-hop')).toBeNull();
    expect(headers.get('x-end-to-end')).toBe('preserve-me');
  });

  it('strips allowlisted response headers named by Connection options', () => {
    const headers = buildFrontendResponseHeaders(
      new Headers({
        Connection: 'X-Request-Id, X-Hop-Only',
        'Content-Type': 'application/json',
        'Proxy-Connection': 'keep-alive',
        'X-Hop-Only': 'response-secret',
        'X-Request-Id': 'request-123'
      })
    );

    expect(headers.get('connection')).toBeNull();
    expect(headers.get('proxy-connection')).toBeNull();
    expect(headers.get('x-hop-only')).toBeNull();
    expect(headers.get('x-request-id')).toBeNull();
    expect(headers.get('content-type')).toBe('application/json');
  });
});

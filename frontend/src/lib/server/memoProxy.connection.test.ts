import { describe, expect, it } from 'vitest';

import { buildBackendRequestHeaders, buildFrontendResponseHeaders } from './memoProxy';

describe('memo proxy Connection header semantics', () => {
  it('strips request headers named by Connection options before forwarding', () => {
    const headers = buildBackendRequestHeaders(
      new Headers({
        Connection: 'X-Hop-Only, X-Second-Hop',
        'X-Hop-Only': 'request-secret',
        'X-Second-Hop': 'request-secret-2',
        'X-End-To-End': 'preserve-me'
      }),
      {}
    );

    expect(headers.get('connection')).toBeNull();
    expect(headers.get('x-hop-only')).toBeNull();
    expect(headers.get('x-second-hop')).toBeNull();
    expect(headers.get('x-end-to-end')).toBe('preserve-me');
  });

  it('strips response headers named by Connection options before returning to the browser', () => {
    const headers = buildFrontendResponseHeaders(
      new Headers({
        Connection: 'X-Hop-Only, X-Second-Hop',
        'X-Hop-Only': 'response-secret',
        'X-Second-Hop': 'response-secret-2',
        'X-End-To-End': 'preserve-me'
      })
    );

    expect(headers.get('connection')).toBeNull();
    expect(headers.get('x-hop-only')).toBeNull();
    expect(headers.get('x-second-hop')).toBeNull();
    expect(headers.get('x-end-to-end')).toBe('preserve-me');
  });
});

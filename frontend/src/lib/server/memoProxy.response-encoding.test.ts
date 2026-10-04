import { describe, expect, it } from 'vitest';

import { buildFrontendResponseHeaders } from '$lib/server/memoProxy';

describe('buildFrontendResponseHeaders response encoding boundary', () => {
  it('removes backend content encoding metadata before returning the decoded body to the browser', () => {
    const headers = buildFrontendResponseHeaders(
      new Headers({
        'Content-Encoding': 'gzip',
        'Content-Type': 'application/json'
      })
    );

    expect(headers.get('content-encoding')).toBeNull();
    expect(headers.get('content-type')).toBe('application/json');
    expect(headers.get('cache-control')).toBe('no-store');
  });
});

import { describe, expect, it } from 'vitest';

import { buildFrontendResponseHeaders } from './memoProxy';

describe('buildFrontendResponseHeaders redirect boundary', () => {
  it('does not let backend redirects steer the browser-facing memo BFF', () => {
    const headers = buildFrontendResponseHeaders(
      new Headers({
        'Cache-Control': 'public, max-age=3600',
        'Content-Type': 'application/json',
        Location: 'https://attacker.example/collect',
        Refresh: '0; url=https://attacker.example/collect'
      })
    );

    expect(headers.get('location')).toBeNull();
    expect(headers.get('refresh')).toBeNull();
    expect(headers.get('content-type')).toBe('application/json');
    expect(headers.get('cache-control')).toBe('no-store');
  });
});

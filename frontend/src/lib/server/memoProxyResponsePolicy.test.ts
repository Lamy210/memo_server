import { describe, expect, it } from 'vitest';

import { buildFrontendResponseHeaders } from './memoProxy';

describe('buildFrontendResponseHeaders browser-origin policy boundary', () => {
  it('strips backend-controlled browser origin policy headers', () => {
    const headers = buildFrontendResponseHeaders(
      new Headers({
        'Alt-Svc': 'h3=":443"',
        'Clear-Site-Data': '"cookies", "storage"',
        'Content-Type': 'application/json',
        'Strict-Transport-Security': 'max-age=31536000; includeSubDomains'
      })
    );

    expect(headers.get('alt-svc')).toBeNull();
    expect(headers.get('clear-site-data')).toBeNull();
    expect(headers.get('strict-transport-security')).toBeNull();
    expect(headers.get('cache-control')).toBe('no-store');
    expect(headers.get('content-type')).toBe('application/json');
  });
});

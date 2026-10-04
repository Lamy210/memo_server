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

  it('strips backend X-Accel control headers before the outer proxy can interpret them', () => {
    const headers = buildFrontendResponseHeaders(
      new Headers({
        'Content-Type': 'application/json',
        'X-Accel-Buffering': 'no',
        'X-Accel-Expires': '3600',
        'X-Accel-Limit-Rate': '1',
        'X-Accel-Redirect': '/internal/private-file'
      })
    );

    expect(headers.get('x-accel-buffering')).toBeNull();
    expect(headers.get('x-accel-expires')).toBeNull();
    expect(headers.get('x-accel-limit-rate')).toBeNull();
    expect(headers.get('x-accel-redirect')).toBeNull();
    expect(headers.get('cache-control')).toBe('no-store');
    expect(headers.get('content-type')).toBe('application/json');
  });
});

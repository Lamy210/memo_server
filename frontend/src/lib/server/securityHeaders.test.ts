import { describe, expect, it } from 'vitest';

import { applyFrontendSecurityHeaders, FRONTEND_SECURITY_HEADERS } from './securityHeaders';

describe('applyFrontendSecurityHeaders', () => {
  it('applies the production browser security baseline', () => {
    const headers = new Headers({
      'Content-Type': 'text/html',
      'X-Frame-Options': 'SAMEORIGIN'
    });

    applyFrontendSecurityHeaders(headers);

    expect(headers.get('content-type')).toBe('text/html');
    for (const [name, value] of Object.entries(FRONTEND_SECURITY_HEADERS)) {
      expect(headers.get(name)).toBe(value);
    }
  });

  it('overwrites weaker values rather than preserving them', () => {
    const headers = new Headers({
      'Referrer-Policy': 'unsafe-url',
      'X-Content-Type-Options': 'off'
    });

    applyFrontendSecurityHeaders(headers);

    expect(headers.get('referrer-policy')).toBe('strict-origin-when-cross-origin');
    expect(headers.get('x-content-type-options')).toBe('nosniff');
  });
});

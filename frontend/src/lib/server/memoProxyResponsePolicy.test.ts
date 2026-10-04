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

  it('strips backend-controlled document isolation and reporting policy headers', () => {
    const headers = buildFrontendResponseHeaders(
      new Headers({
        'Content-Security-Policy': "default-src 'none'; report-to backend-csp",
        'Content-Security-Policy-Report-Only': "default-src 'none'; report-to backend-csp",
        'Content-Type': 'application/json',
        'Cross-Origin-Embedder-Policy': 'require-corp',
        NEL: '{"report_to":"backend","max_age":86400}',
        'Origin-Agent-Cluster': '?1',
        'Report-To': '{"group":"backend","max_age":86400,"endpoints":[{"url":"https://backend.example/reports"}]}',
        'Reporting-Endpoints': 'backend="https://backend.example/reports"'
      })
    );

    expect(headers.get('content-security-policy')).toBeNull();
    expect(headers.get('content-security-policy-report-only')).toBeNull();
    expect(headers.get('cross-origin-embedder-policy')).toBeNull();
    expect(headers.get('nel')).toBeNull();
    expect(headers.get('origin-agent-cluster')).toBeNull();
    expect(headers.get('report-to')).toBeNull();
    expect(headers.get('reporting-endpoints')).toBeNull();
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

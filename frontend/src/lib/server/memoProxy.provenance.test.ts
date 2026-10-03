import { describe, expect, it } from 'vitest';

import { buildBackendRequestHeaders } from './memoProxy';

describe('buildBackendRequestHeaders browser provenance boundary', () => {
  it('strips browser origin, referrer, and Fetch Metadata before calling memo_server', () => {
    const headers = buildBackendRequestHeaders(
      new Headers({
        Origin: 'https://attacker.example',
        Referer: 'https://attacker.example/source',
        'Sec-Fetch-Dest': 'empty',
        'Sec-Fetch-Mode': 'cors',
        'Sec-Fetch-Site': 'cross-site',
        'Sec-Fetch-User': '?1',
        'X-Request-Id': 'browser-request-id'
      }),
      {}
    );

    expect(headers.get('origin')).toBeNull();
    expect(headers.get('referer')).toBeNull();
    expect(headers.get('sec-fetch-dest')).toBeNull();
    expect(headers.get('sec-fetch-mode')).toBeNull();
    expect(headers.get('sec-fetch-site')).toBeNull();
    expect(headers.get('sec-fetch-user')).toBeNull();
    expect(headers.get('x-request-id')).toBe('browser-request-id');
  });
});

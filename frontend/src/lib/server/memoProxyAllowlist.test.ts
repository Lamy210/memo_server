import { describe, expect, it } from 'vitest';

import { buildBackendUrl, InvalidProxyPathError } from './memoProxy';

describe('buildBackendUrl memo API allowlist', () => {
  it.each([
    ['memos', 'http://backend:8080/api/v1/memos'],
    ['memos/search', 'http://backend:8080/api/v1/memos/search'],
    ['memos/018f0c7a-8b7d-7f25-b239-36e6d9f9b001', 'http://backend:8080/api/v1/memos/018f0c7a-8b7d-7f25-b239-36e6d9f9b001']
  ])('allows the current memo API route shape %s', (path, expected) => {
    expect(buildBackendUrl('http://backend:8080', path, '').toString()).toBe(expected);
  });

  it.each([
    undefined,
    '',
    'health',
    'health/ready',
    'admin/users',
    'memos/018f0c7a-8b7d-7f25-b239-36e6d9f9b001/history'
  ])('rejects non-memo or future internal API path %s', (path) => {
    expect(() => buildBackendUrl('http://backend:8080', path, '')).toThrow(
      InvalidProxyPathError
    );
  });

  it('allows the current search query surface', () => {
    expect(
      buildBackendUrl(
        'http://backend:8080',
        'memos/search',
        '?query=snow&tag=work&page=2&limit=20'
      ).toString()
    ).toBe('http://backend:8080/api/v1/memos/search?query=snow&tag=work&page=2&limit=20');
  });

  it.each([
    ['memos', '?query=snow'],
    ['memos/018f0c7a-8b7d-7f25-b239-36e6d9f9b001', '?tag=work'],
    ['memos/search', '?sort=updated_at'],
    ['memos/search', '?page=1&page=2'],
    ['memos/search', '?query=snow&query=flake']
  ])('rejects query surface outside the current memo API contract for %s %s', (path, search) => {
    expect(() => buildBackendUrl('http://backend:8080', path, search)).toThrow(
      'Invalid memo API query'
    );
  });
});

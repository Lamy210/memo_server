import { describe, expect, it } from 'vitest';

import { buildSearchQuery } from './memo';

describe('buildSearchQuery', () => {
  it('uses the backend query parameter contract', () => {
    expect(
      buildSearchQuery({ query: 'hello world', tag: 'rust', page: 2, limit: 20 })
    ).toBe('query=hello+world&tag=rust&page=2&limit=20');
  });

  it('omits empty optional values', () => {
    expect(buildSearchQuery({ query: '', page: 1 })).toBe('page=1');
  });
});

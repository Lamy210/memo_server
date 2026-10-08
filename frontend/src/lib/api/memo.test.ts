import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  ApiError,
  AUTH_REQUIRED_MESSAGE,
  buildMemoListQuery,
  buildSearchQuery,
  fetchMemosPage,
  getApiErrorMessage,
  isUnauthorizedApiError
} from './memo';

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('buildMemoListQuery', () => {
  it('always selects cursor-v1 and defaults to twenty items', () => {
    expect(buildMemoListQuery()).toBe('pagination=cursor-v1&limit=20');
  });

  it('serializes an exclusive continuation cursor without changing it', () => {
    expect(
      buildMemoListQuery({
        limit: 40,
        cursor: 'v1.550e8400-e29b-41d4-a716-446655440003'
      })
    ).toBe(
      'pagination=cursor-v1&limit=40&cursor=v1.550e8400-e29b-41d4-a716-446655440003'
    );
  });

  it('uses the cursor-v1 query contract when fetching a page', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          pagination: 'cursor-v1',
          items: [],
          limit: 20,
          next_cursor: null
        }),
        {
          status: 200,
          headers: { 'Content-Type': 'application/json' }
        }
      )
    );
    vi.stubGlobal('fetch', fetchMock);

    await fetchMemosPage({ cursor: 'v1.550e8400-e29b-41d4-a716-446655440003' });

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(fetchMock.mock.calls[0]?.[0]).toBe(
      '/api/v1/memos?pagination=cursor-v1&limit=20&cursor=v1.550e8400-e29b-41d4-a716-446655440003'
    );
  });
});

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

describe('API error handling', () => {
  it('recognizes unauthorized API errors', () => {
    expect(isUnauthorizedApiError(new ApiError('expired', 401))).toBe(true);
    expect(isUnauthorizedApiError(new ApiError('forbidden', 403))).toBe(false);
    expect(isUnauthorizedApiError(new Error('network'))).toBe(false);
  });

  it('uses a stable message for unauthorized responses', () => {
    expect(getApiErrorMessage(new ApiError('provider-specific detail', 401), 'fallback')).toBe(
      AUTH_REQUIRED_MESSAGE
    );
  });

  it('preserves non-auth API messages and fallback behavior', () => {
    expect(getApiErrorMessage(new ApiError('conflict', 409), 'fallback')).toBe('conflict');
    expect(getApiErrorMessage('unknown', 'fallback')).toBe('fallback');
  });
});

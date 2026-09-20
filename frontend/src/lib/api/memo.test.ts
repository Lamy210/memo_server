import { describe, expect, it } from 'vitest';

import {
  ApiError,
  AUTH_REQUIRED_MESSAGE,
  buildSearchQuery,
  getApiErrorMessage,
  isUnauthorizedApiError
} from './memo';

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

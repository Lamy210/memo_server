import { describe, expect, it } from 'vitest';

import {
  buildMemoBackendFailureResponse,
  fetchMemoBackend,
  MEMO_BFF_BACKEND_REQUEST_TIMEOUT_MS,
  MemoBackendTimeoutError
} from './memoBackendRequest';

describe('fetchMemoBackend', () => {
  it('uses the shared 30 second backend deadline', () => {
    expect(MEMO_BFF_BACKEND_REQUEST_TIMEOUT_MS).toBe(30_000);
  });

  it('aborts a stalled backend request when the deadline expires', async () => {
    const stalledFetch = async (
      _input: RequestInfo | URL,
      init?: RequestInit
    ): Promise<Response> =>
      new Promise((_resolve, reject) => {
        const signal = init?.signal;
        if (!signal) {
          reject(new Error('backend request signal is missing'));
          return;
        }
        signal.addEventListener('abort', () => reject(signal.reason), { once: true });
      });

    await expect(
      fetchMemoBackend(
        stalledFetch,
        new URL('http://backend:8080/api/v1/memos'),
        { method: 'GET' },
        5
      )
    ).rejects.toBeInstanceOf(MemoBackendTimeoutError);
  });
});

describe('buildMemoBackendFailureResponse', () => {
  it('maps backend deadline expiration to gateway timeout', async () => {
    const response = buildMemoBackendFailureResponse(new MemoBackendTimeoutError());

    expect(response.status).toBe(504);
    await expect(response.json()).resolves.toEqual({ message: 'Memo backend request timed out' });
  });

  it('keeps non-timeout backend failures as bad gateway', async () => {
    const response = buildMemoBackendFailureResponse(new Error('connection reset'));

    expect(response.status).toBe(502);
    await expect(response.json()).resolves.toEqual({ message: 'Memo backend is unavailable' });
  });
});

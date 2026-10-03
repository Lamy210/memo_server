export const MEMO_BFF_BACKEND_REQUEST_TIMEOUT_MS = 30_000;

type BackendFetch = (
  input: RequestInfo | URL,
  init?: RequestInit
) => Promise<Response>;

export class MemoBackendTimeoutError extends Error {
  constructor() {
    super('Memo backend request exceeded the configured deadline');
    this.name = 'MemoBackendTimeoutError';
  }
}

export async function fetchMemoBackend(
  fetchFn: BackendFetch,
  target: URL,
  init: RequestInit,
  timeoutMs = MEMO_BFF_BACKEND_REQUEST_TIMEOUT_MS
): Promise<Response> {
  const signal = AbortSignal.timeout(timeoutMs);

  try {
    return await fetchFn(target, { ...init, signal });
  } catch (error) {
    if (signal.aborted) {
      throw new MemoBackendTimeoutError();
    }
    throw error;
  }
}

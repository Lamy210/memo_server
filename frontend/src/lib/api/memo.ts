import type {
  CreateMemoInput,
  Memo,
  SearchParams,
  SearchResult,
  UpdateMemoInput
} from './types';

const API_BASE = '/api/v1';
export const AUTH_REQUIRED_MESSAGE = '認証セッションが無効です。再ログインしてください。';

export class ApiError extends Error {
  constructor(
    message: string,
    public readonly status: number
  ) {
    super(message);
    this.name = 'ApiError';
  }
}

export function isUnauthorizedApiError(error: unknown): error is ApiError {
  return error instanceof ApiError && error.status === 401;
}

export function getApiErrorMessage(error: unknown, fallback: string): string {
  if (isUnauthorizedApiError(error)) return AUTH_REQUIRED_MESSAGE;
  return error instanceof Error ? error.message : fallback;
}

async function request<T>(url: string, init?: RequestInit): Promise<T> {
  const response = await fetch(url, init);
  if (!response.ok) {
    let message = `Request failed with status ${response.status}`;
    try {
      const body = (await response.json()) as { message?: string };
      if (body.message) message = body.message;
    } catch {
      // Keep the HTTP status based fallback when the response is not JSON.
    }
    throw new ApiError(message, response.status);
  }

  if (response.status === 204) return undefined as T;
  return (await response.json()) as T;
}

export function buildSearchQuery(params: SearchParams): string {
  const searchParams = new URLSearchParams();
  if (params.query) searchParams.set('query', params.query);
  if (params.tag) searchParams.set('tag', params.tag);
  if (params.page) searchParams.set('page', params.page.toString());
  if (params.limit) searchParams.set('limit', params.limit.toString());
  return searchParams.toString();
}

export function fetchMemos(): Promise<Memo[]> {
  return request<Memo[]>(`${API_BASE}/memos`);
}

export function fetchMemoById(id: string): Promise<Memo> {
  return request<Memo>(`${API_BASE}/memos/${id}`);
}

export function createMemo(data: CreateMemoInput): Promise<Memo> {
  return request<Memo>(`${API_BASE}/memos`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data)
  });
}

export function updateMemo(id: string, data: UpdateMemoInput): Promise<Memo> {
  return request<Memo>(`${API_BASE}/memos/${id}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(data)
  });
}

export function deleteMemo(id: string): Promise<void> {
  return request<void>(`${API_BASE}/memos/${id}`, { method: 'DELETE' });
}

export function searchMemos(params: SearchParams): Promise<SearchResult<Memo>> {
  const query = buildSearchQuery(params);
  return request<SearchResult<Memo>>(`${API_BASE}/memos/search${query ? `?${query}` : ''}`);
}

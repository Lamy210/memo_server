export const MEMO_BFF_MUTATION_HEADER = 'X-Schnee-Memo-Request';
export const MEMO_BFF_MUTATION_VALUE = '1';

const MUTATION_METHODS = new Set(['POST', 'PUT', 'PATCH', 'DELETE']);

export function isMemoMutationMethod(method: string): boolean {
  return MUTATION_METHODS.has(method.toUpperCase());
}

export function buildMemoRequestInit(init?: RequestInit): RequestInit | undefined {
  if (!isMemoMutationMethod(init?.method ?? 'GET')) return init;

  const headers = new Headers(init?.headers);
  headers.set(MEMO_BFF_MUTATION_HEADER, MEMO_BFF_MUTATION_VALUE);

  return {
    ...init,
    headers
  };
}

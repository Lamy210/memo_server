const MEMO_LIST_QUERY_KEYS = new Set(['pagination', 'limit', 'cursor']);
const MEMO_SEARCH_QUERY_KEYS = new Set(['query', 'tag', 'page', 'limit']);

export class InvalidProxyQueryError extends Error {
  constructor() {
    super('Invalid memo API query');
    this.name = 'InvalidProxyQueryError';
  }
}

export function validateMemoProxyQuery(path: string | undefined, search: string): void {
  const params = new URLSearchParams(search);
  if ([...params].length === 0) {
    return;
  }

  const allowedQueryKeys =
    path === 'memos'
      ? MEMO_LIST_QUERY_KEYS
      : path === 'memos/search'
        ? MEMO_SEARCH_QUERY_KEYS
        : undefined;
  if (!allowedQueryKeys) {
    throw new InvalidProxyQueryError();
  }

  const seen = new Set<string>();
  for (const [name] of params) {
    if (!allowedQueryKeys.has(name) || seen.has(name)) {
      throw new InvalidProxyQueryError();
    }
    seen.add(name);
  }
}

import { describe, expect, it } from 'vitest';

import {
  buildMemoRequestInit,
  isMemoMutationMethod,
  MEMO_BFF_MUTATION_HEADER,
  MEMO_BFF_MUTATION_VALUE
} from './memoBff';

describe('memo BFF mutation protection', () => {
  it.each(['POST', 'PUT', 'PATCH', 'DELETE', 'post'])(
    'marks %s as a mutation method',
    (method) => {
      expect(isMemoMutationMethod(method)).toBe(true);
    }
  );

  it.each(['GET', 'HEAD', 'OPTIONS'])('does not mark %s as a mutation method', (method) => {
    expect(isMemoMutationMethod(method)).toBe(false);
  });

  it('adds the BFF mutation marker while preserving request headers', () => {
    const init = buildMemoRequestInit({
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: '{}'
    });

    const headers = new Headers(init?.headers);
    expect(headers.get('content-type')).toBe('application/json');
    expect(headers.get(MEMO_BFF_MUTATION_HEADER)).toBe(MEMO_BFF_MUTATION_VALUE);
    expect(init?.body).toBe('{}');
  });

  it('does not add mutation metadata to safe requests', () => {
    const init = { method: 'GET' };
    expect(buildMemoRequestInit(init)).toBe(init);
    expect(buildMemoRequestInit()).toBeUndefined();
  });
});

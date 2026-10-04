import { describe, expect, it } from 'vitest';

import {
  UnexpectedBackendContentEncodingError,
  validateMemoBackendResponseEncoding
} from '$lib/server/memoProxy';

describe('memo backend response encoding boundary', () => {
  it('accepts the identity backend response shape', () => {
    expect(() => validateMemoBackendResponseEncoding(new Headers())).not.toThrow();
  });

  it('rejects encoded backend responses instead of guessing whether fetch decoded the body', () => {
    expect(() =>
      validateMemoBackendResponseEncoding(
        new Headers({
          'Content-Encoding': 'gzip'
        })
      )
    ).toThrow(UnexpectedBackendContentEncodingError);
  });
});

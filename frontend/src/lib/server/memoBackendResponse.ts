export class UnexpectedBackendContentEncodingError extends Error {
  constructor() {
    super('Memo backend returned unexpected content encoding');
    this.name = 'UnexpectedBackendContentEncodingError';
  }
}

export function validateMemoBackendResponseEncoding(headers: Headers): void {
  if (headers.has('content-encoding')) {
    throw new UnexpectedBackendContentEncodingError();
  }
}

const INVALID_SERVICE_ORIGIN_RAW_CHARACTER = /[\\\u0000-\u0020\u007f]/;

export class InvalidServiceOriginError extends Error {
  constructor() {
    super('Invalid trusted service origin');
    this.name = 'InvalidServiceOriginError';
  }
}

function hasOriginOnlyRawShape(value: string): boolean {
  if (INVALID_SERVICE_ORIGIN_RAW_CHARACTER.test(value)) {
    return false;
  }

  const schemeEnd = value.indexOf('://');
  if (schemeEnd < 0) {
    return false;
  }

  const remainder = value.slice(schemeEnd + 3);
  const authorityEnd = remainder.search(/[/?#]/);
  const splitAt = authorityEnd < 0 ? remainder.length : authorityEnd;
  const authority = remainder.slice(0, splitAt);
  const suffix = remainder.slice(splitAt);

  return authority.length > 0 && !authority.includes('@') && (suffix === '' || suffix === '/');
}

export function parseTrustedServiceOrigin(value: string): URL {
  let target: URL;
  try {
    target = new URL(value);
  } catch {
    throw new InvalidServiceOriginError();
  }

  const valid =
    (target.protocol === 'http:' || target.protocol === 'https:') &&
    target.hostname.length > 0 &&
    hasOriginOnlyRawShape(value) &&
    target.username === '' &&
    target.password === '' &&
    target.pathname === '/' &&
    target.search === '' &&
    target.hash === '';

  if (!valid) {
    throw new InvalidServiceOriginError();
  }

  return target;
}

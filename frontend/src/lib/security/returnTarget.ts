export const DEFAULT_RETURN_TARGET = '/memos';

const RETURN_TARGET_ORIGIN = 'https://memo-return-target.invalid';
const MAX_RETURN_TARGET_LENGTH = 2048;
const INVALID_RETURN_TARGET_CHARACTER = /[\\\u0000-\u001f\u007f]/;

function normalizeInternalReturnTarget(value: string | null | undefined): string | null {
  if (!value || value.length > MAX_RETURN_TARGET_LENGTH) return null;

  // Only accept an absolute-path reference on this application. In particular,
  // reject scheme-relative URLs ("//example.com") and parser-confusing
  // backslashes before handing the value to the URL parser.
  if (!value.startsWith('/') || value.startsWith('//') || INVALID_RETURN_TARGET_CHARACTER.test(value)) {
    return null;
  }

  try {
    const target = new URL(value, RETURN_TARGET_ORIGIN);
    if (target.origin !== RETURN_TARGET_ORIGIN) return null;

    return `${target.pathname}${target.search}${target.hash}`;
  } catch {
    return null;
  }
}

export function resolveSafeReturnTarget(
  candidate: string | null | undefined,
  fallback = DEFAULT_RETURN_TARGET
): string {
  return (
    normalizeInternalReturnTarget(candidate) ??
    normalizeInternalReturnTarget(fallback) ??
    DEFAULT_RETURN_TARGET
  );
}

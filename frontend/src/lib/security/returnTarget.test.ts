import { describe, expect, it } from 'vitest';

import { DEFAULT_RETURN_TARGET, resolveSafeReturnTarget } from './returnTarget';

describe('resolveSafeReturnTarget', () => {
  it.each([
    ['/', '/'],
    ['/memos', '/memos'],
    ['/memos/new?source=login#editor', '/memos/new?source=login#editor'],
    ['/memos/../memos/search?query=rust', '/memos/search?query=rust'],
    ['/memos?next=https://attacker.example', '/memos?next=https://attacker.example']
  ])('accepts and canonicalizes internal path %s', (candidate, expected) => {
    expect(resolveSafeReturnTarget(candidate)).toBe(expected);
  });

  it.each([
    undefined,
    null,
    '',
    'memos',
    'https://attacker.example/phish',
    'http://attacker.example/phish',
    '//attacker.example/phish',
    '/\\attacker.example/phish',
    '\\attacker.example/phish',
    'javascript:alert(1)',
    'data:text/html,attack',
    '/memos\nhttps://attacker.example'
  ])('rejects unsafe return target %s', (candidate) => {
    expect(resolveSafeReturnTarget(candidate)).toBe(DEFAULT_RETURN_TARGET);
  });

  it('bounds untrusted return-target length', () => {
    expect(resolveSafeReturnTarget(`/memos?${'a'.repeat(2048)}`)).toBe(DEFAULT_RETURN_TARGET);
  });

  it('uses a validated internal fallback', () => {
    expect(resolveSafeReturnTarget('https://attacker.example', '/memos/new')).toBe('/memos/new');
  });

  it('does not trust an unsafe fallback', () => {
    expect(
      resolveSafeReturnTarget('https://attacker.example', '//fallback-attacker.example')
    ).toBe(DEFAULT_RETURN_TARGET);
  });
});

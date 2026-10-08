import { describe, expect, it } from 'vitest';

import { InvalidServiceOriginError, parseTrustedServiceOrigin } from './serviceOrigin';

describe('parseTrustedServiceOrigin', () => {
  it.each([
    ['http://backend:8080', 'http://backend:8080/'],
    ['https://auth.example.com/', 'https://auth.example.com/']
  ])('accepts a strict HTTP(S) origin %s', (value, expected) => {
    expect(parseTrustedServiceOrigin(value).toString()).toBe(expected);
  });

  it.each([
    'ftp://backend:8080',
    'file:///tmp/memo',
    'backend:8080',
    'http://user:secret@backend:8080',
    'http://@backend:8080',
    'http://backend:8080/internal',
    'http://backend:8080/internal/..',
    'http://backend:8080\\internal\\..',
    'http://back\tend:8080',
    'http://backend:8080/?',
    'http://backend:8080/#',
    'http://backend:8080?tenant=a',
    'http://backend:8080#memo'
  ])('rejects unsafe or ambiguous service origin %s', (value) => {
    expect(() => parseTrustedServiceOrigin(value)).toThrow(InvalidServiceOriginError);
  });
});

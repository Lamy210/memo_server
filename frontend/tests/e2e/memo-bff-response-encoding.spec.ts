import { expect, test } from '@playwright/test';

const FIXTURE_ORIGIN = 'http://127.0.0.1:18080';

test('fails closed when the memo backend violates the identity-encoding contract', async ({
  request
}) => {
  const scenario = await request.post(`${FIXTURE_ORIGIN}/__visual__/scenario/encoded`);
  expect(scenario.status()).toBe(200);

  try {
    const response = await request.get('/api/v1/memos');

    expect(response.status()).toBe(502);
    await expect(response.json()).resolves.toEqual({ message: 'Memo backend is unavailable' });
  } finally {
    const reset = await request.post(`${FIXTURE_ORIGIN}/__visual__/scenario/success`);
    expect(reset.status()).toBe(200);
  }
});

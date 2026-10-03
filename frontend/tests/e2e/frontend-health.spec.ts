import { expect, test } from '@playwright/test';

test('serves frontend health locally without exposing backend health routes', async ({ request }) => {
  const frontendHealth = await request.get('/api/v1/health');

  expect(frontendHealth.status()).toBe(200);
  expect(frontendHealth.headers()['cache-control']).toBe('no-store');
  await expect(frontendHealth.json()).resolves.toEqual({
    status: 'ok',
    service: 'frontend'
  });

  const backendReadiness = await request.get('/api/v1/health/ready');
  expect(backendReadiness.status()).toBe(400);
  await expect(backendReadiness.json()).resolves.toEqual({
    message: 'Invalid memo API path'
  });
});

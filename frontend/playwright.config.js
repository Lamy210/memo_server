import { defineConfig } from '@playwright/test';

const identityE2E = process.env.IDENTITY_E2E === '1';

export default defineConfig({
  testDir: 'tests/e2e',
  testIgnore: identityE2E ? undefined : ['**/auth-session.spec.ts'],
  fullyParallel: false,
  workers: 1,
  globalTimeout: 90_000,
  timeout: 30_000,
  expect: {
    timeout: 5_000
  },
  use: {
    baseURL: 'http://127.0.0.1:4173',
    colorScheme: 'light',
    ignoreHTTPSErrors: identityE2E,
    locale: 'ja-JP',
    reducedMotion: 'reduce',
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
    timezoneId: 'Asia/Tokyo',
    viewport: { width: 1440, height: 1000 }
  },
  reporter: [['line'], ['html', { open: 'never', outputFolder: 'playwright-report' }]]
});

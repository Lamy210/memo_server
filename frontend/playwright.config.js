import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: 'tests/e2e',
  fullyParallel: false,
  workers: 1,
  timeout: 30_000,
  expect: {
    timeout: 5_000
  },
  use: {
    baseURL: 'http://127.0.0.1:4173',
    colorScheme: 'light',
    locale: 'ja-JP',
    reducedMotion: 'reduce',
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
    timezoneId: 'Asia/Tokyo',
    viewport: { width: 1440, height: 1000 }
  },
  reporter: [['line'], ['html', { open: 'never', outputFolder: 'playwright-report' }]],
  webServer: [
    {
      command: 'python3 ../scripts/ui-fixture-server.py --port 18080',
      url: 'http://127.0.0.1:18080/healthz',
      reuseExistingServer: false,
      timeout: 30_000
    },
    {
      command:
        'pnpm build && BACKEND_URL=http://127.0.0.1:18080 pnpm preview --host 127.0.0.1 --port 4173 --strictPort',
      url: 'http://127.0.0.1:4173/memos',
      reuseExistingServer: false,
      timeout: 60_000
    }
  ]
});

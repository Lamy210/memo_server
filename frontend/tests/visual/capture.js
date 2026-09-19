import { chromium } from '@playwright/test';

const BASE_URL = 'http://127.0.0.1:4173';
const OUTPUT_DIR = '.visual-output';

const memoId = '018f0c7a-8b7d-7f25-b239-36e6d9f9b001';
const userId = '12345678-1234-1234-1234-123456789012';
const timestamp = '2026-09-19T08:30:00.000Z';

const memos = [
  {
    id: memoId,
    title: 'UI regression baseline',
    content: 'Visual regression testing keeps layout changes reviewable before merge.',
    tags: ['ui', 'ci', 'playwright'],
    user_id: userId,
    created_at: timestamp,
    updated_at: timestamp,
    version: 3
  },
  {
    id: '018f0c7a-8b7d-7f25-b239-36e6d9f9b002',
    title: 'Release checklist',
    content: 'Run checks, inspect visual diffs, review the smoke test, then merge.',
    tags: ['release', 'quality'],
    user_id: userId,
    created_at: timestamp,
    updated_at: timestamp,
    version: 2
  },
  {
    id: '018f0c7a-8b7d-7f25-b239-36e6d9f9b003',
    title: 'Architecture notes',
    content: 'ScyllaDB is authoritative. Redis and Elasticsearch are rebuildable projections.',
    tags: ['architecture', 'backend'],
    user_id: userId,
    created_at: timestamp,
    updated_at: timestamp,
    version: 7
  }
];

/**
 * @param {import('@playwright/test').Route} route
 * @param {unknown} body
 * @param {number} [status]
 */
function json(route, body, status = 200) {
  return route.fulfill({
    status,
    contentType: 'application/json',
    body: JSON.stringify(body)
  });
}

/** @param {import('@playwright/test').Page} page */
async function installApiFixture(page) {
  await page.route('**/api/v1/**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const pathname = url.pathname;

    if (request.method() === 'GET' && pathname.endsWith('/memos/search')) {
      return json(route, {
        items: [memos[0], memos[2]],
        total: 2,
        page: 1,
        total_pages: 1
      });
    }

    if (request.method() === 'GET' && pathname.endsWith(`/memos/${memoId}`)) {
      return json(route, memos[0]);
    }

    if (request.method() === 'GET' && pathname.endsWith('/memos')) {
      return json(route, memos);
    }

    if (request.method() === 'POST' && pathname.endsWith('/memos')) {
      return json(route, { ...memos[0], version: 1 }, 201);
    }

    if (request.method() === 'PATCH' && pathname.includes('/memos/')) {
      return json(route, { ...memos[0], version: 4 });
    }

    if (request.method() === 'DELETE' && pathname.includes('/memos/')) {
      return route.fulfill({ status: 204 });
    }

    return json(route, { message: 'Unexpected visual-test API request' }, 500);
  });
}

/**
 * @param {import('@playwright/test').Page} page
 * @param {string} pathname
 * @param {string} filename
 * @param {string} readyText
 */
async function capture(page, pathname, filename, readyText) {
  await page.goto(new URL(pathname, BASE_URL).toString(), { waitUntil: 'networkidle' });
  await page.getByText(readyText, { exact: false }).first().waitFor();
  await page.evaluate(() => document.fonts.ready);
  await page.screenshot({
    path: `${OUTPUT_DIR}/${filename}`,
    fullPage: true,
    animations: 'disabled'
  });
}

const browser = await chromium.launch();
const context = await browser.newContext({
  viewport: { width: 1440, height: 1000 },
  deviceScaleFactor: 1,
  colorScheme: 'light',
  locale: 'ja-JP',
  timezoneId: 'Asia/Tokyo',
  reducedMotion: 'reduce'
});

const page = await context.newPage();
await installApiFixture(page);

await capture(page, '/memos', 'memos.png', 'UI regression baseline');
await capture(page, '/memos/search?query=architecture', 'search.png', 'Architecture notes');
await capture(page, '/memos/new', 'new.png', '新しいメモ');
await capture(page, `/memos/${memoId}/edit`, 'edit.png', 'UI regression baseline');

await browser.close();

import { chromium } from '@playwright/test';

const BASE_URL = 'http://127.0.0.1:4173';
const OUTPUT_DIR = '.visual-output';
const memoId = '018f0c7a-8b7d-7f25-b239-36e6d9f9b001';

/**
 * @param {import('@playwright/test').Page} page
 * @param {string} pathname
 * @param {string} filename
 * @param {string | RegExp} readyText
 */
async function capture(page, pathname, filename, readyText) {
  await page.goto(new URL(pathname, BASE_URL).toString(), { waitUntil: 'networkidle' });

  try {
    await page.getByText(readyText, { exact: false }).first().waitFor();
  } catch (error) {
    const bodyText = await page.locator('body').innerText().catch(() => '<body unavailable>');
    console.error(`Visual capture did not reach expected text "${readyText}" at ${page.url()}`);
    console.error(bodyText);
    await page.screenshot({
      path: `${OUTPUT_DIR}/${filename.replace('.png', '-failure.png')}`,
      fullPage: true,
      animations: 'disabled'
    });
    throw error;
  }

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
page.on('console', (message) => console.log(`browser:${message.type()}: ${message.text()}`));
page.on('pageerror', (error) => console.error(`browser:pageerror: ${error.message}`));

await capture(page, '/memos', 'memos.png', 'UI regression baseline');
await capture(page, '/memos/search?query=architecture', 'search.png', 'Architecture notes');
await capture(page, '/memos/new', 'new.png', '新しいメモ');
await capture(
  page,
  `/memos/${memoId}/edit`,
  'edit.png',
  'Visual regression testing keeps layout changes reviewable before merge.'
);

await browser.close();

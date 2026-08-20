import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { chromium } from 'playwright-core';

const baseUrl = process.env.VIEWER_URL ?? 'http://127.0.0.1:5173';
const executablePath = process.env.PLAYWRIGHT_EXECUTABLE_PATH
  ?? '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge';
const outputDirectory = process.env.VISUAL_OUTPUT_DIR
  ?? new URL('../../../temp/playwright/v0.4.0/', import.meta.url).pathname;
await mkdir(outputDirectory, { recursive: true });

const browser = await chromium.launch({ executablePath, headless: true });
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
const pageErrors = [];
page.on('pageerror', (error) => pageErrors.push(error.message));

try {
  await page.goto(baseUrl, { waitUntil: 'networkidle' });
  await page.fill('#server-url', 'https://127.0.0.1:8443');
  await page.fill('#ca-cert-path', '/tmp/acceptance-ca.pem');
  await page.fill('#login-username', 'admin');
  await page.fill('#login-password', 'acceptance');
  await page.click('#login-btn');
  await page.locator('#queue-page').waitFor({ state: 'visible' });
  await page.click('#more-menu-button');
  await page.click('#admin-console-btn');
  await page.locator('#admin-console-dialog').waitFor({ state: 'visible' });
  await page.click('[data-admin-tab="retrieval"]');
  await page.getByText('华东影像中心 PACS', { exact: true }).first().waitFor({ state: 'visible' });

  assert.equal(await page.locator('.retrieval-source-row').count(), 2, '应列出可配置的已批准设备');
  assert.equal(await page.locator('.retrieval-query-form').isVisible(), true, '应显示外部 PACS 查询表单');
  assert.equal(await page.locator('.retrieval-job').count(), 2, '应显示持久化拉取任务');
  assert.equal(await page.getByRole('button', { name: '取消拉取' }).count(), 1, '运行中任务应可取消');
  await page.screenshot({ path: `${outputDirectory}/external-pacs-config.png`, fullPage: true });

  await page.fill('.retrieval-query-form label:nth-of-type(2) input', 'P-2026');
  await page.fill('.retrieval-query-form label:nth-of-type(3) input', 'CT');
  await page.click('.retrieval-query-form button[type="submit"]');
  await page.locator('.retrieval-results .admin-row', { hasText: '胸部薄层 CT' })
    .waitFor({ state: 'visible' });
  assert.equal(await page.locator('.retrieval-results .admin-row').count(), 2, '查询应展示两项远端检查');
  assert.equal(await page.getByRole('button', { name: '拉取入库' }).count(), 2, '每项检查应有拉取入口');
  await page.locator('.retrieval-results .admin-row').last().scrollIntoViewIfNeeded();
  await page.screenshot({ path: `${outputDirectory}/external-pacs-query-results.png`, fullPage: true });

  await page.locator('.retrieval-job').last().scrollIntoViewIfNeeded();
  assert.equal(await page.getByText('已处理 74/128 · 成功 72 · 失败 1 · 警告 1').count(), 1, '运行任务应展示 Pending 子操作计数');
  assert.equal(await page.getByText('已处理 96/96 · 成功 96 · 失败 0 · 警告 0').count(), 1, '完成任务应展示最终计数');
  await page.screenshot({ path: `${outputDirectory}/external-pacs-retrieval-jobs.png`, fullPage: true });

  assert.deepEqual(pageErrors, [], `浏览器脚本错误: ${pageErrors.join(' | ')}`);
  console.log(JSON.stringify({ result: 'passed', outputDirectory }, null, 2));
} finally {
  await browser.close();
}

import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { chromium } from 'playwright-core';

const baseUrl = process.env.VIEWER_URL ?? 'http://127.0.0.1:5173';
const executablePath = process.env.PLAYWRIGHT_EXECUTABLE_PATH
  ?? '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge';
const outputDirectory = process.env.VISUAL_OUTPUT_DIR
  ?? new URL('../../../temp/report-review-acceptance/', import.meta.url).pathname;
await mkdir(outputDirectory, { recursive: true });

const browser = await chromium.launch({ executablePath, headless: true });
const page = await browser.newPage({ viewport: { width: 470, height: 800 } });
const pageErrors = [];
page.on('pageerror', (error) => pageErrors.push(error.message));

try {
  await page.goto(`${baseUrl}/?mode=report`, { waitUntil: 'networkidle' });

  await page.evaluate(() => {
    document.getElementById('login-screen').hidden = true;
    document.getElementById('app-shell').style.display = 'none';
    const root = document.getElementById('report-window-root');
    root.hidden = false;
    const set = (id, value) => { document.getElementById(id).textContent = value; };
    set('rw-title', '诊断报告 · 陈晓华');
    set('rw-status', '草稿 · 编辑中');
    document.getElementById('rw-status').dataset.status = 'draft';
    set('rw-patient-strip', '陈晓华 · P-20260818-001 · 女 · 54 岁 · CT · 胸部薄层 CT · 2026-08-18');
    set('rw-workitem-text', '待书写');
    document.getElementById('rw-workitem').hidden = false;
    document.getElementById('rw-create').hidden = true;
    document.getElementById('rw-review-start').hidden = true;
    document.getElementById('rw-findings').innerHTML = '<p>右肺上叶见磨玻璃结节影，直径约 6 mm。</p>';
    document.getElementById('rw-impression').innerHTML = '<p>右肺上叶磨玻璃结节，建议随访。</p>';
    document.getElementById('rw-findings').contentEditable = 'true';
    document.getElementById('rw-impression').contentEditable = 'true';
    document.getElementById('rw-positive').disabled = false;
    document.getElementById('rw-save').hidden = false;
    document.getElementById('rw-submit').hidden = false;
    document.getElementById('rw-approve').hidden = true;
    document.getElementById('rw-modify').hidden = true;
    document.getElementById('rw-review-comment-row').hidden = true;
    set('rw-author', '李医生');
    set('rw-reviewer', '--');
  });

  assert.equal(await page.locator('#app-shell').evaluate((node) => getComputedStyle(node).display), 'none');
  assert.equal(await page.locator('#report-window-root').isVisible(), true);
  assert.equal(await page.locator('#rw-sign').count(), 0, '独立报告窗不应存在作者直签按钮');
  assert.equal(await page.locator('#report-sign').count(), 0, '主界面报告工作台不应存在直签按钮');
  assert.equal(await page.locator('#rw-save').isVisible(), true);
  assert.equal(await page.locator('#rw-submit').isVisible(), true);
  assert.equal(await page.locator('#rw-approve').isVisible(), false);
  assert.equal(await page.locator('#rw-modify').isVisible(), false);
  assert.equal(await page.locator('#rw-findings').getAttribute('contenteditable'), 'true');
  await page.screenshot({ path: `${outputDirectory}/standalone-author-report.png` });

  await page.evaluate(() => {
    const set = (id, value) => { document.getElementById(id).textContent = value; };
    set('rw-title', '诊断报告 · 陈晓华 · 待审核');
    set('rw-status', '待审核');
    document.getElementById('rw-status').dataset.status = 'submitted';
    set('rw-workitem-text', '报告已提交，可开始审核');
    document.getElementById('rw-review-start').hidden = false;
    document.getElementById('rw-review-start').disabled = false;
    document.getElementById('rw-save').hidden = true;
    document.getElementById('rw-submit').hidden = true;
    document.getElementById('rw-approve').hidden = true;
    document.getElementById('rw-modify').hidden = true;
    document.getElementById('rw-review-comment-row').hidden = true;
    document.getElementById('rw-findings').contentEditable = 'false';
    document.getElementById('rw-impression').contentEditable = 'false';
    document.getElementById('rw-positive').disabled = true;
  });
  assert.equal(await page.locator('#rw-review-start').isVisible(), true);
  assert.equal(await page.locator('#rw-review-start').isEnabled(), true);
  assert.equal(await page.locator('#rw-save').isVisible(), false);
  assert.equal(await page.locator('#rw-submit').isVisible(), false);
  await page.screenshot({ path: `${outputDirectory}/standalone-reviewer-queue.png` });

  await page.evaluate(() => {
    const set = (id, value) => { document.getElementById(id).textContent = value; };
    set('rw-title', '诊断报告 · 陈晓华 · 审核');
    set('rw-status', '审核中');
    document.getElementById('rw-status').dataset.status = 'under_review';
    set('rw-workitem-text', '审核中 · 王医生');
    document.getElementById('rw-review-start').hidden = true;
    document.getElementById('rw-review-start').disabled = true;
    document.getElementById('rw-save').hidden = true;
    document.getElementById('rw-submit').hidden = true;
    document.getElementById('rw-approve').hidden = false;
    document.getElementById('rw-modify').hidden = false;
    document.getElementById('rw-review-comment-row').hidden = false;
    document.getElementById('rw-review-comment').value = '影像所见表述可更精确';
    document.getElementById('rw-findings').contentEditable = 'false';
    document.getElementById('rw-impression').contentEditable = 'false';
    document.getElementById('rw-positive').disabled = true;
    set('rw-author', '李医生');
    set('rw-reviewer', '王医生');
  });
  assert.equal(await page.locator('#report-window-root').isVisible(), true);
  assert.equal(await page.locator('#rw-save').isVisible(), false);
  assert.equal(await page.locator('#rw-submit').isVisible(), false);
  assert.equal(await page.locator('#rw-approve').isVisible(), true);
  assert.equal(await page.locator('#rw-modify').isVisible(), true);
  assert.equal(await page.locator('#rw-findings').getAttribute('contenteditable'), 'false');
  assert.equal(await page.locator('#rw-review-comment-row').isVisible(), true);
  await page.screenshot({ path: `${outputDirectory}/standalone-reviewer-report.png` });

  const metrics = await page.evaluate(() => ({
    viewportWidth: window.innerWidth,
    scrollWidth: document.documentElement.scrollWidth,
    rootWidth: document.getElementById('report-window-root').getBoundingClientRect().width,
  }));
  assert.equal(metrics.scrollWidth <= metrics.viewportWidth, true, '独立报告窗不得横向溢出');
  assert.equal(metrics.rootWidth <= metrics.viewportWidth, true, '独立报告窗应完整位于视口内');
  console.log(JSON.stringify({ result: 'passed', outputDirectory, metrics, pageErrors }, null, 2));
} finally {
  await browser.close();
}

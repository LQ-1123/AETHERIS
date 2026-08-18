import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { chromium } from 'playwright-core';

const baseUrl = process.env.VIEWER_URL ?? 'http://127.0.0.1:5173';
const executablePath = process.env.PLAYWRIGHT_EXECUTABLE_PATH
  ?? '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge';
const outputDirectory = process.env.VISUAL_OUTPUT_DIR
  ?? new URL('../../../temp/acceptance/', import.meta.url).pathname;
await mkdir(outputDirectory, { recursive: true });

const browser = await chromium.launch({ executablePath, headless: true });
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
const pageErrors = [];
const consoleErrors = [];
page.on('pageerror', (error) => pageErrors.push(error.message));
page.on('console', (message) => {
  if (message.type() === 'error') consoleErrors.push(message.text());
});

try {
  await page.goto(`${baseUrl}/?acceptance=queue`, { waitUntil: 'networkidle' });
  await page.locator('#login-form').waitFor({ state: 'visible' });
  assert.equal(await page.locator('#import-menu').count(), 0, '不应保留本地上传入库入口');
  assert.equal((await page.locator('#open-btn').innerText()).includes('本地打开'), true, '本地阅片入口应明确标注为本地打开');
  await page.fill('#server-url', 'https://127.0.0.1:8443');
  await page.fill('#ca-cert-path', '/tmp/acceptance-ca.pem');
  await page.fill('#login-username', 'admin');
  await page.fill('#login-password', 'acceptance');
  await page.click('#login-btn');
  await page.locator('#queue-body tr').first().waitFor({ state: 'visible' });
  assert.equal(await page.locator('#login-screen').isHidden(), true, '登录成功后应隐藏登录页');
  assert.equal(await page.locator('#queue-page').isVisible(), true, '登录后应自动进入患者队列首页');
  assert.equal(await page.locator('#queue-back').isHidden(), true, '尚未打开序列时不应返回空白阅片区');
  assert.equal(await page.locator('#queue-body tr').count(), 50, '首页应显示 50 项');
  assert.equal(await page.locator('#workspace').isHidden(), true, '队列页打开时应隐藏阅片工作区');
  assert.equal(await page.locator('#worklist-panel').isHidden(), true, '旧工作列表侧栏不应可见');
  assert.equal(await page.locator('.queue-edit-button').count(), 50, '管理员应在每个检查行看到编辑标签入口');
  assert.equal(await page.locator('#queue-next').isEnabled(), true, '超过一页时下一页应可用');
  assert.equal(await page.getByText('UNAUTHORIZED').count(), 0, '未授权设备数据不得出现');

  await page.locator('.queue-edit-button').first().click();
  await page.locator('#tag-editor-dialog').waitFor({ state: 'visible' });
  assert.equal(await page.locator('#tag-editor-title').textContent(), '编辑检查标签', '队列标签入口应打开检查级编辑器');
  assert.match(
    await page.locator('[data-tag-keyword="AccessionNumber"]').inputValue(),
    /^ACC-\d+$/,
    '标签编辑器应加载当前检查号',
  );
  await page.click('#tag-editor-close');

  const initialRequest = await latestQueueRequest(page);
  assert.equal(initialRequest.args.sort, 'study_date');
  assert.equal(initialRequest.args.order, 'desc');
  assert.equal(initialRequest.args.offset, 0);
  const statusColors = await page.locator('.worklist-report-badge').evaluateAll((badges) =>
    Object.fromEntries(badges.map((badge) => [
      badge.getAttribute('data-status'),
      getComputedStyle(badge).color,
    ])),
  );
  assert.deepEqual(
    Object.keys(statusColors).sort(),
    ['locked', 'pending', 'signed', 'writing'],
    '首页应覆盖四种报告状态',
  );
  assert.equal(new Set(Object.values(statusColors)).size, 4, '四态徽标应使用可区分的文字颜色');
  const desktopMetrics = await layoutMetrics(page);
  assertLayout(desktopMetrics, 'desktop');
  await page.screenshot({ path: `${outputDirectory}/queue-page-desktop.png`, fullPage: true });

  await page.click('#queue-next');
  await page.locator('#queue-page-label', { hasText: '第 2 页' }).waitFor({ state: 'visible' });
  assert.equal(await page.locator('#queue-body tr').count(), 5, '第二页应显示剩余 5 项');
  assert.equal((await latestQueueRequest(page)).args.offset, 50);
  await page.click('#queue-previous');
  await page.locator('#queue-page-label', { hasText: '第 1 页' }).waitFor({ state: 'visible' });

  await page.click('[data-queue-sort="patient_name"]');
  await waitForRequest(page, (request) => request.args.sort === 'patient_name' && request.args.order === 'asc');
  assert.equal(await page.locator('#queue-page-label').textContent(), '第 1 页', '排序后应回到第 1 页');
  const patientHeader = page.locator('[data-queue-sort="patient_name"]').locator('..');
  assert.equal(await patientHeader.getAttribute('aria-sort'), 'ascending');
  await page.click('[data-queue-sort="patient_name"]');
  await waitForRequest(page, (request) => request.args.sort === 'patient_name' && request.args.order === 'desc');
  assert.equal(await patientHeader.getAttribute('aria-sort'), 'descending');

  const requestsBeforeRefresh = await queueRequestCount(page);
  await page.click('#queue-refresh');
  await page.waitForFunction((count) => window.__queueAcceptance.queueRequests.length > count, requestsBeforeRefresh);

  await page.fill('#queue-query', '张');
  await page.fill('#queue-date-from', '2026-08-18');
  await page.fill('#queue-date-to', '2026-08-18');
  await page.selectOption('#queue-modality', 'CT');
  await page.fill('#queue-body-part', 'CHEST');
  await page.selectOption('#queue-report-status', 'pending');
  await page.fill('#queue-institution', '中心医院');
  await page.click('.queue-filter-submit');
  await page.getByText('QUEUE-SPECIAL', { exact: true }).waitFor({ state: 'visible' });
  assert.equal(await page.locator('#queue-body tr').count(), 1, '组合筛选应只留下目标检查');
  const filterRequest = await latestQueueRequest(page);
  assert.equal(filterRequest.args.query, '张');
  assert.equal(filterRequest.args.modality, 'CT');
  assert.equal(filterRequest.args.bodyPart, 'CHEST');
  assert.equal(filterRequest.args.reportStatus, 'pending');
  assert.equal(filterRequest.args.institution, '中心医院');
  assert.equal(filterRequest.args.dateFrom, '2026-08-18');
  assert.equal(filterRequest.args.dateTo, '2026-08-18');
  await page.screenshot({ path: `${outputDirectory}/queue-page-filtered.png`, fullPage: true });

  await page.fill('#queue-query', '不存在');
  await page.click('.queue-filter-submit');
  await page.getByText('没有匹配的检查', { exact: true }).waitFor({ state: 'visible' });
  assert.equal(await page.locator('#queue-body tr').count(), 0, '空结果不应残留旧行');
  await page.screenshot({ path: `${outputDirectory}/queue-page-empty.png`, fullPage: true });

  await clearFilters(page);
  await page.click('.queue-filter-submit');
  await page.locator('#queue-body tr').first().waitFor({ state: 'visible' });
  await page.setViewportSize({ width: 390, height: 844 });
  const mobileMetrics = await layoutMetrics(page);
  assertLayout(mobileMetrics, 'mobile');
  await page.screenshot({ path: `${outputDirectory}/queue-page-mobile.png`, fullPage: true });

  await page.setViewportSize({ width: 1440, height: 900 });
  await clearFilters(page);
  await page.fill('#queue-query', 'QUEUE-SPECIAL');
  await page.click('.queue-filter-submit');
  const targetRow = page.locator('#queue-body tr').first();
  await targetRow.waitFor({ state: 'visible' });
  await targetRow.dblclick();
  await page.locator('#queue-page').waitFor({ state: 'hidden' });
  const openedSeries = await page.evaluate(() => window.__queueAcceptance.openedSeries);
  assert.equal(openedSeries.studyUid.endsWith('.1'), true, '双击应打开筛选到的检查');
  assert.equal(openedSeries.seriesUid.endsWith('.axial'), true, '应优先选择薄层轴位 MPR 序列');
  assert.equal(await page.locator('#workspace').isVisible(), true, '打开序列后应回到阅片器');
  await page.waitForFunction(() => {
    const canvas = document.querySelector('#image-canvas');
    const viewport = document.querySelector('#viewport');
    return canvas instanceof HTMLCanvasElement
      && canvas.width > 0
      && canvas.height > 0
      && viewport instanceof HTMLElement
      && viewport.clientWidth > 0
      && viewport.clientHeight > 0;
  });
  const viewerMetrics = await page.evaluate(() => {
    const workspace = document.querySelector('#workspace').getBoundingClientRect();
    const viewport = document.querySelector('#viewport').getBoundingClientRect();
    const worklist = document.querySelector('#worklist-panel').getBoundingClientRect();
    const canvas = document.querySelector('#image-canvas');
    return {
      workspaceWidth: workspace.width,
      viewportWidth: viewport.width,
      viewportHeight: viewport.height,
      worklistWidth: worklist.width,
      canvasWidth: canvas.width,
      canvasHeight: canvas.height,
      worklistHidden: document.querySelector('#worklist-panel').hidden,
      worklistResizerHidden: document.querySelector('#worklist-resizer').hidden,
      detailsHidden: document.querySelector('#details-panel').hidden,
      detailsResizerHidden: document.querySelector('#details-resizer').hidden,
    };
  });
  assert.equal(viewerMetrics.detailsHidden, true, '检查信息栏必须永久隐藏');
  assert.equal(viewerMetrics.detailsResizerHidden, true, '检查信息栏拖拽条必须永久隐藏');
  assert.equal(viewerMetrics.worklistHidden, false, '阅片后应显示当前患者的检查与序列侧栏');
  assert.equal(viewerMetrics.worklistResizerHidden, false, '当前患者侧栏应保留宽度调整能力');
  assert.equal(viewerMetrics.worklistWidth >= 280, true, '当前患者侧栏应有可用宽度');
  assert.equal(viewerMetrics.viewportWidth < viewerMetrics.workspaceWidth, true, '影像视口应为当前患者侧栏留出空间');
  assert.equal(viewerMetrics.viewportHeight > 0, true, '影像视口必须有可见高度');
  assert.equal(viewerMetrics.canvasWidth > 0 && viewerMetrics.canvasHeight > 0, true, '影像画布尺寸必须非零');
  assert.equal(await page.locator('#patient-list .study-item').count(), 2, '阅片侧栏应显示该患者全部检查');
  assert.equal(await page.locator('#patient-list .series-row').count(), 2, '当前检查应展开并显示全部序列');
  assert.equal((await page.locator('#worklist-count').textContent()).includes('QUEUE-SPECIAL'), true, '侧栏标题应标明当前患者');
  await page.screenshot({ path: `${outputDirectory}/queue-opened-study.png`, fullPage: true });

  await page.locator('#patient-list .study-row').nth(1).click();
  await page.locator('#patient-list .study-item').nth(1).locator('.series-row').first().waitFor({ state: 'visible' });
  const dragSource = await page.locator('#patient-list .study-item').nth(1).locator('.series-row').first().boundingBox();
  const dragTarget = await page.locator('#viewport').boundingBox();
  assert.ok(dragSource && dragTarget, '拖拽源和影像视口必须可见');
  await page.mouse.move(dragSource.x + dragSource.width / 2, dragSource.y + dragSource.height / 2);
  await page.mouse.down();
  await page.mouse.move(dragTarget.x + dragTarget.width / 2, dragTarget.y + dragTarget.height / 2, { steps: 8 });
  await page.mouse.up();
  await page.waitForFunction(() => document.querySelectorAll('#series-grid .series-pane').length >= 2);
  assert.equal(await page.locator('#series-grid .series-pane').count() >= 2, true, '序列拖入阅片区应自动增加分屏');

  await page.click('#queue-btn');
  await page.locator('#queue-page').waitFor({ state: 'visible' });
  assert.equal(await page.locator('#queue-back').isVisible(), true, '已打开序列后队列页应允许返回阅片');
  await page.click('#queue-back');
  assert.equal(await page.locator('#workspace').isVisible(), true, '返回阅片应恢复当前患者侧栏和影像');

  assert.deepEqual(pageErrors, [], `浏览器脚本错误: ${pageErrors.join(' | ')}`);
  assert.deepEqual(consoleErrors, [], `浏览器控制台错误: ${consoleErrors.join(' | ')}`);
  console.log(JSON.stringify({
    result: 'passed',
    desktopMetrics,
    mobileMetrics,
    statusColors,
    openedSeries,
    viewerMetrics,
    queueRequests: await queueRequestCount(page),
    outputDirectory,
  }, null, 2));
} finally {
  await browser.close();
}

async function latestQueueRequest(page) {
  return page.evaluate(() => window.__queueAcceptance.queueRequests.at(-1));
}

async function queueRequestCount(page) {
  return page.evaluate(() => window.__queueAcceptance.queueRequests.length);
}

async function waitForRequest(page, predicate) {
  await page.waitForFunction(
    (source) => {
      const request = window.__queueAcceptance.queueRequests.at(-1);
      return request && Function('request', `return (${source})(request)`)(request);
    },
    predicate.toString(),
  );
}

async function clearFilters(page) {
  await page.fill('#queue-query', '');
  await page.fill('#queue-date-from', '');
  await page.fill('#queue-date-to', '');
  await page.selectOption('#queue-modality', '');
  await page.fill('#queue-body-part', '');
  await page.selectOption('#queue-report-status', '');
  await page.fill('#queue-institution', '');
}

async function layoutMetrics(page) {
  return page.evaluate(() => {
    const rect = (selector) => {
      const bounds = document.querySelector(selector).getBoundingClientRect();
      return { left: bounds.left, top: bounds.top, right: bounds.right, bottom: bounds.bottom };
    };
    const filterRects = [...document.querySelectorAll('.queue-filters label, .queue-filter-submit')]
      .map((element) => {
        const bounds = element.getBoundingClientRect();
        return { left: bounds.left, top: bounds.top, right: bounds.right, bottom: bounds.bottom };
      });
    const overlaps = filterRects.some((left, index) => filterRects.slice(index + 1).some((right) =>
      left.left < right.right && left.right > right.left && left.top < right.bottom && left.bottom > right.top,
    ));
    const tableWrap = document.querySelector('.queue-table-wrap');
    return {
      viewportWidth: window.innerWidth,
      viewportHeight: window.innerHeight,
      documentScrollWidth: document.documentElement.scrollWidth,
      queue: rect('#queue-page'),
      filters: rect('#queue-filters'),
      pagination: rect('.queue-pagination'),
      tableClientWidth: tableWrap.clientWidth,
      tableScrollWidth: tableWrap.scrollWidth,
      filterOverlaps: overlaps,
    };
  });
}

function assertLayout(metrics, label) {
  assert.equal(metrics.documentScrollWidth <= metrics.viewportWidth, true, `${label}: 页面不应横向溢出`);
  assert.equal(metrics.queue.left >= 0 && metrics.queue.right <= metrics.viewportWidth, true, `${label}: 队列应在视口内`);
  assert.equal(metrics.queue.top >= 0 && metrics.queue.bottom <= metrics.viewportHeight, true, `${label}: 队列高度应在视口内`);
  assert.equal(metrics.pagination.bottom <= metrics.viewportHeight, true, `${label}: 分页应保持可见`);
  assert.equal(metrics.filters.bottom <= metrics.pagination.top, true, `${label}: 筛选区不能遮挡分页`);
  assert.equal(metrics.filterOverlaps, false, `${label}: 筛选控件不能重叠`);
}

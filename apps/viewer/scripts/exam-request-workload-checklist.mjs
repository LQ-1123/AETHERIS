import assert from 'node:assert/strict';
import { mkdir, readFile } from 'node:fs/promises';
import { chromium } from 'playwright-core';

const baseUrl = process.env.VIEWER_URL ?? 'http://127.0.0.1:5173';
const outputDirectory = process.env.VISUAL_OUTPUT_DIR
  ?? new URL('../../../temp/exam-request-workload-checklist/', import.meta.url).pathname;
const executablePath = process.env.PLAYWRIGHT_EXECUTABLE_PATH
  ?? '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge';

await mkdir(outputDirectory, { recursive: true });
const browser = await chromium.launch({ executablePath, headless: true });
const page = await browser.newPage({ viewport: { width: 1500, height: 920 }, acceptDownloads: true });
page.setDefaultTimeout(8_000);
const pageErrors = [];
page.on('pageerror', (error) => pageErrors.push(error.message));
page.on('dialog', (dialog) => dialog.accept());

try {
  await login(page, 'tech.zhao');
  assert.equal(await page.locator('#exam-request-btn').isVisible(), true, 'A1: 技师应看到申请单入口');
  assert.equal(await page.getByText('有申请单', { exact: true }).count(), 1, 'C2: 队列应显示申请单标记');
  await shot(page, '01-technician-queue-entry.png');

  await page.click('#exam-request-btn');
  await page.locator('tr[data-request-id="req-1"]').waitFor();
  assert.equal(await page.locator('tr[data-request-id="req-1"]').getByText('编辑', { exact: true }).count(), 1, 'A5: 待执行申请单应可编辑');
  assert.equal(await page.locator('tr[data-request-id="req-2"]').getByText('编辑', { exact: true }).count(), 0, 'A5: 已执行申请单不应可编辑');
  assert.equal(await page.locator('tr[data-request-id="req-3"]').getByText('编辑', { exact: true }).count(), 0, 'D2: 已完成申请单不应可编辑');

  await page.click('#exam-request-new');
  await page.fill('#exam-request-patient-name', '必填校验患者');
  await page.fill('#exam-request-body-part', 'CHEST');
  await page.fill('#exam-request-indication', '咳嗽两周，CT 排除肺炎');
  const callsBeforeValidation = await createCallCount(page);
  await page.click('#exam-request-editor-submit');
  const invalid = await page.locator('#exam-request-patient-id').evaluate((input) => ({
    valid: input.checkValidity(),
    message: input.validationMessage,
  }));
  assert.equal(invalid.valid, false, 'A3: Patient ID 留空必须触发必填校验');
  assert.notEqual(invalid.message, '', 'A3: 浏览器应提供必填提示');
  assert.equal(await createCallCount(page), callsBeforeValidation, 'A3: 校验失败不得调用创建接口');
  await shot(page, '02-required-field-validation.png');
  await page.click('#exam-request-editor-cancel');

  await page.click('#exam-request-new');
  await page.fill('#exam-request-patient-id', 'P-CHECK-001');
  await page.fill('#exam-request-patient-name', '清单患者');
  await page.selectOption('#exam-request-modality', 'CT');
  await page.fill('#exam-request-body-part', 'CHEST');
  await page.selectOption('#exam-request-type', '平扫');
  await page.fill('#exam-request-indication', '咳嗽两周，CT 排除肺炎');
  await shot(page, '03-new-request-form.png');
  await page.click('#exam-request-editor-submit');
  const createdRow = page.locator('tr[data-request-id="req-new"]');
  await createdRow.getByText('待执行', { exact: true }).waitFor();
  const createCall = await page.evaluate(() => window.__examRequestAcceptance?.calls.find((call) => call.command === 'create_exam_request' && call.args.patientId === 'P-CHECK-001'));
  assert.equal(createCall?.args.modality, 'CT', 'A2: 创建参数模态错误');
  assert.equal(createCall?.args.bodyPart, 'CHEST', 'A2: 创建参数部位错误');
  assert.equal(createCall?.args.requestType, '平扫', 'A2: 创建参数类型错误');
  await shot(page, '04-pending-request-created.png');

  await createdRow.getByText('编辑', { exact: true }).click();
  await page.fill('#exam-request-indication', '咳嗽两周伴发热，CT 排除肺炎。');
  await page.click('#exam-request-editor-submit');
  await createdRow.getByText('咳嗽两周伴发热，CT 排除肺炎。', { exact: true }).waitFor();
  await shot(page, '05-pending-request-edited.png');

  await page.selectOption('#exam-request-status-filter', 'completed');
  await page.locator('#exam-request-filters button[type="submit"]').click();
  await page.locator('tr[data-request-id="req-3"]').waitFor();
  assert.equal(await page.locator('#exam-request-body tr').count(), 1, 'A4/D2: 已完成筛选结果错误');
  await shot(page, '06-completed-status-filter.png');

  await page.selectOption('#exam-request-status-filter', '');
  await page.locator('#exam-request-filters button[type="submit"]').click();
  await createdRow.getByText('绑定检查', { exact: true }).click();
  const candidate = page.locator('#exam-request-candidates').getByText('上腹部 MR 平扫加增强');
  await candidate.waitFor();
  await shot(page, '07-bind-study-candidate.png');
  await page.getByText('确认绑定', { exact: true }).click();
  await createdRow.getByText('等待报告', { exact: true }).waitFor();
  assert.equal(await createdRow.getByText('编辑', { exact: true }).count(), 0, 'B1/A5: 绑定后不应继续编辑');
  assert.equal(await createdRow.getByText('2026-08-19 · 上腹部 MR 平扫加增强', { exact: true }).count(), 1, 'B1: 关联 Study 未显示');
  await shot(page, '08-bound-request-executed.png');

  await reloadAndLogin(page, 'doctor.li');
  assert.equal(await page.locator('#exam-request-btn').isVisible(), false, 'F1: 医生不应看到申请单管理入口');
  assert.equal(await page.getByText('有申请单', { exact: true }).count(), 1, 'C2: 医生队列应看到申请单标记');
  await shot(page, '09-doctor-queue-request-badge.png');
  await page.click('#more-menu-button');
  assert.equal(await page.locator('#admin-console-btn').isVisible(), false, 'F3: 医生不应看到管理控制台入口');
  await shot(page, '10-doctor-permission-boundary.png');

  await reloadAndLogin(page, 'admin.wang');
  await page.click('#more-menu-button');
  await page.click('#admin-console-btn');
  await page.click('[data-admin-tab="workload"]');
  const adminBody = page.locator('#admin-console-body');
  await adminBody.getByText('李医生', { exact: true }).waitFor();
  assert.equal(await adminBody.getByText('34', { exact: true }).isVisible(), true, 'E4: 技师申请单数错误');
  assert.equal(await adminBody.getByText('21', { exact: true }).isVisible(), true, 'E3: 医生签发版本数错误');
  await shot(page, '11-admin-workload-report.png');

  const dateInputs = adminBody.locator('.admin-workload-filters input[type="date"]');
  await dateInputs.nth(0).fill('2030-01-01');
  await dateInputs.nth(1).fill('2030-01-31');
  await adminBody.locator('.admin-workload-filters button[type="submit"]').click();
  await adminBody.getByText('2030-01-01 至 2030-01-31 · 0 名员工', { exact: true }).waitFor();
  await adminBody.getByText('当前机构没有医生或技师账号。', { exact: true }).waitFor();
  await shot(page, '12-admin-workload-empty-range.png');

  const refreshedDateInputs = adminBody.locator('.admin-workload-filters input[type="date"]');
  await refreshedDateInputs.nth(0).fill('2026-08-01');
  await refreshedDateInputs.nth(1).fill('2026-08-19');
  await adminBody.locator('.admin-workload-filters button[type="submit"]').click();
  await adminBody.getByText('李医生', { exact: true }).waitFor();
  const downloadPromise = page.waitForEvent('download');
  await adminBody.getByText('导出 CSV', { exact: true }).click();
  const download = await downloadPromise;
  assert.equal(download.suggestedFilename(), '工作量-2026-08-01-2026-08-19.csv', 'E6: CSV 文件名错误');
  const downloadPath = await download.path();
  assert.ok(downloadPath, 'E6: CSV 下载文件不可用');
  const csv = await readFile(downloadPath);
  assert.deepEqual([...csv.subarray(0, 3)], [0xef, 0xbb, 0xbf], 'E6: CSV 缺少 UTF-8 BOM');
  const csvText = csv.toString('utf8');
  assert.match(csvText, /赵技师/);
  assert.match(csvText, /李医生/);
  assert.match(csvText, /34/);

  const metrics = await page.evaluate(() => ({
    viewportWidth: innerWidth,
    scrollWidth: document.documentElement.scrollWidth,
  }));
  assert.equal(metrics.scrollWidth <= metrics.viewportWidth, true, '页面不得横向溢出');
  assert.deepEqual(pageErrors, []);
  console.log(JSON.stringify({
    result: 'passed',
    outputDirectory,
    screenshots: 12,
    csv: download.suggestedFilename(),
    metrics,
  }, null, 2));
} finally {
  await browser.close();
}

async function login(page, username, navigate = true) {
  if (navigate) await page.goto(baseUrl, { waitUntil: 'networkidle' });
  await page.fill('#server-url', 'https://127.0.0.1:8443');
  await page.fill('#ca-cert-path', '/tmp/acceptance-ca.pem');
  await page.fill('#login-username', username);
  await page.fill('#login-password', 'acceptance-password');
  await page.click('#login-btn');
  await page.locator('#login-screen').waitFor({ state: 'hidden' });
}

async function reloadAndLogin(page, username) {
  await page.reload({ waitUntil: 'networkidle' });
  await login(page, username, false);
}

async function shot(page, name) {
  await page.screenshot({ path: `${outputDirectory}/${name}`, fullPage: true });
}

async function createCallCount(page) {
  return page.evaluate(() => window.__examRequestAcceptance?.calls.filter((call) => call.command === 'create_exam_request').length ?? 0);
}

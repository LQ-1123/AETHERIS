import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { chromium } from 'playwright-core';

const baseUrl = process.env.VIEWER_URL ?? 'http://127.0.0.1:5173';
const outputDirectory = process.env.VISUAL_OUTPUT_DIR
  ?? new URL('../../../temp/exam-request-acceptance/', import.meta.url).pathname;
const executablePath = process.env.PLAYWRIGHT_EXECUTABLE_PATH
  ?? '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge';
await mkdir(outputDirectory, { recursive: true });
const browser = await chromium.launch({ executablePath, headless: true });
const page = await browser.newPage({ viewport: { width: 1500, height: 920 } });
page.setDefaultTimeout(8_000);
page.on('dialog', (dialog) => dialog.accept());
const pageErrors = [];
page.on('pageerror', (error) => pageErrors.push(error.message));

try {
  await login(page, 'tech.zhao');
  assert.equal(await page.locator('#exam-request-btn').isVisible(), true, '技师必须看到申请单入口');

  const queueRow = page.locator('tr[data-study-uid="1.2.840.113619.2.55.3.604688123.20260819.1"]');
  await queueRow.getByText('开申请单', { exact: true }).click();
  await page.getByText('为已入库检查开具申请单', { exact: true }).waitFor();
  assert.equal(await page.locator('#exam-request-patient-id').inputValue(), 'P-20260819-008');
  assert.equal(await page.locator('#exam-request-patient-id').isEditable(), false, '已有检查患者号不可编辑');
  assert.equal(await page.locator('#exam-request-patient-name').isEditable(), false, '已有检查患者姓名不可编辑');
  await page.selectOption('#exam-request-type', '增强');
  await page.fill('#exam-request-indication', '间断胸痛一周，伴活动后气促，申请排除肺栓塞。');
  await page.screenshot({ path: `${outputDirectory}/existing-study-new-request.png` });
  await page.click('#exam-request-editor-submit');
  const existingRequestRow = page.locator('tr[data-request-id="req-existing-study"]');
  await existingRequestRow.getByText('等待报告', { exact: true }).waitFor();
  const existingStudyCall = await page.evaluate(() => window.__examRequestAcceptance?.calls.find((call) => call.command === 'create_exam_request_for_study'));
  assert.equal(existingStudyCall?.args.studyUid, '1.2.840.113619.2.55.3.604688123.20260819.1');
  assert.equal('patientId' in (existingStudyCall?.args ?? {}), false, '患者号不得由客户端提交');
  assert.equal('patientName' in (existingStudyCall?.args ?? {}), false, '患者姓名不得由客户端提交');
  await page.screenshot({ path: `${outputDirectory}/existing-study-request-created.png`, fullPage: true });

  await page.click('#exam-request-back');
  await page.click('#queue-btn');
  await queueRow.waitFor();
  assert.equal(await queueRow.getByText('开申请单', { exact: true }).count(), 0, '已有申请单的检查不再显示入口');

  await page.click('#exam-request-btn');
  await page.locator('tr[data-request-id="req-1"]').waitFor();
  await page.screenshot({ path: `${outputDirectory}/technician-request-list.png`, fullPage: true });

  await page.click('#exam-request-new');
  await page.fill('#exam-request-patient-id', 'P-20260819-016');
  await page.fill('#exam-request-patient-name', '林海');
  await page.fill('#exam-request-body-part', '上腹部');
  await page.selectOption('#exam-request-modality', 'MR');
  await page.selectOption('#exam-request-type', '平扫+增强');
  await page.fill('#exam-request-indication', '上腹部隐痛两月，肝功能指标异常，进一步评估肝脏占位。');
  await page.screenshot({ path: `${outputDirectory}/technician-new-request.png` });
  await page.click('#exam-request-editor-submit');
  await page.getByText('林海', { exact: true }).waitFor();
  const createCall = await page.evaluate(() => window.__examRequestAcceptance?.calls.find((call) => call.command === 'create_exam_request'));
  assert.equal(createCall?.args.patientId, 'P-20260819-016');

  const firstRow = page.locator('tr[data-request-id="req-new"]');
  await firstRow.getByText('绑定检查', { exact: true }).click();
  await page.locator('#exam-request-candidates').getByText('上腹部 MR 平扫加增强').waitFor();
  await page.screenshot({ path: `${outputDirectory}/technician-bind-study.png` });
  await page.getByText('确认绑定', { exact: true }).click();
  await firstRow.getByText('等待报告', { exact: true }).waitFor();
  const bindCall = await page.evaluate(() => window.__examRequestAcceptance?.calls.find((call) => call.command === 'bind_exam_request'));
  assert.equal(bindCall?.args.requestId, 'req-new');

  await page.reload({ waitUntil: 'networkidle' });
  await login(page, 'admin.wang', false);
  await page.click('#more-menu-button');
  await page.click('#admin-console-btn');
  await page.click('[data-admin-tab="workload"]');
  await page.getByText('李医生', { exact: true }).waitFor();
  assert.equal(await page.getByText('34', { exact: true }).isVisible(), true, '应显示技师申请单数');
  assert.equal(await page.getByText('21', { exact: true }).isVisible(), true, '应显示医生签发数');
  await page.screenshot({ path: `${outputDirectory}/admin-workload.png` });

  const metrics = await page.evaluate(() => ({ viewportWidth: innerWidth, scrollWidth: document.documentElement.scrollWidth }));
  assert.equal(metrics.scrollWidth <= metrics.viewportWidth, true, '页面不得横向溢出');
  assert.deepEqual(pageErrors, []);
  console.log(JSON.stringify({ result: 'passed', outputDirectory, metrics }, null, 2));
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

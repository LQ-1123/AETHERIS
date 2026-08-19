import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { chromium } from 'playwright-core';

const baseUrl = process.env.VIEWER_URL ?? 'http://127.0.0.1:5173';
const executablePath = process.env.PLAYWRIGHT_EXECUTABLE_PATH
  ?? '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge';
const outputDirectory = process.env.VISUAL_OUTPUT_DIR
  ?? new URL('../../../temp/password-reset-acceptance/', import.meta.url).pathname;
await mkdir(outputDirectory, { recursive: true });

const browser = await chromium.launch({ executablePath, headless: true });
const page = await browser.newPage({ viewport: { width: 1440, height: 960 } });
page.setDefaultTimeout(8_000);
const pageErrors = [];
page.on('pageerror', (error) => pageErrors.push(error.message));
page.on('console', (message) => {
  if (message.type() === 'error') pageErrors.push(message.text());
});
page.on('dialog', (dialog) => dialog.accept());

try {
  await page.goto(baseUrl, { waitUntil: 'networkidle' });
  await page.fill('#login-username', 'doctor.li');
  await page.click('#forgot-password-btn');
  await page.fill('#password-reset-password', 'correct horse battery staple');
  await page.fill('#password-reset-confirm', 'correct horse battery staple');
  assert.equal(await page.locator('#password-reset-dialog').isVisible(), true);
  assert.equal(
    await page.locator('#password-reset-dialog').getByText('管理员无法查看密码内容。').isVisible(),
    true,
  );
  await page.screenshot({ path: `${outputDirectory}/forgot-password-request.png` });

  await page.click('#password-reset-submit');
  await page.locator('#login-notice').waitFor({ state: 'visible' });
  const requestCall = await page.evaluate(() => window.__passwordResetAcceptance?.calls
    .find((call) => call.command === 'request_password_reset'));
  assert.equal(requestCall?.args.username, 'doctor.li');
  assert.equal(requestCall?.args.newPassword, 'correct horse battery staple');
  await page.screenshot({ path: `${outputDirectory}/password-reset-submitted.png` });

  // 管理员首次登录、不打开任何检查时，管理控制台入口必须立即出现。
  await page.fill('#ca-cert-path', '/tmp/acceptance-ca.pem');
  await page.fill('#login-username', 'admin.wang');
  await page.fill('#login-password', 'admin-password-secure-2026');
  await page.click('#login-btn');
  await page.locator('#login-screen').waitFor({ state: 'hidden' });
  await page.click('#more-menu-button');
  assert.equal(await page.locator('#admin-console-btn').isVisible(), true);
  const openedSeriesBeforeMenu = await page.evaluate(() => window.__passwordResetAcceptance?.calls
    .some((call) => call.command === 'open_remote_series'));
  assert.equal(openedSeriesBeforeMenu, false, '验证入口显示前不应打开任何检查');
  await page.screenshot({ path: `${outputDirectory}/admin-first-login-menu.png` });
  await page.click('#admin-console-btn');
  await page.click('[data-admin-tab="accounts"]');
  await page.locator('.admin-account-row').first().waitFor();
  assert.equal(await page.getByText('重置密码', { exact: true }).count(), 0,
    '管理员账号列表不应再出现直接重置密码入口');

  await page.click('[data-admin-tab="password-resets"]');
  await page.getByText('批准重置', { exact: true }).waitFor();
  assert.equal(await page.getByText('李医生', { exact: true }).isVisible(), true);
  assert.equal(await page.getByText('管理员只能批准或拒绝，无法查看或修改密码内容。').isVisible(), true);
  await page.screenshot({ path: `${outputDirectory}/admin-password-reset-review.png` });

  await page.getByText('批准重置', { exact: true }).click();
  await page.getByText('暂无待审核的密码重置申请。').waitFor();
  const reviewCall = await page.evaluate(() => window.__passwordResetAcceptance?.calls
    .find((call) => call.command === 'review_password_reset_request'));
  assert.equal(reviewCall?.args.requestId, 17);
  assert.equal(reviewCall?.args.approve, true);
  await page.screenshot({ path: `${outputDirectory}/admin-password-reset-approved.png` });

  await page.click('[data-admin-tab="settings"]');
  await page.getByText('报告审核闭环', { exact: true }).waitFor();
  const reviewSwitch = page.getByRole('switch', { name: '启用报告审核闭环' });
  assert.equal(await reviewSwitch.isChecked(), false);
  await page.screenshot({ path: `${outputDirectory}/institution-review-disabled.png` });
  await page.locator('.admin-switch').click();
  await page.getByText('报告审核闭环已开启，新流程即时生效。').waitFor();
  assert.equal(await reviewSwitch.isChecked(), true);
  const settingCall = await page.evaluate(() => window.__passwordResetAcceptance?.calls
    .find((call) => call.command === 'update_institution_settings'));
  assert.equal(settingCall?.args.reviewRequired, true);
  await page.screenshot({ path: `${outputDirectory}/institution-review-enabled.png` });

  const metrics = await page.evaluate(() => ({
    viewportWidth: window.innerWidth,
    scrollWidth: document.documentElement.scrollWidth,
    dialogWidth: document.getElementById('admin-console-dialog').getBoundingClientRect().width,
  }));
  assert.equal(metrics.scrollWidth <= metrics.viewportWidth, true, '页面不得横向溢出');
  assert.equal(metrics.dialogWidth < metrics.viewportWidth, true, '审核对话框应位于视口内');
  assert.deepEqual(pageErrors, []);
  console.log(JSON.stringify({ result: 'passed', outputDirectory, metrics, pageErrors }, null, 2));
} finally {
  await browser.close();
}

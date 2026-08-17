import { chromium } from 'playwright-core';
import { mkdir } from 'node:fs/promises';

const baseUrl = process.env.VIEWER_URL ?? 'http://127.0.0.1:1420';
const executablePath = process.env.PLAYWRIGHT_EXECUTABLE_PATH
  ?? '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge';
const outputDirectory = process.env.VISUAL_OUTPUT_DIR ?? '/tmp/report-window-visual';
await mkdir(outputDirectory, { recursive: true });

const browser = await chromium.launch({
  executablePath,
  headless: true,
  args: ['--enable-webgl', '--ignore-gpu-blocklist', '--use-angle=swiftshader'],
});
const page = await browser.newPage({ viewport: { width: 470, height: 800 } });
const pageErrors = [];
page.on('pageerror', (error) => pageErrors.push(error.message));
await page.goto(`${baseUrl}/?mode=report`, { waitUntil: 'networkidle' });

await page.evaluate(() => {
  document.getElementById('login-screen').hidden = true;
  document.getElementById('app-shell').style.display = 'none';
  const root = document.getElementById('report-window-root');
  root.hidden = false;

  const set = (id, text) => { const n = document.getElementById(id); if (n) n.textContent = text; };
  set('rw-title', '诊断报告 · 孙钰林');
  set('rw-patient-strip', '孙钰林 · 2608111111435 · 男 · 42 岁 · CT · 头颅CT 1.5×1.0 · 2026-08-08');
  set('rw-status', '未锁定 · 编辑中');
  set('rw-author', 'doctor');
  set('rw-reviewer', '--');
  set('rw-workitem-text', '已领取任务');
  document.getElementById('rw-workitem').hidden = false;
  document.getElementById('rw-release').hidden = false;
  document.getElementById('rw-findings').innerHTML = '<p><b>双肺纹理清晰</b>，未见明显实变影。</p><ul><li>肺实质：未见明显异常</li><li>纵隔：未见明显异常</li></ul>';
  document.getElementById('rw-impression').innerHTML = '<p>胸部 CT 未见明显急性异常。</p>';
  document.getElementById('rw-positive').checked = true;
});

await page.screenshot({ path: `${outputDirectory}/report-window.png`, fullPage: false });
const metrics = await page.evaluate(() => ({
  bodyScrollWidth: document.body.scrollWidth,
  viewportWidth: window.innerWidth,
  findingsWidth: document.getElementById('rw-findings').getBoundingClientRect().width,
  rootHeight: document.getElementById('report-window-root').getBoundingClientRect().height,
}));
console.log(JSON.stringify({ metrics, pageErrors }, null, 2));
await browser.close();

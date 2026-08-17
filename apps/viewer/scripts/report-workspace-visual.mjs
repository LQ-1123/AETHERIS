import { chromium } from 'playwright-core';
import { mkdir } from 'node:fs/promises';

const baseUrl = process.env.VIEWER_URL ?? 'http://127.0.0.1:1420';
const executablePath = process.env.PLAYWRIGHT_EXECUTABLE_PATH
  ?? '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge';
const outputDirectory = process.env.VISUAL_OUTPUT_DIR ?? '/tmp/report-workspace-visual';
await mkdir(outputDirectory, { recursive: true });

const browser = await chromium.launch({
  executablePath,
  headless: true,
  args: ['--enable-webgl', '--ignore-gpu-blocklist', '--use-angle=swiftshader'],
});

const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
const pageErrors = [];
page.on('pageerror', (error) => pageErrors.push(error.message));
await page.goto(baseUrl, { waitUntil: 'networkidle' });

// 进入报告工作台：隐藏登录屏，切 report-mode，注入 mock 内容
await page.evaluate(() => {
  document.querySelector('#login-screen').hidden = true;
  document.querySelector('#app-shell').style.display = 'grid';
  const workspace = document.querySelector('#workspace');
  workspace.classList.add('report-mode');
  for (const sel of ['#worklist-panel', '#viewport', '#details-panel', '#series-navigator']) {
    const node = document.querySelector(sel);
    if (node) node.style.display = 'none';
  }
  const rw = document.querySelector('#report-workspace');
  rw.hidden = false;

  const set = (id, text) => { const n = document.querySelector(id); if (n) n.textContent = text; };
  set('#report-patient-title', '孙钰林');
  set('#report-head-patient', '孙钰林');
  set('#report-head-hospital', '默认机构');
  set('#report-status', '未锁定 · 编辑中');
  set('#rp-patient-id', '2608111111435');
  set('#rp-patient-name', '孙钰林');
  set('#rp-patient-sex', '男');
  set('#rp-patient-age', '42 岁');
  set('#rp-modality', 'CT');
  set('#rp-study-date', '2026-08-08');
  set('#rp-series-desc', '头颅CT 1.5×1.0');
  set('#rp-study-desc', '头颅螺旋扫描（外伤）');
  set('#report-author', 'doctor');
  set('#report-updated-at', '2026-08-17 12:00');
  set('#report-reviewer', '--');
  set('#report-signed-at', '--');
  set('#report-workitem-text', '已领取任务');

  const findings = document.querySelector('#report-findings-editor');
  findings.innerHTML = '<p><b>双肺纹理清晰</b>，未见明显实变影。</p><ul><li>肺实质：未见明显异常</li><li>纵隔：未见明显异常</li></ul>';
  const impression = document.querySelector('#report-impression-editor');
  impression.innerHTML = '<p>胸部 CT 未见明显急性异常。</p>';
  document.querySelector('#report-positive').checked = true;

  const versions = document.querySelector('#report-versions-list');
  versions.innerHTML = '<li><div class="report-version-title">v2 · 2026-08-17 11:00</div></li><li><div class="report-version-title">v1 · 2026-08-17 10:00</div></li>';

  const tree = document.querySelector('#report-template-tree');
  tree.innerHTML = '<div class="report-template-group">chest</div><button type="button" class="report-template-item">CT-胸部</button><div class="report-template-group">head</div><button type="button" class="report-template-item">CT-头颅</button><button type="button" class="report-template-item">MR-头颅</button>';
});

await page.screenshot({ path: `${outputDirectory}/report-workspace.png`, fullPage: true });

const metrics = await page.evaluate(() => {
  const rw = document.querySelector('#report-workspace');
  const doc = document.querySelector('.report-document');
  const grid = document.querySelector('.report-workspace-grid');
  const editors = [...document.querySelectorAll('.report-editor')].map((e) => ({
    id: e.id,
    width: e.getBoundingClientRect().width,
  }));
  return {
    bodyScrollWidth: document.body.scrollWidth,
    viewportWidth: window.innerWidth,
    workspaceWidth: rw.getBoundingClientRect().width,
    gridColumns: grid ? getComputedStyle(grid).gridTemplateColumns : null,
    docWidth: doc.getBoundingClientRect().width,
    editors,
  };
});
console.log(JSON.stringify({ metrics, pageErrors }, null, 2));

await browser.close();

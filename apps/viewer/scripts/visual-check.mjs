import { chromium } from 'playwright-core';
import { mkdir } from 'node:fs/promises';

const baseUrl = process.env.VIEWER_URL ?? 'http://127.0.0.1:1420';
const executablePath = process.env.PLAYWRIGHT_EXECUTABLE_PATH
  ?? '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge';
const outputDirectory = process.env.VISUAL_OUTPUT_DIR ?? '/tmp/remote-pacs-phase6';
await mkdir(outputDirectory, { recursive: true });

const browser = await chromium.launch({
  executablePath,
  headless: true,
  args: ['--enable-webgl', '--ignore-gpu-blocklist', '--use-angle=swiftshader'],
});

try {
  const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
  const browserErrors = [];
  page.on('pageerror', (error) => browserErrors.push(error.message));
  await page.goto(baseUrl, { waitUntil: 'networkidle' });
  await page.screenshot({ path: `${outputDirectory}/login-desktop.png`, fullPage: true });

  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: `${outputDirectory}/login-mobile.png`, fullPage: true });

  await page.setViewportSize({ width: 1440, height: 900 });
  await page.evaluate(() => {
    document.querySelector('#login-screen').hidden = true;
    document.querySelector('#app-shell').style.display = 'grid';
    document.querySelector('#mpr-projection-control').hidden = false;
    document.querySelector('#vr-controls').hidden = true;
  });
  const toolbarMetrics = await page.evaluate(() => {
    const toolbar = document.querySelector('#toolbar');
    return {
      viewportWidth: window.innerWidth,
      scrollWidth: document.documentElement.scrollWidth,
      toolbarScrollWidth: toolbar.scrollWidth,
      toolbarWidth: toolbar.clientWidth,
    };
  });
  await page.screenshot({ path: `${outputDirectory}/mpr-toolbar-desktop.png`, fullPage: true });
  await page.setViewportSize({ width: 2048, height: 900 });
  const wideToolbarMetrics = await page.evaluate(() => {
    const toolbar = document.querySelector('#toolbar');
    return {
      viewportWidth: window.innerWidth,
      scrollWidth: document.documentElement.scrollWidth,
      toolbarScrollWidth: toolbar.scrollWidth,
      toolbarWidth: toolbar.clientWidth,
    };
  });
  await page.screenshot({ path: `${outputDirectory}/mpr-toolbar-wide.png`, fullPage: true });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.evaluate(() => {
    for (const button of document.querySelectorAll('[data-toolbar-menu-button], .toolbar-menu-panel button')) {
      button.disabled = false;
    }
  });
  await page.click('#measurement-menu-button');
  const desktopMenuBounds = await menuBounds(page, '#measurement-menu-panel');
  await page.screenshot({ path: `${outputDirectory}/toolbar-menu-desktop.png`, fullPage: true });
  await page.keyboard.press('Escape');

  await page.setViewportSize({ width: 390, height: 844 });
  await page.locator('#view-menu-button').scrollIntoViewIfNeeded();
  await page.click('#view-menu-button');
  const mobileMenuBounds = await menuBounds(page, '#view-menu-panel');
  await page.screenshot({ path: `${outputDirectory}/toolbar-menu-mobile.png`, fullPage: true });
  await page.keyboard.press('Escape');
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.evaluate(() => {
    document.querySelector('#mpr-projection-control').hidden = true;
    document.querySelector('#vr-controls').hidden = false;
  });
  await page.screenshot({ path: `${outputDirectory}/vr-toolbar-desktop.png`, fullPage: true });
  browserErrors.length = 0;

  await page.evaluate(async () => {
    const host = document.createElement('div');
    host.id = 'visual-volume-host';
    Object.assign(host.style, {
      position: 'fixed',
      inset: '0',
      zIndex: '10000',
      background: '#080b0d',
    });
    const canvas = document.createElement('canvas');
    canvas.id = 'visual-volume-canvas';
    canvas.style.width = '100%';
    canvas.style.height = '100%';
    host.append(canvas);
    document.body.append(host);

    const { VolumeRenderer } = await import('/src/volume-renderer.ts');
    const dimensions = [48, 48, 48];
    const voxels = new Uint16Array(dimensions[0] * dimensions[1] * dimensions[2]);
    for (let z = 0; z < dimensions[2]; z += 1) {
      for (let y = 0; y < dimensions[1]; y += 1) {
        for (let x = 0; x < dimensions[0]; x += 1) {
          const dx = (x - 23.5) / 18;
          const dy = (y - 23.5) / 15;
          const dz = (z - 23.5) / 20;
          const shell = Math.exp(-Math.abs(Math.sqrt(dx * dx + dy * dy + dz * dz) - 0.72) * 12);
          const core = Math.exp(-(dx * dx + dy * dy + dz * dz) * 4);
          voxels[(z * dimensions[1] + y) * dimensions[0] + x] = Math.round(
            Math.min(1, shell * 0.72 + core * 0.8) * 65_535,
          );
        }
      }
    }
    const renderer = new VolumeRenderer(
      canvas,
      voxels.buffer,
      {
        dimensions,
        spacing_mm: [0.8, 0.8, 1.2],
        value_range: [-1000, 1800],
        byte_length: voxels.byteLength,
        available: true,
        unavailable_reason: null,
      },
      { windowCenter: 500, windowWidth: 1800, preset: 'bone_color', quality: 'medium' },
    );
    renderer.resize(window.innerWidth, window.innerHeight);
    window.__phase6VolumeRenderer = renderer;
  });
  await page.waitForTimeout(1200);
  const desktopPixels = await countVolumePixels(page);
  await page.screenshot({ path: `${outputDirectory}/volume-desktop.png` });

  await page.setViewportSize({ width: 390, height: 640 });
  await page.evaluate(() => {
    window.__phase6VolumeRenderer.resize(window.innerWidth, window.innerHeight);
  });
  await page.waitForTimeout(500);
  const mobilePixels = await countVolumePixels(page);
  await page.screenshot({ path: `${outputDirectory}/volume-mobile.png` });
  await page.evaluate(() => window.__phase6VolumeRenderer.dispose());

  if (desktopPixels < 500 || mobilePixels < 100) {
    throw new Error(`体渲染画布像素不足: desktop=${desktopPixels}, mobile=${mobilePixels}`);
  }
  if (toolbarMetrics.scrollWidth > toolbarMetrics.viewportWidth) {
    throw new Error(`桌面布局出现横向溢出: ${JSON.stringify(toolbarMetrics)}`);
  }
  if (wideToolbarMetrics.scrollWidth > wideToolbarMetrics.viewportWidth) {
    throw new Error(`宽屏桌面布局出现横向溢出: ${JSON.stringify(wideToolbarMetrics)}`);
  }
  if (!desktopMenuBounds.insideViewport || !mobileMenuBounds.insideViewport) {
    throw new Error(`工具栏菜单超出视口: ${JSON.stringify({ desktopMenuBounds, mobileMenuBounds })}`);
  }
  if (browserErrors.length) throw new Error(`浏览器错误: ${browserErrors.join(' | ')}`);
  console.log(JSON.stringify({
    toolbarMetrics,
    wideToolbarMetrics,
    desktopMenuBounds,
    mobileMenuBounds,
    desktopPixels,
    mobilePixels,
    outputDirectory,
  }));
} finally {
  await browser.close();
}

async function countVolumePixels(page) {
  return page.evaluate(() => {
    const canvas = document.querySelector('#visual-volume-canvas');
    const context = canvas.getContext('webgl2');
    const volume = window.__phase6VolumeRenderer;
    volume.renderer.render(volume.scene, volume.camera);
    context.finish();
    const pixels = new Uint8Array(canvas.width * canvas.height * 4);
    context.readPixels(0, 0, canvas.width, canvas.height, context.RGBA, context.UNSIGNED_BYTE, pixels);
    let count = 0;
    for (let index = 0; index < pixels.length; index += 4) {
      if (pixels[index] + pixels[index + 1] + pixels[index + 2] > 45) count += 1;
    }
    return count;
  });
}

async function menuBounds(page, selector) {
  return page.locator(selector).evaluate((panel) => {
    const rect = panel.getBoundingClientRect();
    return {
      left: rect.left,
      top: rect.top,
      right: rect.right,
      bottom: rect.bottom,
      insideViewport: rect.left >= 0
        && rect.top >= 0
        && rect.right <= window.innerWidth
        && rect.bottom <= window.innerHeight,
    };
  });
}

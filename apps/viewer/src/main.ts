// 应用入口

import { App } from './app';

// 等待 DOM 和 Tauri 都加载完成
window.addEventListener('DOMContentLoaded', async () => {
  // 等待 Tauri API 就绪
  let retries = 0;
  while (retries < 50) {
    try {
      // 尝试动态导入 Tauri API
      const { invoke } = await import('@tauri-apps/api/core');
      if (typeof invoke === 'function') {
        console.log('Tauri API loaded successfully');
        break;
      }
    } catch (e) {
      console.log(`Waiting for Tauri API... (${retries}/50)`);
      await new Promise(resolve => setTimeout(resolve, 100));
      retries++;
    }
  }

  if (retries >= 50) {
    alert('Tauri API 加载失败，请重启应用');
    return;
  }

  const canvas = document.getElementById('canvas') as HTMLCanvasElement;
  if (!canvas) {
    throw new Error('Canvas element not found');
  }

  const app = new App(canvas);

  // 绑定打开文件按钮
  const openBtn = document.getElementById('open-btn');
  if (openBtn) {
    openBtn.addEventListener('click', () => {
      app.openFile();
    });
  }

  // 绑定键盘快捷键
  window.addEventListener('keydown', (e) => {
    if (e.key === 'o' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      app.openFile();
    }
  });

  console.log('PACS 查看器已启动');
});

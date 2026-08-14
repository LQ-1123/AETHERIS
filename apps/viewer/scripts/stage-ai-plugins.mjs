// 打包前把 ai-plugins 暂存到 src-tauri/staging/，剔除 .venv（开发机产物，含绝对路径不可分发）
import { cpSync, rmSync } from 'node:fs';
import { join } from 'node:path';

const src = new URL('../ai-plugins', import.meta.url).pathname;
const dst = new URL('../src-tauri/ai-plugins', import.meta.url).pathname;
rmSync(dst, { recursive: true, force: true });
cpSync(src, dst, {
  recursive: true,
  filter: (p) => !p.includes('/.venv') && !p.includes('/__pycache__'),
});
console.log('✓ ai-plugins 已暂存（剔除 .venv/__pycache__）→', dst);

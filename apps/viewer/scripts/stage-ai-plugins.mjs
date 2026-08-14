// 打包前把 ai-plugins 暂存到 src-tauri/staging/，剔除 .venv（开发机产物，含绝对路径不可分发）
import { cpSync, rmSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

// 用 fileURLToPath 而非 .pathname:后者在 Windows 上带前导斜杠(/D:/...),会拼出 D:\D:\... 双前缀
const src = fileURLToPath(new URL('../ai-plugins', import.meta.url));
const dst = fileURLToPath(new URL('../src-tauri/ai-plugins', import.meta.url));
rmSync(dst, { recursive: true, force: true });
cpSync(src, dst, {
  recursive: true,
  filter: (p) => !p.includes('/.venv') && !p.includes('/__pycache__'),
});
console.log('✓ ai-plugins 已暂存（剔除 .venv/__pycache__）→', dst);

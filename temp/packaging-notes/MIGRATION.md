# 迁移到另一台电脑

本项目的轻量迁移包只包含源码、依赖锁文件、数据库迁移、必要的前端静态资源和文档。
构建产物、依赖缓存、Git 历史、本机密钥与 PACS 影像数据均未包含。

## 恢复开发环境

1. 安装 Rust、Node.js、PostgreSQL；如需 DIMSE 联调，再安装 DCMTK。
2. 在项目根目录执行 `cp .env.example .env`，按新电脑的数据库和存储路径修改 `.env`。
3. 执行 `cargo build --workspace` 构建服务端。
4. 进入 `apps/viewer`，执行 `npm ci` 和 `npm run build` 构建 Viewer。
5. 如需本地 AI 插件，进入对应的 `apps/viewer/ai-plugins/<插件名>`，执行其 `setup.sh`。

数据库结构会在 `pacsd` 启动时自动迁移。原电脑上的数据库内容、PACS 影像、TLS 私钥和
`.env` 不在轻量迁移包内；如需迁移生产数据，应单独进行数据库备份和影像目录复制。

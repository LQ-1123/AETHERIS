# 临时归档

此目录集中保存不属于当前源码工作区的资料和可重建产物：

- `completed-specs/`：已完成的设计规格
- `document-exports/`：可由源文档重新导出的办公文档
- `historical-plans/`：历史计划与需求分析
- `packaging-notes/`：已被当前打包文档替代的迁移说明
- `unused-images/`：当前文档不再引用的图片
- `generated/`：构建缓存、依赖、安装包、工具输出和系统元数据（Git 忽略）
- `acceptance/`：本地验收截图（Git 忽略）

`generated/` 与 `acceptance/` 可以随时删除或重新生成，不应作为源码或运行数据使用。

开发期间请保留 `apps/viewer/node_modules/`、`apps/viewer/dist/`、
`apps/viewer/src-tauri/local-stack/` 和 `apps/viewer/src-tauri/ai-plugins/` 在原路径；它们虽可
重建，但分别是 Vite/Tauri 启动、桌面打包和本地运行栈的活动输入。只有停止开发服务后，才适合
将旧副本归档到这里。

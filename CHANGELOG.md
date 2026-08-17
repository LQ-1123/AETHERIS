# Changelog

本项目所有值得记录的变更。

## [v0.2.0] — 2026-08-17（增量更新）

> v0.2.0 的打包产物已更新，包含 2026-08-15 之后的全部变更（报告闭环、分栏、状态徽标等）。

### 报告工作台（B2 报告闭环）

- 报告按检查一份（study 级）：去掉领取/释放流程，工作项幂等创建，报告创建/草稿/签发/修订全链路。
- 报告状态徽标接入病人/检查列表：待书写 / 书写中 / 已锁定 / 已签发 四态。
- 结构化报告（template_payload）：所见 / 印象 / 建议 + `is_positive` 阳性标记，签发写入不可变版本快照。
- I2 规则：结构化报告草稿更新必须携带 template_payload，唯一例外是 `clear_template_payload` 迁移开关。
- 报告独立小窗（双屏工作流）：影像与报告分离，`?mode=report` 独立窗口。
- 全屏报告工作台 + 富文本编辑器（所见 / 意见分块）+ 模态模板树（HEAD / CHEST / ABDOMEN）。
- 修订流程（amend）：签发后可发起修订，版本 +1，审计日志记录签发/修订。
- 角色权限：仅 radiologist（医师）可写报告，admin 运行时硬校验拒绝（只读）。

### 界面与交互

- 左侧病人列表与右侧检查信息面板可拖拽调宽（280–520px / 240–420px），宽度持久化到 localStorage。
- 报告状态徽标防遮挡：从绝对定位改为 grid 文档流，长文案自动换行，不再覆盖标题/元数据。
- 管理控制台 tab 修复：文字标签不再竖排堆叠（覆盖 `.segmented` 32px 图标宽度限制）。

### 构建与工程

- 修复 CI 门禁：`cargo fmt` 全量规范化、clippy `-D warnings` 清理（测试补全 `create_report` / `update_report_draft` 新参数、测试库锁改 tokio async）。
- 修复 `detailsWidth` 使用 `Array.prototype.at`（ES2022）超出 tsconfig ES2020 lib 导致的 Windows 构建失败。
- macOS DMG 打包绕过 Tauri bundle_dmg.sh 参数缺陷（`--icon` 约定不匹配），改用 hdiutil 手动创建。
- 双平台产物：`AETHERIS_0.2.0_aarch64.dmg`（macOS Apple Silicon）+ `AETHERIS-Setup-0.2.0-x64.exe`（Windows x64 零依赖安装包，内嵌 PostgreSQL + launcher）。

## [v0.2.0] — 2026-08-15

### GPU Oblique MPR（任意角度多平面重建）

- 进入 MPR 后自动加载 GPU Volume，三视图升级为患者空间联动的斜切面重建。
- 十字交叉线即基准线，拖拽旋转；三平面保持正交且经过同一患者空间中心。
- 双击恢复标准轴向/冠状/矢状；视图显示交面指示、动态方向标签与偏转角度。
- Physical-space accuracy pass：统一 Patient/Voxel/MPR 空间仿射，DICOM IOP/IPP/PixelSpacing 计算几何，支持各向异性体素与真实物理测量。

### 修复

- 进入 MPR 初始帧黑屏、Oblique 旋转方向与鼠标不一致、GLSL `sample` 保留字导致的 GPU 渲染纯黑。

## [v0.1.0] — 2026-08-14

- 首个可用版本：DICOM 网络（DIMSE C-STORE/C-FIND/SCU/SCP）、DICOMweb、持久存储、2D/多窗格阅片、MPR、体渲染、AI 分割、标注、TAG 修订、生命周期、DICOM 路由引擎、管理员控制台（设备/来源归属/用户授权）。

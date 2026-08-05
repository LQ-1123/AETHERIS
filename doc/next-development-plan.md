# Remote PACS 下一阶段开发计划

> 计划确认日期：2026-08-05
> 当前基线：DIMSE 接收与查询、QIDO/WADO、账号权限、远程 Viewer、MPR、
> ROI/共享标注和 DICOM Tag 版本管理已经完成。

> **2026-08-05 实施更新：阶段一已完成。** 已落地通用后台任务与明细、
> PostgreSQL 租约/重试/取消/幂等机制、服务账号与 scoped API Key、API 限流、
> OpenAPI、全局请求 ID，以及支持 JWT/API Key 的 STOW-RS。阶段二已完成，下一步从阶段三开始。

## 开发顺序

后续按照“数据入口与可靠性 → 数据流转 → 科研数据能力 → Viewer 专科能力”推进：

1. 统一后台任务和开放 API 基础。
2. 批量导入、DICOM 去重和 ZIP 导出。
3. DICOM 路由引擎。
4. DICOM 生命周期策略。
5. Mask 标注和高级查询。
6. Viewer 多模态与 3D 增强。

预计总工作量为 18–24 工程周。每个阶段必须独立交付并通过验收，避免八个方向
同时展开后留下多套不一致的任务、权限和入库逻辑。

## 阶段一：后台任务与开放 API 基础（2–3 周）

- 建立 PostgreSQL 后台任务框架，支持状态、明细、进度、取消、租约、重试、
  失败原因和幂等键。
- 导入、导出、路由和生命周期使用统一任务模型；现有 Tag 修订任务保持独立，
  不在本阶段做高风险迁移。
- 增加服务账号和哈希存储的 API Key，权限范围至少包含
  `search/read/upload/export/route/admin`。
- API 使用 `/api/v1` 版本前缀，提供 OpenAPI 文档、请求 ID、统一错误结构、
  限流和审计。
- API Key 只在创建时显示一次，支持过期、吊销和最后使用时间记录。
- 补齐 STOW-RS，使 Viewer、外部平台和路由引擎共用同一上传入口。

## 阶段二：批量导入、去重与 ZIP 导出（3–4 周）

- Viewer 支持选择文件夹、ZIP 和 RAR，通过分块上传会话发送到服务端，支持
  进度、取消和断点续传。
- 服务端按文件魔数识别格式，在隔离目录流式解包；ZIP/RAR 使用
  `compress-tools + libarchive` 读取。
- 限制文件数量、单文件大小、解压总量和压缩比，拒绝路径穿越、符号链接、
  加密包和解压炸弹。
- 非 DICOM 文件作为“跳过”记录，不中断整个批次。
- 所有入口统一经过“DICOM 解析 → UID 图校验 → SHA-256 → 原子落盘 →
  数据库事务”的摄取链路。
- SOP UID 与 SHA-256 均相同视为幂等重复；SOP UID 相同但内容不同视为冲突；
  Study/Series UID 被错误跨层复用视为结构冲突。
- Tag 修订产生的历史 UID 继续参与冲突检测，禁止重新创建第二个逻辑实例。
- 导入任务输出新增、重复、冲突、无效和失败统计，以及逐文件报告、来源和审计。
- 导出支持按 Study 或 Series 选择，默认打包当前有效 DICOM、已发布 DICOM SEG、
  `manifest.json` 和每个文件的 SHA-256；导出包到期自动清理。

## 阶段三：DICOM 路由引擎（3–4 周）

- 目标设备支持 DIMSE AE 和 STOW-RS；DIMSE 配置 AE Title、主机、端口和 TLS，
  STOW 配置 URL、认证和 CA 证书。
- 提供 DIMSE C-ECHO 和 STOW 连接测试，并记录最近成功时间、延迟和错误。
- 路由条件支持来源、机构、模态、BodyPartExamined、Study/Series 描述和指定
  DICOM Tag。
- 第一版固定在实例成功入库后触发，以目标和 SOP UID 生成幂等投递项，防止
  重启、重试和路由环路造成重复发送。
- 实现 DIMSE C-STORE SCU 与 STOW-RS 发送端，支持指数退避、最大重试、
  死信队列和人工重放。
- Viewer 提供目标、规则优先级、启停、连接测试、任务监控和失败重放界面。
- 路由失败不得回滚本地入库，也不得阻塞接收设备的 C-STORE 成功响应。

## 阶段四：DICOM 生命周期策略（2–3 周）

- 首版采用本地热目录、冷目录和隔离区，存储接口保留以后接入 S3/MinIO 的能力。
- 策略支持按模态、检查日期、最后访问时间、机构、标签和存储占用匹配。
- 动作支持转冷层、恢复、隔离和申请清除。
- 文件迁移严格执行“复制 → SHA-256 校验 → 数据库切换 → 验证读取 → 删除源文件”，
  任意失败都必须至少保留一个可读取副本。
- 到期数据默认先进入隔离区；管理员审批并经过可配置宽限期后才能物理清除。
- 增加 Legal Hold，命中的 Study 不得进入清除流程。
- 清除以 Study 为边界，同时处理当前版本、历史版本、DICOM SEG、标注和导出缓存，
  并保留不可变审计记录。
- 策略启用前必须支持预演，展示预计命中数量和空间变化。

## 阶段五：Mask 标注与高级查询（4–5 周）

- 增加分割项目、标签体系、分割版本、Segment 元数据和逐帧稀疏 Mask。
- 编辑数据采用压缩 RLE 或瓦片存储，不把大 Mask 放进现有普通标注 JSONB。
- 工具包括画刷、橡皮、多边形、阈值区域生长、连通域、形态学修整和相邻层插值。
- 支持 Undo/Redo、多人 Revision 冲突和操作审计。
- 发布 Mask 时生成标准 DICOM SEG，写入来源影像引用、Segment Label、算法类型、
  显示颜色和空间几何，并通过正常入库链路归档。
- 导入已有 DICOM SEG 时解析引用关系和 Segment 元数据；找不到来源影像时保留为
  待关联状态。
- 新增 `/api/v1/search/studies` 结构化高级查询，支持性别、检查时年龄、模态、
  部位、日期、标签、分割状态和 Segment Label 组合过滤。
- 年龄按 `StudyDate - PatientBirthDate` 计算，不保存会随时间失真的“当前年龄”。
- 查询结果可以直接发起 ZIP 导出、路由或科研数据集任务。

## 阶段六：Viewer 多模态与 3D（4–6 周）

- CR/DX/MG 完善高位深灰度、反色、方向、常用窗和投影间距可信度；未校准距离
  必须明确标记为探测器平面测量。
- US 支持 RGB、YBR、Palette Color、单帧和多帧 Cine，以及播放速度和帧导航。
- PET 支持 PET Storage SOP Class、Rescale、单位和活度值；所需标签完整时计算
  SUVbw，否则明确显示 SUV 不可用。
- MG 首阶段只完成可靠显示、测量和序列布局，不实现完整专科 Hanging Protocol。
- 保留现有 Rust MPR，复用规则体数据增加 Slab 厚度可调的 MIP 和 MinIP。
- VR 使用 VTK.js GPU 体渲染，支持传递函数、窗宽窗位、采样质量和预设；GPU 或
  WebGL 不满足要求时明确禁用。
- PET/CT 融合、MG 专科挂片和 AI 自动分割进入后续阶段。

## 测试与验收

- 使用 DCMTK 验证 C-ECHO、C-STORE、重复发送、目标离线、重试和路由重放。
- 使用标准 DICOMweb 客户端验证 STOW、QIDO 和 WADO。
- 导入测试覆盖损坏包、加密包、嵌套目录、路径穿越、解压炸弹、重复 UID 和
  同 UID 异内容。
- 生命周期对复制、校验、数据库提交和删除阶段做故障注入。
- 使用标准验证工具检查 DICOM SEG 引用、Frame of Reference、帧几何和 Segment
  元数据，并执行导出后重新导入测试。
- 高级查询使用百万级元数据基准，普通组合查询目标为 p95 小于 300 ms，并验证
  Institution 隔离。
- Viewer 使用真实匿名 CR、DX、MG、US 和 PET 数据集做像素值与截图回归；
  MIP/MinIP/VR 验证方向、非空画面、资源释放和大序列交互。
- 每阶段必须通过 Rust 单元/集成测试、PostgreSQL 迁移测试、TypeScript 测试、
  Clippy 和 Viewer 构建。

## 固定决策

- 文件导入主要由 Viewer 发起并上传服务端，客户端不直接写 PACS 存储目录。
- ZIP 导出默认使用当前有效 DICOM 版本和已发布 DICOM SEG，不默认包含全部历史版本。
- Mask 第一阶段采用手工与半自动算法，不引入 AI 模型和 GPU 推理。
- 生命周期首版使用本地热层、冷层和隔离区，并执行“隔离、审批、宽限期、物理清除”。
- 路由第一版同时支持 DIMSE 和 DICOMweb STOW-RS。
- 外部平台使用服务账号 API，不长期复用个人 Viewer Token。

## 实施进度

- [x] 计划确认并写入项目文档。
- [x] 阶段一：后台任务与开放 API 基础。
- [x] 阶段二：批量导入、去重与 ZIP 导出。
- [ ] 阶段三：DICOM 路由引擎。
- [ ] 阶段四：DICOM 生命周期策略。
- [ ] 阶段五：Mask 标注与高级查询。
- [ ] 阶段六：Viewer 多模态与 3D。

### 阶段一验收记录（2026-08-05）

- 数据库迁移 `0009`/`0010` 在真实 PostgreSQL 应用成功。
- 后台任务的幂等创建、租约领取、进度、重试、取消、明细终态和 Worker 所有权
  已通过集成测试。
- 服务账号创建、一次性 API Key、scope 鉴权、吊销立即失效已通过 HTTP 集成测试。
- STOW-RS 首次上传和相同实例重复上传已通过端到端测试，重复上传只保留一条实例。
- 根 workspace 全 target 测试通过，包含 DCMTK C-ECHO/C-STORE/C-FIND、QIDO、WADO、
  PostgreSQL、文件字节保真与版本化修订测试。
- 根 workspace `cargo clippy --workspace --all-targets -- -D warnings` 通过。
- Viewer 18 项 TypeScript 测试、生产构建、17 项 Tauri Rust 测试和严格 Clippy 通过。

### 阶段二验收记录（2026-08-05）

- 新增迁移 `0011_transfer_jobs.sql`，真实 PostgreSQL 已验证延迟任务释放、机构隔离、
  分块偏移冲突、顺序续传和文件完成状态。
- `/api/v1/imports` 支持文件、文件夹展开、ZIP/RAR 上传，8 MiB 顺序分块、进度、
  取消和错误后按服务端偏移续传；上传临时文件仅使用服务端 UUID 命名。
- ZIP/RAR 统一使用 `compress-tools + libarchive`，按魔数识别并限制条目数、单文件、
  解压总量和压缩比；路径穿越、非普通文件、损坏包和加密 ZIP 测试通过。
- STOW-RS 与批量导入共用摄取函数，统一执行 DICOM 解析、UID 关系校验、不可变落盘、
  SHA-256 去重和同 UID 异内容冲突报告；非 DICOM 条目按 `skipped` 记录。
- `/api/v1/exports` 支持 Study/Series 当前有效版本 ZIP，路径稳定，包含
  `manifest.json`、实例 UID、大小与 SHA-256；导出包可重新由导入归档读取，并在
  24 小时后自动清理。
- Viewer 工作列表增加文件/ZIP/RAR、文件夹导入，Study/Series ZIP 导出、进度和
  取消控件；前端 18 项测试、生产构建、Tauri 17 项测试和严格 Clippy 通过。
- 根 workspace 全 target 测试和严格 Clippy 通过，包含 PostgreSQL、DCMTK、STOW、
  QIDO、WADO、归档安全和导出 manifest/hash 测试。
- 本机没有 RAR 创建工具，未动态生成 RAR/加密 RAR 夹具；RAR 代码路径由已安装的
  libarchive 3.8.9 提供，部署与 CI 必须安装 libarchive，并应补充固定 RAR 夹具回归。

# 检查队列：问题核实与改进方案

日期：2026-08-18
状态：待评审（仅文档，未改代码）
范围：`apps/viewer`（前端）为主，少量涉及 `crates/pacs-db` / `crates/pacs-web`
背景：codex 实现了「高级工作列表：独立患者队列页面」雏形（`next-plan.md`），用户实测发现 4 个问题，本文逐条核实根因并给出改进方案。

---

## 问题一：应用启动后首页是空白阅片区，不是病人列表

### 现象
登录成功后直接进入全屏阅片工作区（`#workspace` 可见、`#viewport` 空白），看不到任何病人列表。用户期望首页就是病人列表（队列页）。

### 证据（代码核实）
- `apps/viewer/src/app.ts:4314-4317`：登录成功流程由 `await this.loadPatients()` 改为 `this.queuePage.refresh()`。
- `apps/viewer/src/queue-page.ts:133-135`：`refresh()` 内部是 `if (this.opened) void this.load();` —— **队列页未打开时 refresh 什么都不做**。
- `apps/viewer/index.html:601`：`<section id="queue-page" hidden …>`，队列页默认隐藏；`#workspace` 默认显示。
- `apps/viewer/src/app.ts:110-118`：队列页只有在点击工具栏 `#queue-btn` 后 `open()` 才显示并加载。

### 根因
登录成功只做了「刷新队列数据」，但从未自动 `queuePage.open()`。旧代码 `loadPatients()` 会填充左侧栏（侧栏在登录后默认可见），因此旧版首页有病人列表；侧栏被替换为全屏队列页后，没有承接「默认展示工作列表」这一行为。

### 改进方案
**推荐（A）**：登录成功后自动打开队列页，作为应用首页。
- 位置：`app.ts` 登录成功分支（现 4317 行 `this.queuePage.refresh()` 处）改为 `this.queuePage.open()`（open 内部会 load，refresh 调用可去掉）。
- 语义：队列页即「工作列表首页」；阅片后点「返回阅片」回到队列页（现有 `close()` 逻辑），符合放射科「进应用先看今天有哪些检查」的工作流。
- 副作用检查：`open()` 会隐藏 `#workspace` 并聚焦返回按钮，需确认不影响登录后的初始化流程（窗口预设加载、转换工具初始化等在 `open()` 之前完成，见 4314 行前后顺序）。

**备选（B）**：启动后保持阅片区，但 `#viewport` 显示空态引导（如「打开工作队列查看检查」按钮）。改动更小，但不满足「首页是病人列表」的诉求，仅作兜底。

---

## 问题二：工具栏「打开序列」与「导入」语义不清 + 导入下拉被裁剪

### 现象
1. 顶部工具栏同时有「打开序列」和「导入」两个按钮，用户不清楚区别，怀疑功能重合。
2. 点击「导入」后下拉菜单被界面裁剪，几乎看不到选项（只有一小条露出或完全不可见）。

### 证据（代码核实）
**功能区别（实际不重合）：**
- 「打开序列」`app.ts:1473 openFiles()` → `chooseDicomFiles()`（`api.ts:68`，仅 dcm/dicom/*）→ `openSeries(paths)`（`api.ts:209` → Tauri `open_series`）：**本地临时解析阅片，不写入 PACS**。
- 「导入」`app.ts:4446 chooseAndImport()` → `chooseImportFiles()`（`api.ts:79`，dcm/dicom/zip/rar/*）或 `chooseImportFolder()`（`api.ts:87`，目录）→ `importToPacs(paths)`（`api.ts:93` → `import_to_pacs`）：**上传到服务端入库（STOW），随后刷新队列**。
- 结论：一个是「本地打开看片（不入库）」，一个是「上传入库（进工作队列）」。功能**不重合**，但两者都是「选 DICOM 文件」的对话框，名称与图标相近，极易混淆。

**下拉被裁剪（真实 bug）：**
- 迁移后的 `#import-menu` 位于 `#toolbar` 内部（`index.html:57-71`）。
- `#toolbar`（`styles.css:164-176`）：`overflow-x: auto; overflow-y: hidden;` —— **overflow-y hidden 会裁剪越出工具栏的子元素**。
- `.import-menu-panel`（`styles.css:1343-1356`）：仍是侧栏时代的 `position: absolute; top: 35px;`（相对 `.import-menu`），从工具栏底边往下展开 35px+，**超出 48px 高的 toolbar 后被裁剪**。
- 对比：其他工具栏下拉 `.toolbar-menu-panel`（`styles.css:254`，`position: fixed; z-index: 90`）与 `.mask-menu-panel`（`styles.css:70` 系，`position: fixed`）都是 fixed 定位、脱离 toolbar 裁剪范围——**只有迁移过来的 import 菜单没改定位方式**。

### 根因
1. 侧栏「导入」菜单迁移到工具栏时，只搬了 DOM 和事件绑定，没有把定位方式从 absolute 改为 fixed，被 toolbar 的 overflow-y:hidden 裁剪。
2. 「打开序列」与「导入」并存是旧功能布局的自然结果（本地打开是阅片能力，导入是入库能力），但 UI 上没有区分语义。

### 改进方案
**砍掉导入功能，不允许由本地数据上传污染服务器**
- 唯一上传病人到服务器的方法只有通过DICOM协议，避免私人图像上传污染数据库，私人文件可以通过打开序列（后续更名为“本地打开”），实现仅看图服务。不提供报告服务，也不将私人图像列入数据库。
---

## 问题三：双击队列行后只能看到一个检查的一个序列，无法浏览该患者全部检查与序列

### 现象
在队列页双击某检查后，阅片区只打开了一个序列（MPR 推荐序列），看不到：
- 该检查的其他序列；
- 该患者的其他检查。

旧侧栏（患者 → 检查 → 序列 三层树）可以提供完整浏览，现在入口被替换掉了。

### 证据（代码核实）
- `apps/viewer/src/queue-page.ts:272-293 openRow()`：`listStudySeries(studyUid)` → `this.recommendSeries(series) ?? series[0]` → `openSeries(studyUid, seriesUid)`——**只打开单个序列**。
- `app.ts:4542-4550 openRemote()` → `activateSeries(() => openRemoteSeries(...))`：激活单个远程序列上下文，`activateSeries` 会整体替换当前 state（`app.ts:1483` 起）。
- `app.ts` 全文无「切换同检查其他序列」的 UI（grep `series` 切换控件无结果）；唯一的多序列/多检查导航是旧侧栏 `renderPatients()` 里的 `study-list` / `series-list`（`app.ts:5038/5079`），侧栏已 `hidden`。
- 设计文档 `next-plan.md:39` 明确「双击跳转直接复用 openRemote，无跨窗状态同步」，`4.4.1` 只开推荐序列——**这是设计取舍，不是 bug，但与用户工作流（浏览整个患者）冲突**。

### 根因
「替换侧栏」决策把唯一的患者/检查/序列浏览树拿掉了，而队列页只提供了「检查级一行 → 双击开一个序列」的极简链路，中间没有承接「患者维度浏览」的替代 UI。

### 改进方案
**推荐：双击后进入看图界面，看图界面可恢复曾经的左侧病列表，但是该列表仅显示检查和检查下的序列。**
- 复用曾经的代码，保留拖拽入工作区分屏的功能。

---

## 问题四：管理员失去「编辑 DICOM 标签」入口

### 现象
管理员（admin/technician）登录后找不到编辑 DICOM tag 的入口；对话框本身还在，但无法触发。

### 证据（代码核实）
- tag 编辑器本体完好：`index.html:774-797` `#tag-editor-dialog`（编辑 DICOM 标签对话框）；`app.ts:4571 openTagEditor()` 逻辑完整。
- **入口按钮只渲染在旧侧栏里**：`app.ts:4990 appendTagEditButton()` 仅被 `renderPatients()` 内部的 3 处调用——患者行（5022）、检查行（5064）、序列行（5131）。侧栏 `#worklist-panel` 被 `hidden`（`index.html:352`）后，这些按钮全部不可见。
- 权限判定 `app.ts:4556 canEditDicomTags()`：`role === 'admin' || role === 'technician'`，逻辑未变。
- 队列页 `queue-page.ts` 行渲染（`renderRow`）**没有**渲染任何 tag 编辑按钮。

### 根因
tag 编辑入口与旧侧栏行渲染耦合，侧栏替换为队列页时未迁移该入口。功能代码（对话框、编辑器、权限、提交逻辑）全部保留，只缺一个触发点。

### 改进方案
**推荐（A）：在队列页行内增加「编辑标签」按钮。**
- `queue-page.ts renderRow()` 对 admin/technician 显示编辑按钮（复用 `TagEditorContext` 的 study 级 scope，`app.ts:4990` 的按钮创建逻辑可抽为共享函数或由 app 注入回调）。
- 队列行是检查级，天然对应 study 级 tag 编辑；patient/series 级入口可后续在「患者上下文视图」（问题三方案 A）里补。

**备选（B）：工具栏「更多」菜单增加「编辑 DICOM 标签」，作用于当前打开的序列（study/series 级）。**
- 入口常驻、不依赖列表；但要求当前已打开序列，队列页场景下要多一步双击。

**备选（C）**：管理员控制台（`admin-console.ts`）加入 tag 编辑入口，把临床操作与管理台分开。改动最大，不推荐作为首选。

---

## 汇总：改动文件与工作量预估

| 问题 | 涉及文件 | 预估 |
| --- | --- | --- |
| 一（首页=队列） | `app.ts` 登录成功分支（1-2 行） | 0.5h |
| 二（裁剪修复） | `styles.css` `.import-menu-panel` 定位 + `app.ts` 展开时坐标计算（参照 `positionMaskMenu`） | 1-2h |
| 二（语义区分） | `index.html` 按钮文案 + 可选副标题 | 0.5h |
| 三（患者上下文浏览） | `queue-page.ts`（或新模块）+ 复用 `list_patient_studies` / 树渲染 | 0.5-1 天 |
| 四（tag 编辑入口） | `queue-page.ts` 行按钮 + `app.ts` 共享按钮创建逻辑 | 2-3h |

## 建议实施顺序与验收

1. **问题二裁剪修复**（纯 bug，影响所有用户使用导入）→ 验收：点击「导入」下拉完整可见，不被裁剪，点击空白处关闭。
2. **问题一首页**（影响首次体验）→ 验收：登录后直接看到队列页，数据已加载，返回/打开链路正常。
3. **问题四入口恢复**（功能回归，管理员受影响）→ 验收：admin/technician 在队列行可见编辑按钮，对话框打开、保存、审计原因必填等原有行为不变；非管理员不可见。
4. **问题三患者上下文**（工作量最大，涉及交互设计）→ 建议单独评审方案 A/B 后实施 → 验收：双击后能浏览该患者全部检查与序列，切换后阅片正常，返回路径清晰。

## 附：本次核实未覆盖 / 需注意

- 本文档未运行构建与测试验证；实施前建议先跑 `cargo test` 与前端 `tsc` 确认基线。
- 问题二结论为「功能不重合」，若产品上仍希望二选一，需先确认「本地打开阅片（不入库）」是否为保留能力（它同时承载 Ctrl+O 快捷键与空态按钮）。
- 侧栏（`#worklist-panel`、`loadPatients`/`renderPatients`）仍为隐藏壳，问题三若选方案 A 可直接复用其渲染逻辑，问题四若选方案 A 可抽出其按钮逻辑；届时可考虑真正删除侧栏代码。

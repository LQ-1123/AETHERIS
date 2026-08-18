# B2-3 报告独立小窗（双屏工作流）· 设计文档

日期：2026-08-17
状态：待用户评审
范围：`apps/viewer`（前端多窗口 + 紧凑单栏 UI；后端零改动）

## 1. 背景与目标

放射科医生普遍双屏：一块屏阅片、一块屏写报告。当前 B2-2 的「报告工作台」是
**全屏替换**模式（影像↔报告二选一），无法边看边写。本设计把报告改为 **独立 Tauri
窗口**（紧凑单栏小窗），医生拖到第二块屏；主窗口始终是阅片器。

## 2. 已确认决策

| 决策点 | 选择 |
| --- | --- |
| 落地方式 | A · 独立报告窗口（Tauri WebviewWindow，系统级独立窗口，可拖到第二屏） |
| 内容形态 | 紧凑单栏（标题栏 + 一行患者信息 + 所见/意见富文本 + 底部动作；模板/修改记录收进抽屉） |

## 3. 架构

### 3.1 多窗口与共享会话

- 主窗口（label 缺省 `main`）= 阅片器；新增 `report` 窗口 = 报告小窗。
- `RemoteState`（含登录 session、access token）已通过 `app.manage(remote)` 全局托管，
  **所有窗口的命令共享同一 session**——报告窗无需重新登录，`list_reports` /
  `work_item_for_series` / `update_report_draft` 等命令在报告窗直接可用。

### 3.2 路由

- 报告窗加载 `index.html?mode=report`；`main.ts` 在 DOMContentLoaded 时检查
  `mode=report`：走报告窗分支（只初始化报告 UI），否则走现有 `new App()`。
- `index.html` 新增 `<div id="report-window-root" hidden>`；报告分支隐藏
  `#login-screen` 与 `#app-shell`，显示该根容器。

### 3.3 窗口创建与聚焦

- 主窗工具栏「报告」按钮 → 调 Rust 命令 `open_report_window()`：
  - 已存在则 `set_focus()` 聚焦；不存在则 `WebviewWindowBuilder::new("report")`
    创建（尺寸约 460×780、`decorations=true` 系统标题栏、可缩放、可拖到第二屏）。
  - 返回前把当前上下文快照存进内存（见 3.4）。
- 报告窗「影像」按钮 → `emit` 事件让主窗聚焦（`app.get_webview_window("main").set_focus()`）。

### 3.4 状态同步（主窗 ↔ 报告窗）

- **方向 1：主窗 → 报告窗（上下文推送）**
  - 主窗在「开窗时」「切换/打开序列时」`emit("report-context", payload)`，payload 含：
    `study_uid / series_uid / modality / patient_name / patient_id / patient_sex /
    patient_birth_date / study_date / study_description / series_description /
    institution_name`，以及当前用户 `{ id, role, display_name, username }`。
  - 报告窗 `listen("report-context")` → 存上下文 → `refresh()`。
- **方向 2：报告窗启动自愈（拉取兜底）**
  - 报告窗 `DOMContentLoaded` 后主动 `invoke("get_report_context")` 拉一次上下文快照
    （Rust 侧内存暂存，主窗 emit 时同步写入），避免「报告窗未就绪时事件已发」的丢失。
- 主窗关闭时关闭报告窗（`on_window_event` 或主窗 destroy 时 report 窗 close），
  避免孤儿窗口。

### 3.5 后端

- **零改动**。is_positive / clear_template_payload / 工作项查询等 B2-2 后端能力已就绪。
- 仅新增两个 Tauri 命令（非 HTTP）：`open_report_window`、`get_report_context`
  （上下文内存快照，存于 App 级托管状态 `ReportWindowState`）。

## 4. 报告窗 UI（紧凑单栏）

```
┌ 系统标题栏（窗口原生）                                 ┐
│ 诊断报告 · 孙钰林                    [编辑中] [影像▸] │
│ 孙钰林 · 2608111111435 · 男 · 42岁 · CT · 2026-08-08 │ ← 一行患者信息
│ ────────────────────────────────────────────────      │
│ 影像所见  [B][I][U][☰]                     [模板▸]    │
│ ┌──────────────────────────────────────────────┐      │
│ │ contenteditable 富文本                        │      │
│ └──────────────────────────────────────────────┘      │
│ 意见  [✓阳性]  质控：无                                │
│ ┌──────────────────────────────────────────────┐      │
│ │ contenteditable 富文本                        │      │
│ └──────────────────────────────────────────────┘      │
│ 报告医生 xx · 审核医生 xx · 时间 ...（一行签名）        │
│ ────────────────────────────────────────────────      │
│ [领取任务] [保存草稿 Ctrl+S] [签发 Ctrl+Enter] [修订]  │
└───────────────────────────────────────────────────────┘
```

- 右侧「模板▸」「修改记录▸」为**可折叠抽屉**（滑出侧栏），收起后是纯单栏。
- 状态徽标复用 `编辑中/已签发/修订中` 三色映射。
- 领取/新建/修订/版本历史逻辑与 B2-2 的 `report-workspace.ts` 一致，抽到
  `report-window.ts`（复用 `api.ts` + `rich-text.ts`）。

## 5. 与现有代码的关系

- 新建：`report-window.ts`（报告窗主逻辑）、`report-window-state`（Rust 上下文快照 +
  两个命令）、`index.html` 的 `#report-window-root` 骨架。
- 复用：`api.ts`（报告/工作项/模板 API）、`rich-text.ts`（sanitize/转换）。
- 退役：`report-workspace.ts`（全屏版）与 `viewerMode: 'report'` 相关分支——被独立
  窗口取代。删除前确保主窗「报告」按钮改走 `open_report_window`。

## 6. 测试与验收

- 纯逻辑：`rich-text` 已有 5 项单测；新增「上下文 payload 序列化」无需额外逻辑。
- Playwright 布局自检：扩展 `scripts/report-workspace-visual.mjs` 思路，加一个
  `report-window` mock 截图（`mode=report` 分支的紧凑单栏），vision 复核无塌陷。
- 手工验收清单：
  1. 主窗开检查 → 点「报告」→ 弹出独立小窗（可拖到第二屏）
  2. 主窗切换序列 → 报告窗患者信息/报告随之切换
  3. 报告窗领取 → 新建 → 富文本书写 → Ctrl+S 保存 → 签发，全链路与全屏版一致
  4. 报告窗点「影像」→ 主窗聚焦；关闭主窗 → 报告窗一并关闭
  5. 未登录（无 session）时开报告窗 → 提示先登录，不崩溃

## 7. 工作量

| 部分 | 估算 |
| --- | --- |
| 多窗口骨架（窗口创建/聚焦/销毁 + main.ts 路由） | ~1 天 |
| 上下文状态同步（emit/listen + 内存快照 + 两个命令） | ~1 天 |
| 报告窗紧凑单栏 UI + 抽屉 | ~1.5 天 |
| 联调 + Playwright 截图 + 验收 | ~0.5 天 |
| **合计** | **约 4 天** |

## 8. 风险与对策

| 风险 | 对策 |
| --- | --- |
| 报告窗未就绪时事件丢失 | 报告窗启动后主动 `get_report_context` 拉取兜底 |
| 双窗事件竞态 | 上下文是「快照」而非流；切换序列时整包替换，无增量合并 |
| 孤儿窗口 | 主窗销毁时联动关闭报告窗 |
| 系统标题栏 vs 自绘标题栏 | 用原生 `decorations`，减少自绘复杂度；状态徽标放正文首行 |

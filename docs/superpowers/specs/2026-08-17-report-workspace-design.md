# B2-2 报告工作台（全屏三栏布局 + 富文本编辑）· 设计文档

日期：2026-08-17
状态：待用户评审
需求依据：`doc/报告编辑界面布局需求分析文档.md` v1.0
范围：`crates/pacs-db` + `crates/pacs-web` + `crates/pacs-auth` + `apps/viewer`

## 1. 背景与目标

B2-1 的基础报告逻辑（模板引擎 + 生命周期 + 领取/释放）已验收，但书写界面是弹窗
表单，不符合放射科报告工作台形态。本设计按需求文档把报告书写升级为 **viewer 内
独立的全屏报告工作台模式**：三栏 + 顶底布局、富文本所见/意见编辑、模板片段插入、
签名/审核信息展示，并保留 B2-1 全部业务逻辑（领取门禁、乐观锁、版本历史）。

## 2. 已确认决策（用户拍板）

| 决策点 | 选择 |
| --- | --- |
| 工作流 | 本轮仍单人 draft→signed；两人审核状态机（已提交/已审核/已发布/退回）后扩 |
| 编辑形态 | 富文本为主；结构化模板改为右侧模板树「插入骨架片段」 |
| 范围裁剪 | 危急值上报、收藏、同屏互动、申请单、报告对比 **均不做**（无占位按钮） |
| 布局 | 独立工作台模式（与 2D/MPR/VR 并列），顶部「影像」按钮切回阅片 |

## 3. 状态映射（单人模型 → 文档状态语义）

| 后端状态 | 工作台显示 | 颜色 | 编辑性 |
| --- | --- | --- | --- |
| draft | 未锁定 · 编辑中 | 蓝 | 可编辑 |
| signed | 已锁定 · 已签发 | 绿 | 只读（需先修订） |
| amending | 修订中 | 黄 | 可编辑 |

文档中的「已提交/已审核/已发布/退回」留待两人审核状态机后扩（后端 status 枚举
本轮不变）。

## 4. 后端变更

### 4.1 迁移 `0024_report_workspace.sql`

```sql
ALTER TABLE diagnostic_reports
    ADD COLUMN is_positive BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE diagnostic_report_versions
    ADD COLUMN is_positive BOOLEAN NOT NULL DEFAULT false;
```

### 4.2 API 扩展

- `POST /reports`、`PUT /reports/{id}/draft`：请求体加 `is_positive: Option<bool>`
  （缺省 false）；`DiagnosticReport` 序列化带 `is_positive`。
- `sign_report`：版本快照写入 `is_positive`。
- 登录响应（pacs-auth）加 `institution_name`（从 institutions.name 查），
  `RemoteUser` 同步——供工作台头部显示医院名称。

### 4.3 患者元数据补全

`open_series` 返回的 `PatientStudyInfo` 增加 `patient_sex: string | null`、
`patient_birth_date: string | null`（Rust metadata 提取 + TS 类型），供工作台
患者信息区显示性别/年龄（年龄=出生日期派生）。

### 4.4 报告内容模型（富文本）

- 富文本报告：`template_payload = NULL`，`findings/impression/recommendation`
  直接存 sanitized HTML（白名单：p/br/b/i/u/ul/ol/li；无 script/style/事件属性）。
- 旧结构化报告（payload 非空）：工作台**只读渲染**（按 payload.structure 渲染
  纯文本）；发起修订时把三列文本转义为 HTML 载入编辑器，此后按富文本模型继续，
  保存时 payload 置 null（一次性迁移，见风险表）。
- 签发校验改为「HTML 提取纯文本后非空」（`htmlToText`），替换原 `trim()` 判断。

## 5. 前端设计

### 5.1 工作台模式

- `viewerMode` 增加 `'report'`；`#report-workspace` 区与 viewport 互斥显示。
- 进入：工具栏「报告」按钮（原报告弹窗入口废弃，`report-panel.ts` 停用）；
  退出：工作台「返回」或「影像」按钮 → 回到 2D 阅片（panes/MPR/VR 状态不销毁）。
- 报告弹窗删除；领取/释放、领取门禁、版本历史逻辑迁入工作台组件。

### 5.2 布局（按需求文档 §2 三栏 + 顶底）

```
┌ 顶栏：返回 | 患者姓名 [◀][▶](v1禁用) [未锁定·编辑中] | [影像][报告] ┐
├ 报告头部：姓名+状态标签+医院名称        [修改记录]          ┬ 右侧模板树 │
├ 患者信息三行网格（只读+锁图标）：                              │ 显示全部 □ │
│  患者号|姓名|性别|年龄  检查部位|部位数|检查类型|检查时间        │ 模态分组   │
│  申请单内容(只读)  检查方法描述(=序列描述,只读)                 │ 点击插入   │
├ 影像所见（富文本 + 工具栏：B I U 列表）                        │ 骨架片段   │
├ 意见 □阳性  质控：无 （富文本）                                │            │
├ 签名区：报告医生|报告时间|审核医生|审核时间（四列）              │            │
└ 底栏：[保存草稿 Ctrl+S] [签发 Ctrl+Enter]        [修订]      ┴────────────┘
```

- 患者信息字段来源：患者号/姓名/性别/年龄（PatientStudyInfo 扩展后）、检查部位
  （series body_part）、检查类型（modality）、检查时间（study_date）、申请单内容
  （study_description + 转诊医生）、检查方法描述（series_description，只读）。
  **就诊类型标签（住院/门诊/急诊）无数据源，本轮隐藏**（需求文档记录在案）。
- 签名区：报告医生=作者（当前用户 display_name）；报告时间=updated_at；审核医生=
  最新版本 signed_by（未签发显示「—」）；审核时间=最新版本 signed_at。
- 右侧模板树：按 modality → body_part 两级分组（来源 listReportTemplates）；
  「显示全部模板」复选框隐藏（无个人模板概念，后扩）。点击模板把「章节标题 +
  字段标签」骨架文本插入当前焦点编辑器。
- 底栏：保存草稿（Ctrl+S）、签发（Ctrl+Enter，确认弹窗）、修订（仅已签发显示，
  输入原因 → 修订）。危急值/更多/取消审核不做。
- 锁定语义：signed → 编辑器 contenteditable=false + 按钮禁用，仅「修订」可用。
- 无报告时：显示领取门禁提示 + 「新建报告」（沿用 B2-1 领取→创建链路，创建后
  直接进入富文本编辑）。

### 5.3 富文本模块（纯逻辑，可单测）

- `rich-text.ts`：`sanitizeHtml(html) → 白名单 HTML`（去 script/style/事件属性/
  危险标签，递归清洗）、`htmlToText(html) → 纯文本`（签发非空校验用）、
  `plainToHtml(text) → 转义 HTML`（旧报告迁移用）。
- 编辑器：`contenteditable` + 工具栏按钮（execCommand 风格受限命令：bold/italic/
  underline/insertUnorderedList/insertOrderedList）。
- 显示侧双保险：保存时 sanitize + 只读渲染时再次 sanitize。

## 6. 测试与验收

- 前端单测（`rich-text.test.ts` + 现有 75 项全绿）：
  1. sanitize：script/onerror/javascript: 注入被剥离；白名单标签保留
  2. htmlToText：列表/换行提取正确；空 HTML（仅 <br>）视为空
  3. plainToHtml：<>& 转义正确
- 后端集成测试：is_positive 创建/草稿往返；sign 后版本表 is_positive 一致；
  登录响应含 institution_name。
- 手工验收清单：
  1. 打开检查 → 领取 → 「报告」进入工作台 → 三栏布局完整、患者信息正确
  2. 所见/意见富文本：加粗/列表输入 → Ctrl+S → 重新进入后格式保留
  3. 阳性勾选 → 保存 → 重开保持；签发后签名区显示审核医生+时间
  4. 签发后全只读；「修订」→ 编辑 → 再签发 → 修改记录（版本历史）可见两版
  5. 「影像」切回阅片 → 窗格/翻层状态未丢 → 再进工作台状态未丢
  6. 旧结构化报告只读渲染正常；修订后转为富文本继续
  7. 注入 <script> 保存 → 重开不执行（sanitize 生效）

## 7. 工作量

| 部分 | 估算 |
| --- | --- |
| 后端（0024 + is_positive API + 登录医院名 + 患者字段 + 测试） | ~1 天 |
| 工作台布局 + 模式切换 + 状态保持 | ~2.5 天 |
| 富文本模块 + 单测 | ~1 天 |
| 模板树片段插入 | ~0.5 天 |
| 联调与验收 | ~1 天 |
| **合计** | **约 6 天** |

## 8. 风险与对策

| 风险 | 对策 |
| --- | --- |
| contenteditable XSS | 保存/显示双重 sanitize + 白名单，单测覆盖注入用例 |
| 旧结构化报告迁移 | 修订时纯文本→HTML 一次性迁移，payload 置 null；只读期原样渲染 |
| 模式切换状态丢失 | report 模式不触碰 panes/MPR 状态，进出仅切换显示 |
| 富文本与派生缓存不变量（I2） | 富文本报告 payload=null，三列 HTML 即主数据；I2 仅约束结构化报告，不冲突 |
| 无数据源字段（就诊类型/申请单/部位数量） | 隐藏或只读展示现有字段，需求文档已注明后扩 |

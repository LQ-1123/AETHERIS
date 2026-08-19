# 工程计划书：报告审核闭环（含账号管理）

日期：2026-08-18（定稿：决策点 1-5 全部拍板）
状态：待审阅
前置：检查队列已完成（codex 提交 `098251c`，四改进点已核实落地）
依据：2026-08-18 头脑风暴客观评估——下一个方向为报告审核闭环（设计已拍板、数据层已就绪、纯内部功能零外部依赖，是「报告闭环」叙事的缺口）

---

## 1. 目标

把报告流程从「起草 → 签发」推进到「起草 → 提交送审 → 审核 → 签发」，合上报告闭环的质控缺口；同时补上审核工作流必需的前置能力——账号管理界面（审核需要多医生账号，目前无创建/改角色界面）。

**核心流程模式（用户拍板）**：审核人直接修正模式——审核人判断报告是否需要修改：
- **不需要修改** → 直接签发；
- **需要修改** → 审核人直接在报告上修改，修改后直接签发（信任审核人，不再退回作者改稿），并**记原作者错误 +1**（质量指标）。

## 2. 现状盘点（已核实）

| 层 | 状态 | 说明 |
| --- | --- | --- |
| 数据层（审核） | ✅ 完整 | 迁移 `0026_report_review_workflow.sql`：5 态 CHECK（draft/submitted/under_review/signed/amending）、`reviewer_fk`/`reviewed_at`/`review_comment`、`report_review_events` 审计表、机构开关 `institutions.review_required`（默认 false）、`user_permission_grants`（review_report 权限位） |
| API 层（审核） | 🟡 部分 | `pacs-web/src/clinical.rs` 已有 `create_report`（836 行）/`sign_report`（956 行）；**缺** submit/review/approve handler；`review_report` 权限位未接入鉴权；`review_required` 未接线 |
| 前端（审核） | ❌ | 报告工作台只有起草/签发；无提交送审、无审核界面、无 5 态展示 |
| 后端（账号） | ✅ 完整 | `pacs-web/src/clinical.rs:21-27`：GET/POST `/users`、PATCH `/users/{id}`、POST `/users/{id}/reset-password` |
| 前端（账号） | 🟡 | `admin-console.ts` 只有「用户设备授权」用到用户列表；无创建/改角色/重置密码/启禁用界面 |

## 3. 状态机（修订后：无退回循环）

```
draft ──submit──▶ submitted ──review_start──▶ under_review ──approve（不改）──▶ signed
                                                   │
                                                   └──approve（修改后）──▶ signed
                                                        （+ reviewer_modified 事件 · 作者错误 +1）
```

- **不再有 rejected → draft 退回流**（用户决策：信任审核人，审核人直接修正）。
- 迁移 0026 的 `report_review_events` CHECK 约束只允许 `submitted / review_started / approved / rejected` 四种 action，需一次小迁移（0029）扩展为含 `reviewer_modified`（审核人修改），`rejected` 常量保留但新流程不使用。
- `review_required` 开关（默认 false）：开启时 signed 只能由审核 approve 达成；关闭时保留现有 draft → signed 直签（单医生/演示环境不卡死）。

## 4. 交付内容

### A. 审核 API handler（后端，先行）

1. `submit_report`：draft → submitted，写 `report_review_events`（submitted）。权限：报告作者本人（`author_fk`）。
2. `review_start`：submitted → under_review，写 `reviewer_fk` + review_started 事件。权限：`review_report` 权限位 + **审核人 ≠ 作者（硬校验）**。
3. `approve_report { modified: bool, content?: {findings, impression, suggestion}, review_comment? }`：under_review → signed。
   - `modified=false`：直接签发，写 approved 事件。
   - `modified=true`：审核人修改内容 → **产生新版本快照（修改人=审核人）** → 签发 → 写 `reviewer_modified` + approved 事件 → **作者错误计数 +1**。
   - 版本快照与签发同事务写入，保持「签发即不可变」（0026 设计意图：`diagnostic_report_versions.reviewed_by/reviewed_at` 已存在，直接复用）。
4. 权限位接入：`Permission` 枚举增加 `review_report`；生效规则 = `role.can(p) OR EXISTS(user_permission_grants)`（0026 注释明确的规则）。
5. `review_required` 开关接线（见 §3）。

**作者错误计数（质量指标）**：不新增冗余列，以 `report_review_events` 为数据源——`COUNT(*) WHERE action='reviewer_modified' GROUP BY report 作者` 即该作者被审核人修正的次数。后续「管理员看工作量」报表直接聚合此数据，无需迁移。

**署名规则（已拍板：方案 A）**：审核人修改后签发，报告作者署名（`author_fk`）保留原作者；审核人作为修改者在版本快照与事件中留痕，报告展示「审核人已修改」标记（谁写的、谁改的都有据可查，符合质控留痕）。

### B. 审核前端工作流

1. **队列页报告状态展示**：现有四态（pending/writing/locked/signed）扩展为 `submitted`（待审核）/`under_review`（审核中）徽标与文案。
   - 注意：`pacs-db` 队列报告状态 CASE 目前把 submitted/under_review 按 author_fk 归入 writing/locked（next-plan 4.2.1 注记），审核上线后需改为显式两态。
2. **报告工作台内嵌审核模式**（决策点 3：内嵌，不另建独立视图）：
   - 作者侧：draft 可「提交送审」；submitted/under_review 只读（显示审核中）；signed 可查看审核时间线。
   - 审核人侧（submitted/under_review 且持有 review_report 且非作者）：进入审核模式——报告内容只读预览 + 「无需修改，直接签发」与「修改后签发」两个操作；选择修改时内容可编辑，编辑后签发；可附审核意见。
   - 审核时间线：`report_review_events` 展示（谁提交/谁审核/是否修改/何时/意见），作者与审核人均可见（质控留痕）。

### C. 账号管理前端

- 管理控制台（`admin-console.ts`）新增「账号管理」页：用户列表（角色/状态）、创建用户、修改角色、重置密码、启用/禁用。复用现有 `/users` API，纯前端。
- 权限：仅 admin（决策点 4：admin 可创建全部角色；technician 不参与审核，无需授予 review_report）。

## 5. 与检查队列的衔接

- 队列页报告状态徽标扩展为 submitted/under_review，「待审核/审核中」检查一眼可见——检查队列「扫一眼待办」的价值延伸。
- 审核入口建议挂在队列页行内（submitted 行 → 「去审核」）或报告工作台，不新增独立导航。

## 6. 决策点（已拍板 + 待确认）

| # | 决策点 | 结论 |
| --- | --- | --- |
| 1 | 审核人直接修正模式 | ✅ 拍板：需要修改时审核人直接改并签发，作者错误 +1；无退回循环 |
| 2 | review_required 默认值 | ✅ 同意：维持 false，上线时管理员开启 |
| 3 | 审核界面形态 | ✅ 同意：报告工作台内嵌审核模式 |
| 4 | 账号角色范围 | ✅ 按推荐：admin 创建全部角色，technician 不参与审核 |
| 5 | **署名规则（新）** | ✅ 拍板：方案 A——作者署名保留原作者 + 「审核人已修改」标记；审核人作为修改者在版本快照与事件中留痕 |

## 7. 工作量与顺序

| 阶段 | 内容 | 预估 |
| --- | --- | --- |
| 0 | 迁移 0029（events action 扩展 reviewer_modified） | 0.5h |
| 1 | 审核 API（submit/review_start/approve 含修改+错误计数 + 权限位 + 开关接线）+ Rust 测试 | 1.5-2 天 |
| 2 | 队列页 5/6 态徽标 + 报告工作台提交送审/审核模式 | 1.5 天 |
| 3 | 账号管理界面 | 0.5 天 |
| 合计 | | 约 4-4.5 天 |

顺序：迁移 → API → 前端（队列/工作台）→ 账号管理（可并行）。

## 8. 验收标准

1. 开启 review_required 后：draft 提交送审 → 审核人（非作者、持有 review_report）可审 → 不改直接签发；或修改后签发。
2. 审核人 = 作者被硬拒绝；无 review_report 权限者不可审核（含 admin 未授予时）。
3. 审核人修改后签发：报告内容为修改后版本，版本快照修改人=审核人，events 含 reviewer_modified + approved，**作者错误计数 +1**（聚合可查）；报告作者署名保留原作者并带「审核人已修改」标记。
4. 队列页 submitted/under_review 状态徽标正确展示。
5. `report_review_events` 时间线作者/审核人可见。
6. 关闭 review_required 时，现有 draft → signed 直签流程完全不变（回归）。
7. 账号管理：admin 可创建/改角色/重置密码/启禁用；radiologist/technician 不可进入。

## 9. 不在本次范围

- 申请单（全新领域模型，需独立设计周期——价值真实但依赖设备端配合）
- C-MOVE/C-GET（dormant：需真实对端联调，与「唯一入库=DICOM 协议」策略的衔接留待后续）
- AI 辅助报告（dormant：设计已拍板但依赖外部 LLM API 与合规细节）
- AI 分割定量化（已按用户决策排除，偏科研）
- 管理员工作量报表（本次只做错误计数数据落点，报表本身留待后续）

## 附

- 本文档仅描述方案与范围，不含代码改动。
- 实施前需先跑 `cargo test` 与前端 `tsc` 确认基线（检查队列提交后未验证）。
- 依据核实：`0026_report_review_workflow.sql`、`pacs-web/src/clinical.rs`（create_report:836/sign_report:956、users 路由:21-27）、`queue-page.ts`（STATUS_LABELS 四态）、`pacs-db/src/worklist.rs`（队列报告状态 CASE）。

# B2 报告撰写闭环 + 结构化模板引擎 · 设计文档

日期：2026-08-16
状态：待用户评审
范围：`crates/pacs-db` + `crates/pacs-web` + `apps/viewer`（后端小改 + 前端大改）

## 1. 背景与目标

后端报告生命周期 API 已完整（创建/草稿/签发/修订/不可变版本历史，revision 乐观锁
409，审计，机构隔离，radiologist 限写），但 viewer 前端零界面——「看完图没有下文」
是产品侧最大断点。本设计把报告闭环接到阅片器：结构化模板驱动的报告书写 + 单人签发
+ 修订 + 版本历史，并补上待诊工作项的领取/释放。

## 2. 已确认的决策（用户拍板）

| 决策点 | 选择 |
| --- | --- |
| 范围 | 报告面板 + 工作项领取/释放 |
| 报告正文 | 结构化模板引擎（章节 + 文本域 + 单选组 + 数值字段） |
| 模板归属 | migration 内置种子模板；模板管理 UI 后续单独做 |
| 测量集成 | 本轮手动复制测量值，一键插入后续 |
| 签发流程 | 沿用现有 draft→sign 单人流程，两人审核流后续 |

## 3. 非目标（本轮不做）

- 模板管理 UI（CRUD 编辑器）
- 两人审核/复核状态机（需改 status 枚举与权限模型）
- DICOM SR 导出（dicom-rs 对 SR 的成熟度风险，另立专项）
- 测量值一键插入报告
- 移动端/云胶片

## 4. 后端设计

### 4.1 Schema（新 migration `0021_report_templates.sql`）

```sql
CREATE TABLE report_templates (
    id             UUID PRIMARY KEY,
    institution_id BIGINT NOT NULL REFERENCES institutions(id),
    name           TEXT NOT NULL,
    modality       TEXT NOT NULL,           -- 'CT' | 'MR' | 'DR' ...（种子模板按模态分类）
    body_part      TEXT,                    -- 'head' | 'chest' | 'abdomen' ...
    version        INTEGER NOT NULL DEFAULT 1,
    structure      JSONB NOT NULL,          -- 模板结构（章节/字段定义，见 4.2）
    builtin        BOOLEAN NOT NULL DEFAULT false, -- 内置种子模板不可删除
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT report_templates_modality_len CHECK (length(btrim(modality)) BETWEEN 1 AND 16)
);
CREATE INDEX report_templates_institution_idx
    ON report_templates(institution_id, modality);
CREATE TRIGGER report_templates_set_updated_at BEFORE UPDATE ON report_templates
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

ALTER TABLE diagnostic_reports
    ADD COLUMN template_payload JSONB;  -- 填写时的结构+值快照（自包含，见 4.3）
```

说明：

- `template_payload` 是**自包含快照**（模板结构 + 填写值），旧报告不受模板后续
  修改影响——模板演化与历史报告解耦。
- `diagnostic_report_versions` 不加 payload 列：已签发版本的 `findings` 渲染文本
  已不可变，够用；如需逐字段复现历史可后续补。
- 不新增 `template_fk` 外键列——快照已含 template_id；避免模板删除/禁用牵动历史
  报告，也少一条 JOIN。模板变更通过 version 递增 + 快照携带 version 追踪。

### 4.2 模板结构 JSON 约定（schema_version 1）

```json
{
  "schema_version": 1,
  "sections": [
    {
      "id": "findings",
      "title": "影像所见",
      "target": "findings",
      "fields": [
        { "id": "f1", "kind": "text", "label": "整体描述", "required": false },
        { "id": "f2", "kind": "choice", "label": "肺实质",
          "options": [
            { "id": "normal", "label": "未见明显异常" },
            { "id": "abnormal", "label": "异常（展开描述）", "expands": true }
          ] },
        { "id": "f3", "kind": "number", "label": "最大结节径", "unit": "mm",
          "min": 0, "max": 300 }
      ]
    },
    {
      "id": "impression",
      "title": "诊断意见",
      "target": "impression",
      "fields": [ { "id": "i1", "kind": "text", "label": "诊断意见", "required": true } ]
    },
    {
      "id": "recommendation",
      "title": "建议",
      "target": "recommendation",
      "fields": [ { "id": "r1", "kind": "text", "label": "建议" } ]
    }
  ]
}
```

字段类型 `text | choice | number`；`choice.expands=true` 的选项被选中时展开一个
描述文本域。`target` 决定该章节渲染进后端哪一列
（findings / impression / recommendation）。

### 4.3 快照与渲染

```json
{
  "template_id": "<uuid>",
  "template_version": 1,
  "structure": { "...": "4.2 结构体原样" },
  "values": {
    "findings.f1": "双肺纹理清晰",
    "findings.f2": { "choice": "abnormal", "description": "右肺上叶小结节" },
    "findings.f3": { "value": 6.5 },
    "impression.i1": "右肺上叶小结节，建议随访",
    "recommendation.r1": "3 个月后复查"
  }
}
```

渲染规则（纯函数，前端实现并单测）：章节按 target 聚合为
「标题\n字段标签：值…」三列文本；`choice` 渲染选项标签（展开项附描述）；
`number` 渲染「值 + 单位」；空字段跳过。**payload 是唯一真源**——每次保存前由
前端从 payload 重新渲染三列文本再提交，杜绝文本与结构化值漂移。

### 4.4 API 变更（向后兼容的扩展）

| 变更 | 说明 |
| --- | --- |
| 新增 `GET /api/v1/report-templates` | 查询参数 `modality` 可选；返回本机构模板（含 structure）。需 `ViewImages` |
| 扩展 `POST /reports` | 请求体加可选 `template_payload: Option<Value>`，校验 JSON、大小 ≤ 1 MiB |
| 扩展 `PUT /reports/{id}/draft` | 请求体加可选 `template_payload: Option<Value>`，随草稿保存 |

工作项领取/释放/指派（`GET /worklist`、`claim`、`release`、`assign`）已存在，
零后端改动。签发/修订/版本历史端点零改动。

### 4.5 种子模板（migration 内置，`builtin=true`）

- CT-头颅（head）：脑实质/脑室系统/颅骨与软组织章节
- CT-胸部（chest）：肺实质/纵隔/胸壁章节
- CT-腹部（abdomen）：实质脏器/空腔脏器/腹膜后章节
- MR-头颅（head）：脑实质/脑室/DWI 章节
- DR-胸部（chest）：肺野/心影/骨性胸廓章节

种子模板内容为**示例结构**，标注「示例模板，请按科室实际用语调整」；
README 保持 research-only 免责。

## 5. 前端设计

### 5.1 api.ts 封装（走现有 `router_write`/`routerGet` 通用代理，无新 Tauri 命令）

`listReportTemplates` / `listReports` / `createReport` / `updateReportDraft` /
`signReport` / `beginReportAmendment` / `listReportVersions` / `listWorklist` /
`claimWorkItem` / `releaseWorkItem`。类型定义进 `types.ts`。

### 5.2 纯逻辑模块（可单测）

- `report-render.ts`：`renderReportText(payload) → { findings, impression,
  recommendation }`；`payloadFromTemplate(template) → 空 payload`；
  `validatePayload(payload) → { ok, errors }`（结构版本、字段 id 存在性、必填）。
- 模板表单渲染仍走 DOM（面板类），但「值 → 表单模型」「表单模型 → 渲染文本」
  的映射抽成纯函数。

### 5.3 报告面板（`report-panel.ts`，参照 `router-panel.ts` 模式）

- 入口：工具栏「报告」按钮（lucide `FileText`，需在 `main.ts` 注册表登记——
  项目已知坑）；右侧抽屉面板。
- 状态机：
  - `无报告`：显示「新建报告」+ 模板选择（按当前检查 modality 过滤）→ 创建
    （POST /reports 带空 payload）→ 进入编辑态
  - `草稿`：模板表单编辑；「保存草稿」PUT draft（带 revision）；「签发」确认后
    POST sign
  - `已签发`：只读渲染 + 版本历史列表（版本号/签发人/时间/修订原因，点开看
    历史渲染文本）；「修订」输入 reason → POST amendments → 回到编辑态
  - `amending`：同草稿编辑态，顶部提示修订原因
- 乐观锁：PUT/sign 返回 409 → 面板顶部提示「报告已被他人修改」+ 一键重新拉取
  （丢弃本地未保存状态，不盲目重试——遵守 api-reference 的约定）。
- 403（非 radiologist）→ 禁用写按钮 + 服务端错误文案展示；当前用户角色若
  viewer 端可获取则提前禁用（实现时确认，否则仅靠服务端拒绝 + 文案）。

### 5.4 工作项领取/释放

- 病人列表/当前检查详情区：当前检查有 `diagnostic_work_item` 时显示
  「领取」（POST claim，带 revision）；已由我领取显示「释放」。
- 409 冲突 → 提示已被他人领取并刷新列表。
- 报告面板打开时若未领取但已有报告 → 只读提示（后端已有权限语义，前端
  仅做提示不硬挡）。

### 5.5 错误处理

| 场景 | 行为 |
| --- | --- |
| revision 冲突（409） | 面板横幅提示 + 重新拉取按钮 |
| 403 radiologist_required | 按钮禁用/错误文案「需要医师角色」 |
| payload 过大/非法 JSON | 客户端预校验拦截，服务端 400 兜底 |
| 模板被删（快照仍在） | 老报告照常渲染（自包含快照），新建时列表无该模板 |

## 6. 测试与验收

- 前端单测（`report-render.test.ts` + 现有 66 项保持全绿）：
  1. 三列文本渲染：章节标题/字段标签/值格式（choice 展开项、number 单位、空字段跳过）
  2. payload 校验：缺 structure、字段 id 不存在、必填缺失
  3. 空 payload 生成与模板结构一致性
- 后端测试：migration 应用；种子模板结构可 JSON 解析；create/draft 带
  template_payload 往返一致；模板列表按 modality 过滤。
- 手工验收清单（GUI）：
  1. 打开检查 → 报告面板新建 → 选 CT-胸部模板 → 填单选/数值/描述 → 保存草稿
  2. 关闭重开面板 → 草稿恢复（从 payload 重新渲染表单）
  3. 签发 → 只读视图 + 版本历史 v1
  4. 修订（reason）→ 编辑 → 再签发 → 版本历史 v2 且 v1 文本不变
  5. 工作列表领取 → 他人/重开视角看到已领取状态；释放后回到待诊
  6. 两个窗口同时改同一草稿 → 后保存者收到 409 提示

## 7. 工作量

| 部分 | 估算 |
| --- | --- |
| 后端（migration + 种子 + API 扩展 + 测试） | ~1.5 天 |
| 前端纯逻辑 + 单测 | ~1 天 |
| 报告面板 UI + 状态机 | ~2 天 |
| 工作项领取/释放 | ~0.5 天 |
| 联调与手工验收 | ~1 天 |
| **合计** | **约 1.5 周** |

## 8. 风险与对策

| 风险 | 对策 |
| --- | --- |
| 模板结构 v1 不满足真实科室 | 快照自包含 + schema_version 演进；结构变更只影响新报告 |
| 文本与结构化值漂移 | payload 唯一真源，保存前统一重渲染 |
| 种子模板临床不专业 | 标注示例模板 + research-only 免责，不声称临床可用 |
| 表单渲染复杂度失控 | 三种字段类型封顶，不引入条件/嵌套引擎 |

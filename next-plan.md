# 高级工作列表：独立患者队列页面 · 设计文档

日期：2026-08-17
状态：已定稿
范围：`crates/pacs-db` + `crates/pacs-web` + `apps/viewer`（服务端查询 + 前端独立页面）
对应路线图：README「Advanced worklist management [ ]」

## 1. 背景与目标

现有工作列表是**嵌入主界面的左侧栏**（`#worklist-panel`，280-520px 可拖宽），分层
展示（患者 → 检查 → 序列），仅支持姓名/PatientID 检索与固定排序。放射科医生需要
一张**独立的全屏队列页**：一眼扫过「今天有哪些检查待写」，按时间/模态/部位/报告
状态/来源医院排序与检索，双击直接进阅片。

本设计把「高级工作列表」落为 viewer 内一个**独立页面**（非侧栏），并扩展服务端
查询支持服务端过滤/排序/分页——数据量上来后不能在内存里排。

## 2. 现状（已核实）

| 事实 | 证据 |
| --- | --- |
| 侧栏工作列表：患者 → 检查 → 序列三层，单击展开，序列行单击打开 | `index.html:338` `#worklist-panel`；`app.ts:5005` `renderPatients()` |
| 患者级聚合已有四态报告计数 | `pacs-db/src/worklist.rs` `PatientSummary`：`pending/writing/locked/signed_studies` |
| 患者级**无**模态/部位/来源医院字段 | `PatientSummary` 结构（worklist.rs:51-68） |
| study 级有 `modalities TEXT[]`（GIN 索引）、series 级有 `modality`/`body_part_examined` 列 | `crates/pacs-db/migrations/0001_imaging.sql:72,94,96` |
| **来源医院未进 API**：`InstitutionName (0008,0080)` 只存在于 `studies.attributes` JSONB | `attributes.rs:39`（`attributes::STUDY` 含 `INSTITUTION_NAME`），但 `StudySummary`/`PatientSummary` 均无该字段 |
| 现有检索仅 `query`（姓名/ID）+ `limit/offset` 分页，**无过滤、无排序参数** | `pacs-web/src/worklist.rs:31-37` `PatientParams` |
| SQL 固定排序 `ORDER BY MAX(st.study_date) DESC` | `pacs-db/src/worklist.rs:133` |
| QIDO-RS 明确不支持 `orderby`（忽略并告警），匹配键支持 Modality/BodyPartExamined/StudyDate 范围 | `qido.rs:38,220-222`；队列页走 worklist API，不扩 QIDO |
| 非 admin 的可见性过滤（授权设备 + trusted 来源）已有现成 SQL 模式 | `worklist.rs:127-129` |
| 打开流程：`openRemote(studyUid, seriesUid)` → `activateSeries` → `openRemoteSeries` | `app.ts:4544` |
| 独立页面先例：`?mode=report` 独立窗口（`main.ts:141` 路由分支 + `report-window.ts`） | `2026-08-17-report-window-design.md` |
| app.ts 已 7840 行 | 队列页逻辑必须独立模块，只做薄接线 |

## 3. 已确认决策（用户拍板）

| 决策点 | 选择（已定） | 备选 |
| --- | --- | --- |
| 页面形态 | **主窗内全屏队列视图**（`#queue-page` 覆盖 `#workspace`，顶部返回按钮）；双击跳转直接复用 `openRemote`，无跨窗状态同步 | `?mode=queue` 独立 Tauri 窗口（双击跳转需 emit/listen 跨窗通信，复杂度高，不推荐） |
| 队列粒度 | **检查级（Study）**：一行=一个检查（含患者信息），模态/部位/报告状态都是检查属性，筛选维度天然对齐 | 患者级（多检查多模态聚合语义模糊，筛选会失真） |
| 排序 | **服务端排序**：表头点击切换 `sort`/`order` 参数，SQL 白名单排序 | 前端排（需全量拉取，违背分页） |
| 与侧栏关系 | **替换现有侧栏工作列表**：队列页成为唯一入口，`#worklist-panel` 侧栏相关代码（`#worklist-toggle`、`renderPatients`、侧栏分页/刷新/导入菜单）移除或标记废弃 | 并存（侧栏保留快速浏览，但工作列表双入口分裂，两份 UI 需同步维护） |
| 来源医院 | 从 `studies.attributes` JSONB 提取（DICOM JSON Model 键 `00080080`），用于展示/过滤/排序；不迁移加列 | 加列 + 迁移回填（改动大，且已有数据无此属性时仍为空） |

## 4. 架构

### 4.1 数据流

```
队列页表格（queue-page.ts）
  └─ api.listQueueStudies(filters, sort, limit, offset)      [Tauri 命令]
       └─ remote.list_queue_studies(...)                     [HTTP GET]
            └─ GET /api/queue/studies?query=&modality=&body_part=
               &report_status=&institution=&date_from=&date_to=
               &sort=&order=&limit=&offset=                   [pacs-web worklist.rs]
                 └─ pacs_db::list_queue_studies(...)          [SQL, 服务端过滤/排序/分页]
```

### 4.2 服务端：`pacs-db` 新增 `list_queue_studies`

签名（与 `list_patients` 同风格，机构边界必带）：

```rust
pub struct QueueFilter<'a> {
    pub query: &'a str,            // 患者姓名/ID 包含匹配（复用 contains_pattern）
    pub modality: Option<&'a str>, // studies.modalities @> ARRAY[modality]（走 GIN）
    pub body_part: Option<&'a str>,// EXISTS(series.body_part_examined = $x)
    pub report_status: Option<&'a str>, // pending | writing | locked | signed
    pub institution: Option<&'a str>,   // attributes->'00080080'->'Value'->>0 精确匹配
    pub date_from: Option<NaiveDate>,
    pub date_to: Option<NaiveDate>,
}

pub enum QueueSort { StudyDate, PatientName, Modality, ReportStatus, Institution }
// 排序列白名单化：只接受枚举，杜绝 SQL 注入；order 仅 asc|desc
```

返回行：

```rust
pub struct QueueStudyRow {
    pub study_uid: String,
    pub patient_id: String,
    pub patient_name: Option<String>,
    pub patient_sex: Option<String>,
    pub patient_birth_date: Option<NaiveDate>,
    pub study_date: Option<NaiveDate>,
    pub study_time: Option<NaiveTime>,
    pub modalities: Vec<String>,
    pub description: Option<String>,
    pub body_parts: Vec<String>,        // array_agg(DISTINCT se.body_part_examined)
    pub report_status: String,          // 四态，见 4.2.1
    pub institution_name: Option<String>, // attributes 提取
    pub series_count: i32,
}
```

SQL 骨架（复用 `list_patients` 的可见性过滤：admin 全见，否则授权设备 + trusted）：

```sql
SELECT st.study_instance_uid,
       p.patient_id, p.name, p.sex, p.birth_date,
       st.study_date, st.study_time, st.modalities, st.description,
       COALESCE(array_agg(DISTINCT se.body_part_examined) FILTER (WHERE se.body_part_examined IS NOT NULL), '{}'),
       CASE WHEN r.id IS NULL THEN 'pending'
            WHEN r.status = 'signed' THEN 'signed'
            WHEN r.author_fk = $user THEN 'writing'
            ELSE 'locked' END,
       st.attributes->'00080080'->'Value'->>0,
       st.number_of_series
FROM studies st
JOIN patients p ON st.patient_fk = p.id AND p.institution_id = $inst
JOIN series se ON se.study_fk = st.id
LEFT JOIN diagnostic_reports r ON r.study_fk = st.id
WHERE st.institution_id = $inst
  AND st.storage_tier <> 'quarantine'
  AND ($is_admin OR (se.source_status='trusted' AND EXISTS(
        SELECT 1 FROM dicom_devices d WHERE d.id=se.source_device_fk
          AND d.status='active' AND EXISTS(
            SELECT 1 FROM user_device_grants g WHERE g.user_fk=$user AND g.device_fk=d.id))))
  -- 动态过滤条件（每个 Option 拼一条 AND，参数化绑定）
GROUP BY st.id, p.id, r.id, st.attributes
ORDER BY <白名单列> <asc|desc> NULLS LAST, st.study_instance_uid
LIMIT $n OFFSET $m
```

#### 4.2.1 报告状态四态语义（与侧栏 `patientReportStatus` 一致）

| 值 | 条件 | 侧栏徽标 |
| --- | --- | --- |
| `pending` | 无报告 | 待书写 |
| `writing` | 报告 draft/amending 且 `author_fk = 当前用户` | 书写中 |
| `locked` | 报告 draft/amending 且 `author_fk ≠ 当前用户` | 已锁定 |
| `signed` | 报告已签发 | 已签发 |

> 现有 `list_patient_studies` 的 `report_status` 只有三态（pending/writing/signed），
> 队列页需要四态（locked 是「他人正在写」的排队信号），故单独写 CASE。
>
> 注：迁移 0026 引入 `submitted`/`under_review` 两态（互审工作流），CASE 的 else
> 分支按 `author_fk` 归入 writing/locked（与 `report-window.ts:162-164` 同规则）；
> 而侧栏 `list_patients` 的 writing/locked 计数只认 `draft`/`amending`，这两态在侧栏
> 患者徽标上会落到「已签发」——此为既有差异，队列页以本 CASE 为准，不做对齐改动。

#### 4.2.2 来源医院提取

`studies.attributes` 是 DICOM JSON Model（PS3.18 附录 F），InstitutionName 形如
`{"00080080":{"vr":"LO","Value":["XX医院"]}}`。提取表达式：
`st.attributes->'00080080'->'Value'->>0`。部分老数据可能缺此键（返回 NULL），
过滤时 `institution` 精确匹配 NULL 自然不命中，行为可预期。

### 4.3 服务端：`pacs-web` 新增队列端点

在 `worklist.rs` 加一条路由（复用 `worklist_routes` 的 `ViewImages` 鉴权层）：

```
GET /api/queue/studies
```

`QueueParams`（serde Deserialize）：`query / modality / body_part / report_status /
institution / date_from / date_to / sort / order / limit / offset`。
- `limit` 沿用 `1..=100` 校验（`MAX_PAGE_SIZE`），`offset ≥ 0`
- `sort` 解析进 `QueueSort`（非法值 400），`order` 仅 `asc|desc`（缺省 `desc`）
- 复用 `MAX_QUERY_CHARS = 128` 限制 query

### 4.4 前端：`queue-page.ts`（新，独立模块）

`app.ts` 只做薄接线：工具栏「工作队列」按钮 → `new QueuePage().open()`。

```
QueuePage
  ├─ mount(): 显示 #queue-page 覆盖层，隐藏 #workspace；注册返回按钮
  ├─ 筛选行：姓名/ID 输入 + 日期起止(date input) + 模态下拉(CT/MR/CR/DX/US/…)
  │           + 部位输入(回车检索) + 报告状态下拉(四态+全部) + 来源医院输入
  ├─ 表格：表头可点击列 = 患者/检查日期/模态/报告状态/来源医院（5 列）
  │         点击切换 sort+order → 重新请求第 1 页
  ├─ 分页：底部「第 N 页 ◀ ▶」，每页 50 行（复用侧栏分页交互模式）
  ├─ 行渲染：QueueStudyRow → 患者名(格式化) / 日期+时间 / 模态徽标 / 部位
  │           / 报告状态徽标(复用四态映射) / 来源医院
  └─ 双击行：e.preventDefault() 防单击选中 →
       void this.openRow(row)
```

#### 4.4.1 双击打开流程（复用现有链路）

```
openRow(row):
  studies = await listStudySeries(row.study_uid)      // 现有 API
  target  = recommendMprSeries(studies) ?? studies[0] // 复用 app.ts:7792 的推荐逻辑
  await openRemote(row.study_uid, target.series_uid)  // 复用 activateSeries 全链路
  隐藏队列页 → 切回阅片视图（打开的序列自然呈现）
```

`recommendMprSeries` 是 app.ts 的模块级函数，直接 import 复用，不复制实现。
无序列（异常数据）时给出提示而非静默失败。

#### 4.4.2 防双击与行选中冲突

- 双击展开语义在侧栏是「单击展开」，队列页是「双击打开」——两者不混：
  队列页行无展开行为，单击仅选中高亮（无副作用），双击才打开。
- 若未来加行内操作按钮，`stopPropagation` 隔离，与侧栏做法一致。

### 4.5 index.html 骨架

`#app-shell` 内新增（与 `#workspace` 平级，默认 `hidden`）：

```html
<section id="queue-page" hidden aria-label="患者队列">
  <header class="queue-nav">
    <button id="queue-back" class="icon-button" title="返回阅片" aria-label="返回阅片">
      <i data-lucide="arrow-left"></i>
    </button>
    <strong>患者队列</strong>
    <span id="queue-count"></span>
    <div class="queue-nav-right">
      <button id="queue-refresh" class="icon-button" title="刷新" aria-label="刷新">
        <i data-lucide="refresh-cw"></i>
      </button>
    </div>
  </header>
  <form id="queue-filters" class="queue-filters"> …筛选控件… </form>
  <div class="queue-table-wrap">
    <table class="queue-table">
      <thead> 5 个可点击表头 </thead>
      <tbody id="queue-body"></tbody>
    </table>
    <div id="queue-status" class="worklist-status" aria-live="polite"></div>
  </div>
  <div class="queue-pagination"> ◀ <span id="queue-page-label">第 1 页</span> ▶ </div>
</section>
```

深色主题直接复用 `styles.css` 的 worklist/表格变量（`--bg`、`--border` 等），
新增 `.queue-table` 一套行样式，视觉与侧栏一致。

### 4.6 入口

- 工具栏新增 `#queue-btn`（图标 `clipboard-list`，title「工作队列」），替换
  `#worklist-toggle`：`#worklist-toggle`/`#worklist-panel` 及其逻辑（`renderPatients`
  等）移除或标记废弃，队列页成为工作列表唯一入口。
- 侧栏内的「导入」菜单（`index.html:345` `#import-menu`）是独立能力，迁移到工具栏
  `#open-btn` 旁或队列页，避免替换后丢失导入入口。
- 打开队列页时隐藏 `#workspace` 并显示 `#queue-page`；「返回阅片」反向切换，
  不清空队列状态（筛选/页码保留，类似 `?mode=report` 的上下文快照思想）。

## 5. 与现有代码的关系

| 文件 | 动作 |
| --- | --- |
| `crates/pacs-db/src/worklist.rs` | 新增 `QueueFilter`/`QueueSort`/`QueueStudyRow`/`list_queue_studies`（+ 单测） |
| `crates/pacs-web/src/worklist.rs` | 新增 `GET /queue/studies` 端点 + `QueueParams` 解析（+ 测试） |
| `apps/viewer/src-tauri/src/remote.rs` | 新增 `list_queue_studies` HTTP 转发（+ commands.rs 命令 + main.rs 注册） |
| `apps/viewer/src/api.ts` | 新增 `listQueueStudies(filters, sort, limit, offset)` |
| `apps/viewer/src/types.ts` | 新增 `QueueStudyRow` 接口 |
| `apps/viewer/src/queue-page.ts` | **新建**：队列页主逻辑（筛选/排序/分页/双击打开） |
| `apps/viewer/src/app.ts` | 薄接线：`#queue-btn` → `QueuePage`；import `recommendMprSeries` 改为导出复用 |
| `apps/viewer/index.html` | 新增 `#queue-page` 骨架 + `#queue-btn` 入口 |
| `apps/viewer/index.html` + `app.ts`（废弃侧栏） | 删除 `#worklist-panel`/`#worklist-toggle` 与 `renderPatients` 等侧栏逻辑；`#import-menu` 迁至工具栏或队列页 |
| `apps/viewer/src/styles.css` | 新增 `.queue-*` 样式（复用主题变量）；侧栏废弃样式清理 |

复用不重写：`formatPersonName` / `formatApiDate` / 四态徽标映射 /
`recommendMprSeries` / `openRemote` / `activateSeries` / `listStudySeries`。

## 6. 测试与验收

### 6.1 单测（Rust）

- `pacs-db`：`list_queue_studies` 过滤组合（模态/部位/报告状态/机构/日期范围）、
  四态 CASE 正确性（含 author_fk 区分 writing/locked）、排序白名单、分页 offset/limit
- `pacs-web`：`QueueParams` 解析（非法 sort/order 400、limit 边界、query 超长 400）

### 6.2 Playwright

- 扩展 `scripts/` 截图自检思路：`queue-page` mock 数据渲染一张完整队列页截图，
  vision 复核无塌陷、表头/筛选行/分页可见。

### 6.3 手工验收清单

1. 工具栏「工作队列」→ 打开全屏队列页；「返回阅片」→ 回主界面
2. 默认按检查日期倒序；点击各表头 → 升/降序切换并回到第 1 页
3. 按模态/部位/报告状态/来源医院/日期范围组合筛选 → 结果与服务端一致
4. 姓名/ID 搜索 → 包含匹配生效
5. 双击一行 → 队列页关闭、阅片器打开该检查推荐序列（MPR 优先）
6. 报告状态徽标四态颜色与侧栏一致；locked 行在他人书写时出现
7. 空结果 → 显示「没有匹配的检查」，不报错
8. 未登录/无权限（非授权设备数据）→ 不出现越权行
9. 分页翻页 + 刷新按钮 → 数据正确刷新

## 7. 工作量

| 部分 | 估算 |
| --- | --- |
| pacs-db 查询（过滤/排序/分页/四态/机构提取 + 单测） | ~1 天 |
| pacs-web 端点 + 参数校验（+ 测试） | ~0.5 天 |
| Tauri 转发（remote/commands/main 三处） | ~0.5 天 |
| queue-page.ts + index.html + styles（筛选/表格/分页/双击） | ~1.5 天 |
| 联调 + Playwright 截图 + 验收 | ~0.5 天 |
| **合计** | **约 4 天** |

## 8. 风险与对策

| 风险 | 对策 |
| --- | --- |
| 替换侧栏后，阅片台内失去「不离开阅片台即浏览工作列表」的快速浏览习惯 | 队列页「返回阅片」+ 双击直达，列表→阅片流程更顺；如确需台内快速切患者，后续可加轻量「迷你队列」浮层（本次不做） |
| 侧栏同时承载「导入」菜单与刷新/分页控件，替换会连带丢失这些能力 | 导入入口迁移到工具栏 `#open-btn` 旁；刷新用队列页 `#queue-refresh`，分页由队列页自带 |
| `attributes` JSONB 提取 InstitutionName 无索引，大数据量下过滤慢 | 队列页按日期范围/分页收敛扫描；确认成为瓶颈后再考虑加生成列或迁移加列 |
| 双击打开需先拉 series 列表，多一次请求 | 队列 API 暂不返回推荐序列（避免耦合 recommendMprSeries 逻辑到后端）；双击时异步拉取，loading 提示 |
| 四态 CASE 与现有三态 `list_patient_studies` 语义并存易混淆 | 队列页用独立 SQL 与独立字段，不修改既有端点；文档标注差异 |
| app.ts 体积 | 队列逻辑全部在 `queue-page.ts`，app.ts 仅 3-5 行接线 |
| 表格行数多时 DOM 压力 | 每页 50 行 + 服务端分页，行渲染为纯文本 DOM，无虚拟滚动需求（现阶段） |

# A1 管理员控制台（设备批准 / 来源归属 / 用户授权）· 设计文档

日期：2026-08-16
状态：待用户评审
范围：`crates/pacs-db` + `crates/pacs-web` + `apps/viewer`（后端小改 + 前端新面板）

## 1. 背景与目标

可见性模型是**设备授权制**（pacs-plan 信任边界）：非管理员只能看到「已归属到设备
→ 设备已批准 → 设备已授权给该用户」的影像；管理员绕过限制。当前问题：

- 管理操作（批准设备、来源归属、用户授权）只有 HTTP API，**viewer 无任何管理界面**。
- 开发库现状：0 设备、0 授权、47 个序列全部 `legacy_unattributed`、0 工作项——
  医生端因此什么都看不到、也领不到任务。
- 工作项 backfill 迁移曾被写过但从未提交（测试库留有 v22 痕迹），需补交。
- 设备只能靠 DIMSE 入站自动观察创建，**没有手动注册端点**——历史数据归属需要
  先有一个活跃设备，闭环断裂。

本设计补上管理端闭环：手动注册设备 + 批准/禁用 + 未归属序列批量归属 + 用户设备
授权 + 工作项 backfill，全部走正规 API（带审计），不引入「按病人直接分配」。

## 2. 已确认决策

| 决策点 | 选择 |
| --- | --- |
| 解锁方式 | 正规路径：先补管理 UI，再经 API 完成归属与授权 |
| 可见性模型 | 保持设备授权制，不新增按病人分配机制 |
| 管理入口 | viewer 内管理员控制台（仅 admin 可见可用） |

## 3. 非目标

- 按病人/按检查直接授权（违背信任边界设计）
- 设备 CRUD 全生命周期（删除/重命名设备本轮不做，仅注册/批准/禁用）
- 审计日志查看 UI（后端已有审计，UI 另立项）
- 工作项状态流转 UI（领取/释放在报告面板已有）

## 4. 后端设计

### 4.1 迁移 `0023_diagnostic_work_items_backfill.sql`

```sql
-- 为历史序列幂等补建工作项（pending）；新入库序列由 record_dimse_origin 实时创建。
INSERT INTO diagnostic_work_items (id, institution_id, series_fk)
SELECT gen_random_uuid(), st.institution_id, se.id
FROM series se JOIN studies st ON st.id = se.study_fk
ON CONFLICT (institution_id, series_fk) DO NOTHING;
```

### 4.2 新端点（全部 admin-only，挂临床路由 admin 段，ManageUsers 中间件已就绪）

| 方法与路径 | 请求/查询 | 说明 |
| --- | --- | --- |
| `POST /api/v1/devices` | `name`, `calling_ae_title`, `source_ip`, `modality_hint?` | 手动注册设备（status='pending'，供批准后归属历史数据） |
| `GET /api/v1/series-sources` | `status=unattributed\|all`, `limit`, `offset` | 列出序列来源状态：unattributed = needs_review/legacy_unattributed；返回 patient/study/series 摘要 + source_status + 设备名 |

复用现有端点（零改动）：`GET /devices`、`POST /devices/{id}/approve`、
`PATCH /devices/{id}`（启用/禁用）、`POST /series/{series_uid}/resolve-source`、
`GET/PUT /users/{user_id}/device-grants`。

### 4.3 语义细节

- 手动注册设备 status='pending'；`approve` 后 status='active'（归属前置条件）。
- `resolve-source` 要求目标设备 active（现有实现已校验）；归属后序列
  source_status='trusted'。
- 工作项由 0023 回填为 pending；医生领取（claim）后可写报告。
- 审计：设备注册/批准/授权/归属均调用现有 `audit()`。

### 4.4 后端测试（沿用 PACS_TEST_DATABASE_URL 可跳过模式）

1. 注册设备 → pending；批准 → active；禁用 → disabled
2. 归属 unattributed 序列到 active 设备 → trusted；到 pending 设备 → 404/冲突
3. series-sources 列表：unattributed 过滤正确、分页正确
4. 0023 回填幂等：重复跑不产生重复工作项
5. 授权 PUT 后 user_device_grants 往返一致

## 5. 前端设计

### 5.1 全链路 API（remote.rs 方法 + commands.rs 命令 + api.ts 封装）

`registerDevice` / `listDevices` / `approveDevice` / `setDeviceStatus` /
`listSeriesSources` / `resolveSeriesSource` / `listUsers` / `listUserDeviceGrants` /
`replaceUserDeviceGrants`。类型进 `types.ts`（`DicomDevice`、`SeriesSourceEntry`、
`AdminUser`、`UserDeviceGrant`）。

### 5.2 管理员控制台（`admin-console.ts`，dialog 模式，参照 router-panel）

- 入口：「更多」菜单加「管理控制台」项；仅 `remoteUser.role === 'admin'` 时
  显示（前端隐藏 + 后端 ManageUsers 双保险）。
- 三个 tab（复用 toolbar-menu/segmented 样式）：
  1. **设备**：状态过滤列表（pending/active/disabled）+「注册设备」表单
     （名称/AE Title/IP/模态）+ 每行「批准」「禁用/启用」。
  2. **来源归属**：未归属序列分页列表（患者/检查/模态/描述/来源状态）+ 每行
     「归属到设备」下拉（active 设备）+ 顶部批量操作「将当前页全部归属到
     选定设备」（逐行调用，单行失败不阻断并汇总提示）。
  3. **用户授权**：用户列表 + 每用户展开勾选设备列表（保存 = PUT grants，
     全量替换语义，与后端一致）。
- 操作后统一刷新 + 顶部错误横幅；409 → 提示刷新重试。

### 5.3 图标

新增 lucide 图标需在 `main.ts` 注册表登记（项目已知坑）：`shield-check`
（控制台入口）、`plug`（设备）、`landmark`（归属）、`user-cog`（授权）。

## 6. 错误处理

| 场景 | 行为 |
| --- | --- |
| 非 admin 调用 | 前端隐藏入口；后端 403 兜底 |
| 归属到 pending/disabled 设备 | 服务端拒绝，展示文案 |
| 批量归属部分失败 | 汇总「成功 n / 失败 m」+ 失败原因列表，不静默 |
| 授权 PUT 冲突 | 提示重新加载该用户授权状态 |

## 7. 验收清单（GUI）

1. admin 登录 → 更多菜单出现「管理控制台」；doctor 登录不出现
2. 设备页：注册设备（doctor 的模拟设备）→ 列表 pending → 批准 → active
3. 来源归属页：47 个 legacy 序列可见 → 批量归属到新设备 → 全部 trusted
4. 用户授权页：给 doctor 勾选该设备 → 保存
5. doctor 重新登录 → 病人列表出现 47 个序列对应的病人；打开检查 →
   报告面板显示工作项可领取
6. 禁用设备后 doctor 端对应数据消失（可见性实时生效）

## 8. 工作量

| 部分 | 估算 |
| --- | --- |
| 后端（0023 + 2 端点 + 测试） | ~1.5 天 |
| 前端 API 全链路 | ~0.5 天 |
| 管理控制台三 tab | ~2.5 天 |
| 联调 + 验收 | ~0.5 天 |
| **合计** | **约 1 周** |

## 9. 风险与对策

| 风险 | 对策 |
| --- | --- |
| 批量归属误操作 | 归属只改 source 字段不删数据；审计全记录；单行确认 UI 提示 |
| 手动注册设备伪造 AE Title | admin-only + 注册即 pending 需批准；文档注明 AE 可伪造，信任链在批准动作 |
| 授权全量替换误删他人授权 | UI 展示当前授权并确认后保存；后端语义与文档一致（PUT=替换） |
| 47 序列归属后工作项仍缺失 | 0023 backfill 先行（迁移顺序保证） |

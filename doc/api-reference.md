# Remote PACS API 接口文档

> 文档版本：1.0  
> 代码基线：2026-08-09 当前仓库实现  
> 适用对象：管理员前端、医生工作站、院内系统集成方、自动化运维程序

## 1. 概述

Remote PACS 将影像查看器与业务窗口解耦：查看器负责影像显示、标注和分割；管理员及医生业务窗口通过 HTTP API 完成账号、设备权限、待诊队列和报告工作。外部系统也可通过 JWT 或服务账号 API Key 调用相应接口。

本文以当前代码为准。运行实例还提供 `GET /api/v1/openapi.json`，但 OpenAPI 主要用于自动检测和交互调试，不一定完整描述所有 Viewer 内部接口；本文是完整的人工集成参考。

### 1.1 API 入口

假设服务器地址为 `https://pacs.example.com`：

| 前缀 | 用途 |
| --- | --- |
| `/api/v1` | 管理、临床业务、服务账号、导入导出、路由、生命周期 |
| `/auth` 或 `/api/v1/auth` | 用户登录、刷新令牌、退出、修改密码；两者功能相同 |
| `/api` | Viewer 工作列表、标注、分割 |
| `/api/dicom` | DICOM 标签修订与版本历史 |
| `/dicomweb` | QIDO-RS、WADO-RS、STOW-RS |
| `/api-checker` | 浏览器 API 检测中心 |

生产环境应只通过 HTTPS 暴露接口。若部署使用自签发 CA，调用方应将该 CA 导入系统或客户端信任库；不要长期使用 `curl -k` 跳过证书验证。

## 2. 通用约定

### 2.1 用户 JWT 认证

除登录等公开端点外，请求一般携带：

```http
Authorization: Bearer <access_token>
```

登录示例：

```bash
curl --request POST 'https://pacs.example.com/api/v1/auth/login' \
  --header 'Content-Type: application/json' \
  --data '{"username":"doctor.a","password":"your-password"}'
```

成功响应包含 `access_token`、`refresh_token` 和 `user`。访问令牌用于业务 API，刷新令牌仅用于刷新和退出。客户端不应记录或通过 URL 传递任何令牌。

### 2.2 服务账号 API Key

后端集成程序可使用管理员创建的服务账号密钥：

```http
Authorization: Bearer pacs_sk_<secret>
```

API Key 只在创建时完整返回一次，应立即存入密钥管理系统。当前 Scope：

| Scope | 能力 |
| --- | --- |
| `search` | 查询范围 |
| `read` | 读取范围 |
| `upload` | 导入及 STOW 上传 |
| `export` | 导出影像 |
| `route` | DICOM 路由管理和发送 |
| `admin` | 生命周期管理 |

服务账号 Scope 不等于用户角色，也不具备医生的设备授权模型。仅在接口明确支持 API Key 时使用；账号管理、工作列表、报告和 Viewer 人工操作使用用户 JWT。

### 2.3 数据格式

- JSON 请求使用 `Content-Type: application/json`，响应通常为 JSON。
- ID 字段应按接口定义使用整数或 UUID，UUID 示例：`550e8400-e29b-41d4-a716-446655440000`。
- `study_uid`、`series_uid`、`sop_uid` 是 DICOM UID 字符串，不能当作数字处理。
- 日期使用 `YYYY-MM-DD`；时间使用带时区的 RFC 3339，例如 `2026-08-09T10:30:00+08:00`。
- 可选字段一般可省略或传 `null`；不要用空字符串代替 `null`，除非字段明确允许。
- 布尔值必须是 JSON `true`/`false`。
- 路径中的 DICOM UID、病人 ID 等动态值应做 URL 编码。
- 列表接口的分页方式并未完全统一，应以各接口说明为准；不要假设存在全局 cursor。

### 2.4 状态码与错误

常见状态码：`200` 成功、`201` 创建成功、`204` 无响应体、`400` 参数错误、`401` 未认证、`403` 权限不足、`404` 不存在或不可见、`409` 版本/状态冲突、`422` 请求体校验失败、`500` 服务端错误。

新版临床接口通常返回：

```json
{"error":{"code":"conflict","message":"工作项已被其他医生领取"}}
```

部分旧接口返回：

```json
{"error":"错误说明"}
```

客户端应同时兼容 `error.message` 和字符串 `error`。为防止泄露病人或影像是否存在，无设备权限的临床资源通常也返回 `404`，而不是 `403`。

### 2.5 并发控制

工作项和报告采用 `revision` 乐观锁。先读取最新对象，再把其 `revision` 随写请求提交。收到 `409` 后应重新读取数据，不应盲目重试旧请求。

## 3. 角色与设备可见范围

系统角色是固定枚举：

| 角色 | 查看影像 | 上传 | 写报告 | 账号管理 | DICOM 标签修订 |
| --- | ---: | ---: | ---: | ---: | ---: |
| `admin` | 是 | 是 | 权限层面是；临床签发流程仅接受医师 | 是 | 是 |
| `radiologist` | 是 | 否 | 是 | 否 | 仅查看修订历史 |
| `technician` | 是 | 是 | 否 | 否 | 是 |
| `viewer` | 是 | 否 | 否 | 否 | 否 |

非管理员的影像可见性还受 Device Grant 限制。例如给医生 A 授予 CT1、CT2、MR1，则其工作列表、Viewer 查询、DICOMweb 读取、标注和分割只能访问来源属于这三台设备的影像。

设备由 DIMSE Calling AE Title 与来源 IP 识别。首次出现的设备为 `pending`，批准前医生不可见；可将设备设为 `disabled`。无法自动归属的历史数据标记为 `legacy_unattributed`，默认仅管理员可见。管理员可以手工解决 Series 的来源设备。

## 4. 认证 API

以下路径可在 `/auth` 或 `/api/v1/auth` 下调用。

| 方法与路径 | 认证 | 请求体 | 说明 |
| --- | --- | --- | --- |
| `POST /login` | 无 | `username`, `password` | 登录并取得两种令牌和用户信息 |
| `POST /refresh` | 无 | `refresh_token` | 轮换/刷新令牌 |
| `POST /logout` | 无 | `refresh_token` | 撤销刷新会话 |
| `POST /change-password` | 无 | `username`, `old_password`, `new_password` | 修改自己的密码 |

```json
{
  "username": "doctor.a",
  "password": "StrongPassword"
}
```

登录响应的用户对象包含整数 `id`、`institution_id`、`username`、`display_name`、`role`、`is_active`、`must_change_password`、`last_login_at`、`created_at` 等字段。管理员重置密码后，用户的 `must_change_password` 会变为 `true`。

## 5. 管理员 API

本节 Base URL 为 `/api/v1`，要求管理员 JWT（`ManageUsers`）。

### 5.1 角色与用户

| 方法与路径 | 请求 | 说明 |
| --- | --- | --- |
| `GET /roles` | — | 获取固定角色列表 |
| `GET /users` | — | 获取本机构全部用户 |
| `POST /users` | 见下 | 创建用户并可一次性分配设备 |
| `PATCH /users/{user_id}` | 可选 `display_name`, `role`, `is_active` | 修改用户 |
| `GET /users/{user_id}/device-grants` | — | 查询设备授权 |
| `PUT /users/{user_id}/device-grants` | `device_ids` | 全量替换设备授权，不是增量追加 |
| `POST /users/{user_id}/reset-password` | `temporary_password` | 管理员重置密码 |
| `POST /users/{user_id}/revoke-sessions` | — | 撤销该用户全部会话 |

创建用户：

```json
{
  "username": "doctor.a",
  "display_name": "张医生",
  "role": "radiologist",
  "temporary_password": "TemporaryStrongPassword",
  "device_ids": [
    "550e8400-e29b-41d4-a716-446655440000",
    "b3c0f88c-d7c0-4a64-8a2e-215b80e164eb"
  ]
}
```

替换授权：

```json
{"device_ids":["550e8400-e29b-41d4-a716-446655440000"]}
```

传空数组表示清空全部设备授权。用户名会规范化为小写，长度为 3～64，只允许小写字母、数字、`.`、`_`、`-`，且首字符须为字母或数字。

### 5.2 来源设备

| 方法与路径 | 请求/查询 | 说明 |
| --- | --- | --- |
| `GET /devices` | `status` 可选 | 列出发现的来源设备 |
| `POST /devices/{device_id}/approve` | `name`, `modality_hint` | 批准并命名设备 |
| `PATCH /devices/{device_id}` | `status` | 设为 `active` 或 `disabled` |
| `POST /series/{series_uid}/resolve-source` | `device_id` | 手工指定 Series 的来源 |

```json
{"name":"CT 1号机","modality_hint":"CT"}
```

设备过滤 `status` 应使用服务返回的状态值（主要为 `pending`、`active`、`disabled`）。手工归属会影响该 Series 此后的权限判断，操作前应核实来源。

## 6. 医生工作列表与报告

Base URL 为 `/api/v1`。读取需要 `ViewImages`，领取、释放、写报告及签发流程要求 `radiologist`。管理员虽然有通用权限，但报告 handler 的临床签发身份仍限定为医师。

### 6.1 待诊工作列表

| 方法与路径 | 请求/查询 | 说明 |
| --- | --- | --- |
| `GET /worklist` | `date=YYYY-MM-DD`, `status` 可选 | 获取设备授权过滤后的待诊序列 |
| `POST /worklist/{work_id}/claim` | `revision` | 排他领取工作项 |
| `POST /worklist/{work_id}/release` | `revision` | 释放自己的工作项 |
| `POST /worklist/{work_id}/assign` | `doctor_id`, `revision` | 管理员指派医生 |
| `GET /studies/{study_uid}/clinical-context` | — | 获取打开 Viewer/报告窗口需要的上下文 |

```json
{"revision":1}
```

```json
{"doctor_id":42,"revision":1}
```

领取是排他的。两位医生提交相同旧 `revision` 时只有一位成功，另一位收到冲突响应。建议医生端进入病人后先领取，并在离开未完成任务时释放。

### 6.2 报告

| 方法与路径 | 请求/查询 | 说明 |
| --- | --- | --- |
| `GET /reports` | 必填 `study_uid` | 查询指定 Study 的报告 |
| `POST /reports` | `study_uid`, `covered_series_uids` | 为 Study 创建报告 |
| `PUT /reports/{report_id}/draft` | `revision`, `findings`, `impression`, `recommendation` | 保存草稿 |
| `POST /reports/{report_id}/sign` | `revision` | 正式签发 |
| `POST /reports/{report_id}/amendments` | `reason` | 对已签发报告发起修订 |
| `GET /reports/{report_id}/versions` | — | 获取不可变版本历史 |

创建报告：

```json
{
  "study_uid": "1.2.840.113619.2.55.3.123",
  "covered_series_uids": ["1.2.840.113619.2.55.3.123.1"]
}
```

保存草稿：

```json
{
  "revision": 3,
  "findings": "双肺纹理清晰，未见明显实变影。",
  "impression": "胸部 CT 未见明显急性异常。",
  "recommendation": "结合临床，必要时随访。"
}
```

签发后正文不应直接覆盖；需先以 `{"reason":"补充临床信息后修订"}` 发起 amendment，再保存新版本。第三方系统应保留版本关系，而不是只缓存最新文本。

## 7. Viewer 数据 API

Base URL 为 `/api`，要求具有 `ViewImages` 的用户 JWT，并执行设备范围过滤。

| 方法与路径 | 查询 | 说明 |
| --- | --- | --- |
| `GET /patients` | `query`, `limit`, `offset` | 搜索可见病人 |
| `GET /patients/{patient_id}/studies` | — | 获取病人的可见检查 |
| `GET /studies/{study_uid}/series` | — | 获取检查下的可见 Series |

典型流程：搜索病人 → 获取 Study → 获取 Series → 使用 DICOMweb 获取 metadata/instance/frame。`limit`、`offset` 只适用于明确提供它们的端点。

### 7.1 个人窗宽窗位预设

个人预设绑定当前 JWT 用户并按 DICOM 模态分类，不会修改影像自身的 Window Center/Width 标签。

| 方法与路径 | 说明 |
| --- | --- |
| `GET /api/window-presets` | 列出当前用户的全部个人预设 |
| `POST /api/window-presets` | 保存新的个人预设 |
| `PATCH /api/window-presets/{preset_id}` | 重命名当前用户的预设 |
| `DELETE /api/window-presets/{preset_id}` | 删除当前用户的预设 |

创建请求示例：

```json
{"modality":"CT","name":"我的肺窗","center":-600,"width":1500,"function":"LINEAR"}
```

`function` 仅接受 `LINEAR`、`LINEAR_EXACT` 或 `SIGMOID`。同一用户、同一模态下的名称不区分大小写且不能重复；重名返回 `409`。重命名请求只提交 `{"name":"新名称"}`，其他用户的预设按不存在返回 `404`。

## 8. 共享标注 API

Base 路径：`/api/studies/{study_uid}/series/{series_uid}/annotations`。要求可查看目标设备影像。

| 方法与路径 | 说明 |
| --- | --- |
| `GET .../annotations?since=<RFC3339>` | 获取全部或指定时间后的变更 |
| `POST .../annotations` | 新建标注 |
| `PATCH .../annotations/{annotation_id}` | 乐观锁更新或软删除 |

创建示例：

```json
{
  "id": "3ae46967-3255-49d8-aa90-33d850955d57",
  "schema_version": 1,
  "kind": "length",
  "coordinate_space": "image",
  "sop_instance_uid": "1.2.3.4.5",
  "frame_number": 1,
  "mpr_plane": null,
  "geometry": {"start":[120.5,80.0],"end":[183.2,94.5]}
}
```

更新示例：

```json
{
  "expected_revision": 2,
  "geometry": {"start":[121.0,80.0],"end":[185.0,95.0]},
  "deleted": false
}
```

`geometry` 的结构由 `kind` 和 Viewer 工具约定；同步客户端应保存未知字段，并以服务端 revision 处理冲突。

## 9. 分割 API

Base 路径：`/api/studies/{study_uid}/series/{series_uid}/segmentations`。

| 方法与路径 | 说明 |
| --- | --- |
| `GET /segmentations` | 列出分割项目 |
| `POST /segmentations` | 创建项目及初始 Segment |
| `DELETE /segmentations/{project_id}` | 删除项目 |
| `GET /segmentations/{project_id}/segments` | 列出 Segment |
| `PATCH /segmentations/{project_id}/segments/{segment_id}` | 更新 Segment 元数据 |
| `GET /segmentations/{project_id}/masks` | 获取项目 Mask |
| `PUT /segmentations/{project_id}/segments/{segment_id}/mask` | 写入单个 Mask |
| `GET /segmentations/{project_id}/segments/{segment_id}/masks` | 获取指定 Segment 的 Mask |
| `PUT /segmentations/{project_id}/segments/{segment_id}/masks` | 批量写入 Mask |

创建示例：

```json
{
  "id": "ae0f7398-c574-439b-86c2-4304281862a9",
  "segment_id": 1,
  "name": "肺结节分割",
  "segment_label": "Nodule",
  "segment_description": "右肺上叶结节",
  "color": [255, 64, 64],
  "algorithm_type": "MANUAL",
  "tags": {"source":"viewer"}
}
```

Mask 示例：

```json
{
  "sop_instance_uid": "1.2.3.4.5",
  "frame_number": 1,
  "rows": 512,
  "cols": 512,
  "encoding": "rle-v1",
  "data_base64": "AAECAwQ...",
  "expected_revision": 0
}
```

当前编码仅支持 `rle-v1`。`frame_number` 与 DICOMweb 一致从 1 开始；`color` 为 `[R,G,B]`，每项 0～255。大体积 Mask 请求应遵守部署侧 body size 限制。

## 10. DICOM 标签修订 API

Base URL 为 `/api/dicom`。标签修改需要 `EditDicomTags`；修订历史需要 `ViewDicomRevisions`。

| 方法与路径 | 说明 |
| --- | --- |
| `GET /schema` | 获取允许修改的标签与规则 |
| `POST /transformations/preview` | 预演变更，返回 job 与确认令牌 |
| `GET /transformations` | 列出转换任务 |
| `POST /transformations` | 使用确认令牌提交执行 |
| `GET /transformations/{job_id}` | 获取任务状态 |
| `GET /instances/by-sop/{sop_uid}/revisions` | 按 SOP UID 查历史 |
| `GET /instances/{logical_id}/revisions` | 按内部逻辑 ID 查历史 |
| `POST /instances/{logical_id}/rollback` | 预演回滚到指定版本 |

预演：

```json
{
  "mode": "clinical_correction",
  "target": {"target_type":"study","key":"1.2.3.4"},
  "rules": [],
  "reason": "修正登记信息"
}
```

确认执行：

```json
{"job_id":"550e8400-e29b-41d4-a716-446655440000","confirmation_token":"one-time-token"}
```

回滚预演：

```json
{"version_id":123,"reason":"撤销错误修订"}
```

目标类型为 `patient`、`study`、`series` 或 `instance`。调用方应先读取 `/schema` 构造规则；预演结果确认无误后再提交，不应自行假定可编辑标签。

## 11. 导入与导出 API

Base URL 为 `/api/v1`。导入支持用户 `UploadImages` 或 API Key `upload`；导出支持用户 `ViewImages` 或 API Key `export`。

### 11.1 分块导入

| 方法与路径 | 说明 |
| --- | --- |
| `POST /imports` | 创建导入任务 |
| `POST /imports/{job_id}/files` | 登记一个待上传文件 |
| `PUT /imports/{job_id}/files/{upload_id}?offset=N` | 上传原始二进制 chunk |
| `POST /imports/{job_id}/complete` | 完成并开始处理 |
| `GET /imports/{job_id}` | 查询状态 |
| `DELETE /imports/{job_id}` | 取消任务 |

```json
{"idempotency_key":"his-order-20260809-001"}
```

```json
{
  "relative_name": "study/series/image0001.dcm",
  "size": 1048576,
  "sha256": "可选的十六进制SHA-256"
}
```

Chunk 请求体不是 JSON：

```bash
curl --request PUT \
  'https://pacs.example.com/api/v1/imports/<job_id>/files/<upload_id>?offset=0' \
  --header 'Authorization: Bearer <token-or-api-key>' \
  --header 'Content-Type: application/octet-stream' \
  --data-binary '@image0001.dcm'
```

后续 chunk 的 `offset` 必须对应服务端期望偏移。网络失败后先查询任务/上传状态，再决定续传位置。

### 11.2 导出

| 方法与路径 | 说明 |
| --- | --- |
| `POST /exports` | 创建 Study 或 Series 导出任务 |
| `GET /exports/{job_id}` | 查询状态 |
| `DELETE /exports/{job_id}` | 取消任务 |
| `GET /exports/{job_id}/download` | 下载已完成的导出包 |

```json
{
  "study_instance_uid": "1.2.3.4",
  "series_instance_uid": null,
  "idempotency_key": "download-order-001"
}
```

下载接口返回二进制内容。客户端应读取响应的 `Content-Type` 和 `Content-Disposition`，不要把响应按 JSON 解析。

## 12. DICOM 路由 API

Base URL 为 `/api/v1/router`。要求管理员 JWT 或 API Key `route` Scope。

| 方法与路径 | 说明 |
| --- | --- |
| `GET /node` | 获取本地路由节点信息 |
| `GET /destinations` | 列出目标端 |
| `POST /destinations` | 创建目标端 |
| `PUT /destinations/{id}` | 全量更新目标端 |
| `DELETE /destinations/{id}` | 删除目标端 |
| `POST /destinations/{id}/test` | 测试连通性 |
| `POST /destinations/{id}/approve` | 批准目标端 |
| `GET /peers?limit=N` | 获取观察到的 DIMSE 对端 |
| `GET /series?limit=N` | 获取可发送 Series |
| `GET /rules` / `POST /rules` | 列出/创建路由规则 |
| `PUT /rules/{id}` / `DELETE /rules/{id}` | 更新/删除规则 |
| `POST /send` | 手工发送 Study 或 Series |
| `GET /deliveries?limit=N` | 获取投递记录 |
| `POST /deliveries/{id}/replay` | 重放失败投递 |

目标端支持 `dimse` 和 `stow`：

```json
{
  "name": "院内归档节点",
  "protocol": "dimse",
  "enabled": true,
  "host": "10.10.1.20",
  "port": 104,
  "called_ae_title": "ARCHIVE",
  "calling_ae_title": "REMOTE_PACS",
  "use_tls": false,
  "stow_url": null,
  "auth_token": null,
  "ca_pem": null
}
```

STOW 目标使用 `stow_url`，可选 `auth_token` 和 `ca_pem`。读取目标端时密钥和 CA 原文不会回传，仅返回是否已配置的标志。

路由规则：

```json
{
  "destination_id": "550e8400-e29b-41d4-a716-446655440000",
  "name": "CT 自动归档",
  "priority": 100,
  "enabled": true,
  "source_ae_title": "CT1",
  "modality": "CT",
  "body_part_examined": null,
  "study_description": null,
  "series_description": null,
  "tag_matches": {}
}
```

手工发送：

```json
{
  "destination_id": "550e8400-e29b-41d4-a716-446655440000",
  "study_instance_uid": "1.2.3.4",
  "series_instance_uid": null
}
```

新目标需经过批准后用于正式投递。`priority` 决定规则匹配次序；部署方应避免多个规则产生意外重复发送。

## 13. 生命周期 API

Base URL 为 `/api/v1/lifecycle`。要求管理员 JWT 或 API Key `admin` Scope。这些接口可能移动或永久清除影像，应只供受控管理程序使用。

| 方法与路径 | 说明 |
| --- | --- |
| `GET /summary` | 各存储层级、占用和 hold/purge 汇总 |
| `GET /jobs` | 生命周期后台任务 |
| `GET /policies` / `POST /policies` | 列出/创建策略 |
| `PUT /policies/{id}` / `DELETE /policies/{id}` | 更新/删除策略 |
| `POST /policies/{id}/preview` | 预演策略 |
| `POST /policies/{id}/run` | 执行策略 |
| `GET /studies` | 获取生命周期 Study 列表 |
| `POST /studies/{study_uid}/move` | 移动存储层级 |
| `POST /studies/{study_uid}/restore` | 恢复到热存储 |
| `POST /studies/{study_uid}/holds` | 创建法律保留 |
| `GET /holds` / `DELETE /holds/{id}` | 列出/释放保留 |
| `GET /purge-requests` / `POST /purge-requests` | 列出/创建清除申请 |
| `POST /purge-requests/{id}/approve` | 批准并设置宽限期 |
| `POST /purge-requests/{id}/reject` | 拒绝申请 |
| `GET /events` | 查询生命周期事件 |

策略：

```json
{
  "name": "两年前 CT 转冷存储",
  "priority": 100,
  "enabled": false,
  "target_tier": "cold",
  "modalities": ["CT"],
  "study_date_before": "2024-08-09",
  "last_accessed_before": null,
  "tag_matches": {},
  "minimum_study_bytes": null,
  "minimum_storage_used_percent": 80.0
}
```

新策略必须先保持 `enabled:false` 创建并调用 preview；定义变化后需要重新预演，只有当前定义已预演才能启用。层级值为 `hot`、`cold`、`quarantine`。

移动、保留、清除：

```json
{"target_tier":"cold"}
```

```json
{"reason":"医疗纠纷证据保全","expires_at":"2027-08-09T00:00:00+08:00"}
```

```json
{"study_instance_uid":"1.2.3.4","reason":"依法超过保存期限"}
```

```json
{"grace_hours":168}
```

清除是审批加宽限期流程，而不是即时删除。存在有效 Legal Hold 的 Study 不应被清除。执行前应通过 preview、事件记录及备份策略完成复核。

## 14. 服务账号管理

Base URL 为 `/api/v1`。管理接口要求管理员 JWT。

| 方法与路径 | 说明 |
| --- | --- |
| `GET /service-accounts` | 列出服务账号 |
| `POST /service-accounts` | 创建服务账号 |
| `POST /service-accounts/{account_id}/keys` | 创建密钥，完整 secret 只返回一次 |
| `DELETE /service-accounts/{account_id}/keys/{key_id}` | 撤销密钥 |
| `DELETE /service-accounts/{account_id}` | 停用服务账号 |
| `GET /service-auth/whoami` | 使用 API Key 检查身份和 Scope |
| `GET /openapi.json` | 获取机器可读 API 定义 |

创建服务账号时会同时创建第一把密钥：

```json
{
  "name": "HIS 影像导出服务",
  "scopes": ["export"],
  "expires_at": "2027-08-09T00:00:00+08:00"
}
```

成功响应为 `account`、`key_id`、`api_key`；其中 `api_key` 只展示一次。新增密钥的请求体只有可选 `expires_at`：

```json
{"expires_at":"2027-08-09T00:00:00+08:00"}
```

建议每个外部系统独立账号、按最小权限分配 Scope、定期轮换密钥，并在轮换验证成功后撤销旧 key。

## 15. DICOMweb

Base URL 为 `/dicomweb`。读取要求用户 JWT `ViewImages` 并执行设备过滤；STOW 支持具有上传权限的用户 JWT 或 API Key `upload`。

| 标准 | 方法与路径 | 响应 |
| --- | --- | --- |
| QIDO-RS | `GET /studies` | `application/dicom+json`，无结果可能为 `204` |
| QIDO-RS | `GET /studies/{study_uid}/series` | DICOM JSON |
| QIDO-RS | `GET /studies/{study_uid}/series/{series_uid}/instances` | DICOM JSON |
| WADO-RS | `GET /studies/{study_uid}/series/{series_uid}/instances/{sop_uid}` | `application/dicom` |
| WADO-RS | `GET .../{sop_uid}/metadata` | DICOM JSON metadata |
| WADO-RS | `GET .../{sop_uid}/frames/{frames}` | `multipart/related` |
| STOW-RS | `POST /studies` | 存储 DICOM 实例 |

帧编号从 1 开始，多个帧用逗号分隔，例如 `/frames/1,2,5`。帧响应是 multipart，调用方必须按响应头中的 boundary 解析，不能直接把整个 body 当成单张图片。

STOW 示例：

```http
POST /dicomweb/studies HTTP/1.1
Authorization: Bearer pacs_sk_xxx
Content-Type: multipart/related; type="application/dicom"; boundary=DicomBoundary

--DicomBoundary
Content-Type: application/dicom

<DICOM bytes>
--DicomBoundary--
```

QIDO 查询和 DICOM JSON 遵循 DICOMweb 标签对象格式。调用方应使用响应的实际 `Content-Type`，尤其注意 `204` 没有 JSON body。

## 16. API 检测中心

浏览器访问：

```text
https://pacs.example.com/api-checker
```

检测中心会读取 `/api/v1/openapi.json` 并补充内部路由，支持：

1. 使用用户名和密码登录并自动携带访问令牌；
2. 查看接口列表、填写 path/query/body 后单独发送请求；
3. 对受保护路由做认证防护扫描；
4. 对 GET 接口执行冒烟检测；
5. 生成 cURL 命令；
6. 导出 JSON 检测结果。

建议的验收顺序：先检测 OpenAPI 和登录；再用管理员账号测试用户/设备接口；用医生账号验证授权设备可见、未授权设备返回 404；最后用测试数据验证 claim 冲突、报告 revision、DICOMweb 内容类型和导入导出。检测中心会真实调用 API，写操作可能修改数据，应在测试环境使用专用账号和数据。

## 17. 外部集成建议

- 管理员 UI、医生业务窗口与 Viewer 使用相同用户 JWT，但各自只调用所需 API；Viewer 打开由 clinical-context 提供的 Study/Series，再通过 DICOMweb 取图。
- 对 `401` 尝试一次 refresh；refresh 失败则要求重新登录。不要对 `403`、`404` 自动换管理员凭证重试。
- 对 `409` 重新获取最新 revision，并让用户确认合并结果。
- 不在浏览器 LocalStorage、日志、错误上报或 URL 中长期暴露刷新令牌和 API Key；具体令牌存储方案应结合部署安全模型决定。
- 服务账号执行导入、导出、路由或生命周期操作时，使用独立账号和最小 Scope。
- 所有病人、报告、影像导出和生命周期操作应进入合规审计；外部系统也应记录操作者、请求 ID、时间和业务原因，但不得记录影像正文或密码。
- DICOM UID、病人 ID 和 AE Title 不应自行改写大小写或转成数字。
- 网络重试只用于明确幂等的读取，或已提供 `idempotency_key`/可确认状态的任务。不要无条件重放签发、修订、路由发送和删除类请求。

## 18. 快速联调示例

```bash
# 1. 登录
curl -sS 'https://pacs.example.com/api/v1/auth/login' \
  -H 'Content-Type: application/json' \
  -d '{"username":"doctor.a","password":"your-password"}'

# 2. 获取今日待诊列表（替换 TOKEN 和日期）
curl -sS 'https://pacs.example.com/api/v1/worklist?date=2026-08-09' \
  -H 'Authorization: Bearer TOKEN'

# 3. QIDO 查询可见 Study
curl -sS 'https://pacs.example.com/dicomweb/studies' \
  -H 'Authorization: Bearer TOKEN' \
  -H 'Accept: application/dicom+json'

# 4. 检查服务账号身份
curl -sS 'https://pacs.example.com/api/v1/service-auth/whoami' \
  -H 'Authorization: Bearer pacs_sk_xxx'
```

部署地址、端口、TLS CA、初始管理员账号及上传体积限制属于部署配置，不由 API 契约固定。联调时以目标环境配置和 `/api/v1/openapi.json` 返回为准。

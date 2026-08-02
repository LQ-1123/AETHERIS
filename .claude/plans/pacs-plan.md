# remote_pacs 实施计划

自建 PACS：Rust 服务端 + Tauri 桌面查看器，可分发、多账号、共享平台数据库。
目标：快、可靠。标记 `【可改】` 的是决策点。

## 已确认的选型

| 项 | 选择 |
|---|---|
| DICOM 接口 | DIMSE(C-ECHO/STORE/FIND/MOVE/GET SCP) + DICOMweb(QIDO/WADO/STOW-RS) |
| DICOM 底座 | dicom-rs 0.10 生态，不用 Orthanc/dcm4chee |
| 元数据库 | PostgreSQL(已装 14) |
| 查看器 | Tauri 2 桌面客户端，可分发 |
| 多用户 | 账号体系 + 共享平台数据库 |
| 工具链 | rustup(换掉 Homebrew rust) |
| AI | 只预留接口，第一阶段不做 |

## 环境实测结论(已在你机器上验证)

`tauri 2.11.5`、`dicom 0.10`、`dicom-ul 0.10 (async)`、`axum 0.8.9`、
`sqlx 0.8.6`、`dicom-pixeldata 0.10 (openjp2)` 全部编译通过，约 35s。

**换 rustup 是安全的**：`brew uses --installed rust` 为空(没有其他 formula
依赖它)，`~/.cargo/bin` 为空(没有 cargo install 的工具会失效)。
换到 1.97.1 后 `sqlx` 可以用 0.9(需要 rustc ≥1.94)。注意 `sqlx 0.9` 我**没有
实测过**，只验证了 0.8.6；升级后先跑一次编译确认。

还需处理：Postgres 的 `sunyulin` 用户没密码，现在 `psql` 连不上；
DCMTK 没装(`brew install dcmtk`)，这是 DIMSE 互操作的唯一测试基准。

## 关键事实：dicom-rs 给到哪一层

- `dicom-ul` **只有传输层** —— association 协商、PDU 读写、P-DATA 分帧，
  含 sync/async 和 TLS。`ServerAssociationOptions`/`AsyncServerAssociation` 可直接用。
- **DIMSE 服务类要自己写** —— C-FIND/C-MOVE/C-GET 的命令集组装、状态机、
  pending 响应流都没有。参考 `dicom-storescp`:命令集固定 Implicit VR LE，
  按 `COMMAND_FIELD` 分派(C-ECHO-RQ `0x0030`、C-STORE-RQ `0x0001`、
  C-FIND-RQ `0x0020`、C-MOVE-RQ `0x0021`)。这是工作量大头。
- `dicom-toolkit-net` 有完整 find/move/get SCP，但是**另一套独立生态**
  (自带 core/data/dict),混用等于一个二进制两个 DICOM 对象模型。不建议，
  可以拿来当参考实现。

### 编解码器能力(实测)

| 传输语法 | 解码 | 编码 |
|---|---|---|
| Implicit/Explicit VR LE | ✅ | ✅ |
| JPEG baseline/lossless | ✅ | ✅ |
| JPEG-LS | ✅ | ✅ |
| **JPEG 2000** | ✅ (`openjp2`,纯 Rust,无需 cmake) | ❌ **无编码器** |
| RLE Lossless | ✅ | ❌ (upstream TODO #125) |

能接收和显示 J2K(CT/MR 常见),但不能转码成 J2K。要压缩归档得自己绑
OpenJPEG C 库，近期不做。【可改】

## 架构

### 信任边界(多账号下最重要的决定)

**客户端绝不直连 Postgres。** 软件要分发到不同机器、不同账号，如果客户端
内嵌数据库连接串，等于把库凭据发给每个用户 —— 无法做权限控制、无法吊销、
无法轮换，任何人可以直接 `DELETE FROM studies`。

所以：客户端只通过 HTTPS 访问 `pacsd`,服务端独占数据库连接。

```
                    ┌─────────── pacsd (唯一持有 DB 凭据) ───────────┐
影像设备 ──DIMSE──> │  DIMSE 监听 :11112    HTTPS 监听 :8443        │
(AE Title 白名单)   │        │                      │               │
                    │        └──── pacs-db ────────┘               │
Tauri 客户端 ──────>│              (Postgres)      + 文件存储        │
(账号+token, HTTPS) └────────────────────────────────────────────────┘
```

两条入口的认证模型完全不同，要分开设计：DIMSE 侧只有 AE Title(协议本身
无认证，可伪造),靠白名单 + 网络隔离 + TLS;HTTP 侧是账号密码 + token。

### Workspace 布局

```
crates/
  pacs-core/      领域模型(Patient/Study/Series/Instance)、UID、错误类型
  pacs-store/     文件落盘、fsync 语义、路径策略、孤儿清理
  pacs-db/        Postgres 访问层、迁移、查询构造(sqlx)
  pacs-dimse/     ★ 自研 DIMSE:命令集编解码 + 各 SCP 服务
  pacs-auth/      账号、密码哈希(argon2)、token、RBAC、审计
  pacs-web/       axum: QIDO/WADO/STOW-RS + 认证 API
  pacs-codec/     像素解码、缩略图、帧提取(CPU 密集,独立线程池)
  pacs-ai/        ☆ 只有 trait 和 job 表,不含实现
  pacsd/          服务端主程序:配置、日志、两个监听器
apps/viewer/      Tauri 2 客户端(也能脱离服务端打开本地 DICOM 文件)
```

### 账号与权限

- 密码用 `argon2` 0.5.3 哈希(不是 bcrypt/sha)。
- **短期 access token(15 分钟) + 不透明 refresh token(存库,可吊销)**。
  纯 JWT 无法吊销 —— 用户离职、设备丢失时必须能立刻断访问，医疗场景这是硬需求。
  `jsonwebtoken` 11 的 MSRV 是 1.88,换 rustup 后没问题。
- 角色:`admin` / `radiologist`(读写报告) / `technician`(上传) / `viewer`(只读)。
- 预留 `institution_id` 列做机构隔离，即使第一版单机构也先留 —— 后期加多租户
  要改所有查询，早留成本几乎为零。【可改】
- **审计日志**:谁在何时访问了哪个 patient/study。医疗合规的硬要求，
  且必须记在 DB 而不是只写文件。

### 数据模型

四层 `patients/studies/series/instances` 外键级联，加 `users`/`sessions`/
`audit_log`/`ai_jobs`。每层存**结构化列**(索引 + C-FIND 匹配) + 一列 `JSONB`
原始标签(应对任意查询键,不用改表)。

索引覆盖真实查询键:`PatientID`、`StudyInstanceUID`、`AccessionNumber`、
`StudyDate`、`ModalitiesInStudy`、`PatientName`。DICOM 通配 `*` `?` 转 SQL
`LIKE`,日期范围 `20240101-20240131` 转 `BETWEEN` —— 这层转换是 C-FIND
正确性核心，单独测。

### 存储布局

```
<root>/<hash(StudyUID)[0:2]>/<hash[2:4]>/<StudyUID>/<SeriesUID>/<SOPUID>.dcm
```

两级哈希分片控制单目录 fanout(几十万子目录会拖死文件系统),同时保持
study/series 局部性 —— WADO 拉整个 series 是顺序读同一目录。DB 存相对路径,
根目录可迁移。【可改:也可按接收日期分片,利于归档,代价是 study 跨天分裂】

### 可靠性:C-STORE 落盘顺序

响应 success 之前必须真的持久化:

1. 写临时文件 `.tmp/<uuid>` → `fsync(file)`
2. `rename()` 到最终路径 → `fsync(parent_dir)`(rename 不保证目录项持久)
3. DB 事务提交
4. 才发 C-STORE-RSP `0x0000`

任何一步失败 → 回滚 + 返回对应 DIMSE 错误码,不留半截数据。崩溃恢复:启动
扫 `.tmp/` 清残留 + 定期核对孤儿文件。重复 SOPInstanceUID 走幂等 upsert
不报错(设备重传很常见)。

### 性能设计

- **网络 I/O 全 async(tokio),像素解码走 `spawn_blocking`/rayon**。解码是
  CPU 密集,跑在 async runtime 上会堵死 executor。这个边界一开始就要划对。
- 一个 association 内多个 C-STORE 复用同一 DB 连接和事务批次。
- 入库时**预生成缩略图**,列表页不解码原图。
- **帧数据用 Tauri 的 `register_asynchronous_uri_scheme_protocol` 走自定义
  协议直传二进制**,不走 JSON IPC。走 IPC 要 base64,一个 512×512×16bit 帧
  膨胀 33% 还要 JS 侧解码,序列滚动会卡。已确认该 API 在 2.11.5 存在。
- WADO-RS 帧级 LRU 缓存 + `Range` 支持;`sqlx` 连接池 + 预编译语句。

目标(需实测验证,不是承诺):C-STORE 未压缩 CT ≥200 instance/s(基本是磁盘
带宽上限);100 万 study 下 study 级 C-FIND p95 <50ms;WADO 单帧热缓存
p95 <100ms;CT 序列滚动 ≥30fps。第 2 阶段起就建 benchmark。

## 查看器:两个正确性陷阱

你要的功能:打开本地 DICOM、CT 序列浏览、X 光显示、窗宽窗位、缩放平移、测距。
其中两处如果按直觉实现会出错:

### 1. 测距在 X 光上和 CT 上不是同一个问题

CT 的 `PixelSpacing (0028,0030)` 是重建平面上的真实毫米,直接乘就行。

投影 X 光(CR/DX/XA)不行。`ImagerPixelSpacing (0018,1164)` 是**探测器平面**的
间距,而解剖结构离探测器有距离,会被放大约 SID/SOD 倍。DICOM 标准 §10.7
(Basic Pixel Spacing Calibration Macro)规定:若 `PixelSpacing` 存在且**未**做
几何放大校正,它的值应与 `ImagerPixelSpacing` 相同。所以:

- 两个都有且**不相等** → `PixelSpacing` 是已校正值,用它,并读
  `PixelSpacingCalibrationType (0028,0A02)` / `...Description (0028,0A04)`
  说明校正来源。
- 只有 `ImagerPixelSpacing`(或两者相等) → **这是探测器平面距离,不是解剖
  真实距离**。可用 `EstimatedRadiographicMagnificationFactor (0018,1114)`
  校正,但该标签常常缺失。
- 这种情况下 UI 必须**明确标注"探测器平面,未校正"**,并提供用户手动校准
  (对已知尺寸物体定标)。直接显示 "42.3 mm" 是在暗示一个不存在的精度 ——
  这类问题真实导致过器械召回(BfArM 07207-15 就是 GE 的相关通告)。

所以测距功能要分:CT 直接算;X 光标注不确定性 + 支持手动校准。【范围可改,
但"不加标注直接显示毫米"不建议】

### 2. 显示管线的顺序不能错

正确顺序:`存储值 → Rescale(Slope/Intercept) → VOI(窗宽窗位) → Photometric
反转 → 输出`。常见错误:

- CT 忘做 `RescaleSlope/Intercept` → HU 值全错,窗宽窗位对不上。
- **`PHOTOMETRIC_INTERPRETATION` 为 `MONOCHROME1` 时灰度是反的**(0=白)。
  X 光里 `MONOCHROME1` 很常见,漏判会得到负片。
- `WindowCenter/Width` 可以是多值(VM>1),要选第一组或让用户切。
- `VOILUTFunction` 为 `SIGMOID` 时不是线性窗宽,按线性算会有偏差。
- CT 序列排序**不要用 `InstanceNumber`** —— 不可靠。应按
  `ImagePositionPatient (0020,0032)` 投影到 `ImageOrientationPatient
  (0020,0037)` 算出的切片法向量上排序。

### 3. 本地打开文件


查看器要能不连服务端直接开本地 `.dcm`,所以 `pacs-core` + `pacs-codec` 要能
被客户端复用(它们不依赖 DB)。这也让查看器可以独立测试。

## AI 接口预留(不实现)

只做三件事,加起来很小:

1. `ai_jobs` 表:`id / study_uid / model_name / status / created_at / result JSONB`
2. `pacs-ai` 里一个 trait:`async fn infer(&self, study: &StudyRef) -> Result<Findings>`
3. `findings` 表预留(关联 study/series/instance + JSONB 结果 + 模型版本)

不写任何模型代码、不引 onnxruntime/torch 依赖。这样将来接入时不用改表结构和
核心流程。

## 分阶段实施

> **进度**:阶段 0–4 已完成并通过验收(2026-08-02)。
> 全量测试 `cargo test --workspace -- --test-threads=1` 全绿,
> `cargo clippy --workspace --all-targets -- -D warnings` 无警告。
> 下一步:阶段 5(DICOMweb)。

**阶段 0 — 环境** ✅
`brew uninstall rust` → 装 rustup → `rust-toolchain.toml` 锁 1.97.1;
设 Postgres 密码、建 `pacs` 库;`brew install dcmtk`;workspace 骨架;
CI(fmt/clippy/test)。升级后先验证 `sqlx 0.9` 能编过。

**阶段 1 — 存储 + 数据库** ✅
`pacs-core` 领域模型;`pacs-db` 迁移和表结构;`pacs-store` 落盘 + fsync。
交付:一个 DICOM 文件能正确入库入盘,有单元测试。

**阶段 2 — C-ECHO + C-STORE SCP** ✅
`pacs-dimse` 命令集编解码基础设施 → C-ECHO → C-STORE。
交付:`echoscu`/`storescu` 打通 —— 第一个能证明"是个 PACS"的里程碑。建 ingest benchmark。

**阶段 3 — 账号体系 + TLS** ✅
因为要分发多账号,这个提前到第 3 阶段(不是最后)。`pacs-auth`:账号、argon2、
token、RBAC、审计日志;HTTPS + rustls。**在暴露任何网络接口之前完成。**

**阶段 4 — C-FIND SCP** ✅
Study/Patient Root 信息模型;通配和日期范围匹配;pending 流式响应。
交付:`findscu` 各层级查询正确。匹配语义最易错,重点测。

实际落点:
- `pacs-core::query` —— 标识符解析与五种匹配类型的分类,**按 VR 分派**
  (通配符只对 AE/CS/LO/PN/SH 生效,范围只对 DA/TM/DT)。纯逻辑,可穷举测试。
- `pacs-db::find` —— 常量列表把 DICOM 标签映射到 SQL 列,按层级作用域裁剪;
  PN 匹配走 `name_normalized`、返回走原始 `name`。
- `pacs-dimse::find` —— pending 流式响应 + 结束状态。
- 验收:`crates/pacsd/tests/find_interop.rs`,11 个 `findscu` 真实查询。

两条实现中确认过的取舍:
- **不支持的匹配键忽略并回 `0xFF01`**,不是静默忽略、也不是失败。静默忽略会让
  对方以为过滤生效了(实际返回得更多);失败则把一个笔误放大成整次查询不可用。
- **结果超过 `DEFAULT_LIMIT` 报错而不截断**。截断会让对方以为"结果就这么多",
  一次静默漏掉的检查比一次明确的失败危险得多。

已知局限(记录在 `pacs-dimse::find` 模块文档里):C-CANCEL 在响应流中途到达时
不会提前中止 —— `dicom-ul` 的 association 不能拆分读写两半,`receive()` 也不是
cancel-safe。结果集在发送前已全部取出、发送阶段纯输出,通常几毫秒走完;
中途到达的 C-CANCEL 会在下一轮被读到并忽略(绝不中止 association)。

**阶段 5 — DICOMweb**
`pacs-web`: QIDO-RS(复用阶段 4 查询层)、WADO-RS(含 `/frames`)、STOW-RS,
全部带认证。`pacs-codec` 解码和缩略图。交付:HTTP 侧可用,查看器可开工。

**阶段 6 — Tauri 查看器**
本地文件打开 → 单帧显示(Rescale/VOI/Photometric 管线) → 窗宽窗位 → 缩放
平移 → CT 序列浏览(按空间位置排序 + 预取) → 测距(CT 精确 / X 光带校准)。
帧数据走自定义协议直传。

**阶段 7 — 分发**
`tauri-plugin-updater` 自动更新;代码签名(macOS 公证);首次启动配置服务器
地址;打包 CI。

**阶段 8 — 加固**
C-MOVE/C-GET SCP(要反向做 SCU 连目的地 AE,最复杂的状态机);AE Title
白名单;备份恢复演练;压力测试;脱敏工具。

## 安全提醒

- DIMSE 协议**无认证**(AE Title 可伪造),DICOMweb 默认也没有。默认只绑
  `127.0.0.1`,绑其他地址必须显式配置。阶段 3 之前不要接真实网络。
- 客户端不内嵌 DB 凭据(见"信任边界")。
- 真实病人数据涉及 HIPAA/GDPR/《个人信息保护法》,脱敏工具早做。

## 待你确认

1. 客户端和服务端之间**不直连数据库**这个约束能接受吗?这决定整个 API 设计。
2. 用户账号谁来建?管理员后台 / 命令行工具 / 自助注册+审批?
3. X 光测距的不确定性标注方案接受吗?还是有其他偏好?
4. 部署形态:服务端跑在你自己的机器/内网服务器/云?影响 TLS 证书方案
   (自签 vs Let's Encrypt)。


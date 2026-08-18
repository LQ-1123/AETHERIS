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

> **进度（2026-08-03）**：阶段 0–4 已完成；阶段 5 的 QIDO-RS/WADO-RS
> 读取侧已完成，STOW-RS 待做；阶段 6 的本地 Viewer MVP 已完成，远程工作列表待做。
> Viewer 的 TypeScript 构建、9 项前端单元测试、7 项独立 Rust 测试和 Clippy 已通过。
> 根 workspace 的数据库互操作测试仍需在可访问本机 PostgreSQL/DCMTK 的环境复验。

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

**阶段 5 — DICOMweb（读取侧完成，上传待做）**
`pacs-web`:QIDO-RS(复用阶段 4 查询层)和 WADO-RS(含 `/frames`)已完成并带认证；
STOW-RS 尚未实现。`pacs-codec` 已提供帧解码和显示管线。

**阶段 6 — Tauri 查看器（本地 MVP 完成）**
已完成单文件多帧和多文件 CT/MR 序列打开、显示管线、窗宽窗位、窗预设、
光标锚定缩放、平移、按空间位置排序、前后帧预取、病人信息和两点测距。
测距区分 CT 精确值、X 光探测器平面 caveat 和仅像素；帧数据走自定义协议直传。
尚未完成远程登录、QIDO 工作列表和 WADO 打开流程。

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

## 已确认(原"待你确认")

1. **客户端不直连数据库** —— 已确认,按此设计。
2. **账号由管理员后台创建** —— 已确认。引导入口是 `pacsd admin` 命令行
   (总得有第一个管理员),后续账号走后台。
3. **X 光测距的标注方案** —— 已确认:参照 Orthanc 等平台的做法,
   优先读 DICOM tag,有准确数据就显示毫米,没有就明说没有并给出像素值作参考。
   实现见 `pacs-core::spacing`,细节记在下面。
4. **部署形态** —— 先跑在自己的机器上,TLS 用自签证书(`pacsd` 启动时自动生成)。

### 测距标定的判定(阶段 6 查看器按此实现界面)

`pacs-core::spacing::resolve` 把结果分三档,界面据此决定怎么显示:

| 档 | 何时 | 界面 |
|----|------|------|
| `Calibrated` | 断层模态的 `PixelSpacing`、声明已标定的间距、单区域超声、放大率已校正 | 直接给毫米 |
| `DetectorPlane` | 投影影像只有 `ImagerPixelSpacing`,或投影影像的 `PixelSpacing` 无 `CalibrationType` | 给毫米**并显示 caveat** |
| `None` | 没有任何间距属性 | 只给像素数,显示 `reason` |

两个容易搞反、已用测试固定住的点:

- **偏差方向是「高估」**。射线源 → 病灶 → 探测器,病灶投影到探测器上是放大的
  (放大率 = SID/SOD > 1),`ImagerPixelSpacing` 换算出的是那个放大影子的尺寸,
  **比真实解剖结构大**。胸片典型放大率约 1.2,即偏大两成。说成"低估"会让
  医生以为真实病灶更大,判断正好反过来。
- **`PixelSpacing` 是「行\列」顺序**,不是 x/y;而超声的 `PhysicalDeltaX/Y`
  里 X 是列方向。两者顺序相反,弄反会让非正方像素的影像长宽互换。

未做:多区域超声(同屏 B 超 + 多普勒频谱)每个区域标定不同,目前退回像素。
阶段 6 查看器有了测量点坐标之后,可以按点落在哪个区域来选标定。

  storescu -v \
    -aet TEST_SCU \
    -aec REMOTE_PACS \
    -xs\
    +sd +r +sp '*.dcm' \
    127.0.0.1 11112 \
    "dicom_data/导出的检查影像(DICOM)_冯俊峰"

## 阶段 9 - DICOM 标签修订与回滚

> **范围修订日期：2026-08-04。** 本阶段只保留临床标签修订、不可变版本、任务历史和
> 回滚。创建脱敏副本与 Calling AE 自动规范化已从产品、API、执行器和数据库中删除。

### 目标与边界

支持按病人、检查或序列批量修改白名单内的 DICOM 标签，用于患者信息纠错和检查信息
规范录入。所有修改先预览、填写原因并确认，再由持久化后台任务生成不可变的新修订。

本阶段明确不做：

- 不创建脱敏 DICOM 副本，不提供 ZIP、manifest、伪名项目或下载接口；
- 不按 Calling AE、设备或模板自动修改接收后的 DICOM；
- 不修改 PixelData，不擦除图像像素中的文字；
- 不做病人合并/拆分；
- 不允许修改 SOP Class、Transfer Syntax 或像素解释字段；
- 不原地覆盖已经归档的原始文件。

字符集容错属于接收和解析基础能力，不属于 Calling AE 自动规范化。系统继续在元数据
字符集声明错误或缺失时执行可靠的文本解码兜底，并将派生修订统一写为 UTF-8。

### 产品约束

1. 原始 DICOM 永久不可变，修改和回滚都生成新文件与新版本。
2. QIDO、WADO、C-FIND 和工作列表只读取当前版本，历史版本只从修订历史访问。
3. 每次派生重映射 Study、Series、SOP Instance UID，并更新对象内引用 UID。
4. PixelData 的语义哈希在转换前后必须一致。
5. 技术员与管理员可修改白名单标签；放射科医生只能查看修订历史；viewer 为只读。
6. PatientID 变更必须先检查机构内冲突，禁止隐式合并病人。
7. 预览锁定基础版本，确认时基础版本变化则拒绝执行并要求重新预览。
8. 版本激活、当前投影更新和审计写入必须在同一个数据库事务内完成。
9. 回滚不是切换旧指针，而是从指定历史版本生成新的当前修订。
10. 所有查询、任务和审计必须按机构隔离。

### 临床标签白名单

| 层级 | 允许修改的标签 |
|---|---|
| Patient | PatientName、PatientID、IssuerOfPatientID、PatientBirthDate、PatientSex |
| Study | AccessionNumber、StudyID、StudyDescription、ReferringPhysicianName |
| Series | SeriesDescription、SeriesNumber、BodyPartExamined、ProtocolName |

允许的动作只有 `replace`、`empty` 和 `remove`。服务端按 DICOM VR/VM 校验全部输入；
前端校验只用于即时反馈，不能代替服务端校验。

### 数据模型

- `dicom_instance_versions`：逻辑实例、版本号、来源版本、派生 UID、存储路径、SHA-256、
  元数据快照、创建人、原因和时间；
- `instances.logical_instance_id` 与 `instances.current_version_id`：稳定身份和当前版本；
- `dicom_transform_jobs`：仅允许 `clinical_correction`、`rollback` 两种模式；
- `dicom_transform_items`：每个任务的来源版本、输出版本、文件和状态；
- `series.protocol_name`：补齐手工修订白名单需要的当前投影字段。

迁移 `0006_remove_deidentification_and_normalization.sql` 清理旧的脱敏项目、伪名映射、
转换模板、Calling AE 策略、归档字段以及自动重试/串行化字段。迁移 `0003` 至 `0005`
不改写，以保证已经部署的数据库迁移历史稳定。

### 执行流程

1. 预览目标实例，校验规则，生成任务级 UID 图、逐标签差异和确认 token。
2. 确认时校验用户、机构、token 有效期和所有基础版本。
3. 后台任务把全部输出写入 staging，重新解析并校验 UID 与 PixelData 哈希。
4. 全部文件成功后移动到不可变 `derived/<job-id>/...` 路径。
5. 单个数据库事务创建版本、更新当前投影、完成任务项并写审计。
6. 数据库激活失败时只清理该任务的未引用派生文件，绝不删除原始归档。
7. 任一实例失败都不能出现部分激活。

### HTTP 与 Viewer

保留的 API：

- `GET /api/dicom/schema`
- `POST /api/dicom/transformations/preview`
- `POST/GET /api/dicom/transformations`
- `GET /api/dicom/transformations/{id}`
- `GET /api/dicom/instances/by-sop/{sop_uid}/revisions`
- `GET /api/dicom/instances/{logical_id}/revisions`
- `POST /api/dicom/instances/{logical_id}/rollback`

Viewer 保留编辑标签、差异确认、任务中心、修订历史和回滚入口。不存在脱敏按钮、下载
动作、Calling AE 设置或模板管理入口。

### 验收与故障注入

- 单元测试覆盖白名单、VR/VM、UID 引用图、PixelData 哈希和像素风险分类；
- 数据库/API 测试覆盖 backfill、当前版本唯一性、并发冲突、PatientID 冲突、机构隔离、
  权限矩阵、回滚生成新版本以及审计事务性；
- 在写文件、移动文件和数据库提交阶段注入失败，验证没有部分激活；
- 用 DCMTK 对派生文件执行 `dcmdump` 和 `storescu`/`storescp` 往返验证；
- 验证所有原始文件 SHA-256 不变，当前投影与 WADO 返回标签一致。

### 与 Orthanc 的差距及后续迭代

1. **协议完整性**：STOW-RS、C-MOVE/C-GET、Storage Commitment、更多 Transfer Syntax
   转码和更完整的 DICOMweb 批量能力；
2. **运维可靠性**：在线备份恢复、对象存储、配额、生命周期、主从/高可用、监控告警、
   存储一致性巡检和灾难恢复演练；
3. **生态扩展**：Orthanc 插件体系、Lua/回调、路由转发、外部工作列表、LDAP/OIDC；
4. **查询与管理**：管理后台、重复检查处理、病人合并/拆分和数据导入导出队列；
5. **临床 Viewer**：MPR/MIP、PET/CT 融合、SEG/RT/SR、挂片协议、报告工作流和校准显示。

后续优先级建议仍是“备份恢复、Storage Commitment、STOW-RS/C-MOVE”，再扩展高级
Viewer。标签修订不能代替归档完整性、灾备和协议互操作能力。

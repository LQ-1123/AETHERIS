# Remote PACS 系统功能总结

> 文档基线：2026-08-05，基于 `main` 分支当前代码（包含共享标注、CT 测量和 DICOM Tag 版本化修订）。本文只描述已经落地的能力；仅预留或尚未实现的项目统一列在文末。

## 一、系统总体架构

Remote PACS 采用“Rust 服务端 + PostgreSQL + 文件归档 + Tauri 桌面客户端”的架构。`pacsd` 是唯一持有数据库凭据的进程，同时提供 DIMSE 服务和 HTTPS API；影像设备通过 DIMSE 发送或查询影像，Viewer 只通过 HTTPS 访问服务端，不直接连接 PostgreSQL。服务端按 `pacs-core`、`pacs-store`、`pacs-db`、`pacs-dimse`、`pacs-auth`、`pacs-web`、`pacs-codec` 等 Rust crate 拆分领域逻辑，客户端使用 Tauri 2、Rust、TypeScript、Vite 和 Canvas 实现桌面阅片。

## 二、服务端功能

### 1. DICOM 连通性检查（C-ECHO SCP）

系统可以接收影像设备或 DCMTK `echoscu` 发起的 C-ECHO 请求，用于确认 PACS 的 AE Title、端口和 DIMSE 协议栈是否可用。该功能由 `dicom-ul` 完成 Association 与 PDU 传输，自研的 `pacs-dimse` 负责 DIMSE 命令集解析、Presentation Context 协商和 C-ECHO-RSP 状态响应；命令集始终按 DICOM 标准要求使用 Implicit VR Little Endian 解码。

### 2. DICOM 影像接收（C-STORE SCP）

系统可以接收 CT、MR、CR、DX 等标准存储 SOP Class 的 DICOM 实例，并在成功响应前完成文件持久化和数据库入库。`pacs-dimse` 接收 C-STORE 数据集，`pacs-core` 校验 Study、Series、SOP UID 并提取元数据，`pacs-store` 先写临时文件、执行 `fsync`、原子 `rename` 并同步目录，`pacs-db` 再用 PostgreSQL 事务写入病人、检查、序列和实例索引；只有两步都成功后才返回 `0x0000`，从而避免设备收到成功后删除源文件却造成 PACS 数据丢失。

### 3. 原始影像保真和幂等入库

接收后的原始 DICOM 数据集字节不会先解码再编码，服务端只补齐 Part 10 文件元信息，因此可保留发送方的原始传输语法和像素数据。文件使用 Study UID 的 SHA-256 前两字节做两级哈希分片，再按 Study/Series/SOP UID 组织目录；相同 SOP UID、相同内容的重传按幂等成功处理，相同 SOP UID 但内容不同则拒绝覆盖不可变原件。实现主要使用 Tokio 异步文件 I/O、SHA-256、严格 UID 类型和 PostgreSQL 唯一约束。

### 4. DICOM 元数据分层索引

系统将 DICOM 元数据整理为 Institution、Patient、Study、Series、Instance 五层关系，保存患者姓名和 ID、检查日期和描述、Accession Number、模态、序列描述、实例号、传输语法、文件大小和哈希等字段。`pacs-core` 基于 `dicom-rs` 从数据集提取领域模型，`pacs-db` 使用 SQLx 将其事务化写入 PostgreSQL，并通过唯一约束、外键和索引保证层级关系与查询性能；原始文件仍保存在文件系统，数据库只保存可检索的临床投影和相对存储路径。

### 5. DICOM 字符集容错和中文乱码修复

系统不会完全信任 `(0008,0005) SpecificCharacterSet` 的声明，而是在 `dicom-rs` 正常解码后进行第二层校验。`pacs-core::text` 支持单值和多值 ISO-2022 声明、日文和韩文扩展，并可从可逆中间结果恢复原始字节，再保守尝试严格 UTF-8 与 GB18030；只有候选结果通过字节合法性、替换字符和 CJK 可信度判断时才覆盖声明结果，最后清除非法控制字符并将内存文本统一为 UTF-8。该处理只规范化数据库、查询响应和派生修订中的文本，不会改写已经归档的原始 DICOM 文件。

### 6. DIMSE 分层查询（C-FIND SCP）

系统支持 Patient Root 和 Study Root 查询模型的 C-FIND，可在 Patient、Study、Series、Image 层级返回多条 Pending 响应并以最终状态结束。查询逻辑按 DICOM VR 区分精确匹配、通配符匹配和日期/时间范围匹配，患者姓名使用规范化字段检索；不支持的匹配键会通过 `0xFF01` 明确提示，而不是静默假装过滤成功，结果过多则要求调用端收窄查询。协议层由自研 `pacs-dimse` 实现，查询解析由 `pacs-core` 完成，SQL 由 SQLx 参数化执行。

### 7. DICOMweb 查询（QIDO-RS）

系统提供检查、序列和实例三级 QIDO-RS 查询接口，支持查询条件、`limit`、`offset` 分页以及 DICOM JSON Model 响应。HTTP 层使用 Axum，复用 C-FIND 的查询模型和 PostgreSQL 索引，URL 中的 UID 会先通过同一套安全 UID 类型校验；没有结果时返回 204，不支持的参数通过 HTTP Warning 头报告。全部接口要求 HTTPS Bearer Token 和 `ViewImages` 权限，并按登录账号的 Institution 隔离数据。

### 8. DICOMweb 取回（WADO-RS）

系统可以按 Study UID、Series UID 和 SOP UID 获取完整 DICOM 实例、DICOM 元数据或指定帧。`pacs-web` 负责 WADO-RS 路由、Accept 处理和鉴权，`pacs-db` 定位当前有效版本，`pacs-store` 将受控相对路径解析到存储根，帧级请求再通过 `pacs-codec` 和 `dicom-pixeldata` 解码；接口使用 Rust/Axum/Reqwest 兼容的标准 HTTP 响应，供 Viewer 下载完整序列。

### 9. PACS 病人工作列表

系统提供面向 Viewer 的病人、检查和序列工作列表，可按患者姓名或 Patient ID 搜索，并支持分页、展开病人检查和继续展开序列。该接口不是让客户端拼装原始 QIDO JSON，而是由 `pacs-db::worklist` 直接聚合 PostgreSQL 中的层级数据和实例数量，再由 Axum 返回适合界面展示的 JSON；所有查询都使用登录用户的 Institution ID 作为数据边界。

### 10. 账号登录和会话管理

系统支持用户名密码登录、短期 Access Token、Refresh Token 轮换和退出时吊销令牌链。密码使用 Argon2 哈希，Access Token 使用 HS256 JWT，Refresh Token 只在数据库保存 SHA-256 哈希；旧 Refresh Token 被重复使用时可识别为令牌重放并吊销整条轮换链。Viewer 只接受 HTTPS 地址，并要求用户选择服务端自签 CA 证书，由 Rustls/Reqwest 建立受信连接。

### 11. 固定角色权限和机构隔离

系统定义 Admin、Radiologist、Technician、Viewer 四种固定角色，并集中映射查看影像、上传、报告、用户管理、审计、删除、Tag 修改和修订历史等权限。当前实际接入的主要权限包括影像查看、Tag 修改和修订历史：Admin 与 Technician 可以修改 Tag，Admin、Technician 与 Radiologist 可以查看修订历史，所有能查看影像的角色都可使用共享标注。权限由 Axum 中间件在整棵路由上校验，数据库查询同时带 Institution ID，避免桌面客户端接触数据库凭据或跨机构读取 HTTP 数据。

### 12. 审计记录

系统使用 PostgreSQL `audit_log` 保存用户名快照、时间、动作、结果、患者/检查/序列/实例标识和附加 JSON 信息。当前登录、令牌刷新、部分账号安全动作、DICOM 修订任务以及共享标注创建和修改已经接入审计；关键修订的版本激活与审计写入处于同一个数据库事务，审计失败会使临床投影更新一并回滚。实现位于 `pacs-auth::audit` 和 `pacs-db::transformations`。

### 13. DICOM Tag 手工修订

系统支持在患者、检查或序列层级批量修订白名单内的 DICOM Tag，并可选择替换、置空或删除。当前白名单包含 PatientName、PatientID、IssuerOfPatientID、PatientBirthDate、PatientSex、AccessionNumber、StudyID、StudyDescription、ReferringPhysicianName、SeriesDescription、SeriesNumber、BodyPartExamined 和 ProtocolName；前端先读取服务端 schema，再按目标层级生成编辑表单。服务端使用 `dicom-rs` 在内存中修改对象，执行 VR、长度、日期、性别和层级校验，受保护标签和 PixelData 不允许直接修改。

### 14. Tag 修订预览、确认和后台执行

每次 Tag 修改必须填写原因并先生成预览，预览会汇总旧值、新值、受影响的病人/检查/序列/实例数量和像素风险，再签发 15 分钟有效的一次性确认令牌。确认后任务进入 PostgreSQL 队列，由 `pacs-web` 后台 Worker 执行；任务会检查源版本是否仍为当前版本，防止用户基于过期数据覆盖别人的修改，并在任务列表中暴露 Previewed、Queued、Running、Succeeded、Failed 或 Blocked 状态。

### 15. 不覆盖原件的版本化修订

Tag 修改不会覆盖原始归档，而是为每个逻辑实例写入新的不可变派生文件和 `dicom_instance_versions` 版本记录。任务会为受影响的 UID 图一致地生成新 UID、递归更新引用 UID、写入 Source Image Sequence 和 Derivation Description，同时对修改前后的 PixelData 计算 SHA-256，像素发生变化就阻止激活；派生文件先进入暂存区，随后与数据库当前版本指针、临床查询投影和审计记录一起事务化激活。这套机制使用 UUID 逻辑实例标识、PostgreSQL 乐观并发控制、文件原子移动和不可变版本链实现。

### 16. 修订历史和回滚

系统可以按当前 SOP UID 或逻辑实例查看完整版本历史，包括版本号、来源版本、派生类型、修改原因、创建人和时间。回滚同样必须填写原因、预览并确认，它不会删除后续历史或把旧文件直接改成当前文件，而是以选定历史版本为来源创建一个新的 Rollback 版本，因此整个修改轨迹始终可追溯。服务端通过版本表和当前版本指针实现，Viewer 提供修订历史与回滚对话框。

### 17. 共享阅片标注服务

系统将长度、箭头、椭圆 ROI、矩形 ROI、角度和 CT 点探针标注保存到独立的 `viewer_annotations` 表，不修改 DICOM 文件。原始二维图像使用 SOP UID、帧号和图像坐标，MPR 使用 Axial/Coronal/Sagittal 平面和患者空间坐标；记录带 Institution、创建人、修改人、Revision、Schema Version、软删除时间和更新时间。更新时客户端必须提交期望 Revision，冲突返回 HTTP 409，从而防止多人覆盖；服务端使用 PostgreSQL JSONB 保存几何结构，并为创建、更新、删除和恢复写入审计。

### 18. TLS、配置和服务启动

`pacsd` 从 `.env` 或进程环境读取数据库地址、存储根、DIMSE 监听地址、AE Title 和 HTTPS 地址，启动时自动执行 SQLx 数据库迁移、清理临时文件，并在缺少证书时用 `rcgen` 生成本地 CA 和服务器证书。DIMSE 和 HTTPS 由 Tokio 并发运行，HTTPS 使用 Axum Server 与 Rustls；默认只监听 `127.0.0.1:11112` 和 `127.0.0.1:8443`，绑定到非回环地址时会给出安全警告。首个管理员账号可以通过 `cargo run -p pacsd -- admin ...` 命令创建。

## 三、桌面 Viewer 功能

### 19. 本地 DICOM 文件和序列打开

Viewer 可以直接选择一个或多个本地 DICOM 文件，不依赖 WADO-RS 下载或数据库查询，支持单文件单帧、单文件多帧和同一 Series 的多文件灰度序列。Tauri Rust 后端使用 `dicom-rs` 解析文件并返回统一元数据，TypeScript 前端负责状态与界面；多文件序列会校验 Study/Series UID、尺寸、方向和患者空间几何，缺少可靠几何时明确拒绝，不会用文件名或 InstanceNumber 猜测 CT 切片顺序。

### 20. 远程登录、工作列表和序列下载

Viewer 登录后会显示可搜索、分页和逐层展开的病人工作列表，点击序列即可查询全部实例 UID、逐个通过 WADO-RS 下载，并在本地临时目录中打开完整序列。Rust 侧 `remote.rs` 使用 Reqwest、Rustls、自签 CA、JWT 自动刷新和下载取消标记，TypeScript 侧显示实时下载进度；退出或关闭序列后，Tauri 持有的临时目录随资源句柄释放，客户端始终不直接访问数据库。

### 21. 图像组识别和安全排序

同一 Series 中若混有定位像、不同方向或不同尺寸，Viewer 会按 ImageOrientationPatient、ImagePositionPatient、法向量和尺寸拆分为多个可选择图像组，默认打开帧数最多的主堆栈。组内切片按患者空间法向投影排序，并对重复位置、不规则间距和不一致几何给出警告；该功能由 `pacs-core::geometry` 的向量计算与 Tauri `state.rs` 的分组逻辑完成，是 MPR 和准确空间测量的基础。

### 22. DICOM 灰度显示管线

Viewer 按“Stored Value → Rescale Slope/Intercept → VOI 窗宽窗位 → MONOCHROME1/2 光度处理 → 屏幕灰度”的顺序显示影像，支持 8 位和 16 位灰度以及 `dicom-pixeldata` 可解码的压缩传输语法。`pacs-codec` 将显示运算预计算为查找表，Rust 后端提供原始帧字节和 LUT，前端 Canvas 将每个存储值映射为 8 位灰度；这种实现既保证 CT HU 与窗宽窗位正确，也避免每次鼠标移动都重复执行完整浮点管线。

### 23. 二维序列浏览

二维阅片支持滚轮切片、上一帧/下一帧按钮、滑条跳转和键盘导航，并显示当前帧数、图像尺寸、实例号、患者信息、检查信息、模态、描述和像素间距可信度。缩放采用 `Ctrl/Cmd + 滚轮` 的光标锚定算法，平移支持工具按钮和中键拖动，重置按钮恢复初始适配状态；影像和标注分别使用两个叠加 Canvas，视图变换由共享坐标矩阵统一处理。

### 24. 窗宽窗位和窗预设

用户可以用鼠标拖动实时调整 Window Center 与 Window Width，也可以从 DICOM 自带 WindowCenter/WindowWidth 多值中选择预设，并使用内置 CT 预设一键切换常见窗。前端只更新 ViewState 并请求或复用 LUT，Canvas 重新映射当前帧，不会修改原始像素；当前 WL、WW 会实时显示在工具栏，重置视图时恢复初始显示参数。

### 25. 反色、翻转、旋转和标注显隐

Viewer 支持一键反色、水平翻转、垂直翻转、顺时针或逆时针旋转 90 度，以及单独显示或隐藏标注。图像方向操作保存在视图状态中，由 Renderer 的 Canvas 变换矩阵组合实现，标注层复用同一矩阵，因此测量点和图像在缩放、平移、翻转及旋转后仍保持对齐；这些操作仅影响当前显示，不改写归档影像。

### 26. 多平面重建（MPR）

对于具有可靠 ImagePositionPatient、ImageOrientationPatient 和像素间距的规则薄层序列，Viewer 可以构建 Axial、Coronal、Sagittal 三个联动视图。Tauri Rust 后端把切片解码为体数据，以患者坐标系建立三个正交平面，并用三线性插值按需重采样；前端显示三视图、方向标记、切片计数和联动十字线，拖动十字线或滚轮切换任一平面时会同步更新另外两个平面。几何不规则或切片位置重复时会拒绝 MPR，避免生成看似正常但空间错误的重建图像。

### 27. 长度和角度测量

长度工具通过两个控制点计算距离，角度工具通过三个点计算夹角，完成后可继续选中、移动和调整端点。距离计算由 `pacs-core::spacing` 区分三种可信度：CT 等断层影像使用已标定 PixelSpacing 输出毫米，投影影像可能显示“探测器平面、未校正”的毫米值，没有可靠间距时只显示像素；角度基于图像或患者空间向量计算，不受画布缩放比例影响。

### 28. 箭头、椭圆和矩形标注

Viewer 提供箭头、椭圆 ROI 和矩形 ROI 工具，标注完成后可以单击选中、整体移动、拖动控制柄改变大小，也可以按 Delete/Backspace 删除。标注模型以图像坐标或患者坐标保存几何，而不是保存屏幕像素，Renderer 每次根据当前缩放、平移、翻转和旋转重新投影到覆盖层 Canvas；图标使用 Lucide，交互和命中测试由 TypeScript `annotations.ts` 与 `app.ts` 实现。

### 29. CT 点值与 ROI 统计

CT 点探针读取光标所在位置的原始 Stored Value，再应用 Rescale Slope/Intercept 得到 CT 值；椭圆和矩形 ROI 会计算像素数、平均值、标准差、最小值、最大值和面积。计算在 Tauri Rust 后端对原始二维帧或 MPR 的浮点重采样切片执行，不使用已经窗宽窗位映射后的 8 位显示灰度，因此结果不随当前窗设置变化；存在物理间距时面积使用平方毫米，否则明确退回像素面积。

### 30. 标注撤销、重做和批量清理

Viewer 对创建、移动、缩放、删除以及清除标注提供有界 Undo/Redo 历史，支持工具栏按钮和 `Ctrl/Cmd+Z`、`Ctrl/Cmd+Shift+Z`。清除当前图像和清除整个序列会先确认，序列级清除在历史中作为一次原子操作，因此一次撤销即可恢复全部被清除标注；该逻辑由 TypeScript 命令快照实现，并在帧切换和 2D/MPR 模式间维持一致状态。

### 31. 共享标注自动同步和冲突保护

远程 PACS 序列中的标注会自动保存到服务端，客户端每 5 秒按更新时间增量刷新其他用户的修改，并提供失败后的手动重试按钮。创建时使用客户端 UUID，修改、删除和恢复时携带服务器 Revision，409 冲突会提示用户刷新而不会静默覆盖；本地打开的 DICOM 没有服务器资源标识，因此标注明确显示“未同步”并仅在当前会话保存。MPR 标注采用患者空间坐标，所以能在不同客户端的三平面重建中稳定复现。

### 32. Viewer 内的 Tag 编辑与任务管理

具备权限的用户可以直接从病人、检查或序列条目打开 Tag 编辑器，填写修改项和原因，查看差异预览后确认任务；任务面板可以查看处理进度和失败信息。Viewer 通过 Tauri Command 调用 Rust 远程客户端，再访问 `/api/dicom` 的版本化修订 API，前端不会自行改写 DICOM 文件；权限不足时入口隐藏，最终权限仍由服务端再次校验。

### 33. Viewer 内的修订历史与回滚

打开远程序列后，Viewer 可按当前帧的 SOP UID 查询逻辑实例修订历史，展示每个版本的来源、操作者、原因和时间，并可选择历史版本执行“预览回滚”和“确认回滚”。该界面复用与 Tag 修改相同的一次性确认机制，服务端仍会创建新版本而不是破坏历史，因此前端看到的当前实例会在任务完成并重新加载后切换到新的 SOP UID。

### 34. Viewer 性能和资源控制

帧数据通过 `pacs-frame://` Tauri 自定义协议直接以二进制传给 WebView，避免 JSON/Base64 膨胀；前端维护约 128 MiB 的 LRU 帧缓存并预取当前帧前后各两帧，Rust 后端维护约 512 MiB 的解码帧缓存。像素解码和 MPR 数值计算放在阻塞线程或 Rayon 并行任务中，不占用异步网络执行器；请求版本号用于丢弃过期帧响应，系列关闭和 MPR 取消会释放缓存及临时文件。

## 四、主要技术组成

| 层级 | 当前技术 |
|---|---|
| 服务端运行时 | Rust 2024、Tokio、Axum、Tower |
| DICOM 协议与对象 | `dicom-rs 0.10`、`dicom-ul`、自研 DIMSE 服务类 |
| 像素解码 | `dicom-pixeldata`、OpenJPEG、`pacs-codec` LUT |
| 数据库 | PostgreSQL 14、SQLx、事务、JSONB、数据库迁移 |
| 文件归档 | Tokio File I/O、`fsync`、原子 `rename`、SHA-256 哈希分片 |
| 认证与传输 | Argon2、JWT HS256、Refresh Token 轮换、Rustls、自签 CA |
| 桌面客户端 | Tauri 2、Rust、TypeScript、Vite、HTML Canvas、Lucide Icons |
| 测量与 MPR | 患者空间几何、向量计算、三线性插值、Rayon、Canvas 覆盖层 |
| 自动化验证 | Rust 单元/集成测试、真实 PostgreSQL、DCMTK 互操作、Vitest、Clippy |


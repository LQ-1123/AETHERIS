<p align="center">
  <img src="./logo.jpg" width="112" alt="AETHERIS 标志">
</p>

<h1 align="center">AETHERIS</h1>

<p align="center">
  <strong>从影像接入到审核签发，让医学影像工作流完整连通。</strong><br>
  自托管 PACS · 原生桌面阅片器 · 临床工作流 · 本地 AI
</p>

<p align="center">
  <a href="https://github.com/LQ-1123/AETHERIS/releases">下载安装</a>
  ·
  <a href="doc/releases/v0.3.0.md">发布说明</a>
  ·
  <a href="doc/api-reference.md">API 参考</a>
  ·
  <a href="README.md">English</a>
</p>

<p align="center">

![Release](https://img.shields.io/badge/Release-v0.3.0-58b8c7)
![Rust](https://img.shields.io/badge/Rust-1.97%2B-e7e9eb?logo=rust&logoColor=white)
![Tauri](https://img.shields.io/badge/Tauri-2-58b8c7?logo=tauri&logoColor=white)
![PostgreSQL](https://img.shields.io/badge/PostgreSQL-14%2B-336791?logo=postgresql&logoColor=white)
![DICOM](https://img.shields.io/badge/DICOM-DIMSE%20%7C%20DICOMweb-58b8c7)
![Platform](https://img.shields.io/badge/Desktop-Windows%20%7C%20macOS-e7e9eb)
![License](https://img.shields.io/badge/License-MIT-58b8c7)

</p>

[English](README.md) · [简体中文](README.zh-CN.md)

---

## 产品一览

**AETHERIS** 是一套以 Rust PACS 核心和 Tauri 原生桌面应用为基础的自托管医学影像平台。它把 DICOM 接入、持久存储、患者检查队列、诊断可视化、检查申请单、报告书写、独立审核、机构管理和本地 AI 集成在同一套机构级系统中。

| 产品界面 | 已交付工作流 |
| --- | --- |
| **PACS 核心** | DIMSE / DICOMweb 接入、元数据索引、影像对象持久存储、认证、审计、路由、生命周期任务和临床 API |
| **桌面阅片器** | 患者队列、本地 DICOM 打开、2D 阅片、多序列分屏、GPU 斜切 MPR、MIP/MinIP、体渲染、测量、标注、Mask 和本地 AI 分割 |
| **临床工作台** | 检查申请、Study 匹配、独立报告窗口、草稿提交、独立审核、不可变签发版本、账号管理和工作量统计 |

Viewer 专注影像显示与交互；工作流状态和权限由服务端统一管理，并在机构范围内隔离和审计。

## 最新版本 v0.3.0

v0.3.0 将患者检查队列、检查申请、影像阅片、报告书写、独立审核、账号管理和工作量统计连成一套完整桌面工作流。

| 平台 | 安装包 | SHA-256 |
| --- | --- | --- |
| macOS Apple Silicon | [AETHERIS_0.3.0_aarch64.dmg](https://github.com/LQ-1123/AETHERIS/releases/download/v0.3.0/AETHERIS_0.3.0_aarch64.dmg) | `f454761759d07acca4bccaf9d0a1af447425a9edaaebbf12c98719d8782dfa78` |
| Windows 10/11 x64 | [AETHERIS-Setup-0.3.0-x64.exe](https://github.com/LQ-1123/AETHERIS/releases/download/v0.3.0/AETHERIS-Setup-0.3.0-x64.exe) | `a926a5c479071f6b9d41722fa3a9c6915047e3b733ce98e86e198df4b614ea67` |

两个桌面安装包都包含 Viewer、`pacsd` 和本地 PostgreSQL 运行环境。完整变更与验证记录见 [v0.3.0 发布说明](doc/releases/v0.3.0.md)。

## 临床工作流

<p align="center">
  <img src="doc/diagrams/readme/clinical-workflow.svg" width="100%" alt="AETHERIS 从检查申请到报告审核签发的临床工作流">
</p>

同一个 Study 身份贯穿完整流程：创建申请单、接收或匹配影像、诊断阅片、报告草拟、提交审核、审核签发、申请单完成和工作量入账。

### 患者队列与检查申请单

患者检查队列是桌面端的业务入口。它支持服务端分页、排序，以及患者、日期、模态、部位、报告状态和来源机构组合筛选。双击检查即可在 Viewer 中打开完整的检查与序列上下文。

检查申请单支持两种业务顺序：

- 先创建申请单，随后接收影像，再由技师手动匹配检查。
- 直接从已有检查创建申请单；患者信息和 Study 关联由服务端解析。

申请单记录模态、部位、检查类型、临床指征、预约信息和关联检查，并按待执行、已执行、已完成状态留痕。

<table>
  <tr>
    <td width="50%"><img src="doc/screenshots/queue-page-desktop.png" alt="带筛选条件和报告状态的患者检查队列"></td>
    <td width="50%"><img src="doc/screenshots/03-new-request-form.png" alt="检查申请单创建表单"></td>
  </tr>
  <tr>
    <td align="center"><sub>患者检查队列 · 筛选、排序、报告状态和来源机构</sub></td>
    <td align="center"><sub>检查申请单 · 患者、模态、部位、检查类型和临床指征</sub></td>
  </tr>
</table>

### 医学影像阅片

原生 Viewer 以同一套阅片工具处理本地文件和远程 PACS 检查：

- 窗宽窗位、窗预设、缩放、平移、序列播放、反色、翻转和旋转。
- 最多 3×3 多序列分屏，每个窗格维护独立交互状态。
- 基于患者物理空间的 GPU 斜切 MPR，联动横断、冠状和矢状面。
- MIP、MinIP、GPU 体渲染和真实物理尺度测量。
- 长度、角度、箭头、矩形、椭圆、ROI 统计和共享标注同步。
- 3D 稀疏 Mask 与本地 AI 分割，影像推理在本机执行。

<table>
  <tr>
    <td width="50%"><img src="doc/img/多窗口图像.png" alt="AETHERIS 多序列分屏阅片"></td>
    <td width="50%"><img src="doc/img/多角度MPR重建.png" alt="AETHERIS GPU 斜切 MPR"></td>
  </tr>
  <tr>
    <td align="center"><sub>多序列对比 · 独立窗格与统一检查上下文</sub></td>
    <td align="center"><sub>GPU 斜切 MPR · 可旋转参考线与患者空间三平面联动</sub></td>
  </tr>
  <tr>
    <td width="50%"><img src="doc/screenshots/volume-rendering.png" alt="AETHERIS GPU 体渲染"></td>
    <td width="50%"><img src="doc/screenshots/ai-segmentation.png" alt="AETHERIS 本地 AI 肺叶分割"></td>
  </tr>
  <tr>
    <td align="center"><sub>GPU 体渲染 · 窗宽窗位、旋转、平移和缩放</sub></td>
    <td align="center"><sub>本地 AI 分割 · 可编辑 3D Mask 与定量结果</sub></td>
  </tr>
</table>

### 报告书写与独立审核

报告在独立桌面窗口中打开，影像交互不会被报告表单打断。一份报告对应一次检查，包含结构化影像所见、意见、建议、阳性标记、报告模板、关联申请信息和完整审核时间线。

机构开启审核闭环后，报告医生可以**保存草稿**或**提交审核**。拥有 `review_report` 权限的审核人开始审核后，可以直接签发原报告，也可以修改后签发。系统保留报告医生、审核医生、审核意见、审计事件和不可变签发版本；报告作者不能审核自己的报告。

<table>
  <tr>
    <td width="50%" align="center"><img src="doc/screenshots/standalone-author-report.png" width="300" alt="带保存草稿和提交审核按钮的独立医生报告窗口"></td>
    <td width="50%" align="center"><img src="doc/screenshots/standalone-reviewer-report.png" width="300" alt="带审核和签发操作的独立审核报告窗口"></td>
  </tr>
  <tr>
    <td align="center"><sub>报告医生工作台 · 保存草稿 / 提交审核</sub></td>
    <td align="center"><sub>审核医生工作台 · 无需修改直接签发 / 修改后签发</sub></td>
  </tr>
</table>

### 管理控制台与工作量

管理控制台集中提供机构范围内的设备、账号、密码重置审核、来源归属、用户权限、工作量和报告审核设置。

- 创建、启用、停用机构账号并吊销会话。
- 设置首次登录强制修改密码。
- 用户在登录页提交用户名和新密码；管理员只能批准或驳回请求，无法查看密码内容。
- 注册 DICOM 设备、绑定来源机构，并向用户授予设备可见范围。
- 授予报告审核权限并开启机构报告审核闭环。
- 按用户和日期统计草稿、待审核、审核中、签发版本、完成审核、审核修改和申请单数量，并导出 CSV。

<table>
  <tr>
    <td width="50%"><img src="doc/screenshots/11-admin-workload-report.png" alt="AETHERIS 管理员工作量报表"></td>
    <td width="50%"><img src="doc/screenshots/admin-password-reset-review.png" alt="AETHERIS 密码重置审核队列"></td>
  </tr>
  <tr>
    <td align="center"><sub>工作量报表 · 报告、审核、版本和申请单统计</sub></td>
    <td align="center"><sub>密码重置审核 · 管理员审批请求但不读取新密码</sub></td>
  </tr>
</table>

## 系统架构

<p align="center">
  <img src="doc/diagrams/readme/system-architecture.svg" width="100%" alt="AETHERIS 系统架构">
</p>

Rust 服务统一承载 DIMSE、DICOMweb、认证和临床 API。PostgreSQL 保存影像索引与工作流元数据，影像对象存储保留 DICOM 数据集，后台 Worker 执行路由、生命周期、传输、修订和 AI 任务。桌面端通过认证后的 HTTPS 访问服务边界，数据库连接信息只存在于服务端。

## 医学影像数据流

<p align="center">
  <img src="doc/diagrams/readme/imaging-data-flow.svg" width="100%" alt="AETHERIS DICOM 医学影像数据流">
</p>

接收的 Part 10 对象先进入持久存储，再写入患者、检查、序列和实例层级索引。QIDO-RS 提供元数据查询，WADO-RS 向 Viewer 返回对象与帧。可视化、报告、标注和本地 AI 都使用服务端解析的 Study 身份，原始 DICOM 对象始终作为持久数据源保留。

## 安全与权限边界

<p align="center">
  <img src="doc/diagrams/readme/security-boundary.svg" width="100%" alt="AETHERIS 权限与安全边界">
</p>

所有安全决策都在服务端边界执行：

- TLS 保护 HTTP 端点，用户会话使用签名令牌。
- 密码采用 Argon2id 哈希，密码重置请求由服务端审核流处理。
- 固定角色配合显式权限授权，保护敏感操作。
- 患者队列、DICOMweb 取回、报告、申请单、设备、任务和管理 API 全部执行机构过滤。
- 影像进入机构范围前校验设备身份和来源归属。
- 认证、数据访问、报告审核、密码重置、设备、权限、路由、修订和生命周期操作全部写入审计事件。

## 标准与互操作

此处只列出已经交付的协议能力。

| 协议族 | 服务 | 已交付行为 |
| --- | --- | --- |
| DIMSE | C-ECHO SCP | 连通性验证 |
| DIMSE | C-STORE SCP | 持久接收 DICOM 对象并协商传输语法，包括 RLE Lossless |
| DIMSE | C-FIND SCP | 患者、检查、序列和实例层级查询 |
| DICOMweb | QIDO-RS | 认证后按机构范围查询元数据 |
| DICOMweb | WADO-RS | 认证后取回对象、实例、元数据和帧 |
| DICOMweb | STOW-RS | Multipart DICOM Part 10 接入 |

服务端保留原始 DICOM 字节，兼容常见字符集差异，执行幂等索引，并在持久存储路径成功后才确认 C-STORE 对象接收。

## 安装与运行

### 桌面安装包

从 [GitHub Releases](https://github.com/LQ-1123/AETHERIS/releases) 下载当前 DMG 或 EXE。桌面安装包通过 AETHERIS 应用初始化并启动本地 PACS 运行栈：

| 平台 | 应用数据 |
| --- | --- |
| macOS Apple Silicon | 应用程序包和随应用分发的本地服务运行栈 |
| Windows 10/11 x64 | 程序位于 `C:\Program Files\AETHERIS`；数据库、影像、配置和日志位于 `C:\ProgramData\AETHERIS` |

### Docker 服务栈

Docker Compose 会启动 PostgreSQL、`pacsd`、持久 DICOM 存储和 DCMTK 模拟器：

```bash
cp .env.example .env
# 设置强 POSTGRES_PASSWORD、PACS_ADMIN_PASSWORD 和 PACS_JWT_SECRET。
docker compose up -d --build
docker compose logs -f pacsd
```

服务端点：

- PACS API 与 DICOMweb：`https://127.0.0.1:8443`
- DIMSE SCP：`127.0.0.1:11112`，AE Title 为 `REMOTE_PACS`
- API 检测页面：`https://127.0.0.1:8443/api-checker`
- DCMTK 模拟器：`http://127.0.0.1:8787`

### 开发运行

环境要求：Rust 1.97+、Node.js 20+、PostgreSQL 14+、`libarchive` 以及 Tauri 2 对应平台依赖。

启动服务端：

```bash
cp .env.example .env
# 在 .env 中配置 DATABASE_URL、PACS_JWT_SECRET 和存储路径。
cargo run -p pacsd
```

在另一个终端启动桌面应用：

```bash
cd apps/viewer
npm ci
npm run tauri dev
```

构建桌面分发包：

```bash
cd apps/viewer
npm ci
npm run tauri build
```

Windows 一体化安装器可通过手动触发的 [Build Windows Installer](.github/workflows/build-windows.yml) 工作流构建，也可在准备好的 Windows 主机上运行 `packaging/windows/build.ps1`。

## 工程验证

仓库 CI 执行格式检查、Lint、单元与集成测试、PostgreSQL 数据库测试和 DCMTK 真实互操作流量。v0.3.0 发布记录同时包含桌面前端测试和安装包完整性验证。

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

cd apps/viewer
npm ci
npm run build
npm test
```

## 文档

- [v0.3.0 发布说明](doc/releases/v0.3.0.md)
- [临床与管理 API 参考](doc/api-reference.md)
- [系统介绍与工程设计](doc/remote-pacs-system-introduction.md)
- [已实现功能总结](doc/system-function-summary.md)
- [DCMTK 测试平台集成](doc/dcmtk-test-platform-integration.md)
- 运行时 API 检测页面 `/api-checker`

## 使用边界

AETHERIS 面向医学影像研究、工程验证和受控机构部署。临床使用需要由部署机构完成系统验证、安全配置、操作规程和法规审查。请按照适用的医疗数据制度保护账号、TLS 密钥、数据库、DICOM 存储、导出文件和界面截图。

## License

[MIT](LICENSE)

# Next Plan · v0.4.0 — DICOM 互联互通收尾（C-MOVE / C-GET）

日期：2026-08-19
状态：已定稿（决策点 1-5 全部拍板）
范围：`crates/pacs-dimse`（协议层）+ `crates/pacsd`（服务接线）+ `crates/pacs-web`（API）+ `apps/viewer`（管理界面）
对应路线图：README「DICOM Networking」表中 C-MOVE SCP 🚧 / C-GET SCP 🚧（Track A 遗留欠债）
叙事契合：v0.3.0 已移除客户端导入、唯一入库途径=DICOM 协议——C-MOVE/C-GET 补上「从外部 PACS 合规拉取」的主动进数据能力，且与 v0.3.0 的来源机构（institution_name）字段治理衔接。

---

## 1. 背景与目标

### 1.1 为什么现在做
- **兑现承诺**：README 的 DICOM 能力表 C-MOVE/C-GET 标 🚧 并注明「incoming requests are currently rejected (association aborted)」——这是 Track A 审计点名过的超前声明遗留（README 曾提前标 ✅，后改回 🚧）。v0.4.0 把它真正补上。
- **补齐「拉」的入库途径**：v0.3.0 砍掉客户端导入后，数据只有「对方主动推（C-STORE/STOW）」一条进路；C-MOVE/C-GET 让本院 PACS 能**主动从外部 PACS 拉取**（历史影像对比、转诊/会诊、分院影像中心取图），且拉到的数据天然走现有 C-STORE 入库链路（幂等、去重、来源归属），合规边界与 v0.3.0 一致。
- **工程量适中**：`pacs-dimse` 自研骨架已预留（命令码已定义、C-FIND 查询可复用、发起端有 c_echo/c_store 先例），是「在骨架上补服务类」而非从零造。

### 1.2 目标
1. **C-MOVE SCP**（接收他人拉取请求，把图推给指定目的地）——兑现 README 🚧 → ✅。
2. **C-GET SCP**（同连接返回图）——同上（实现简单，兼容性不如 MOVE，见 §3.2）。
3. **C-MOVE SCU**（主动向外部 PACS 发起拉取）+ 管理界面——业务价值核心。
4. **安全收口**：Move Destination 白名单，防「被第三方利用推送数据到任意地址」。

---

## 2. 现状盘点（已核实）

| 项 | 现状 | 证据 |
| --- | --- | --- |
| 命令码 | ✅ 已定义 `CGetRq (0x0010)` / `CMoveRq (0x0021)` | `pacs-dimse/src/command.rs:39-43` |
| SCP 对未知命令 | abort association（MOVE/GET 请求当前直接被拒） | `scp.rs:145-153` |
| SCP 已有服务类 | C-ECHO / C-STORE / C-FIND | `scp.rs:109-138` |
| C-FIND 查询处理 | ✅ QueryRetrieveLevel 匹配（MOVE 的查询语义复用） | `find.rs`（FindHandler/FindRequest） |
| SOP Class 解析 | ✅ Query/Retrieve SOP Class UID 可解析 | `sop_class.rs`（find_uids_resolve_to_query_retrieve…测试） |
| SCU 发起端 | ✅ 有 `c_echo` / `c_store`；❌ 无 find/move 发起端 | `client.rs:74/116` |
| 原始 DICOM 读取 | ✅ WADO-RS `retrieve_instance`（取回字节） | `pacs-web/src/wado.rs:48` |
| 落盘 | ✅ pacs-store（fsync 顺序，原字节保留） | `pacs-store/src/lib.rs` |
| 入库链路 | ✅ C-STORE → `ingest.rs`（幂等、来源归属、设备授权） | `pacs-web/src/ingest.rs` |
| 设备模型 | ✅ dicom_devices（AE/IP/状态）+ observed_dicom_peers（自动记录对端） | 迁移 0012/0013/0019 |
| DIMSE 服务接线 | ✅ pacsd `DimseServer::bind(ae_title, dimse_bind)` | `pacsd/src/main.rs:233-245` |
| 互操作测试工具 | ✅ CI 已装 DCMTK（echoscu/storescu；**movescu/findscu 可用于新测试**） | `.github/workflows/ci.yml` |

**结论**：协议层骨架完整，缺的是 MOVE/GET 两个服务类（SCP）+ find/move 发起端（SCU）+ 管理界面 + 安全白名单。

---

## 3. 设计

### 3.1 C-MOVE SCP（核心交付）

```
外部 SCU ──C-MOVE-RQ(查询条件 + MoveDestination=本院AE)──▶ C-MOVE SCP
                                                            │
                1. 解析查询（复用 C-FIND 匹配语义）
                2. 校验 MoveDestination ∈ 白名单（见 §3.4）
                3. 查询匹配的 Instance（study/series 级）
                4. 逐个：从库读出原始字节（复用 wado retrieve 逻辑）
                         → 另开连接到 Destination，c_store 推送
                5. 回 C-MOVE-RSP（Pending：已推送 n / 失败 m；成功 0x0000）
```

- **状态机**：接收 RQ → 校验 → 逐条推送（每条 Pending 响应）→ 完成（最终响应 0x0000 或 0xFE00 Failed 等错误码）。
- **CCancelRq**：SCP 侧已有取消命令码处理（`scp.rs:138`），MOVE 长任务须响应取消（停止推送，回 CANCELLED）。
- **查询层级**：支持 Study Root / Patient Root Query-Retrieve（`sop_class.rs` 已能解析），首期先做 Study Root（匹配 study 拉全部序列）。
- **数据读取**：从存储层取原始字节（WADO-RS 同源），不重新编码（保持「Preserve original DICOM bytes」原则）。

### 3.2 C-GET SCP（次优先级）

- 与 C-MOVE 的区别：数据**在同一 association 内返回**（不需要另开连接到 Destination）。
- 实现更简单（无 Destination 校验、无多连接），但**兼容性差**（大量 PACS 只实现 MOVE 不实现 GET）。
- 工作量小（约 C-MOVE 的 1/3），建议与 MOVE 同批实现，README 一并勾掉。

### 3.3 C-MOVE SCU + 管理界面（业务价值核心）

**主动拉取的数据路径（关键：零新入库代码）**：
```
管理员在界面配置外部 PACS（AE/IP，保存到 dicom_devices 或新表 external_pacs）
  → C-FIND（需新增 find 发起端）查询匹配检查
  → 发起 C-MOVE-RQ，MoveDestination = 本院 AE
  → 外部 PACS 用 C-STORE 把图推回来 → 走现有 ingest 入库链路（幂等/去重/来源归属）
```

- **新增发起端**：`client.rs` 增 `c_find`、`c_move`（仿 `c_echo`/`c_store`）。
- **外部 PACS 配置**：复用 `dicom_devices`（已含 AE/IP/status；`observed_dicom_peers` 可辅助发现对端）还是新表 `external_pacs`？——决策点 3。
- **前端**：管理控制台新增「外部 PACS / 拉取」tab：对端配置（AE/IP/端口）+ 查询（患者/日期/模态）+ 拉取任务列表与进度（Pending 计数）+ 结果（入库检查数）。
- **拉取权限**：仅 admin 可配置对端与发起拉取；拉取入库的数据沿用机构边界与设备授权（非授权用户不可见）。

### 3.4 安全设计（必须）

| 风险 | 对策 |
| --- | --- |
| 攻击者发 MOVE-RQ 让本院 PACS 往任意地址推数据（DICOM 滥用） | **Move Destination 白名单**：SCP 只接受 MoveDestination ∈ {本院 AE（回推自用）} ∪ {active 的 dicom_devices}；白名单外拒绝（0x0122 Move Destination Unknown） |
| 未授权发起拉取 | 仅 admin（管理界面）；SCP 侧鉴权沿用现有 association 授权模型（设备级） |
| 拉取数据越权可见 | 沿用 ingest 的来源归属 + user_device_grants 模型，无新暴露面 |
| 长任务资源占用 | MOVE 推送并发上限（如同时最多 4 个推送连接）+ 可取消（CCancelRq） |

### 3.5 与 v0.3.0 的衔接
- 拉取入库的检查出现在队列页，来源机构（institution_name）来自外部 PACS 的 `(0008,0080)`——来源医院治理已有字段承载，无需新设计。
- 拉取数据可走申请单「从已入库检查创建申请单」（v0.3.0 已支持），形成「拉图 → 建申请 → 报告 → 审核」完整链。

---

## 4. 决策点（需拍板）

| # | 决策点 | 结论（已拍板） |
| --- | --- | --- |
| 1 | 首期范围 | **同期做**：SCP（MOVE+GET）与 SCU（主动拉取+管理界面）同批实施 |
| 2 | 查询层级 | **按检查**：Study Root（拉取该检查全部序列）；Patient Root 后续 |
| 3 | 外部 PACS 配置存储 | **复用** `dicom_devices` + 新增 `is_retrieval_source` 标记 |
| 4 | C-GET 是否本期 | **做**（同批实现，README 一并勾掉） |
| 5 | 推送并发上限 | **4**（可调） |

---

## 5. 工作量与顺序

| 阶段 | 内容 | 预估 |
| --- | --- | --- |
| 1 | C-MOVE SCP（状态机 + 查询复用 + 存储读取 + 推送 + 取消 + Move Destination 白名单）+ 单测 | 1-1.5 天 |
| 2 | C-GET SCP（同连接返回）+ 单测 | 0.5 天 |
| 3 | SCU 发起端（c_find / c_move）+ pacsd 接线 | 0.5-1 天 |
| 4 | 管理界面（外部 PACS 配置 + 查询 + 拉取任务 + 进度）+ API | 1 天 |
| 5 | 互操作测试（DCMTK movescu/findscu 反向验证 SCP；模拟外部 PACS 验证 SCU）+ 文档（README 勾选、发布说明） | 0.5-1 天 |
| 合计 | | 约 4-5 天 |

## 6. 验收标准

1. **SCP**：DCMTK `movescu` 对本院 AE 发起拉取（查询匹配）→ 图被推送到指定 Destination，计数响应正确；`findscu` 查询可用。
2. **C-GET SCP**：`getscu`（或等效）同连接拉取成功。
3. **白名单**：MoveDestination 不在白名单 → 拒绝（0x0122），不产生任何推送。
4. **SCU 主动拉取**：管理界面配置外部 PACS → 查询 → 发起拉取 → 数据经 C-STORE 入库 → 队列页可见、来源机构正确、非授权用户不可见。
5. **取消**：拉取中取消 → 停止推送并回 CANCELLED，无脏数据。
6. **回归**：C-ECHO / C-STORE / C-FIND 行为不变；`cargo test` / `clippy` / fmt 全绿；前端 tsc 全绿。
7. **README**：DICOM 能力表 C-MOVE/C-GET SCP 🚧 → ✅，删除「rejected (association aborted)」注释；CHANGELOG 增补 v0.4.0。

## 7. 不在本次范围

- DICOMweb 扩展（STOW-RS JSON 变体、QIDO orderby 等）——既有路线图项，另行安排
- Modality Worklist（设备端 C-FIND MWL 拉申请单）——申请单增强，另行安排
- 加密/证书链加固（DIMSE over TLS 已有基础，不做增强）
- 多站点复制/分布式存储——路线图远期项

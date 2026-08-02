# remote_pacs

自建 PACS：Rust 服务端 + Tauri 桌面查看器。可分发、多账号、共享平台数据库。

实施计划见 [`.claude/plans/pacs-plan.md`](.claude/plans/pacs-plan.md)。

## 结构

```
crates/
  pacs-core/    领域模型、UID 校验、DICOM 元数据提取
  pacs-store/   文件落盘、fsync 语义、两级哈希分片路径
  pacs-db/      Postgres 访问层、迁移、入库事务
  pacs-dimse/   自研 DIMSE 服务类(C-ECHO/STORE/FIND/MOVE/GET SCP)
  pacs-auth/    账号、argon2 哈希、token、RBAC、审计
  pacs-web/     axum: QIDO/WADO/STOW-RS + 认证 API
  pacs-codec/   像素解码、缩略图、帧提取
  pacs-ai/      AI 接口预留(仅 trait 与任务表，无实现)
  pacsd/        服务端主程序
apps/viewer/    Tauri 2 客户端(可脱离服务端打开本地 DICOM)
```

进度：阶段 0（环境）、阶段 1（存储 + 数据库）、阶段 2（C-ECHO/C-STORE SCP）已完成，
阶段 3（账号体系 + TLS）待开始。

## 试一试

启动服务端后用 DCMTK 打一发：

```sh
cargo run -p pacsd                                   # 默认监听 127.0.0.1:11112
echoscu   -aec REMOTE_PACS 127.0.0.1 11112           # C-ECHO
storescu  -aec REMOTE_PACS 127.0.0.1 11112 x.dcm     # C-STORE
```

影像会落到 `PACS_STORAGE_ROOT` 下，索引进 Postgres。

## 开发环境

需要 Rust 1.97.1(由 `rust-toolchain.toml` 自动选择)、PostgreSQL、DCMTK。

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

### 数据库

服务端独占数据库连接，客户端不直连。连接串通过 `.env` 提供：

```sh
cp .env.example .env   # 然后填入真实密码
```

本机开发环境：`postgresql@14`（brew）跑在 **5433** 端口——5432 被系统级
PostgreSQL 18（`/Library/PostgreSQL/18`，另一套独立安装）占用，两者互不影响。
`pacs`（开发）和 `pacs_test`（集成测试）两个库、`pacs` 角色已建，密码在 `~/.pgpass`：

```sh
/opt/homebrew/opt/postgresql@14/bin/psql -h 127.0.0.1 -p 5433 -U pacs -d pacs
```

数据库迁移在 `pacsd` 启动时自动应用，也随二进制编译进去，部署时不用带 SQL 文件。

### 测试

`pacs-db` 的集成测试跑在真实 Postgres 上（那些 SQL 只有真库能验证），
`pacsd` 的互操作测试会启动服务端并用 DCMTK 的 `echoscu`/`storescu` 打真实流量
（自己写的客户端测自己写的服务端，只能证明两边的误解一致）。

两者都需要 `PACS_TEST_DATABASE_URL`，互操作测试还需要 DCMTK。缺少时会打印提示
并跳过；`CI` 环境变量存在时跳过会直接判失败，避免"绿了但其实没测"。

测试库不存在会自动创建，不用先手动 `createdb`。

### 基准

```sh
cargo run --release -p pacsd --example bench_ingest -- 200 8 512
#                                                      份数 并发 边长
```

测的是 C-STORE 回成功之前必须完成的链路：解析 → 落盘 fsync → 事务提交。
必须 `--release`，debug 构建下 DICOM 解析慢一个数量级。

## 设计要点

几个关键约束，改动前请先读计划里的对应章节：

- **客户端绝不直连数据库。** 软件要分发到不同机器和账号，内嵌连接串等于把库
  凭据发给每个用户 —— 无法做权限控制、无法吊销、无法轮换。
- **UID 在入库前必须校验。** UID 直接作为文件路径分量使用，而它来自外部设备。
  `pacs_core::Uid` 保证构造成功的值都是安全的单级路径名。
- **C-STORE 回成功之前数据必须真的持久化。** 顺序是：写临时文件 → fsync →
  rename → fsync 父目录 → 数据库事务提交 → 才回 `0x0000`。设备收到成功响应
  后真的会删本地副本。
- **命令集永远是 Implicit VR Little Endian**，与数据集协商出的传输语法无关
  （PS3.7 §6.3.1）。按协商结果去解命令集，遇到显式 VR 的连接就会解出乱码。
- **落盘的数据集字节与发送方原样一致。** 只在前面拼文件元信息，不解码再重编码
  —— 影像资料的保真性不该被我们的编码器改写。
- **CT 序列排序不能用 `InstanceNumber`。** 要按 `ImagePositionPatient` 在切片
  法向量上的投影排。

## 安全须知

- DIMSE 协议本身无认证（AE Title 可伪造），DICOMweb 默认也没有鉴权。
  服务端默认只绑 `127.0.0.1`，绑其他地址会在启动日志里告警。
- 账号体系与 TLS 完成前（阶段 3），不要接入真实临床网络或公网。
- 真实病人数据涉及 HIPAA / GDPR /《个人信息保护法》合规要求。

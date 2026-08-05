# DCMTK 测试平台与 Remote PACS 对接文档

> 适用版本：阶段三 DICOM Router 完成后的 Remote PACS  
> 文档日期：2026-08-05

## 1. 对接范围

DCMTK 测试平台需要同时模拟两个方向：

1. 模拟 CT、MR 等设备，使用 `echoscu`、`storescu` 向 PACS 发送 C-ECHO 和 C-STORE。
2. 模拟接收设备，使用 `storescp` 接收 PACS Router 发出的 C-ECHO 和 C-STORE。

当前 Router 管理 API 支持 HTTP GET、POST、PUT、DELETE；Router 的 DIMSE 出站操作为
C-ECHO 和 C-STORE。HTTP GET/POST 与 DICOM C-GET 是两套不同协议，当前阶段不把
DICOM C-GET/C-MOVE 作为 Router 出站能力。

## 2. 固定网关、AE Title 与端口

### 2.1 PACS 默认配置

| 用途 | 地址或标识 | 默认值 |
| --- | --- | --- |
| PACS DIMSE AE Title | Called AE Title | `REMOTE_PACS` |
| PACS DIMSE 网关 | 本机地址 | `127.0.0.1` |
| PACS DIMSE 端口 | TCP | `11112` |
| PACS HTTPS API 网关 | Base URL | `https://127.0.0.1:8443` |
| STOW-RS 上传 | HTTPS POST | `https://127.0.0.1:8443/dicomweb/studies` |
| Router 管理 API | HTTPS | `https://127.0.0.1:8443/api/v1/router` |
| OpenAPI | HTTPS GET | `https://127.0.0.1:8443/api/v1/openapi.json` |

对应服务端环境变量：

```dotenv
PACS_DIMSE_BIND=127.0.0.1:11112
PACS_AE_TITLE=REMOTE_PACS
PACS_HTTP_BIND=127.0.0.1:8443
```

`127.0.0.1` 只允许同一操作系统内的进程访问。测试平台在 Docker 或其他机器上时，
必须根据第 3 节修改监听地址和网关。

### 2.2 DCMTK 模拟设备推荐配置

第一台模拟设备使用：

| 配置项 | 推荐值 |
| --- | --- |
| 设备名称 | `DCMTK Simulator 1` |
| 设备 AE Title | `DCMTK_SIM_1` |
| `storescp` 监听地址 | `0.0.0.0` |
| `storescp` 监听端口 | `11113` |
| 接收目录 | `./data/received/device-1` |
| 发送到 PACS 的 Called AE | `REMOTE_PACS` |
| 发送到 PACS 的 Calling AE | `DCMTK_SIM_1` |

模拟多台设备时，每台设备必须使用独立 AE Title 和监听端口：

| 设备 | AE Title | 端口 |
| --- | --- | --- |
| 设备 1 | `DCMTK_SIM_1` | `11113` |
| 设备 2 | `DCMTK_SIM_2` | `11114` |
| 设备 3 | `DCMTK_SIM_3` | `11115` |

不推荐测试平台使用标准端口 `104`。该端口低于 1024，在 Unix/macOS 上通常需要
管理员权限，也更容易与现有 DICOM 服务冲突。

## 3. 网关选择

网关必须按数据流方向选择。Router 中填写的 `host` 是“从 PACS 看向模拟器”的地址；
`echoscu/storescu` 中填写的地址是“从模拟器看向 PACS”的地址。

`0.0.0.0` 只表示“监听本机所有网卡”，不能作为连接目标填写到 Router、`echoscu`
或 `storescu`。局域网中的目标网关应填写对端主机 IP，例如 `192.168.1.20`，不是
路由器的默认网关地址（例如 `192.168.1.1`）。macOS 可用 `ipconfig getifaddr en0`
查看当前 Wi-Fi IPv4 地址。

### 3.1 PACS 与模拟器都直接运行在同一台 Mac

这是首选开发方式，无需暴露服务到局域网。

| 方向 | 目标网关 | 端口 |
| --- | --- | --- |
| 模拟器 -> PACS | `127.0.0.1` | `11112` |
| PACS Router -> 模拟器 | `127.0.0.1` | `11113` |
| 管理 API -> PACS | `https://127.0.0.1` | `8443` |

PACS `.env` 保持默认值即可。

### 3.2 PACS 运行在 Mac，模拟器运行在 Docker

PACS 必须允许容器访问 DIMSE 端口：

```dotenv
PACS_DIMSE_BIND=0.0.0.0:11112
PACS_AE_TITLE=REMOTE_PACS
PACS_HTTP_BIND=127.0.0.1:8443
```

修改后重启 `pacsd`。网络参数如下：

| 方向 | 目标网关 | 端口 | 说明 |
| --- | --- | --- | --- |
| Docker 模拟器 -> PACS | `host.docker.internal` | `11112` | Docker Desktop 访问 Mac 宿主机 |
| PACS Router -> 模拟器 | `127.0.0.1` | `11113` | 容器需发布 `11113:11113` |
| Mac 上的管理程序 -> PACS | `https://127.0.0.1` | `8443` | 使用 PACS CA 证书 |

容器启动示意：

```sh
docker run --rm \
  -p 11113:11113 \
  -v "$PWD/data/received:/received" \
  dcmtk-simulator \
  storescp -v +xa -pm -aet DCMTK_SIM_1 -od /received 11113
```

PACS 当前自动生成的 HTTPS 证书只包含 `localhost`、`127.0.0.1` 和 `::1`。容器直接
访问 `https://host.docker.internal:8443` 会发生证书名称不匹配。首版测试平台应在
Mac 宿主机调用 Router API；如果必须从容器调用，应为 `host.docker.internal` 配置
包含正确 SAN 的测试证书，不能把关闭 TLS 校验作为正式方案。

### 3.3 PACS 与模拟器运行在同一 Docker 网络

PACS 容器内配置：

```dotenv
PACS_DIMSE_BIND=0.0.0.0:11112
PACS_AE_TITLE=REMOTE_PACS
PACS_HTTP_BIND=0.0.0.0:8443
```

假设 Compose 服务名分别为 `pacsd` 和 `dcmtk-sim`：

| 方向 | 目标网关 | 端口 |
| --- | --- | --- |
| 模拟器 -> PACS | `pacsd` | `11112` |
| PACS Router -> 模拟器 | `dcmtk-sim` | `11113` |
| 容器外管理 API -> PACS | `https://127.0.0.1` | 发布的 `8443` |

Router 目的地的 `host` 必须填写 `dcmtk-sim`，不要填写容器自己的 `127.0.0.1`。

### 3.4 PACS 与模拟器位于两台局域网主机

PACS 配置：

```dotenv
PACS_DIMSE_BIND=0.0.0.0:11112
PACS_AE_TITLE=REMOTE_PACS
PACS_HTTP_BIND=0.0.0.0:8443
```

假设 PACS IP 为 `192.168.1.20`，模拟器 IP 为 `192.168.1.30`：

| 方向 | 目标网关 | 端口 |
| --- | --- | --- |
| 模拟器 -> PACS | `192.168.1.20` | `11112` |
| PACS Router -> 模拟器 | `192.168.1.30` | `11113` |
| 管理 API -> PACS | `https://192.168.1.20` | `8443` |

防火墙至少允许两台主机之间的 TCP `11112` 和 `11113`。远程 HTTPS 还需要服务端
证书的 SAN 包含 PACS 的域名或 `192.168.1.20`；当前只包含回环地址的开发证书不适用。
DIMSE 明文模式没有可靠身份认证，只能在隔离测试网使用。

## 4. DCMTK 测试平台的实现方式

### 4.1 进程模型

每个模拟设备对应一个独立 `storescp` 子进程。测试平台负责：

1. 分配唯一 AE Title、监听端口和接收目录。
2. 使用参数数组启动子进程，不拼接未经校验的 Shell 命令。
3. 保存 PID，采集 stdout/stderr，并检测进程是否退出。
4. 使用 `echoscu` 测试模拟设备到 PACS 的连通性。
5. 使用 `storescu` 向 PACS 发送指定文件或目录。
6. 扫描接收目录，记录 PACS Router 发回的文件及接收时间。

建议的数据结构：

```json
{
  "id": "device-1",
  "name": "CT Simulator 1",
  "ae_title": "DCMTK_SIM_1",
  "listen_host": "0.0.0.0",
  "listen_port": 11113,
  "pacs_host": "127.0.0.1",
  "pacs_port": 11112,
  "pacs_ae_title": "REMOTE_PACS",
  "receive_directory": "./data/received/device-1",
  "status": "stopped"
}
```

AE Title 必须为 1-16 个字符。主机和目录只能来自平台配置或经过严格校验的输入；不要
允许用户把换行、Shell 元字符或任意父目录路径传给进程启动器。

### 4.2 启动模拟接收设备

```sh
mkdir -p ./data/received/device-1

storescp \
  -v \
  +xa \
  -pm \
  -aet DCMTK_SIM_1 \
  -od ./data/received/device-1 \
  11113
```

- `+xa`：接受 DCMTK 支持的所有 Transfer Syntax。
- `-pm`：允许测试未知 SOP Class；生产设备模拟可去掉并使用明确配置文件。
- `-aet`：模拟设备的 Called AE Title。
- `-od`：接收文件目录，该目录必须预先创建。

Remote PACS Router 会按原始文件的 SOP Class 和 Transfer Syntax 发起协商。因此，
`storescp` 必须接受被路由文件原有的 Transfer Syntax，否则投递会失败并进入重试。

### 4.3 模拟设备测试 PACS C-ECHO

```sh
echoscu \
  -v \
  -aet DCMTK_SIM_1 \
  -aec REMOTE_PACS \
  127.0.0.1 \
  11112
```

Docker 中把 `127.0.0.1` 换成 `host.docker.internal`。成功条件是进程退出码为 `0`，
日志中出现成功的 C-ECHO 响应。

### 4.4 模拟设备向 PACS 发送 DICOM

发送单个文件：

```sh
storescu \
  -v \
  -aet DCMTK_SIM_1 \
  -aec REMOTE_PACS \
  127.0.0.1 \
  11112 \
  ./fixtures/ct/image-0001.dcm
```

发送目录：

```sh
storescu \
  -v \
  -aet DCMTK_SIM_1 \
  -aec REMOTE_PACS \
  +sd \
  +r \
  127.0.0.1 \
  11112 \
  ./fixtures/ct
```

测试平台应同时记录 DCMTK 进程退出码和每个 SOP 的响应状态。只有 C-STORE-RSP
状态为 `0x0000` 才算 PACS 接收成功。

### 4.5 推荐的测试平台控制 API

以下 API 属于 DCMTK 测试平台自身，不是 Remote PACS 的既有接口：

| 方法 | 路径 | 用途 |
| --- | --- | --- |
| GET | `/api/devices` | 查看全部模拟设备、PID 和状态 |
| POST | `/api/devices` | 新建设备并分配 AE/端口 |
| POST | `/api/devices/{id}/start` | 启动对应 `storescp` |
| POST | `/api/devices/{id}/stop` | 停止对应 `storescp` |
| POST | `/api/devices/{id}/echo-pacs` | 执行 `echoscu` |
| POST | `/api/devices/{id}/send` | 使用 `storescu` 发送文件或目录 |
| GET | `/api/devices/{id}/received` | 列出 Router 发来的 DICOM |
| GET | `/api/devices/{id}/logs` | 查看最近的 DCMTK 日志 |

“在线”至少应区分两类状态：

- `process_status`：`storescp` 子进程是否存活。
- `pacs_connectivity`：最近一次从模拟设备到 PACS 的 C-ECHO 是否成功。

PACS Viewer 中显示的 Router 设备状态是相反方向，即 PACS 到模拟器的 C-ECHO 或
C-STORE 结果。两个方向必须分别测试，不能只用进程存活代替网络连通性。

Remote PACS 还会按 Calling AE 和来源 IP 自动登记成功建立过 Association 的入站设备。
C-ECHO 这类短连接释放后显示为“最近连接”；Association 尚未释放时显示为“连接中”。
入站连接的来源端口是临时端口，不能作为回传端口，因此自动发现记录不会自动变成
Router 目的地，回传仍需明确配置设备的固定监听端口。

## 5. 在 PACS 中注册 DCMTK 模拟器

可以直接在 Viewer 的 DICOM Router 界面创建目的地，也可以调用 API。

### 5.1 获取管理员 JWT

PACS HTTPS 使用自签 CA。CA 文件默认位于：

```text
./data/storage/tls/ca.crt
```

登录：

```sh
curl --cacert ./data/storage/tls/ca.crt \
  https://127.0.0.1:8443/auth/login \
  -H 'Content-Type: application/json' \
  --data '{"username":"viewer","password":"<ADMIN_PASSWORD>"}'
```

后续请求使用返回的 `access_token`：

```text
Authorization: Bearer <ACCESS_TOKEN>
```

也可以使用具有 `route` scope 的服务账号 API Key，格式为 `pacs_sk_*`。API Key
只在创建时显示一次，不应写入源码或日志。

### 5.2 创建 DIMSE Router 目的地

PACS 与模拟器都直接运行在同一台 Mac 时：

```sh
curl --cacert ./data/storage/tls/ca.crt \
  https://127.0.0.1:8443/api/v1/router/destinations \
  -H 'Authorization: Bearer <ACCESS_TOKEN>' \
  -H 'Content-Type: application/json' \
  --data '{
    "name": "DCMTK Simulator 1",
    "protocol": "dimse",
    "enabled": true,
    "host": "127.0.0.1",
    "port": 11113,
    "called_ae_title": "DCMTK_SIM_1",
    "calling_ae_title": "REMOTE_PACS",
    "use_tls": false
  }'
```

响应中的 `id` 是后续连接测试、规则和手工发送使用的 `destination_id`。

### 5.3 从 PACS 测试模拟器连接

```sh
curl --cacert ./data/storage/tls/ca.crt \
  -X POST \
  https://127.0.0.1:8443/api/v1/router/destinations/<DESTINATION_ID>/test \
  -H 'Authorization: Bearer <ACCESS_TOKEN>' \
  -H 'Content-Type: application/json' \
  --data '{}'
```

成功时返回的目的地应包含：

```json
{
  "status": "online",
  "last_latency_ms": 12,
  "last_error": null
}
```

失败时 HTTP 请求仍可能返回目的地对象，但 `status` 为 `offline`，具体错误在
`last_error`。Viewer 中的“测试连接”按钮调用的是同一接口。

## 6. 路由数据到模拟器

### 6.1 手工发送 Study 或 Series

发送整个 Study：

```sh
curl --cacert ./data/storage/tls/ca.crt \
  https://127.0.0.1:8443/api/v1/router/send \
  -H 'Authorization: Bearer <ACCESS_TOKEN>' \
  -H 'Content-Type: application/json' \
  --data '{
    "destination_id": "<DESTINATION_ID>",
    "study_instance_uid": "<STUDY_INSTANCE_UID>"
  }'
```

只发送一个 Series：

```json
{
  "destination_id": "<DESTINATION_ID>",
  "study_instance_uid": "<STUDY_INSTANCE_UID>",
  "series_instance_uid": "<SERIES_INSTANCE_UID>"
}
```

接口返回 `202 Accepted`：

```json
{
  "queued": 120,
  "skipped_as_duplicate": 0,
  "job_ids": ["..."]
}
```

同一个不可变实例版本向同一目的地只投递一次。再次发送会增加
`skipped_as_duplicate`，不会重复调用 `storescp`。

### 6.2 创建自动路由规则

下面的规则把由 `DCMTK_SIM_1` 发送到 PACS 的胸部 CT 自动路由回指定模拟接收设备：

```sh
curl --cacert ./data/storage/tls/ca.crt \
  https://127.0.0.1:8443/api/v1/router/rules \
  -H 'Authorization: Bearer <ACCESS_TOKEN>' \
  -H 'Content-Type: application/json' \
  --data '{
    "destination_id": "<DESTINATION_ID>",
    "name": "DCMTK chest CT",
    "priority": 10,
    "enabled": true,
    "source_ae_title": "DCMTK_SIM_1",
    "modality": "CT",
    "body_part_examined": "CHEST",
    "tag_matches": {}
  }'
```

规则还支持 Study Description、Series Description 和 DICOM JSON Tag 精确匹配。例如
只匹配男性患者：

```json
{
  "tag_matches": {
    "00100040": {
      "vr": "CS",
      "Value": ["M"]
    }
  }
}
```

设置了 `source_ae_title` 的规则只匹配 DIMSE C-STORE 入口。文件、ZIP、RAR 和
STOW-RS 导入没有 Calling AE Title，因此不会命中此条件。自动路由只在新增实例成功
入库后触发；重复实例不会再次触发。如需发送已存在的数据，应使用手工发送接口。

## 7. 投递状态、重试与死信

查询最近投递：

```sh
curl --cacert ./data/storage/tls/ca.crt \
  'https://127.0.0.1:8443/api/v1/router/deliveries?limit=200' \
  -H 'Authorization: Bearer <ACCESS_TOKEN>'
```

状态含义：

| 状态 | 含义 |
| --- | --- |
| `queued` | 等待投递或等待下一次重试 |
| `running` | 正在建立关联或发送 C-STORE |
| `succeeded` | 目标返回成功状态 |
| `dead_letter` | 五次尝试均失败，等待人工处理 |

一次投递最多尝试五次。前四次失败后分别等待约 5、10、20、40 秒；第五次失败进入
`dead_letter`。启动 `storescp` 并修复网络后，使用以下接口重放：

```sh
curl --cacert ./data/storage/tls/ca.crt \
  -X POST \
  https://127.0.0.1:8443/api/v1/router/deliveries/<DELIVERY_ID>/replay \
  -H 'Authorization: Bearer <ACCESS_TOKEN>' \
  -H 'Content-Type: application/json' \
  --data '{}'
```

只有 `dead_letter` 状态允许人工重放。

## 8. 故障模拟

DCMTK `storescp` 可用于验证 Router 的异常处理：

| 场景 | 操作 |
| --- | --- |
| 设备离线 | 停止 `storescp` 或使用未监听端口 |
| 拒绝 Association | 启动 `storescp --refuse ...` |
| 收到 C-STORE 后中断 | 启动 `storescp --abort-after ...` |
| 传输过程中中断 | 启动 `storescp --abort-during ...` |
| 响应超时 | 启动 `storescp --sleep-during 20 ...` |
| Transfer Syntax 不接受 | 不使用 `+xa`，只允许指定 Transfer Syntax |

Router 默认单次 DIMSE 连接、读写超时为 15 秒。故障测试结束后必须停止故障进程，
恢复正常 `storescp`，再执行死信重放。

## 9. 最小验收清单

- [ ] `echoscu DCMTK_SIM_1 -> REMOTE_PACS:11112` 成功。
- [ ] `storescu` 发送一份 DICOM，PACS Viewer 可以查询和打开。
- [ ] PACS Router 对 `DCMTK_SIM_1:11113` 的连接测试显示 `online`。
- [ ] 手工发送一个 Series，模拟器收到的 SOP 数量与 PACS 当前实例数一致。
- [ ] 创建自动规则后，发送一份新 UID 的 DICOM，Router 自动生成投递。
- [ ] 重复发送同一文件，PACS 不新增实例，Router 不新增重复投递。
- [ ] 停止 `storescp` 后投递进入重试，五次失败后进入 `dead_letter`。
- [ ] 恢复 `storescp` 并执行重放，投递变为 `succeeded`。
- [ ] 两台不同 AE/端口的模拟设备可以独立显示连接状态和投递记录。
- [ ] 接收文件的 StudyInstanceUID、SeriesInstanceUID、SOPInstanceUID 与源文件一致。

## 10. 常见错误

| 错误 | 原因与处理 |
| --- | --- |
| `Connection refused` | 目标端口没有监听，或 Docker 未发布端口 |
| `No route to host` | 网关/IP 错误或防火墙阻断 |
| `Called AE title not recognized` | `-aec` 与目标 SCP 的 AE Title 不一致 |
| `Presentation context rejected` | 目标不接受该 SOP Class 或 Transfer Syntax，模拟器可先使用 `+xa -pm` |
| Router 显示 `offline` | 查看 `last_error`，从 PACS 主机测试目标 host/port，而不是只在模拟器内测试 |
| Docker 中访问 `127.0.0.1:11112` 失败 | 该地址指向容器自身，应改为 `host.docker.internal:11112` |
| HTTPS 证书名称不匹配 | 访问地址不在证书 SAN 中，需要签发匹配网关名称/IP 的证书 |
| 重复上传后没有自动路由 | 自动路由只对新增实例触发；使用新 UID 或手工发送已有 Study/Series |

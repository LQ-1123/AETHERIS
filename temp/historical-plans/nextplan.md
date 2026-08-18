PACS 查看器项目总结与交接文档

## 2026-08-03 实施更新

本地 Viewer MVP 已完成。下方原“待实现功能”和阶段安排保留为历史交接记录，
实际状态以本节为准。

已新增：

- 单文件多帧，以及同一 Study/Series 的多文件灰度序列打开。
- 多文件序列按 `ImagePositionPatient`/`ImageOrientationPatient` 安全排序；缺少
  几何、混合序列或混入多帧实例时明确拒绝。
- 离屏 Canvas + `drawImage` 渲染，修复原 `putImageData` 无法缩放平移的问题。
- 普通滚轮切片、`Ctrl + 滚轮`光标锚定缩放、中键拖动平移、重置视图和键盘/滑条切片。
- 病人和检查信息、窗预设、8/16-bit 灰度帧契约。
- 两点测距、逐帧会话标注、删除，以及 calibrated/detector/pixel 三档提示。
- 前端 128 MiB LRU + 前后 2 帧预取，后端 512 MiB LRU 和阻塞线程解码。
- 9 项 TypeScript 单元测试、7 项 Viewer Rust 测试及 Clippy 严格检查。

下一开发顺序：远程登录与服务器地址配置 → QIDO 工作列表 → WADO 打开序列 →
STOW-RS → 角度/ROI、导出和分发。真实大序列的 30fps/内存目标仍需专项基准验证。

已完成功能

1. 查找表（LUT）模块 ✅

文件: crates/pacs-codec/src/lut.rs
- 功能: 将显示管线（存储值 → Rescale → VOI → 光度反转）预计算为 65536 项的查找表
- 用途: 窗宽窗位交互时只需查表，无需重新计算整个管线
- 测试: 10 条单元测试通过，变异测试验证
- 提交: 12fbfb9

2. Tauri 后端核心 ✅

目录: apps/viewer/src-tauri/

状态管理 (state.rs)

- ViewerState: 全局状态，管理已打开的实例
- FrameCache: LRU 缓存，512 MiB 上限，避免重复解码
- open(): 打开 DICOM 文件，解析管线和像素间距
- get_frame_bytes(): 获取帧数据，带缓存，支持 1-based 帧号转换
- build_lut(): 生成 256 级灰度查找表

命令接口 (commands.rs)

- open_dicom: 打开本地 DICOM 文件，返回元数据
- close_instance: 关闭实例，释放资源
- build_lut: 生成指定窗宽窗位的查找表

自定义协议 (protocol.rs)

- pacs-frame://localhost/{handle}/{frame}: 自定义 URI 协议
- 帧数据直传，绕过 JSON IPC 序列化
- 异步处理，使用 tauri::async_runtime::spawn
- 添加 CORS 头支持跨域访问
- 详细的调试日志

配置 (tauri.conf.json)

- 独立 workspace，不污染服务端 CI
- 权限配置：dialog:allow-open, dialog:allow-message
- withGlobalTauri: true 启用全局对象
- 移除 devUrl，使用构建后的静态文件（解决 __TAURI_INTERNALS__ 注入问题）

3. 前端实现 ✅

目录: apps/viewer/

类型定义 (src/types.ts)

- DisplayMetadata: 后端返回的元数据（尺寸、帧数、窗口预设、间距）
- ViewState: 前端视图状态（当前帧、窗宽窗位、缩放、平移）

API 层 (src/api.ts)

- 动态导入 Tauri API（解决静态导入失败问题）
- openDicomFile(): 文件选择器 + 调用后端打开
- buildLut(): 生成查找表
- loadFrame(): 通过 pacs-frame:// 协议加载帧数据
- 完整的类型声明和错误处理

渲染引擎 (src/renderer.ts)

- Renderer 类：管理 Canvas 渲染
- loadFrame(): 加载帧的 Uint16Array 数据
- render(): 应用 LUT 将 16 位转 8 位灰度，绘制到 Canvas
- applyTransform(): 应用缩放和平移变换

主应用 (src/app.ts)

- App 类：协调 UI、渲染器、后端 API
- 窗宽窗位拖动交互（左右调窗位，上下调窗宽）
- 滚轮缩放（代码已实现，待测试）
- 状态管理和 UI 更新

UI (index.html)

- 深色主题设计
- 工具栏：打开文件按钮 + 信息显示
- Canvas 视口：居中显示
- 操作提示面板

配置

- package.json: 版本匹配（@tauri-apps/api: ~2.1.0, @tauri-apps/cli: ~2.1.0）
- vite.config.ts: Vite 开发配置
- tsconfig.json: TypeScript 严格模式

4. 已验证功能 ✅

- ✅ 打开本地 DICOM 文件（文件选择对话框）
- ✅ 后端解析并返回元数据
- ✅ 通过自定义协议加载帧数据
- ✅ Canvas 显示影像
- ✅ 窗宽窗位拖动实时调整
- ✅ LUT 查找表应用
- ✅ 显示基本信息（帧数、尺寸、窗位窗宽）

---
待实现功能

1. 基础交互增强

- 滚轮缩放: 代码已实现（app.ts:135），需测试和微调灵敏度
- 平移: 中键/右键拖动平移图像
- 重置视图: 按钮恢复初始缩放和平移

2. 多帧支持

- 序列滚动: 键盘上下键或鼠标滚轮（Shift+滚轮）切换帧
- 进度条: 显示当前帧位置，支持拖动跳转
- 播放控制: 播放/暂停按钮，自动循环播放序列

3. 测量工具

- 测距: 基于 PixelSpacing 计算真实距离（毫米）
- 角度测量: 三点定义角度
- ROI 统计: 矩形/圆形 ROI 的平均值、标准差
- 标注持久化: 保存到状态，切换帧时保留

4. 病人信息显示

- DICOM 标签读取: PatientName, PatientID, StudyDate, Modality 等
- 信息面板: 左侧或右侧可折叠面板
- 序列信息: SeriesDescription, AcquisitionTime

5. 窗口预设

- 预设列表: 显示 window_presets 中的预设（如 CT 的肺窗、纵隔窗）
- 快捷切换: 点击或快捷键快速应用预设

6. 性能优化

- 帧缓存预加载: 预加载前后几帧到前端缓存
- Worker 解码: 将 LUT 应用放到 Web Worker
- 虚拟滚动: 大序列（数百帧）的高效渲染

7. 导出功能

- 截图: 导出当前视图为 PNG/JPEG
- 视频导出: 多帧序列导出为 MP4
- DICOM 导出: 应用窗宽窗位后导出为新 DICOM

8. 服务端集成（未来）

- WADO-RS 支持: 从服务端加载影像（已有 pacs-server 的 WADO-RS 实现）
- QIDO-RS 搜索: 集成服务端的检索接口
- 工作列表: 显示待阅读的检查

---
技术债务与已知问题

1. 开发体验

- 热重载缺失: 前端每次修改需要 npm run build 重新构建
  - 解决方案: 恢复 devUrl，并找到正确的方式让 __TAURI_INTERNALS__ 在外部服务器下注入
  - 或: 使用 Tauri 2.x 的 dev 配置，启用前端热重载

2. 错误处理

- 帧加载失败: 目前只弹 alert，应该显示友好的错误提示
- 文件格式不支持: 应该提前检测并给出明确提示

3. 类型安全

- api.ts 中 selected 的类型处理比较粗糙（(selected as any).path || selected[0]）
- 应该明确 @tauri-apps/plugin-dialog 的返回类型

4. 测试

- 前端单元测试: 零测试覆盖
- 集成测试: 未测试完整的文件打开 → 显示 → 交互流程
- 端到端测试: 可以用 Tauri 的测试框架

5. 可访问性

- 键盘导航不完整
- 屏幕阅读器支持缺失
- 缺少 ARIA 标签

---
后续计划（优先级排序）

阶段 1: 完善核心交互（1-2 天）

目标: 让查看器达到基本可用状态

1. 测试并修复滚轮缩放
  - 文件: src/app.ts:135
  - 验证缩放中心点是否正确
  - 调整灵敏度
2. 实现多帧滚动
  - 添加键盘监听（上下键）
  - 添加进度条组件
  - 实现帧切换逻辑
3. 病人信息面板
  - 后端: 从 DICOM 提取标签（PatientName, StudyDate 等）
  - 前端: 左侧信息面板 UI
  - 状态: 在 DisplayMetadata 中添加字段
4. 窗口预设快速切换
  - 前端: 工具栏添加预设下拉菜单
  - 点击预设立即应用

验收标准:
- 能流畅滚动多帧序列
- 能查看病人基本信息
- 能快速切换窗口预设

---
阶段 2: 测量工具（2-3 天）

目标: 支持临床常用的测量功能

1. 测距工具
  - UI: 工具栏添加"测距"按钮
  - 交互: 点击两点画线，显示距离（毫米）
  - 逻辑: 基于 PixelSpacing 计算
  - 渲染: 在 Canvas 上叠加标注层
2. 角度测量
  - 交互: 点击三点定义角度
  - 显示: 角度值和辅助线
3. 标注管理
  - 状态: 存储所有标注
  - 删除: 选中标注后按 Delete 键删除
  - 切换帧: 保留当前帧的标注

技术要点:
- 使用单独的 Canvas 层叠加标注，避免污染原始图像
- 标注数据结构: { type: 'line' | 'angle', points: Point[], value: number }

验收标准:
- 测距精度符合 PixelSpacing
- 标注在缩放平移时正确跟随
- 切换帧时标注正确显示/隐藏

---
阶段 3: 性能优化（1-2 天）

目标: 流畅处理大序列（100+ 帧）

1. 帧缓存预加载
  - 策略: 加载当前帧的前后 5 帧
  - 存储: 使用 Map<number, ArrayBuffer> 缓存
  - 清理: LRU 策略，限制前端缓存总量
2. 虚拟滚动
  - 仅渲染可见帧的前后几帧
  - 减少内存占用
3. Web Worker
  - 将 LUT 应用移到 Worker
  - 主线程只负责 Canvas 绘制

验收标准:
- 100 帧序列滚动无卡顿
- 内存占用稳定（不随帧数线性增长）

---
阶段 4: 导出与分享（1-2 天）

目标: 支持截图和导出

1. 截图功能
  - 按钮: 工具栏添加"截图"按钮
  - 实现: canvas.toDataURL('image/png')
  - 保存: 使用 Tauri 的文件保存对话框
2. 视频导出（可选）
  - 依赖: ffmpeg 或前端库（如 gif.js）
  - 实现: 逐帧渲染后合成

验收标准:
- 截图包含当前窗宽窗位效果
- 文件保存到用户指定位置

---
阶段 5: 服务端集成（未来，3-5 天）

目标: 连接到 pacs-server

1. WADO-RS 加载
  - 配置: 添加服务端地址设置
  - API: 替换本地文件打开为 WADO-RS 请求
  - 协议: http://server/studies/{study}/series/{series}/instances/{instance}
2. QIDO-RS 检索
  - UI: 添加搜索界面
  - 查询: PatientName, StudyDate, Modality
  - 结果: 列表显示，点击打开
3. 工作列表
  - 显示: 待阅读的检查
  - 状态: 已读/未读标记

技术要点:
- 需要 pacs-server 运行在本地或远程
- CORS 配置
- 认证（JWT）

---
环境与依赖

开发环境

- OS: macOS (Darwin 25.6.0)
- Rust: 使用 rsproxy 中国镜像
- Node.js: npm 管理前端依赖
- Tauri: 2.x（Rust 2.11.5, JS 2.1.0）

关键依赖

- 后端: tauri, tauri-plugin-dialog, dicom, pacs-core, pacs-codec
- 前端: @tauri-apps/api, @tauri-apps/plugin-dialog, vite, typescript

构建与运行

# 开发模式
cd apps/viewer
npm run build        # 先构建前端
npm run tauri dev    # 启动 Tauri

# 生产构建
npm run tauri build  # 打包为 .dmg (macOS)

---
项目结构

apps/viewer/
├── src-tauri/              # Rust 后端
│   ├── src/
│   │   ├── main.rs         # 入口
│   │   ├── state.rs        # 状态管理
│   │   ├── commands.rs     # Tauri 命令
│   │   └── protocol.rs     # 自定义协议
│   ├── Cargo.toml
│   └── tauri.conf.json     # Tauri 配置
├── src/                    # TypeScript 前端
│   ├── main.ts             # 入口
│   ├── app.ts              # 主应用逻辑
│   ├── api.ts              # Tauri API 封装
│   ├── renderer.ts         # Canvas 渲染引擎
│   └── types.ts            # 类型定义
├── index.html              # UI
├── package.json            # 前端依赖
├── vite.config.ts          # Vite 配置
└── tsconfig.json           # TypeScript 配置

---
关键技术决策记录

1. 为什么移除 devUrl？

- 问题: 使用外部 Vite 服务器时，window.__TAURI_INTERNALS__ 无法注入
- 原因: Tauri 2.x 在 devUrl 模式下不注入全局对象
- 解决: 移除 devUrl，每次修改需要 npm run build 重新构建
- 代价: 失去热重载
- 未来: 寻找 Tauri 2.x 的正确配置方式恢复热重载

2. 为什么使用自定义协议？

- 问题: 帧数据（~1MB）通过 JSON IPC 序列化慢
- 方案: pacs-frame:// 协议直传 ArrayBuffer
- 优势: 零拷贝，512 MiB 后端缓存复用

3. 为什么动态导入 Tauri API？

- 问题: 静态导入 import { invoke } from '@tauri-apps/api/core' 报 invoke is undefined
- 原因: Vite 预构建时机问题
- 解决: 改用 await import('@tauri-apps/api/core') 延迟加载

---
联系与文档

- 项目仓库: remote_pacs/apps/viewer/
- 核心 Crate: pacs-core, pacs-codec（在 crates/ 目录）
- 服务端: pacs-server（未来集成）
- 记忆文件: /Users/sunyulin/.claude/projects/-Users-sunyulin-Documents-vscode-remote-pacs/memory/

---
下一步行动（建议）

1. 立即: 测试滚轮缩放，验证基本交互
2. 本周: 完成阶段 1（多帧滚动 + 病人信息）
3. 下周: 实现阶段 2（测距工具）
4. 月底: 性能优化（阶段 3）

优先级: 先保证核心功能稳定可用，再添加高级特性。

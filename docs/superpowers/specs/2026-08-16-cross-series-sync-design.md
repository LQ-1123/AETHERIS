# B1 跨序列同步 + 扫描定位线接线 · 设计文档

日期：2026-08-16
状态：待用户评审
路径：`apps/viewer`（纯前端，无需改 Rust 层）

## 1. 背景与目标

`series-sync.ts` 的几何纯函数（`nearestParallelFrameIndex`、`crossReferenceSegment`、
`framesAreParallel` 等）已带单测就绪，但**尚未被 app.ts / renderer.ts 引用**。本设计把
它们接入 UI：多窗格分屏时，同组窗格联动翻层与窗宽窗位，并在每个 2D 窗格叠加绘制
其他序列当前帧的扫描定位线（cross-reference lines）。

这是挂片协议（B2）与对比阅片的共同物理地基；也是竞品对比矩阵中 🔴 项
「多序列同步/自动联动 + 扫描定位线」的收口。

## 2. 现状（已核实）

- `FrameMetadata` 已含 `position` / `orientation`（前后端患者空间几何字段已打通，
  git 提交 `81b1331` / `b0560b7`）。
- `spacing.row_mm / col_mm` 来自 `SpacingInfo`，可直接喂给几何函数。
- 翻层统一入口：`paneWheel`（滚轮）、键盘翻层、`setFrame()`；窗宽窗位入口在 LUT
  更新路径。
- 风险：`app.ts` 已 7404 行，同步逻辑必须抽到独立模块，app.ts 只做薄接线。

## 3. 范围与非目标

**范围**（已与用户确认）：

- 翻层同步（患者空间最近切片映射）
- 窗宽窗位同步（可独立开关）
- 2D 窗格定位线叠加（按序列着色）
- 缺几何的序列：显式排除 + 窗格角标提示（不猜、不映射）

**非目标**（本轮不做）：

- 缩放/平移同步（像素间距不同时的 mm 锚定换算，复杂度翻倍，列入后续）
- MPR / VR 窗格参与同步
- 缺几何时的等比帧号降级映射
- 挂片协议（B2 复用本设计的同步组模型，但自身独立设计）

## 4. 架构与组件

### 4.1 sync-controller.ts（新，纯逻辑模块）

与 `series-sync.ts` 同风格的纯函数 + 轻量状态，**不依赖 DOM**，全部可单测：

```
interface SyncConfig {
  scroll: boolean;      // 翻层同步总开关
  window: boolean;      // WW/WL 同步开关
}

interface SyncGroupState {
  groupId: string;                        // 组标识（默认 'main'）
  memberIds: Set<number>;                 // 参与同步的 pane 索引
  excludedIds: Map<number, string>;       // 排除的 pane → 原因（如 '缺几何'）
}

computeTargetFrameIndex(source: FrameMetadata,
                        targetFrames: FrameMetadata[]): number | null
  → 调 nearestParallelFrameIndex（帧字段适配 FrameMetadata → SeriesGeometryFrame）

frameHasGeometry(frame: FrameMetadata): boolean
  → position / orientation / spacing.row_mm / spacing.col_mm 均非 null

computeTargetWindow(source: {center,width}, target: WindowPreset[]): 无
  → WW/WL 广播无需计算，直接透传（见 4.3）
```

适配层：`FrameMetadata.position/orientation` 直接对应 `SeriesGeometryFrame`；
`rowSpacingMm = spacing.row_mm`、`colSpacingMm = spacing.col_mm`。

### 4.2 app.ts 薄接线

- 状态：每个 `SeriesPane` 增加 `syncGroupId` 与 `syncExcludedReason` 字段
  （`string | null`）。
- 翻层传播：`paneWheel` / 键盘翻层 / `setFrame` 在**当前帧变更后**触发
  `propagateScroll(sourcePaneIndex)`——对组内其余 pane 计算
  `computeTargetFrameIndex` 并 `setFrame`（走现有异步帧请求路径，天然带
  `windowFrameRequest` 版本防串帧）。
- WW/WL 传播：LUT 更新后触发 `propagateWindow(sourcePaneIndex)`，组内其余 pane
  直接套用 center/width（**不做预设匹配**，透传数值）。
- 重入守卫：`syncing` 标志 + 来源 pane 记录；传播触发的 `setFrame` 不再二次传播。
  所有传播回调比较新旧帧号/窗值，相同则短路。

### 4.3 renderer.ts 定位线叠加

- 每个 2D 窗格在现有标注叠加层之上绘制**其他序列**当前帧的平面交线：
  `crossReferenceSegment(otherFrame, thisFrame)` 返回图像坐标线段。
- 序列着色：复用现有序列颜色体系（缺色则按 pane 索引取调色板色）。
- 重绘时机：本帧变化、任一其他 pane 帧变化、视图变换（zoom/pan）后。
- 平行平面（`crossReferenceSegment` 返回 null）或非 2D 窗格：不绘制，不报错。
- 开关：随「联动」总开关控制，或独立「定位线」开关（默认随总开关）。

### 4.4 UI

- 工具栏新增两个独立开关按钮（`icon-button` + lucide 图标，默认开）：
  「翻层联动」（link 图标，`sync-scroll-button`）与「窗位联动」（sun 图标，
  `sync-window-button`）。定位线绘制随「翻层联动」总开关。
- 排除角标：几何缺失的 pane 在窗格顶部显示「缺几何 · 未同步」角标，
  悬浮提示原因。
- 手动/自动双模式语义：总开关=自动联动；**点击角标**切换该 pane 的手动退出
  /重新加入（角标显示「已退出联动」），退出原因记录在 `syncExcludedReason`。

## 5. 数据流

```
滚轮/键盘/setFrame(源窗格)
  → 帧号变更
  → sync-controller.computeTargetFrameIndex × (组内其余 pane)
  → 各 pane setFrame(目标帧)         [syncing 守卫, 不二次传播]
  → renderer 重绘帧 + 定位线

WW/WL 变更(源窗格)
  → sync-controller(透传 center/width)
  → 组内其余 pane 套用窗值 → LUT 重算 → 重绘
```

## 6. 错误处理与边界

| 场景 | 行为 |
| --- | --- |
| 目标序列缺几何（position/orientation/spacing 任一 null） | pane 从同步组排除，角标「缺几何 · 未同步」 |
| 序列间平面不平行 | `nearestParallelFrameIndex` 返回 null，不传播该目标 |
| 层厚/间距不一致 | 最近切片映射仍工作；不额外标注（映射跳变是物理事实，文档说明即可） |
| 用户手动退出同步 | 点击 pane 角标切换，原因记入 `syncExcludedReason`（'手动'），可随时重新加入 |
| 传播期间用户继续翻层 | `syncing` 守卫保证无环；新输入正常排队处理 |
| 帧请求竞态 | 复用现有 `windowFrameRequest` 版本机制，无需新方案 |

## 7. 测试与验收

- **单元测试**（沿用 series-sync.test.ts 模式，新建 sync-controller.test.ts）：
  1. 平行序列最近切片映射正确（含间距不同）
  2. 缺几何帧返回 null、`frameHasGeometry` 判定正确
  3. 组内成员增减、排除原因记录
  4. 传播短路逻辑（同帧号/同窗值不触发）
- **手工验收清单**（双序列 CT fixture，或现有远程演示数据）：
  1. 打开两个平行序列 → 滚轮翻层，另一窗格跟随到患者空间最近帧
  2. 调整窗宽窗位 → 另一窗格同步变化；关闭子开关 → 不变化
  3. 定位线随翻层实时更新，颜色区分序列
  4. 打开缺几何序列（如个别超声）→ 角标出现、不参与同步
  5. 快速连续翻层 → 无循环、无卡顿（帧请求版本无串帧）
- **回归**：现有 9+ 组前端测试与 Rust 测试保持全绿。

## 8. 工作量

单人约 1 周：sync-controller（0.5d）+ app.ts 接线与重入守卫（2d）+ 定位线渲染（2d）+
UI 开关与角标（1d）+ 测试与手工验收（1.5d）。

## 9. 后续扩展

- 挂片协议（B2）：直接复用同步组模型与 pane 字段。
- 缩放/平移同步：新增 mm 锚定换算层，落在 `sync-controller` 内，接口不变。
- 定位线点击跳层、双斜位定位线手柄：渲染层扩展。

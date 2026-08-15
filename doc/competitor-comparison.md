# AETHERIS 与竞品/参考系统功能差距对比

> 留痕日期：2026-08-15
> 对比对象：
> - 《小赛看看》DICOM Viewer
> - RadiAnt DICOM Viewer
> - 用户上传的《易影云影像平台产品使用说明书 V2.0》PDF
> - 云图医疗 DICOM 文档站：<https://blog.iyunto.net/wordpress/docs/dicom/影像操作>
>
> 结论：AETHERIS 在 PACS 后端/平台能力上明显领先纯阅片器；但 Viewer 仍偏向“2D 阅片 + 基础 3D 演示”，距离专业放射科阅片工作站还有不少交互差距。

---

## 1. 当前 AETHERIS 能力快照

### 平台/后端

- DIMSE：C-ECHO / C-STORE / C-FIND / C-MOVE / C-GET SCP
- DICOMweb：QIDO-RS / WADO-RS / STOW-RS（Part10）
- PostgreSQL 元数据索引、持久可靠存储
- RBAC、JWT、审计日志、TAG 修订/回滚
- DICOM Router、生命周期管理、报告生命周期 API
- 本地 AI Worker：肺部分割（lungmask R231）、胸部血管等插件
- Docker / macOS / Windows 分发

### 当前 Viewer

- 2D：窗宽窗位、窗预设、缩放/平移/旋转/翻转/反色、序列导航、Cine、多帧、多文件序列
- 测量：距离、角度、点探针、椭圆/矩形 ROI、CT/HU、SUVbw
- 标注：箭头、ROI 标注、共享标注同步
- MPR：轴位/冠状/矢状正交三平面、十字线联动、MIP/MinIP、Slab 厚度、MPR 内测量/Mask
- VR：GPU 体渲染、预设（灰度/软组织/骨/骨彩/肺/PET）、质量等级、窗宽窗位滑块
- Mask：手动画笔/橡皮、3D Mask 体积、AI 分割
- 工作列表：病人/检查/序列浏览、导入/导出 DICOM、站点分享

---

## 2. 用户已识别的 3 个问题

### 2.1 多窗口分屏看图 —— 确认缺失，优先级最高

当前 AETHERIS 只有：

- 2D 单视口
- MPR 固定 3 窗
- VR 单窗

没有布局选择器，不能把多个序列/检查拖到 1×1、2×2、2×3、3×3、九宫格、六宫格中对比。

参照：

- 小赛看看：九宫格、六宫格、2D 混排、图像对比
- RadiAnt：分屏最多 5 列 × 4 行，最多 16 个独立窗口，可拖缩略图到窗格
- 易影云 PDF：窗口布局、图像布局、医用灰阶竖屏分屏
- 云图医疗：布局选择、显示模式、缩略图、自动联动

### 2.2 MPR 多角度 MPR —— 确认缺失，工程量较大

当前 `MprPlane` 只有 `axial / coronal / sagittal`，后端 `build_planes()` 生成的是世界坐标轴方向的固定平面，前端只能移动十字线，不能旋转切平面。

参照：

- 小赛看看：MPR 支持旋转切平面；2.6.2 增加曲面重建 CPR
- RadiAnt：MPR 支持 coronal / sagittal / axial / oblique
- 云图医疗：MPR 文档明确“冠状、矢状、斜位、曲面”
- 易影云 PDF：新版 MPR 可旋转定位线，旧版 MPR 支持 X/Y/Z 轴旋转

需要补：

- 任意斜位/旋转切面（oblique MPR）
- 曲面重建（CPR）
- 定位线手柄：移动单根、旋转单根、整体移动
- MPR 窗宽窗位“图像联动”开关

### 2.3 VR 窗宽窗位调整 —— 已有基础版，仍需专业交互

代码现状（不是完全缺失）：

- `index.html` 有 `vr-window-center` / `vr-window-width` 滑块
- `app.ts` 有 `updateVolumeWindow()`
- `VolumeRenderer` 有 `setWindow(center, width)`
- VR 预设切换会同步设置窗宽窗位
- VR 界面显示 `VR WL ... WW ...`

但缺少：

- VR 视口内鼠标拖动调窗宽窗位
- 传递函数/透明度编辑器
- 裁剪、去床板、阈值分割
- VR 内测距/标注/导出

参照：

- RadiAnt VR：Adjust window、颜色/透明度、Scalpel 裁剪、Restore volume
- 小赛看看：3D 裁剪、去床板、阈值分割、VOI 测量
- 云图医疗 VR：查看模式、VR 预设、中心点、方向选择

---

## 3. 详细功能差距矩阵

| 功能域 | AETHERIS 现状 | 小赛看看 | RadiAnt | 易影云/云图医疗 | 差距程度 |
|---|---|---|---|---|---|
| 2D 多窗口布局 | 无，只有单视口 | 九宫格/六宫格/2D混排/对比 | 5×4分屏、16窗口 | 窗口布局/图像布局/双屏 | 🔴 高 |
| 多序列同步/自动联动 | MPR 内十字联动，2D 无 | 同步/自动联动/扫描定位线 | 自动/手动同步、cross-reference lines | 同步、自动联动、扫描定位线 | 🔴 高 |
| MPR | 正交三平面 + MIP/MinIP + Slab + 十字线 | 旋转切平面、MIP、CPR、3D窗口 | 正交 + 斜位 MPR | 新版/旧版 MPR、任意旋转、MIP | 🔴 高 |
| VR | 基础体渲染 + 预设 + 滑块 W/L | 3D裁剪、去床板、阈值分割、VOI | 调窗、颜色/透明度、裁剪、测距、导出 | VR 预设/方向/中心点 | 🟠 中高 |
| PET/CT 融合、配准 | 无 | PET/CT、SPECT/CT、PET/MR 融合、手动配准 | PET-CT Fusion、TIC | 检查对比 | 🟠 中高 |
| 测量/ROI | 距离/角度/点探针/椭圆/矩形 ROI、SUVbw | ROI/VOI、HU/SUV、ROI 复制、手动勾画 | 长度/椭圆/角度/Cobb/偏差/多边形/铅笔 | 距离/面积/角度/灰阶/保存 | 🟠 中 |
| 文字/自由标注 | 无文字标注，无自由画笔 | 手动勾画；用户希望文字注释 | Pencil、Arrow、文字类标注 | 文字标注、箭头 | 🟠 中 |
| 导出/截图 | 服务端 DICOM 导出（ZIP），无截图/视频 | PNG/MP4、匿名化、DICOM 保存 | JPEG/BMP/MP4/WMV/DICOM、剪贴板 | JPEG/PNG、报告打印、云胶片 | 🟠 中 |
| 伪彩/图像滤镜 | 无伪彩 | 伪彩 | Sharpen/Smooth/Edge/Emboss | 伪彩、反色、扫描参数 | 🟡 中低 |
| 报告工作流 UI | 后端有 report API，但前端未接报告书写/审核界面 | 不侧重 | 不侧重 | 申请单/报告/审核/打印/云胶片完整流程 | 🔴 高（若做云 PACS） |
| 平台/PACS 后端 | DIMSE、DICOMweb、RBAC、审计、路由、生命周期、本地 AI 分割 | 单机阅片为主 | 单机 + PACS 客户端 | 云平台、用户/机构/统计/移动端 | ✅ AETHERIS 领先 |
| 自定义工具栏/快捷键/双屏/移动端 | 无 | 快捷键、Win/macOS | 自定义快捷键、多语言、多屏 | 双屏、自定义工具栏、移动 APP | 🟡 中低 |

---

## 4. 建议落地优先级

### P0：放射科阅片基本盘

1. 多窗口分屏布局
   - 布局状态：1×1、1×2、2×2、2×3、3×3 等
   - 每个窗格独立 `Renderer` / `ViewState`
   - 支持从序列列表拖入窗格
   - 支持窗格内切换序列
   - 后续支持跨窗同步、跨窗对照

2. 多角度 MPR / CPR
   - `Plane` 从枚举扩展为“原点 + X 轴 + Y 轴”的任意平面
   - 前端增加定位线旋转手柄
   - 再做曲面重建（CPR）路径绘制和重采样

3. VR 窗宽窗位 + 交互增强
   - 保留现有滑块，增加 VR 视口鼠标拖动调窗
   - 增加传递函数/透明度编辑
   - 增加裁剪/去床板
   - 再做 VR 内测量和导出

4. 2D 多序列同步 + 定位线
   - 多窗格后补同步滚动、同步窗宽窗位、同步缩放平移
   - 在 2D 视图中显示其他序列的扫描定位线 / cross-reference lines

### P1：提升阅片效率

- 截图/导出 PNG、视频 MP4
- 文字标注、自由画笔
- 伪彩/颜色映射
- 更多测量：Cobb 角、多边形、偏差距离、手动校准
- ROI 复制/粘贴
- 自定义工具栏和快捷键设置

### P2：向云 PACS 工作流靠拢

- 前端接入已有报告 API：报告书写、审核、签发出报告
- 申请单/云胶片/二维码分享
- 检查对比、历史检查
- 移动端/多屏模式
- 数据统计、用户/机构管理界面

---

## 5. 参考资料

### 小赛看看

- App Store：<https://apps.apple.com/cn/app/%E5%B0%8F%E8%B5%9B%E7%9C%8B%E7%9C%8Bdicom-viewer/id1590273176?mt=12>
- 天极下载：<https://m.yesky.com/pcsoft/304162.html>
- 官网：<https://beedicom.com/>

### RadiAnt

- CD/DVD 功能页：<https://www.radiantviewer.com/store/features/cddvd/>
- 用户手册 PDF：<https://radiantviewer.com/dicom-viewer-manual/PDF/radiantmanual302.pdf>

### 云图医疗 DICOM 文档站

- 影像操作：<https://blog.iyunto.net/wordpress/docs/dicom/%e5%bd%b1%e5%83%8f%e6%93%8d%e4%bd%9c>
- MPR：<https://blog.iyunto.net/wordpress/docs/dicom/mpr>
- VR：<https://blog.iyunto.net/wordpress/docs/dicom/vr>

### 用户上传资料

- 《易影云影像平台产品使用说明书 V2.0》PDF：仓库内 `.dsh-paste/pasted-1786800124656-b5dbe554.pdf`

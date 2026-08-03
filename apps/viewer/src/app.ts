// 主应用逻辑

import { openDicomFile, closeInstance, buildLut } from './api';
import { Renderer } from './renderer';
import type { ViewState } from './types';

export class App {
  private state: ViewState | null = null;
  private renderer: Renderer;
  private canvas: HTMLCanvasElement;

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
    this.renderer = new Renderer(canvas);
    this.setupEventListeners();
  }

  /**
   * 打开 DICOM 文件
   */
  async openFile(): Promise<void> {
    try {
      const metadata = await openDicomFile();
      if (!metadata) {
        return; // 用户取消
      }

      // 关闭之前打开的实例
      if (this.state) {
        await closeInstance(this.state.metadata.handle);
      }

      // 初始化状态
      const preset = metadata.window_presets[0];
      this.state = {
        metadata,
        currentFrame: 0,
        windowCenter: preset?.center ?? 0,
        windowWidth: preset?.width ?? 4096,
        zoom: 1.0,
        panX: 0,
        panY: 0,
        lut: null,
      };

      // 生成初始 LUT
      await this.updateLut();

      // 加载第一帧
      await this.loadCurrentFrame();

      // 更新 UI
      this.updateUI();
    } catch (error) {
      console.error('Failed to open file:', error);
      alert(`打开文件失败: ${error}`);
    }
  }

  /**
   * 更新 LUT
   */
  private async updateLut(): Promise<void> {
    if (!this.state) return;

    const lut = await buildLut(
      this.state.metadata.handle,
      this.state.windowCenter,
      this.state.windowWidth
    );
    this.state.lut = lut;
  }

  /**
   * 加载当前帧
   */
  private async loadCurrentFrame(): Promise<void> {
    if (!this.state) return;

    const { handle, rows, cols } = this.state.metadata;
    await this.renderer.loadFrame(handle, this.state.currentFrame, rows, cols);
    this.renderer.render(this.state);
  }

  /**
   * 设置窗宽窗位
   */
  async setWindow(center: number, width: number): Promise<void> {
    if (!this.state) return;

    this.state.windowCenter = center;
    this.state.windowWidth = width;

    await this.updateLut();
    this.renderer.render(this.state);
  }

  /**
   * 切换帧
   */
  async setFrame(frame: number): Promise<void> {
    if (!this.state) return;

    this.state.currentFrame = Math.max(0, Math.min(frame, this.state.metadata.frame_count - 1));
    await this.loadCurrentFrame();
    this.updateUI();
  }

  /**
   * 设置缩放
   */
  setZoom(zoom: number): void {
    if (!this.state) return;

    this.state.zoom = Math.max(0.1, Math.min(zoom, 10));
    this.renderer.render(this.state);
  }

  /**
   * 设置平移
   */
  setPan(x: number, y: number): void {
    if (!this.state) return;

    this.state.panX = x;
    this.state.panY = y;
    this.renderer.render(this.state);
  }

  /**
   * 设置事件监听器
   */
  private setupEventListeners(): void {
    // 窗宽窗位拖动
    let isDragging = false;
    let startX = 0;
    let startY = 0;
    let startCenter = 0;
    let startWidth = 0;

    this.canvas.addEventListener('mousedown', (e) => {
      if (!this.state) return;
      isDragging = true;
      startX = e.clientX;
      startY = e.clientY;
      startCenter = this.state.windowCenter;
      startWidth = this.state.windowWidth;
    });

    this.canvas.addEventListener('mousemove', async (e) => {
      if (!isDragging || !this.state) return;

      const dx = e.clientX - startX;
      const dy = e.clientY - startY;

      // 左右拖动调整窗位，上下拖动调整窗宽
      const newCenter = startCenter + dx * 4;
      const newWidth = Math.max(1, startWidth + dy * 4);

      await this.setWindow(newCenter, newWidth);
    });

    window.addEventListener('mouseup', () => {
      isDragging = false;
    });

    // 滚轮缩放
    this.canvas.addEventListener('wheel', (e) => {
      e.preventDefault();
      if (!this.state) return;

      const delta = e.deltaY > 0 ? 0.9 : 1.1;
      this.setZoom(this.state.zoom * delta);
    });
  }

  /**
   * 更新 UI 显示
   */
  private updateUI(): void {
    if (!this.state) return;

    const infoEl = document.getElementById('info');
    if (infoEl) {
      const { metadata, currentFrame, windowCenter, windowWidth } = this.state;
      infoEl.textContent = `帧: ${currentFrame + 1}/${metadata.frame_count} | ` +
        `窗位: ${windowCenter.toFixed(0)} | 窗宽: ${windowWidth.toFixed(0)} | ` +
        `尺寸: ${metadata.cols}×${metadata.rows}`;
    }
  }
}

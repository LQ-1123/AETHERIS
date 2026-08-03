// Canvas 渲染引擎

import type { ViewState } from './types';
import { loadFrame } from './api';

export class Renderer {
  private canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D;
  private frameData: Uint16Array | null = null;

  constructor(canvas: HTMLCanvasElement) {
    this.canvas = canvas;
    const ctx = canvas.getContext('2d');
    if (!ctx) {
      throw new Error('Failed to get 2D context');
    }
    this.ctx = ctx;
  }

  /**
   * 加载帧数据
   */
  async loadFrame(handle: number, frame: number, rows: number, cols: number): Promise<void> {
    const buffer = await loadFrame(handle, frame);
    this.frameData = new Uint16Array(buffer);

    // 调整 canvas 尺寸
    this.canvas.width = cols;
    this.canvas.height = rows;
  }

  /**
   * 渲染当前帧
   *
   * 用 LUT 把 Uint16Array 转成 8 位灰度，然后绘制到 canvas
   */
  render(state: ViewState): void {
    if (!this.frameData || !state.lut) {
      return;
    }

    const { rows, cols } = state.metadata;
    const imageData = this.ctx.createImageData(cols, rows);
    const pixels = imageData.data;

    // 应用 LUT：frameData[i] 是 16 位原始值，lut[value] 是 8 位灰度
    for (let i = 0; i < this.frameData.length; i++) {
      const rawValue = this.frameData[i];
      const gray = state.lut[rawValue];
      const offset = i * 4;
      pixels[offset] = gray;     // R
      pixels[offset + 1] = gray; // G
      pixels[offset + 2] = gray; // B
      pixels[offset + 3] = 255;  // A
    }

    // 清空并绘制
    this.ctx.clearRect(0, 0, this.canvas.width, this.canvas.height);
    this.ctx.putImageData(imageData, 0, 0);

    // 应用缩放和平移
    this.applyTransform(state);
  }

  /**
   * 应用缩放和平移变换
   */
  private applyTransform(state: ViewState): void {
    const { zoom, panX, panY } = state;

    // 重新绘制时需要先获取当前图像
    const imageData = this.ctx.getImageData(0, 0, this.canvas.width, this.canvas.height);

    // 重置变换
    this.ctx.setTransform(1, 0, 0, 1, 0, 0);
    this.ctx.clearRect(0, 0, this.canvas.width, this.canvas.height);

    // 应用新变换
    this.ctx.translate(panX, panY);
    this.ctx.scale(zoom, zoom);

    // 绘制
    this.ctx.putImageData(imageData, 0, 0);
  }

  /**
   * 清空画布
   */
  clear(): void {
    this.ctx.clearRect(0, 0, this.canvas.width, this.canvas.height);
    this.frameData = null;
  }
}

import {
  imageToScreen,
  measurementValue,
  renderTransform,
  screenToImage,
  type ImageGeometry,
  type ViewportSize,
} from './geometry';
import type { FrameMetadata, LengthMeasurement, Point, ViewState, ViewTransform } from './types';

export class Renderer {
  private imageContext: CanvasRenderingContext2D;
  private overlayContext: CanvasRenderingContext2D;
  private sourceCanvas = document.createElement('canvas');
  private sourceContext: CanvasRenderingContext2D;
  private sourceImageData: ImageData | null = null;
  private frameData: Uint8Array | Uint16Array | null = null;
  private frame: FrameMetadata | null = null;
  private viewport: ViewportSize = { width: 1, height: 1 };

  constructor(
    private readonly imageCanvas: HTMLCanvasElement,
    private readonly overlayCanvas: HTMLCanvasElement,
  ) {
    const imageContext = imageCanvas.getContext('2d');
    const overlayContext = overlayCanvas.getContext('2d');
    const sourceContext = this.sourceCanvas.getContext('2d');
    if (!imageContext || !overlayContext || !sourceContext) {
      throw new Error('Canvas 2D 上下文不可用');
    }
    this.imageContext = imageContext;
    this.overlayContext = overlayContext;
    this.sourceContext = sourceContext;
  }

  resize(width: number, height: number): void {
    this.viewport = { width: Math.max(1, width), height: Math.max(1, height) };
    const ratio = window.devicePixelRatio || 1;
    for (const canvas of [this.imageCanvas, this.overlayCanvas]) {
      canvas.width = Math.round(this.viewport.width * ratio);
      canvas.height = Math.round(this.viewport.height * ratio);
      canvas.style.width = `${this.viewport.width}px`;
      canvas.style.height = `${this.viewport.height}px`;
    }
    this.imageContext.setTransform(ratio, 0, 0, ratio, 0, 0);
    this.overlayContext.setTransform(ratio, 0, 0, ratio, 0, 0);
  }

  setFrame(buffer: ArrayBuffer, frame: FrameMetadata): void {
    const bytesPerPixel = frame.bits_allocated / 8;
    const expected = frame.rows * frame.cols * bytesPerPixel;
    if (buffer.byteLength !== expected) {
      throw new Error(`帧数据长度异常: 收到 ${buffer.byteLength} 字节，预期 ${expected} 字节`);
    }
    this.frameData = frame.bits_allocated === 8 ? new Uint8Array(buffer) : new Uint16Array(buffer);
    this.frame = frame;
    this.prepareSourceCanvas(frame);
  }

  applyLut(lut: Uint8Array): void {
    if (!this.frameData || !this.frame) return;
    const imageData = this.prepareSourceCanvas(this.frame);
    const output = imageData.data;
    for (let index = 0; index < this.frameData.length; index += 1) {
      const gray = lut[this.frameData[index]] ?? 0;
      const offset = index * 4;
      output[offset] = gray;
      output[offset + 1] = gray;
      output[offset + 2] = gray;
      output[offset + 3] = 255;
    }
    this.sourceContext.putImageData(imageData, 0, 0);
  }

  setGrayFrame(buffer: ArrayBuffer, frame: FrameMetadata): void {
    const pixels = new Uint8Array(buffer);
    const expected = frame.rows * frame.cols;
    if (pixels.length !== expected) {
      throw new Error(`MPR 切面长度异常: 收到 ${pixels.length} 字节，预期 ${expected} 字节`);
    }
    this.frameData = pixels;
    this.frame = frame;
    const imageData = this.prepareSourceCanvas(frame);
    for (let index = 0; index < pixels.length; index += 1) {
      const offset = index * 4;
      imageData.data[offset] = pixels[index];
      imageData.data[offset + 1] = pixels[index];
      imageData.data[offset + 2] = pixels[index];
      imageData.data[offset + 3] = 255;
    }
    this.sourceContext.putImageData(imageData, 0, 0);
  }

  render(
    state: ViewState,
    measurements: LengthMeasurement[],
    draft: LengthMeasurement | null,
    selectedId: string | null,
  ): void {
    const ratio = window.devicePixelRatio || 1;
    this.imageContext.setTransform(ratio, 0, 0, ratio, 0, 0);
    this.imageContext.clearRect(0, 0, this.viewport.width, this.viewport.height);
    this.overlayContext.setTransform(ratio, 0, 0, ratio, 0, 0);
    this.overlayContext.clearRect(0, 0, this.viewport.width, this.viewport.height);
    const frame = state.metadata.frames[state.currentFrame];
    if (!frame || !this.frameData) return;

    this.renderView(state, frame, measurements, draft, selectedId, null);
  }

  renderMpr(
    view: ViewTransform,
    frame: FrameMetadata,
    measurements: LengthMeasurement[],
    draft: LengthMeasurement | null,
    selectedId: string | null,
    crosshair: Point,
  ): void {
    const ratio = window.devicePixelRatio || 1;
    this.imageContext.setTransform(ratio, 0, 0, ratio, 0, 0);
    this.imageContext.clearRect(0, 0, this.viewport.width, this.viewport.height);
    this.overlayContext.setTransform(ratio, 0, 0, ratio, 0, 0);
    this.overlayContext.clearRect(0, 0, this.viewport.width, this.viewport.height);
    if (!this.frameData) return;
    this.renderView(view, frame, measurements, draft, selectedId, crosshair);
  }

  renderMprOverlay(
    view: ViewTransform,
    frame: FrameMetadata,
    measurements: LengthMeasurement[],
    draft: LengthMeasurement | null,
    selectedId: string | null,
    crosshair: Point,
  ): void {
    const ratio = window.devicePixelRatio || 1;
    this.overlayContext.setTransform(ratio, 0, 0, ratio, 0, 0);
    this.overlayContext.clearRect(0, 0, this.viewport.width, this.viewport.height);
    this.renderOverlay(view, frame, measurements, draft, selectedId, crosshair);
  }

  private renderView(
    view: ViewTransform,
    frame: FrameMetadata,
    measurements: LengthMeasurement[],
    draft: LengthMeasurement | null,
    selectedId: string | null,
    crosshair: Point | null,
  ): void {

    const image = imageGeometry(frame);
    const transform = renderTransform(this.viewport, image, view);
    this.imageContext.imageSmoothingEnabled = false;
    this.imageContext.drawImage(
      this.sourceCanvas,
      transform.originX,
      transform.originY,
      transform.width,
      transform.height,
    );

    this.renderOverlay(view, frame, measurements, draft, selectedId, crosshair);
  }

  private renderOverlay(
    view: ViewTransform,
    frame: FrameMetadata,
    measurements: LengthMeasurement[],
    draft: LengthMeasurement | null,
    selectedId: string | null,
    crosshair: Point | null,
  ): void {
    for (const measurement of measurements) {
      this.drawMeasurement(measurement, frame, view, measurement.id === selectedId, false);
    }
    if (draft) this.drawMeasurement(draft, frame, view, false, true);
    if (crosshair) this.drawCrosshair(crosshair, frame, view);
  }

  toImage(point: Point, state: ViewState): Point {
    const frame = state.metadata.frames[state.currentFrame];
    return screenToImage(point, this.viewport, imageGeometry(frame), state);
  }

  toScreen(point: Point, state: ViewState): Point {
    const frame = state.metadata.frames[state.currentFrame];
    return imageToScreen(point, this.viewport, imageGeometry(frame), state);
  }

  toImageFor(point: Point, frame: FrameMetadata, view: ViewTransform): Point {
    return screenToImage(point, this.viewport, imageGeometry(frame), view);
  }

  toScreenFor(point: Point, frame: FrameMetadata, view: ViewTransform): Point {
    return imageToScreen(point, this.viewport, imageGeometry(frame), view);
  }

  getViewport(): ViewportSize {
    return this.viewport;
  }

  clear(): void {
    this.frameData = null;
    this.frame = null;
    this.sourceImageData = null;
    this.imageContext.clearRect(0, 0, this.viewport.width, this.viewport.height);
    this.overlayContext.clearRect(0, 0, this.viewport.width, this.viewport.height);
  }

  private prepareSourceCanvas(frame: FrameMetadata): ImageData {
    if (this.sourceCanvas.width !== frame.cols || this.sourceCanvas.height !== frame.rows) {
      this.sourceCanvas.width = frame.cols;
      this.sourceCanvas.height = frame.rows;
      this.sourceImageData = null;
    }
    if (!this.sourceImageData) {
      this.sourceImageData = this.sourceContext.createImageData(frame.cols, frame.rows);
    }
    return this.sourceImageData;
  }

  private drawMeasurement(
    measurement: LengthMeasurement,
    frame: FrameMetadata,
    view: ViewTransform,
    selected: boolean,
    draft: boolean,
  ): void {
    const start = this.toScreenFor(measurement.start, frame, view);
    const end = this.toScreenFor(measurement.end, frame, view);
    const color = selected ? '#ffffff' : draft ? '#7fd6ff' : '#ffd166';
    const context = this.overlayContext;
    context.save();
    context.strokeStyle = color;
    context.fillStyle = color;
    context.lineWidth = selected ? 2.5 : 1.75;
    context.setLineDash(draft ? [5, 4] : []);
    context.beginPath();
    context.moveTo(start.x, start.y);
    context.lineTo(end.x, end.y);
    context.stroke();
    context.setLineDash([]);
    for (const point of [start, end]) {
      context.beginPath();
      context.arc(point.x, point.y, selected ? 4 : 3, 0, Math.PI * 2);
      context.fill();
    }

    const distance = measurementValue(measurement.start, measurement.end, frame.spacing);
    const caveat = frame.spacing.confidence === 'detector' ? '  探测器平面' : '';
    const label = `${distance.value.toFixed(distance.unit === 'mm' ? 1 : 0)} ${distance.unit}${caveat}`;
    const midpoint = { x: (start.x + end.x) / 2, y: (start.y + end.y) / 2 };
    context.font = '12px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
    const metrics = context.measureText(label);
    const labelX = Math.min(
      Math.max(6, midpoint.x + 8),
      this.viewport.width - metrics.width - 14,
    );
    const labelY = Math.min(Math.max(20, midpoint.y - 8), this.viewport.height - 8);
    context.fillStyle = 'rgba(10, 14, 18, 0.86)';
    context.fillRect(labelX - 5, labelY - 14, metrics.width + 10, 19);
    context.fillStyle = color;
    context.fillText(label, labelX, labelY);
    context.restore();
  }

  private drawCrosshair(point: Point, frame: FrameMetadata, view: ViewTransform): void {
    const screen = this.toScreenFor(point, frame, view);
    const transform = renderTransform(this.viewport, imageGeometry(frame), view);
    const left = Math.max(0, transform.originX);
    const right = Math.min(this.viewport.width, transform.originX + transform.width);
    const top = Math.max(0, transform.originY);
    const bottom = Math.min(this.viewport.height, transform.originY + transform.height);
    const context = this.overlayContext;
    context.save();
    context.strokeStyle = '#45d4e3';
    context.lineWidth = 1;
    context.setLineDash([5, 4]);
    context.beginPath();
    context.moveTo(left, screen.y);
    context.lineTo(right, screen.y);
    context.moveTo(screen.x, top);
    context.lineTo(screen.x, bottom);
    context.stroke();
    context.setLineDash([]);
    context.beginPath();
    context.arc(screen.x, screen.y, 4, 0, Math.PI * 2);
    context.stroke();
    context.restore();
  }
}

export function imageGeometry(frame: FrameMetadata): ImageGeometry {
  return {
    rows: frame.rows,
    cols: frame.cols,
    columnOverRow: frame.spacing.column_over_row,
  };
}

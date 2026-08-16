import {
  imageToScreen,
  measurementValue,
  renderTransform,
  screenToImage,
  type ImageGeometry,
  type ViewportSize,
} from './geometry';
import { angleDegrees, annotationLabel, annotationPoints } from './annotations';
import type { MaskLayer } from './masks';
import type { Annotation, FrameMetadata, Point, ViewState, ViewTransform } from './types';

export class Renderer {
  private imageContext: CanvasRenderingContext2D;
  private overlayContext: CanvasRenderingContext2D;
  private sourceCanvas = document.createElement('canvas');
  private sourceContext: CanvasRenderingContext2D;
  private sourceImageData: ImageData | null = null;
  private maskCanvas = document.createElement('canvas');
  private maskContext: CanvasRenderingContext2D;
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
    const maskContext = this.maskCanvas.getContext('2d');
    if (!imageContext || !overlayContext || !sourceContext || !maskContext) {
      throw new Error('Canvas 2D 上下文不可用');
    }
    this.imageContext = imageContext;
    this.overlayContext = overlayContext;
    this.sourceContext = sourceContext;
    this.maskContext = maskContext;
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
    const bytesPerPixel = frame.pixel_format === 'rgb8' ? 3 : frame.bits_allocated / 8;
    const expected = frame.rows * frame.cols * bytesPerPixel;
    if (buffer.byteLength !== expected) {
      throw new Error(`帧数据长度异常: 收到 ${buffer.byteLength} 字节，预期 ${expected} 字节`);
    }
    this.frameData = frame.pixel_format === 'gray16' ? new Uint16Array(buffer) : new Uint8Array(buffer);
    this.frame = frame;
    const imageData = this.prepareSourceCanvas(frame);
    if (frame.pixel_format === 'rgb8') {
      const pixels = this.frameData as Uint8Array;
      for (let index = 0; index < frame.rows * frame.cols; index += 1) {
        const source = index * 3;
        const target = index * 4;
        imageData.data[target] = pixels[source];
        imageData.data[target + 1] = pixels[source + 1];
        imageData.data[target + 2] = pixels[source + 2];
        imageData.data[target + 3] = 255;
      }
      this.sourceContext.putImageData(imageData, 0, 0);
    }
  }

  applyLut(lut: Uint8Array): void {
    if (!this.frameData || !this.frame || this.frame.pixel_format === 'rgb8') return;
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

  /** 将 GPU MPR 渲染结果直接绘制到图像 Canvas（与 Overlay 使用同一套视口变换）。 */
  drawExternalImage(source: CanvasImageSource): void {
    this.imageContext.save();
    this.imageContext.clearRect(0, 0, this.viewport.width, this.viewport.height);
    this.imageContext.drawImage(source, 0, 0, this.viewport.width, this.viewport.height);
    this.imageContext.restore();
  }

  render(
    state: ViewState,
    annotations: Annotation[],
    draft: Annotation | null,
    selectedId: string | null,
    annotationsVisible = true,
    masks: MaskLayer[] = [],
  ): void {
    const ratio = window.devicePixelRatio || 1;
    this.imageContext.setTransform(ratio, 0, 0, ratio, 0, 0);
    this.imageContext.clearRect(0, 0, this.viewport.width, this.viewport.height);
    this.overlayContext.setTransform(ratio, 0, 0, ratio, 0, 0);
    this.overlayContext.clearRect(0, 0, this.viewport.width, this.viewport.height);
    const frame = state.metadata.frames[state.currentFrame];
    if (!frame || !this.frameData) return;

    this.renderView(state, frame, annotations, draft, selectedId, null, annotationsVisible, masks);
  }

  renderMpr(
    view: ViewTransform,
    frame: FrameMetadata,
    annotations: Annotation[],
    draft: Annotation | null,
    selectedId: string | null,
    crosshair: Point | null,
    annotationsVisible = true,
    masks: MaskLayer[] = [],
  ): void {
    const ratio = window.devicePixelRatio || 1;
    this.imageContext.setTransform(ratio, 0, 0, ratio, 0, 0);
    this.imageContext.clearRect(0, 0, this.viewport.width, this.viewport.height);
    this.overlayContext.setTransform(ratio, 0, 0, ratio, 0, 0);
    this.overlayContext.clearRect(0, 0, this.viewport.width, this.viewport.height);
    if (!this.frameData) return;
    this.renderView(view, frame, annotations, draft, selectedId, crosshair, annotationsVisible, masks);
  }

  renderMprOverlay(
    view: ViewTransform,
    frame: FrameMetadata,
    annotations: Annotation[],
    draft: Annotation | null,
    selectedId: string | null,
    crosshair: Point | null,
    annotationsVisible = true,
    masks: MaskLayer[] = [],
  ): void {
    const ratio = window.devicePixelRatio || 1;
    this.overlayContext.setTransform(ratio, 0, 0, ratio, 0, 0);
    this.overlayContext.clearRect(0, 0, this.viewport.width, this.viewport.height);
    this.renderOverlay(view, frame, annotations, draft, selectedId, crosshair, annotationsVisible, masks);
  }

  private renderView(
    view: ViewTransform,
    frame: FrameMetadata,
    annotations: Annotation[],
    draft: Annotation | null,
    selectedId: string | null,
    crosshair: Point | null,
    annotationsVisible: boolean,
    masks: MaskLayer[],
  ): void {

    const image = imageGeometry(frame);
    const transform = renderTransform(this.viewport, image, view);
    this.imageContext.save();
    this.imageContext.imageSmoothingEnabled = false;
    this.imageContext.filter = view.inverted ? 'invert(1)' : 'none';
    this.imageContext.translate(transform.centerX, transform.centerY);
    this.imageContext.scale(transform.scale, transform.scale);
    this.imageContext.rotate(view.rotation * Math.PI / 180);
    this.imageContext.scale(view.flipHorizontal ? -1 : 1, view.flipVertical ? -1 : 1);
    this.imageContext.scale(image.columnOverRow, 1);
    this.imageContext.drawImage(
      this.sourceCanvas,
      -frame.cols / 2,
      -frame.rows / 2,
      frame.cols,
      frame.rows,
    );
    this.imageContext.restore();

    this.renderOverlay(view, frame, annotations, draft, selectedId, crosshair, annotationsVisible, masks);
  }

  private renderOverlay(
    view: ViewTransform,
    frame: FrameMetadata,
    annotations: Annotation[],
    draft: Annotation | null,
    selectedId: string | null,
    crosshair: Point | null,
    annotationsVisible: boolean,
    masks: MaskLayer[] = [],
  ): void {
    if (masks.length) this.drawMasks(masks, frame, view);
    if (annotationsVisible) {
      for (const annotation of annotations) {
        this.drawAnnotation(annotation, frame, view, annotation.id === selectedId, false);
      }
      if (draft) this.drawAnnotation(draft, frame, view, false, true);
    }
    if (crosshair) this.drawCrosshair(crosshair, frame, view);
  }

  private drawMasks(masks: MaskLayer[], frame: FrameMetadata, view: ViewTransform): void {
    if (this.maskCanvas.width !== frame.cols || this.maskCanvas.height !== frame.rows) {
      this.maskCanvas.width = frame.cols;
      this.maskCanvas.height = frame.rows;
    }
    const pixels = this.maskContext.createImageData(frame.cols, frame.rows);
    for (const layer of masks) {
      if (layer.rows !== frame.rows || layer.cols !== frame.cols || layer.data.length !== frame.rows * frame.cols) {
        continue;
      }
      const alpha = Math.round(Math.max(0, Math.min(1, layer.opacity)) * 255);
      for (let index = 0; index < layer.data.length; index += 1) {
        if (!layer.data[index]) continue;
        const offset = index * 4;
        pixels.data[offset] = layer.color[0];
        pixels.data[offset + 1] = layer.color[1];
        pixels.data[offset + 2] = layer.color[2];
        pixels.data[offset + 3] = alpha;
      }
    }
    this.maskContext.clearRect(0, 0, frame.cols, frame.rows);
    this.maskContext.putImageData(pixels, 0, 0);
    const image = imageGeometry(frame);
    const transform = renderTransform(this.viewport, image, view);
    const context = this.overlayContext;
    context.save();
    context.imageSmoothingEnabled = false;
    context.translate(transform.centerX, transform.centerY);
    context.scale(transform.scale, transform.scale);
    context.rotate(view.rotation * Math.PI / 180);
    context.scale(view.flipHorizontal ? -1 : 1, view.flipVertical ? -1 : 1);
    context.scale(image.columnOverRow, 1);
    context.drawImage(this.maskCanvas, -frame.cols / 2, -frame.rows / 2, frame.cols, frame.rows);
    context.restore();
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

  private drawAnnotation(
    annotation: Annotation,
    frame: FrameMetadata,
    view: ViewTransform,
    selected: boolean,
    draft: boolean,
  ): void {
    const points = annotationPoints(annotation).map((point) => this.toScreenFor(point, frame, view));
    const color = annotation.syncState === 'error'
      ? '#ff7a7a'
      : selected ? '#ffffff' : draft || annotation.syncState === 'pending' ? '#7fd6ff' : '#ffd166';
    const context = this.overlayContext;
    context.save();
    context.strokeStyle = color;
    context.fillStyle = color;
    context.lineWidth = selected ? 2.5 : 1.75;
    context.setLineDash(draft ? [5, 4] : []);
    this.drawAnnotationPath(context, annotation, points, frame, view);
    context.setLineDash([]);
    if (annotation.kind === 'arrow') drawArrowHead(context, points[0], points[1], color);
    for (const point of points) {
      context.beginPath();
      context.arc(point.x, point.y, selected ? 4 : 2.5, 0, Math.PI * 2);
      selected ? context.fill() : context.stroke();
    }

    const label = this.annotationValue(annotation, frame);
    if (label) this.drawLabel(label, points[points.length - 1], color);
    context.restore();
  }

  private drawAnnotationPath(
    context: CanvasRenderingContext2D,
    annotation: Annotation,
    points: Point[],
    frame: FrameMetadata,
    view: ViewTransform,
  ): void {
    context.beginPath();
    if (annotation.kind === 'ellipse_roi') {
      const imageStart = annotation.start;
      const imageEnd = annotation.end;
      const center = { x: (imageStart.x + imageEnd.x) / 2, y: (imageStart.y + imageEnd.y) / 2 };
      const rx = Math.abs(imageEnd.x - imageStart.x) / 2;
      const ry = Math.abs(imageEnd.y - imageStart.y) / 2;
      for (let index = 0; index <= 64; index += 1) {
        const angle = index / 64 * Math.PI * 2;
        const screen = this.toScreenFor(
          { x: center.x + Math.cos(angle) * rx, y: center.y + Math.sin(angle) * ry },
          frame,
          view,
        );
        if (index === 0) context.moveTo(screen.x, screen.y);
        else context.lineTo(screen.x, screen.y);
      }
      context.closePath();
    } else if (annotation.kind === 'rectangle_roi') {
      const corners = [
        annotation.start,
        { x: annotation.end.x, y: annotation.start.y },
        annotation.end,
        { x: annotation.start.x, y: annotation.end.y },
      ].map((point) => this.toScreenFor(point, frame, view));
      context.moveTo(corners[0].x, corners[0].y);
      for (const corner of corners.slice(1)) context.lineTo(corner.x, corner.y);
      context.closePath();
    } else if (annotation.kind === 'angle') {
      context.moveTo(points[0].x, points[0].y);
      context.lineTo(points[1].x, points[1].y);
      context.lineTo(points[2].x, points[2].y);
    } else if (annotation.kind === 'point_probe') {
      const point = points[0];
      context.moveTo(point.x - 7, point.y);
      context.lineTo(point.x + 7, point.y);
      context.moveTo(point.x, point.y - 7);
      context.lineTo(point.x, point.y + 7);
    } else {
      context.moveTo(points[0].x, points[0].y);
      context.lineTo(points[1].x, points[1].y);
    }
    context.stroke();
  }

  private annotationValue(annotation: Annotation, frame: FrameMetadata): string | null {
    const statistics = annotationLabel(annotation);
    if (statistics) return statistics;
    if (annotation.kind === 'length') {
      const distance = measurementValue(annotation.start, annotation.end, frame.spacing);
      const caveat = frame.spacing.confidence === 'detector' ? '  探测器平面' : '';
      return `${distance.value.toFixed(distance.unit === 'mm' ? 1 : 0)} ${distance.unit}${caveat}`;
    }
    if (annotation.kind === 'angle') return `${angleDegrees(annotation).toFixed(1)}°`;
    if (annotation.measurementError) return annotation.measurementError;
    return annotation.syncState === 'error' ? '同步失败' : null;
  }

  private drawLabel(label: string, anchor: Point, color: string): void {
    const context = this.overlayContext;
    const lines = label.split('\n');
    context.font = '12px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
    const width = Math.max(...lines.map((line) => context.measureText(line).width));
    const x = Math.min(Math.max(6, anchor.x + 8), this.viewport.width - width - 14);
    const y = Math.min(Math.max(20, anchor.y - 8), this.viewport.height - lines.length * 16 - 4);
    context.fillStyle = 'rgba(10, 14, 18, 0.88)';
    context.fillRect(x - 5, y - 14, width + 10, lines.length * 16 + 3);
    context.fillStyle = color;
    lines.forEach((line, index) => context.fillText(line, x, y + index * 16));
  }

  private drawCrosshair(point: Point, frame: FrameMetadata, view: ViewTransform): void {
    const screen = this.toScreenFor(point, frame, view);
    const transform = renderTransform(this.viewport, imageGeometry(frame), view);
    const left = Math.max(0, transform.centerX - transform.width / 2);
    const right = Math.min(this.viewport.width, transform.centerX + transform.width / 2);
    const top = Math.max(0, transform.centerY - transform.height / 2);
    const bottom = Math.min(this.viewport.height, transform.centerY + transform.height / 2);
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

function drawArrowHead(
  context: CanvasRenderingContext2D,
  start: Point,
  end: Point,
  color: string,
): void {
  const angle = Math.atan2(end.y - start.y, end.x - start.x);
  const size = 12;
  context.save();
  context.fillStyle = color;
  context.beginPath();
  context.moveTo(end.x, end.y);
  context.lineTo(end.x - Math.cos(angle - Math.PI / 6) * size, end.y - Math.sin(angle - Math.PI / 6) * size);
  context.lineTo(end.x - Math.cos(angle + Math.PI / 6) * size, end.y - Math.sin(angle + Math.PI / 6) * size);
  context.closePath();
  context.fill();
  context.restore();
}

export function imageGeometry(frame: FrameMetadata): ImageGeometry {
  return {
    rows: frame.rows,
    cols: frame.cols,
    columnOverRow: frame.spacing.column_over_row,
  };
}

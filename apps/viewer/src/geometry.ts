import type { Point, SpacingInfo, ViewTransform } from './types';

export interface ViewportSize {
  width: number;
  height: number;
}

export interface ImageGeometry {
  rows: number;
  cols: number;
  columnOverRow: number;
}

export interface RenderTransform {
  originX: number;
  originY: number;
  width: number;
  height: number;
  scale: number;
}

export function fitScale(viewport: ViewportSize, image: ImageGeometry): number {
  const displayWidth = image.cols * saneAspect(image.columnOverRow);
  if (viewport.width <= 0 || viewport.height <= 0 || displayWidth <= 0 || image.rows <= 0) {
    return 1;
  }
  return Math.min(viewport.width / displayWidth, viewport.height / image.rows);
}

export function renderTransform(
  viewport: ViewportSize,
  image: ImageGeometry,
  view: ViewTransform,
): RenderTransform {
  const aspect = saneAspect(image.columnOverRow);
  const scale = fitScale(viewport, image) * view.zoom;
  const width = image.cols * aspect * scale;
  const height = image.rows * scale;
  return {
    originX: viewport.width / 2 + view.panX - width / 2,
    originY: viewport.height / 2 + view.panY - height / 2,
    width,
    height,
    scale,
  };
}

export function imageToScreen(
  point: Point,
  viewport: ViewportSize,
  image: ImageGeometry,
  view: ViewTransform,
): Point {
  const transform = renderTransform(viewport, image, view);
  return {
    x: transform.originX + point.x * saneAspect(image.columnOverRow) * transform.scale,
    y: transform.originY + point.y * transform.scale,
  };
}

export function screenToImage(
  point: Point,
  viewport: ViewportSize,
  image: ImageGeometry,
  view: ViewTransform,
): Point {
  const transform = renderTransform(viewport, image, view);
  return {
    x: (point.x - transform.originX) / (saneAspect(image.columnOverRow) * transform.scale),
    y: (point.y - transform.originY) / transform.scale,
  };
}

export function zoomAt(
  view: ViewTransform,
  cursor: Point,
  nextZoom: number,
  viewport: ViewportSize,
  image: ImageGeometry,
): ViewTransform {
  const currentFit = fitScale(viewport, image);
  const centerX = viewport.width / 2;
  const centerY = viewport.height / 2;
  const localX = (cursor.x - centerX - view.panX) / (currentFit * view.zoom);
  const localY = (cursor.y - centerY - view.panY) / (currentFit * view.zoom);
  return {
    zoom: nextZoom,
    panX: cursor.x - centerX - localX * currentFit * nextZoom,
    panY: cursor.y - centerY - localY * currentFit * nextZoom,
  };
}

export function clampToImage(point: Point, image: ImageGeometry): Point {
  return {
    x: Math.max(0, Math.min(image.cols, point.x)),
    y: Math.max(0, Math.min(image.rows, point.y)),
  };
}

export function measurementValue(start: Point, end: Point, spacing: SpacingInfo): {
  value: number;
  unit: 'mm' | 'px';
} {
  const dx = end.x - start.x;
  const dy = end.y - start.y;
  if (spacing.row_mm != null && spacing.col_mm != null) {
    return {
      value: Math.hypot(dx * spacing.col_mm, dy * spacing.row_mm),
      unit: 'mm',
    };
  }
  return { value: Math.hypot(dx, dy), unit: 'px' };
}

export function pointToSegmentDistance(point: Point, start: Point, end: Point): number {
  const dx = end.x - start.x;
  const dy = end.y - start.y;
  if (dx === 0 && dy === 0) return Math.hypot(point.x - start.x, point.y - start.y);
  const t = Math.max(
    0,
    Math.min(1, ((point.x - start.x) * dx + (point.y - start.y) * dy) / (dx * dx + dy * dy)),
  );
  return Math.hypot(point.x - (start.x + t * dx), point.y - (start.y + t * dy));
}

function saneAspect(value: number): number {
  return Number.isFinite(value) && value > 0 ? value : 1;
}

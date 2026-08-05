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
  centerX: number;
  centerY: number;
  width: number;
  height: number;
  scale: number;
}

export function fitScale(
  viewport: ViewportSize,
  image: ImageGeometry,
  rotation: ViewTransform['rotation'] = 0,
): number {
  const sourceWidth = image.cols * saneAspect(image.columnOverRow);
  const quarterTurn = rotation === 90 || rotation === 270;
  const displayWidth = quarterTurn ? image.rows : sourceWidth;
  const displayHeight = quarterTurn ? sourceWidth : image.rows;
  if (viewport.width <= 0 || viewport.height <= 0 || displayWidth <= 0 || displayHeight <= 0) {
    return 1;
  }
  return Math.min(viewport.width / displayWidth, viewport.height / displayHeight);
}

export function renderTransform(
  viewport: ViewportSize,
  image: ImageGeometry,
  view: ViewTransform,
): RenderTransform {
  const aspect = saneAspect(image.columnOverRow);
  const scale = fitScale(viewport, image, view.rotation) * view.zoom;
  const sourceWidth = image.cols * aspect * scale;
  const sourceHeight = image.rows * scale;
  const quarterTurn = view.rotation === 90 || view.rotation === 270;
  const width = quarterTurn ? sourceHeight : sourceWidth;
  const height = quarterTurn ? sourceWidth : sourceHeight;
  return {
    centerX: viewport.width / 2 + view.panX,
    centerY: viewport.height / 2 + view.panY,
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
  let x = (point.x - image.cols / 2) * saneAspect(image.columnOverRow);
  let y = point.y - image.rows / 2;
  if (view.flipHorizontal) x = -x;
  if (view.flipVertical) y = -y;
  [x, y] = rotate(x, y, view.rotation);
  return { x: transform.centerX + x * transform.scale, y: transform.centerY + y * transform.scale };
}

export function screenToImage(
  point: Point,
  viewport: ViewportSize,
  image: ImageGeometry,
  view: ViewTransform,
): Point {
  const transform = renderTransform(viewport, image, view);
  let x = (point.x - transform.centerX) / transform.scale;
  let y = (point.y - transform.centerY) / transform.scale;
  [x, y] = rotate(x, y, ((360 - view.rotation) % 360) as ViewTransform['rotation']);
  if (view.flipHorizontal) x = -x;
  if (view.flipVertical) y = -y;
  return { x: x / saneAspect(image.columnOverRow) + image.cols / 2, y: y + image.rows / 2 };
}

export function zoomAt(
  view: ViewTransform,
  cursor: Point,
  nextZoom: number,
  viewport: ViewportSize,
  image: ImageGeometry,
): ViewTransform {
  const currentFit = fitScale(viewport, image, view.rotation);
  const centerX = viewport.width / 2;
  const centerY = viewport.height / 2;
  const localX = (cursor.x - centerX - view.panX) / (currentFit * view.zoom);
  const localY = (cursor.y - centerY - view.panY) / (currentFit * view.zoom);
  return {
    zoom: nextZoom,
    panX: cursor.x - centerX - localX * currentFit * nextZoom,
    panY: cursor.y - centerY - localY * currentFit * nextZoom,
    rotation: view.rotation,
    flipHorizontal: view.flipHorizontal,
    flipVertical: view.flipVertical,
    inverted: view.inverted,
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

function rotate(x: number, y: number, rotation: ViewTransform['rotation']): [number, number] {
  if (rotation === 90) return [-y, x];
  if (rotation === 180) return [-x, -y];
  if (rotation === 270) return [y, -x];
  return [x, y];
}

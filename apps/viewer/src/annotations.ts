import { pointToSegmentDistance } from './geometry';
import type { Annotation, AnnotationKind, Point } from './types';

export interface AnnotationHit {
  annotation: Annotation;
  handle: number | null;
}

export interface AnnotationHistoryEntry {
  key: string;
  before: Annotation[];
  after: Annotation[];
}

export interface AnnotationHistoryBatch {
  changes: AnnotationHistoryEntry[];
}

export type AnnotationHistoryItem = AnnotationHistoryEntry | AnnotationHistoryBatch;

export class AnnotationHistory {
  private undoEntries: AnnotationHistoryItem[] = [];
  private redoEntries: AnnotationHistoryItem[] = [];

  constructor(private readonly limit = 100) {}

  push(entry: AnnotationHistoryEntry): void {
    this.pushItem(cloneHistoryEntry(entry));
  }

  pushBatch(changes: AnnotationHistoryEntry[]): void {
    if (!changes.length) return;
    this.pushItem({ changes: changes.map(cloneHistoryEntry) });
  }

  private pushItem(entry: AnnotationHistoryItem): void {
    this.undoEntries.push(cloneHistoryItem(entry));
    if (this.undoEntries.length > this.limit) this.undoEntries.shift();
    this.redoEntries = [];
  }

  undo(): AnnotationHistoryItem | null {
    const entry = this.undoEntries.pop();
    if (!entry) return null;
    this.redoEntries.push(cloneHistoryItem(entry));
    return cloneHistoryItem(entry);
  }

  redo(): AnnotationHistoryItem | null {
    const entry = this.redoEntries.pop();
    if (!entry) return null;
    this.undoEntries.push(cloneHistoryItem(entry));
    return cloneHistoryItem(entry);
  }

  get canUndo(): boolean {
    return this.undoEntries.length > 0;
  }

  get canRedo(): boolean {
    return this.redoEntries.length > 0;
  }

  clear(): void {
    this.undoEntries = [];
    this.redoEntries = [];
  }
}

export function createAnnotation(kind: AnnotationKind, point: Point, id: string): Annotation {
  if (kind === 'point_probe') return { id, kind, point: { ...point } };
  if (kind === 'angle') {
    return { id, kind, start: { ...point }, vertex: { ...point }, end: { ...point } };
  }
  return { id, kind, start: { ...point }, end: { ...point } };
}

export function annotationPoints(annotation: Annotation): Point[] {
  if (annotation.kind === 'point_probe') return [annotation.point];
  if (annotation.kind === 'angle') return [annotation.start, annotation.vertex, annotation.end];
  return [annotation.start, annotation.end];
}

export function updateAnnotationPoint(annotation: Annotation, index: number, point: Point): void {
  if (annotation.kind === 'point_probe') annotation.point = { ...point };
  else if (annotation.kind === 'angle') {
    if (index === 0) annotation.start = { ...point };
    else if (index === 1) annotation.vertex = { ...point };
    else annotation.end = { ...point };
  } else if (index === 0) annotation.start = { ...point };
  else annotation.end = { ...point };
  clearStatistics(annotation);
}

export function translateAnnotation(annotation: Annotation, delta: Point): void {
  for (let index = 0; index < annotationPoints(annotation).length; index += 1) {
    const point = annotationPoints(annotation)[index];
    updateAnnotationPoint(annotation, index, { x: point.x + delta.x, y: point.y + delta.y });
  }
}

export function annotationHitTest(
  annotation: Annotation,
  screenPoint: Point,
  toScreen: (point: Point) => Point,
  tolerance = 8,
): { handle: number | null; distance: number } | null {
  const points = annotationPoints(annotation).map(toScreen);
  for (let index = 0; index < points.length; index += 1) {
    const distance = Math.hypot(screenPoint.x - points[index].x, screenPoint.y - points[index].y);
    if (distance <= tolerance) return { handle: index, distance };
  }
  if (annotation.kind === 'point_probe') return null;
  if (annotation.kind === 'ellipse_roi') {
    const center = midpoint(points[0], points[1]);
    const rx = Math.abs(points[1].x - points[0].x) / 2;
    const ry = Math.abs(points[1].y - points[0].y) / 2;
    if (rx < 1 || ry < 1) return null;
    const normalized = Math.hypot((screenPoint.x - center.x) / rx, (screenPoint.y - center.y) / ry);
    return Math.abs(normalized - 1) * Math.min(rx, ry) <= tolerance
      ? { handle: null, distance: 0 }
      : null;
  }
  if (annotation.kind === 'rectangle_roi') {
    const [a, b] = points;
    const corners = [a, { x: b.x, y: a.y }, b, { x: a.x, y: b.y }];
    const distance = Math.min(
      ...corners.map((point, index) =>
        pointToSegmentDistance(screenPoint, point, corners[(index + 1) % corners.length]),
      ),
    );
    return distance <= tolerance ? { handle: null, distance } : null;
  }
  const segments = annotation.kind === 'angle'
    ? [[points[0], points[1]], [points[1], points[2]]]
    : [[points[0], points[1]]];
  const distance = Math.min(...segments.map(([a, b]) => pointToSegmentDistance(screenPoint, a, b)));
  return distance <= tolerance ? { handle: null, distance } : null;
}

export function cloneAnnotations(annotations: Annotation[]): Annotation[] {
  return annotations.map((annotation) => structuredClone(annotation));
}

export function annotationLabel(annotation: Annotation): string | null {
  const statistics = annotation.kind === 'point_probe' || annotation.kind === 'ellipse_roi' || annotation.kind === 'rectangle_roi'
    ? annotation.statistics
    : undefined;
  if (!statistics) return null;
  const unit = statistics.unit ? ` ${statistics.unit}` : '';
  if (annotation.kind === 'point_probe') return `${formatNumber(statistics.mean)}${unit}`;
  const area = statistics.area == null
    ? ''
    : `  ${formatNumber(statistics.area)} ${statistics.area_unit === 'mm2' ? 'mm²' : 'px²'}`;
  return `均值 ${formatNumber(statistics.mean)}${unit}  SD ${formatNumber(statistics.standard_deviation)}\n最小 ${formatNumber(statistics.minimum)}  最大 ${formatNumber(statistics.maximum)}${area}`;
}

export function angleDegrees(annotation: Annotation): number {
  if (annotation.kind !== 'angle') return 0;
  const first = Math.atan2(annotation.start.y - annotation.vertex.y, annotation.start.x - annotation.vertex.x);
  const second = Math.atan2(annotation.end.y - annotation.vertex.y, annotation.end.x - annotation.vertex.x);
  let degrees = Math.abs((second - first) * 180 / Math.PI) % 360;
  if (degrees > 180) degrees = 360 - degrees;
  return degrees;
}

function clearStatistics(annotation: Annotation): void {
  if (annotation.kind === 'point_probe' || annotation.kind === 'ellipse_roi' || annotation.kind === 'rectangle_roi') {
    annotation.statistics = undefined;
    annotation.measurementError = undefined;
  }
}

function midpoint(a: Point, b: Point): Point {
  return { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
}

function formatNumber(value: number): string {
  return Math.abs(value) >= 100 ? value.toFixed(0) : value.toFixed(1);
}

function cloneHistoryEntry(entry: AnnotationHistoryEntry): AnnotationHistoryEntry {
  return { key: entry.key, before: cloneAnnotations(entry.before), after: cloneAnnotations(entry.after) };
}

function cloneHistoryItem(entry: AnnotationHistoryItem): AnnotationHistoryItem {
  return 'changes' in entry
    ? { changes: entry.changes.map(cloneHistoryEntry) }
    : cloneHistoryEntry(entry);
}

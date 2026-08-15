import type { Point } from './types';

/**
 * Pure geometry helpers for cross-series synchronization and cross-reference
 * (localizer) lines in the 2D viewer.
 *
 * These helpers intentionally accept a minimal structural interface instead of
 * `FrameMetadata` so they can be unit-tested before the Rust layer exposes the
 * patient-space geometry fields.
 */

export interface SeriesGeometryFrame {
  rows: number;
  cols: number;
  /** ImagePositionPatient, patient position of the center of voxel (0, 0). */
  position: [number, number, number] | null;
  /** ImageOrientationPatient: row direction followed by column direction. */
  orientation:
    | [number, number, number, number, number, number]
    | null;
  rowSpacingMm: number | null;
  colSpacingMm: number | null;
}

export interface FramePlane {
  origin: [number, number, number];
  rowDirection: [number, number, number];
  columnDirection: [number, number, number];
  normal: [number, number, number];
}

export interface CrossReferenceSegment {
  start: Point;
  end: Point;
}

const ORIENTATION_TOLERANCE = 1e-4;

export function dot(a: number[], b: number[]): number {
  return a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
}

export function cross(
  a: [number, number, number],
  b: [number, number, number],
): [number, number, number] {
  return [
    a[1] * b[2] - a[2] * b[1],
    a[2] * b[0] - a[0] * b[2],
    a[0] * b[1] - a[1] * b[0],
  ];
}

export function normalize(value: [number, number, number]): [number, number, number] | null {
  const length = Math.hypot(value[0], value[1], value[2]);
  if (!Number.isFinite(length) || length <= 0) return null;
  return [value[0] / length, value[1] / length, value[2] / length];
}

/** Extract the patient-space plane of one DICOM frame. */
export function framePlane(frame: SeriesGeometryFrame): FramePlane | null {
  if (!frame.position || !frame.orientation) return null;
  const rowDirection = normalize(frame.orientation.slice(0, 3) as [number, number, number]);
  const columnDirection = normalize(frame.orientation.slice(3, 6) as [number, number, number]);
  if (!rowDirection || !columnDirection) return null;
  const normal = normalize(cross(rowDirection, columnDirection));
  if (!normal) return null;
  return {
    origin: [...frame.position] as [number, number, number],
    rowDirection,
    columnDirection,
    normal,
  };
}

/** True when two frames describe parallel image planes. */
export function framesAreParallel(
  left: SeriesGeometryFrame,
  right: SeriesGeometryFrame,
  tolerance = ORIENTATION_TOLERANCE,
): boolean {
  const leftPlane = framePlane(left);
  const rightPlane = framePlane(right);
  if (!leftPlane || !rightPlane) return false;
  return Math.abs(dot(leftPlane.normal, rightPlane.normal)) >= 1 - tolerance;
}

/** Signed patient position of a frame along the plane normal, in millimetres. */
export function slicePositionMm(frame: SeriesGeometryFrame): number | null {
  const plane = framePlane(frame);
  if (!plane) return null;
  return dot(plane.origin, plane.normal);
}

/**
 * Map a source frame to the index of the closest parallel target frame by
 * patient-space slice position. Returns null when geometry is unavailable or
 * the target stack is empty.
 */
export function nearestParallelFrameIndex(
  source: SeriesGeometryFrame,
  targetFrames: SeriesGeometryFrame[],
): number | null {
  const sourcePosition = slicePositionMm(source);
  if (sourcePosition == null || targetFrames.length === 0) return null;
  let bestIndex: number | null = null;
  let bestDistance = Number.POSITIVE_INFINITY;
  for (let index = 0; index < targetFrames.length; index += 1) {
    const position = slicePositionMm(targetFrames[index]);
    if (position == null || !framesAreParallel(source, targetFrames[index])) continue;
    const distance = Math.abs(position - sourcePosition);
    if (distance < bestDistance) {
      bestDistance = distance;
      bestIndex = index;
    }
  }
  return bestIndex;
}

/**
 * Intersection of two image planes, projected to the image coordinates of the
 * target frame. Used to draw cross-reference/localizer lines.
 */
export function crossReferenceSegment(
  reference: SeriesGeometryFrame,
  target: SeriesGeometryFrame,
  extentMm = 800,
): CrossReferenceSegment | null {
  const referencePlane = framePlane(reference);
  const targetPlane = framePlane(target);
  if (
    !referencePlane
    || !targetPlane
    || !reference.rowSpacingMm
    || !reference.colSpacingMm
    || !target.rowSpacingMm
    || !target.colSpacingMm
  ) {
    return null;
  }
  const direction = cross(targetPlane.normal, referencePlane.normal);
  const lengthSquared = dot(direction, direction);
  if (lengthSquared <= 1e-12) return null; // parallel planes do not intersect
  const constantTarget = dot(targetPlane.origin, targetPlane.normal);
  const constantReference = dot(referencePlane.origin, referencePlane.normal);
  const referenceCrossDirection = cross(referencePlane.normal, direction);
  const directionCrossTarget = cross(direction, targetPlane.normal);
  const point: [number, number, number] = [
    (constantTarget * referenceCrossDirection[0] + constantReference * directionCrossTarget[0])
      / lengthSquared,
    (constantTarget * referenceCrossDirection[1] + constantReference * directionCrossTarget[1])
      / lengthSquared,
    (constantTarget * referenceCrossDirection[2] + constantReference * directionCrossTarget[2])
      / lengthSquared,
  ];
  const unitDirection = normalize(direction);
  if (!unitDirection) return null;

  const project = (patient: [number, number, number]): Point => {
    const relative: [number, number, number] = [
      patient[0] - targetPlane.origin[0],
      patient[1] - targetPlane.origin[1],
      patient[2] - targetPlane.origin[2],
    ];
    return {
      x: dot(relative, targetPlane.columnDirection) / target.colSpacingMm!,
      y: dot(relative, targetPlane.rowDirection) / target.rowSpacingMm!,
    };
  };

  const startPatient: [number, number, number] = [
    point[0] - unitDirection[0] * extentMm,
    point[1] - unitDirection[1] * extentMm,
    point[2] - unitDirection[2] * extentMm,
  ];
  const endPatient: [number, number, number] = [
    point[0] + unitDirection[0] * extentMm,
    point[1] + unitDirection[1] * extentMm,
    point[2] + unitDirection[2] * extentMm,
  ];
  return { start: project(startPatient), end: project(endPatient) };
}

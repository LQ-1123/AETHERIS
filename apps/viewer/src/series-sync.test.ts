import { describe, expect, it } from 'vitest';
import {
  crossReferenceSegment,
  framePlane,
  framesAreParallel,
  nearestParallelFrameIndex,
  slicePositionMm,
  type SeriesGeometryFrame,
} from './series-sync';

function axialFrame(position: [number, number, number]): SeriesGeometryFrame {
  return {
    rows: 512,
    cols: 512,
    position,
    orientation: [1, 0, 0, 0, 1, 0],
    rowSpacingMm: 0.5,
    colSpacingMm: 0.5,
  };
}

describe('series-sync geometry', () => {
  it('extracts normal and slice position for axial frames', () => {
    const frame = axialFrame([0, 0, 4]);
    const plane = framePlane(frame);
    expect(plane?.normal).toEqual([0, 0, 1]);
    expect(slicePositionMm(frame)).toBeCloseTo(4, 8);
  });

  it('detects parallel and non-parallel planes', () => {
    expect(framesAreParallel(axialFrame([0, 0, 0]), axialFrame([10, 5, 3]))).toBe(true);
    expect(
      framesAreParallel(
        axialFrame([0, 0, 0]),
        {
          ...axialFrame([0, 0, 0]),
          orientation: [0, 1, 0, 0, 0, -1],
        },
      ),
    ).toBe(false);
  });

  it('maps a source slice position to the closest parallel target frame', () => {
    const target = Array.from({ length: 5 }, (_, index) => axialFrame([0, 0, index]));
    expect(nearestParallelFrameIndex(axialFrame([0, 0, 2.4]), target)).toBe(2);
    expect(nearestParallelFrameIndex(axialFrame([0, 0, 9]), target)).toBe(4);
  });

  it('returns null when geometry is missing', () => {
    const missing: SeriesGeometryFrame = {
      rows: 10,
      cols: 10,
      position: null,
      orientation: null,
      rowSpacingMm: null,
      colSpacingMm: null,
    };
    expect(nearestParallelFrameIndex(missing, [missing])).toBeNull();
    expect(framePlane(missing)).toBeNull();
  });

  it('projects the intersection of perpendicular planes onto the target image', () => {
    // Axial target at z=0, sagittal reference plane x=25.
    const target = axialFrame([0, 0, 0]);
    const sagittal: SeriesGeometryFrame = {
      rows: 512,
      cols: 512,
      position: [25, 0, 0],
      orientation: [0, 1, 0, 0, 0, -1],
      rowSpacingMm: 0.5,
      colSpacingMm: 0.5,
    };
    const segment = crossReferenceSegment(sagittal, target, 400);
    expect(segment).not.toBeNull();
    // The sagittal plane x=25 is parallel to the target's y axis, so the
    // line has constant image row (x/rowSpacing) and spans the image columns.
    expect(segment!.start.y).toBeCloseTo(25 / 0.5, 8);
    expect(segment!.end.y).toBeCloseTo(25 / 0.5, 8);
    expect(segment!.start.x).not.toBeCloseTo(segment!.end.x, 8);
    expect(Math.abs(segment!.start.x)).toBeCloseTo(800, 8);
    expect(Math.abs(segment!.end.x)).toBeCloseTo(800, 8);
  });

  it('returns null for parallel planes', () => {
    expect(
      crossReferenceSegment(axialFrame([0, 0, 0]), axialFrame([0, 0, 4])),
    ).toBeNull();
  });
});


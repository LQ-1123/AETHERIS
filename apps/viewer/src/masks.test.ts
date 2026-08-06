import { describe, expect, it } from 'vitest';
import {
  createMask,
  decodeMaskRle,
  encodeMaskRle,
  createMaskVolume,
  calculateMaskStatistics,
  paintMaskVolumePlane,
  paintMaskSourcePlane,
  paintMaskStroke,
  restoreMaskSlices,
} from './masks';
import type { MprMetadata } from './types';

describe('mask editing and RLE', () => {
  it('round-trips sparse and full masks', () => {
    for (const source of [
      Uint8Array.from([0, 0, 1, 1, 0, 1, 0, 0]),
      Uint8Array.from([1, 1, 1, 1]),
      Uint8Array.from([0, 0, 0, 0]),
    ]) {
      expect(decodeMaskRle(encodeMaskRle(source), source.length)).toEqual(source);
    }
  });

  it('interpolates a continuous circular brush stroke', () => {
    const mask = createMask(20, 20);
    paintMaskStroke(mask, 20, 20, { x: 2, y: 10 }, { x: 17, y: 10 }, 1.5, 1);
    for (let x = 2; x <= 16; x += 1) expect(mask[10 * 20 + x]).toBe(1);
    paintMaskStroke(mask, 20, 20, { x: 8, y: 10 }, { x: 12, y: 10 }, 1.5, 0);
    expect(mask[10 * 20 + 10]).toBe(0);
  });

  it('rejects RLE with the wrong pixel count', () => {
    expect(() => decodeMaskRle(encodeMaskRle(Uint8Array.of(0, 1)), 3)).toThrow();
  });

  it('edits one MPR layer and maps it back to source voxels', () => {
    const metadata: MprMetadata = {
      stack_index: 0,
      dimensions: [5, 5, 5],
      source_spacing_mm: [1, 1, 1],
      source_origin: [0, 0, 0],
      source_x_axis: [1, 0, 0],
      source_y_axis: [0, 1, 0],
      source_normal: [0, 0, 1],
      source_slices: [],
      patient_bounds_min: [0, 0, 0],
      patient_bounds_max: [4, 4, 4],
      initial_crosshair: [2, 2, 2],
      planes: [{
        plane: 'axial', rows: 5, cols: 5, slice_count: 5,
        pixel_spacing_mm: 1, slice_spacing_mm: 1,
        origin: [0, 0, 0], x_axis: [1, 0, 0], y_axis: [0, 1, 0], normal: [0, 0, 1],
      }],
    };
    const volume = createMaskVolume(5, 5, 5);
    const changed = paintMaskVolumePlane(volume, metadata, 'axial', 2, { x: 2.5, y: 2.5 }, { x: 2.5, y: 2.5 }, 1, 1);
    expect(changed).toEqual(new Set([2]));
    expect(volume.sourceSlices.get(2)?.[2 * 5 + 2]).toBe(1);
    expect(volume.sourceSlices.get(1)).toBeUndefined();
  });

  it('reports sparse voxel count, physical volume, and maximum extent', () => {
    const volume = createMaskVolume(4, 4, 3);
    volume.sourceSlices.set(0, Uint8Array.from([1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
    volume.sourceSlices.set(2, Uint8Array.from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]));
    expect(calculateMaskStatistics(volume, [2, 3, 4])).toEqual({
      voxelCount: 2,
      volumeMm3: 48,
      maximumDiameterMm: Math.hypot(6, 9, 8),
    });
  });

  it('captures and restores one source-layer brush edit', () => {
    const volume = createMaskVolume(5, 5, 1);
    const before = new Map<number, Uint8Array | null>();
    const changed = paintMaskSourcePlane(
      volume,
      0,
      { x: 2.5, y: 2.5 },
      { x: 2.5, y: 2.5 },
      1,
      { rowMm: 1, colMm: 1 },
      1,
      before,
    );
    expect(changed).toEqual(new Set([0]));
    expect(volume.sourceSlices.get(0)?.[12]).toBe(1);
    restoreMaskSlices(volume, before);
    expect(volume.sourceSlices.get(0)).toBeUndefined();
  });
});

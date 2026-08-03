import { describe, expect, it } from 'vitest';
import {
  consumeMprWheel,
  mprImageForPatient,
  patientPointForMprImage,
  sliceForPatientPoint,
} from './app';
import type { MprPlaneMetadata } from './types';

const axial: MprPlaneMetadata = {
  plane: 'axial',
  rows: 101,
  cols: 201,
  slice_count: 51,
  pixel_spacing_mm: 0.5,
  slice_spacing_mm: 1,
  origin: [100, -20, -25],
  x_axis: [-1, 0, 0],
  y_axis: [0, 1, 0],
  normal: [0, 0, 1],
};

describe('MPR patient-space geometry', () => {
  it('maps a patient point to the nearest linked slice', () => {
    expect(sliceForPatientPoint({ x: 0, y: 0, z: -2.6 }, axial)).toBe(22);
    expect(sliceForPatientPoint({ x: 0, y: 0, z: 100 }, axial)).toBe(50);
  });

  it('round-trips image coordinates through patient space', () => {
    const image = { x: 80.25, y: 42.5 };
    const patient = patientPointForMprImage(image, 17, axial);
    const restored = mprImageForPatient(patient, 17, axial);
    expect(restored.x).toBeCloseTo(image.x, 8);
    expect(restored.y).toBeCloseTo(image.y, 8);
  });

  it('moves vertically and horizontally with independent touchpad deltas', () => {
    const accumulator = { x: 0, y: 0 };
    expect(consumeMprWheel(accumulator, 15, 12, false)).toEqual({ x: 0, y: 0 });
    expect(consumeMprWheel(accumulator, 15, 18, false)).toEqual({ x: 1, y: 1 });
    expect(accumulator).toEqual({ x: 0, y: 0 });
  });

  it('maps shift plus a vertical mouse wheel to horizontal movement only', () => {
    const accumulator = { x: 0, y: 0 };
    expect(consumeMprWheel(accumulator, 0, -60, true)).toEqual({ x: -2, y: 0 });
    expect(accumulator).toEqual({ x: 0, y: 0 });
  });
});

import { describe, expect, it } from 'vitest';
import {
  consumeMprWheel,
  mprImageForPatient,
  patientPointForMprImage,
  sliceForPatientPoint,
} from './app';
import type { MprPlaneMetadata } from './types';

const anisotropicOblique: MprPlaneMetadata = {
  plane: 'oblique',
  rows: 300,
  cols: 300,
  slice_count: 80,
  pixel_spacing_mm: 0.7,
  spacing_x_mm: 0.7,
  spacing_y_mm: 0.9,
  slice_spacing_mm: 5,
  origin: [100, -20, -25],
  x_axis: [-1, 0, 0],
  y_axis: [0, 1, 0],
  normal: [0, 0, 1],
};

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

  it('round-trips anisotropic spacingX/Y through patient space', () => {
    const image = { x: 33.25, y: 48.5 };
    const patient = patientPointForMprImage(image, 7, anisotropicOblique);
    const restored = mprImageForPatient(patient, 7, anisotropicOblique);
    expect(restored.x).toBeCloseTo(image.x, 8);
    expect(restored.y).toBeCloseTo(image.y, 8);
    expect(patient.x - anisotropicOblique.origin[0]).toBeCloseTo(-33.25 * 0.7, 8);
    expect(patient.y - anisotropicOblique.origin[1]).toBeCloseTo(48.5 * 0.9, 8);
  });

  it('preserves a known physical length when mapped through a rotated plane', () => {
    const standard = axial;
    const oblique: MprPlaneMetadata = {
      ...anisotropicOblique,
      plane: 'oblique',
      origin: [0, 0, 0],
      x_axis: [0, Math.SQRT1_2, Math.SQRT1_2],
      y_axis: [1, 0, 0],
      normal: [0, Math.SQRT1_2, -Math.SQRT1_2],
    };
    const firstPatient = patientPointForMprImage({ x: 10, y: 20 }, 0, standard);
    const secondPatient = patientPointForMprImage({ x: 30, y: 20 }, 0, standard);
    const firstOblique = mprImageForPatient(firstPatient, 0, oblique);
    const secondOblique = mprImageForPatient(secondPatient, 0, oblique);
    const physicalLength = Math.hypot((30 - 10) * standard.pixel_spacing_mm, (20 - 20) * standard.pixel_spacing_mm);
    const restoredLength = Math.hypot(
      patientPointForMprImage(secondOblique, 0, oblique).x - patientPointForMprImage(firstOblique, 0, oblique).x,
      patientPointForMprImage(secondOblique, 0, oblique).y - patientPointForMprImage(firstOblique, 0, oblique).y,
      patientPointForMprImage(secondOblique, 0, oblique).z - patientPointForMprImage(firstOblique, 0, oblique).z,
    );
    expect(physicalLength).toBeCloseTo(10, 8);
    expect(restoredLength).toBeCloseTo(10, 8);
  });

  it('maps shift plus a vertical mouse wheel to horizontal movement only', () => {
    const accumulator = { x: 0, y: 0 };
    expect(consumeMprWheel(accumulator, 0, -60, true)).toEqual({ x: -2, y: 0 });
    expect(accumulator).toEqual({ x: 0, y: 0 });
  });
});

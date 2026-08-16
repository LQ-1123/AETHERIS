import { describe, expect, it } from 'vitest';
import {
  applyMat4,
  computePlaneStackGeometry,
  invertMat4,
  multiplyMat4,
  patientToVoxelMat4,
  physicalSpacingAlong,
  planeToPatientMat4,
  voxelToPatientMat4,
  type VolumeGeometry,
} from './patient-space';

const isotropic: VolumeGeometry = {
  origin: [10, 20, 30],
  xAxis: [1, 0, 0],
  yAxis: [0, 1, 0],
  normal: [0, 0, 1],
  spacingMm: [0.7, 0.9, 5.0],
  dimensions: [200, 200, 100],
};

describe('patient-space affine transforms', () => {
  it('patient-to-voxel matrix maps origin to zero and axis steps to voxel indices', () => {
    const matrix = patientToVoxelMat4(isotropic);
    expect(applyMat4(matrix, isotropic.origin)[0]).toBeCloseTo(0, 10);
    expect(applyMat4(matrix, [10 + 0.7, 20, 30])[0]).toBeCloseTo(1, 10);
    expect(applyMat4(matrix, [10, 20 + 0.9, 30])[1]).toBeCloseTo(1, 10);
  });

  it('voxel-to-patient inverts patient-to-voxel', () => {
    const toVoxel = patientToVoxelMat4(isotropic);
    const toPatient = voxelToPatientMat4(isotropic);
    const roundTrip = multiplyMat4(toPatient, toVoxel);
    const inverse = invertMat4(toVoxel);
    for (const point of [[0, 0, 0], [11.2, -3.4, 80], [0, 0, 40]] as const) {
      const voxel = applyMat4(toVoxel, [...point]);
      const restored = applyMat4(inverse, voxel);
      expect(restored[0]).toBeCloseTo(point[0], 8);
      expect(restored[1]).toBeCloseTo(point[1], 8);
      expect(restored[2]).toBeCloseTo(point[2], 8);
      expect(roundTrip[0]).toBeCloseTo(1, 10);
    }
  });

  it('honors a rotated ImageOrientationPatient when mapping patient to voxel', () => {
    const rotated: VolumeGeometry = {
      origin: [5, -10, 20],
      xAxis: [0, 1, 0],
      yAxis: [-1, 0, 0],
      normal: [0, 0, 1],
      spacingMm: [0.7, 0.9, 5.0],
      dimensions: [200, 200, 100],
    };
    const matrix = patientToVoxelMat4(rotated);
    expect(applyMat4(matrix, rotated.origin)[0]).toBeCloseTo(0, 10);
    const xStep: [number, number, number] = [
      rotated.origin[0] + rotated.xAxis[0] * 0.7,
      rotated.origin[1] + rotated.xAxis[1] * 0.7,
      rotated.origin[2] + rotated.xAxis[2] * 0.7,
    ];
    const yStep: [number, number, number] = [
      rotated.origin[0] + rotated.yAxis[0] * 0.9,
      rotated.origin[1] + rotated.yAxis[1] * 0.9,
      rotated.origin[2] + rotated.yAxis[2] * 0.9,
    ];
    expect(applyMat4(matrix, xStep)[0]).toBeCloseTo(1, 10);
    expect(applyMat4(matrix, yStep)[1]).toBeCloseTo(1, 10);
    expect(physicalSpacingAlong([1, 0, 0], rotated)).toBeCloseTo(0.9, 10);
  });

  it('physical spacing along a direction honors anisotropic voxels', () => {
    expect(physicalSpacingAlong([1, 0, 0], isotropic)).toBeCloseTo(0.7, 10);
    expect(physicalSpacingAlong([0, 1, 0], isotropic)).toBeCloseTo(0.9, 10);
    expect(physicalSpacingAlong([0, 0, 1], isotropic)).toBeCloseTo(5.0, 10);
    expect(physicalSpacingAlong([Math.SQRT1_2, 0, Math.SQRT1_2], isotropic))
      .toBeCloseTo(1 / Math.hypot(Math.SQRT1_2 / 0.7, Math.SQRT1_2 / 5.0), 10);
  });

  it('plane-to-patient matrix inverts with round-trip precision', () => {
    const plane = {
      plane: 'oblique' as const,
      rows: 10,
      cols: 10,
      slice_count: 5,
      pixel_spacing_mm: 0.7,
      spacing_x_mm: 0.7,
      spacing_y_mm: 0.9,
      slice_spacing_mm: 5.0,
      origin: [10, 20, 30] as [number, number, number],
      x_axis: [1, 0, 0] as [number, number, number],
      y_axis: [0, 1, 0] as [number, number, number],
      normal: [0, 0, 1] as [number, number, number],
    };
    const inverse = invertMat4(planeToPatientMat4(plane));
    for (const point of [[2.5, 3.5, 1], [-4, 9, 2]] as const) {
      const patient = applyMat4(planeToPatientMat4(plane), [...point]);
      const restored = applyMat4(inverse, patient);
      expect(restored[0]).toBeCloseTo(point[0], 8);
      expect(restored[1]).toBeCloseTo(point[1], 8);
      expect(restored[2]).toBeCloseTo(point[2], 8);
    }
  });

  it('computes separate spacingX/Y for a rotated anisotropic plane', () => {
    const xAxis: [number, number, number] = [0, 1, 0];
    const yAxis: [number, number, number] = [0, 0, 1];
    const normal: [number, number, number] = [1, 0, 0];
    const geometry = computePlaneStackGeometry(
      isotropic,
      [0, 0, 0],
      [100, 100, 100],
      xAxis,
      yAxis,
      normal,
    );
    expect(geometry.spacingXmm).toBeCloseTo(0.9, 10);
    expect(geometry.spacingYmm).toBeCloseTo(5.0, 10);
    expect(geometry.cols).toBeGreaterThan(1);
    expect(geometry.rows).toBeGreaterThan(1);
  });
});

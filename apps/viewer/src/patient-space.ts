import type { MprPlaneMetadata } from './types';

export type Vec3 = [number, number, number];
export type Mat4 = [
  number, number, number, number,
  number, number, number, number,
  number, number, number, number,
  number, number, number, number,
];

export interface VolumeGeometry {
  origin: Vec3;
  xAxis: Vec3;
  yAxis: Vec3;
  normal: Vec3;
  spacingMm: [number, number, number];
  dimensions: [number, number, number];
}

export interface PlaneStackGeometry {
  rows: number;
  cols: number;
  sliceCount: number;
  spacingXmm: number;
  spacingYmm: number;
  sliceSpacingMm: number;
  origin: Vec3;
}

export function identityMat4(): Mat4 {
  return [
    1, 0, 0, 0,
    0, 1, 0, 0,
    0, 0, 1, 0,
    0, 0, 0, 1,
  ];
}

export function multiplyMat4(left: Mat4, right: Mat4): Mat4 {
  const result = new Array<number>(16).fill(0) as Mat4;
  for (let column = 0; column < 4; column += 1) {
    for (let row = 0; row < 4; row += 1) {
      let value = 0;
      for (let index = 0; index < 4; index += 1) {
        value += left[row * 4 + index] * right[index * 4 + column];
      }
      result[row * 4 + column] = value;
    }
  }
  return result;
}

export function invertMat4(matrix: Mat4): Mat4 {
  // Row-major general 4x4 Gauss-Jordan inverse.
  const m = matrix.slice() as Mat4;
  const inverse = identityMat4();
  for (let column = 0; column < 4; column += 1) {
    let pivot = column;
    let pivotValue = Math.abs(m[pivot * 4 + column]);
    for (let row = column + 1; row < 4; row += 1) {
      const candidate = Math.abs(m[row * 4 + column]);
      if (candidate > pivotValue) {
        pivotValue = candidate;
        pivot = row;
      }
    }
    if (pivotValue < 1e-12) throw new Error('无法对奇异矩阵求逆');
    if (pivot !== column) {
      for (let index = 0; index < 4; index += 1) {
        [m[column * 4 + index], m[pivot * 4 + index]] = [m[pivot * 4 + index], m[column * 4 + index]];
        [inverse[column * 4 + index], inverse[pivot * 4 + index]] = [inverse[pivot * 4 + index], inverse[column * 4 + index]];
      }
    }
    const divisor = m[column * 4 + column];
    for (let index = 0; index < 4; index += 1) {
      m[column * 4 + index] /= divisor;
      inverse[column * 4 + index] /= divisor;
    }
    for (let row = 0; row < 4; row += 1) {
      if (row === column) continue;
      const factor = m[row * 4 + column];
      if (Math.abs(factor) < 1e-15) continue;
      for (let index = 0; index < 4; index += 1) {
        m[row * 4 + index] -= factor * m[column * 4 + index];
        inverse[row * 4 + index] -= factor * inverse[column * 4 + index];
      }
    }
  }
  return inverse;
}

export function applyMat4(matrix: Mat4, point: Vec3): Vec3 {
  const homogeneous = [point[0], point[1], point[2], 1];
  const result: Vec3 = [
    matrix[0] * homogeneous[0] + matrix[1] * homogeneous[1] + matrix[2] * homogeneous[2] + matrix[3] * homogeneous[3],
    matrix[4] * homogeneous[0] + matrix[5] * homogeneous[1] + matrix[6] * homogeneous[2] + matrix[7] * homogeneous[3],
    matrix[8] * homogeneous[0] + matrix[9] * homogeneous[1] + matrix[10] * homogeneous[2] + matrix[11] * homogeneous[3],
  ];
  return result;
}

export function patientToVoxelMat4(volume: VolumeGeometry): Mat4 {
  const { origin, xAxis, yAxis, normal, spacingMm } = volume;
  return [
    xAxis[0] / spacingMm[0], xAxis[1] / spacingMm[0], xAxis[2] / spacingMm[0], -dot(xAxis, origin) / spacingMm[0],
    yAxis[0] / spacingMm[1], yAxis[1] / spacingMm[1], yAxis[2] / spacingMm[1], -dot(yAxis, origin) / spacingMm[1],
    normal[0] / spacingMm[2], normal[1] / spacingMm[2], normal[2] / spacingMm[2], -dot(normal, origin) / spacingMm[2],
    0, 0, 0, 1,
  ];
}

export function voxelToPatientMat4(volume: VolumeGeometry): Mat4 {
  const { origin, xAxis, yAxis, normal, spacingMm } = volume;
  return [
    xAxis[0] * spacingMm[0], yAxis[0] * spacingMm[1], normal[0] * spacingMm[2], origin[0],
    xAxis[1] * spacingMm[0], yAxis[1] * spacingMm[1], normal[1] * spacingMm[2], origin[1],
    xAxis[2] * spacingMm[0], yAxis[2] * spacingMm[1], normal[2] * spacingMm[2], origin[2],
    0, 0, 0, 1,
  ];
}

export function planeToPatientMat4(plane: MprPlaneMetadata): Mat4 {
  const spacingX = plane.spacing_x_mm ?? plane.pixel_spacing_mm;
  const spacingY = plane.spacing_y_mm ?? plane.pixel_spacing_mm;
  const [originX, originY, originZ] = plane.origin;
  return [
    plane.x_axis[0] * spacingX, plane.y_axis[0] * spacingY, plane.normal[0] * plane.slice_spacing_mm, originX,
    plane.x_axis[1] * spacingX, plane.y_axis[1] * spacingY, plane.normal[1] * plane.slice_spacing_mm, originY,
    plane.x_axis[2] * spacingX, plane.y_axis[2] * spacingY, plane.normal[2] * plane.slice_spacing_mm, originZ,
    0, 0, 0, 1,
  ];
}

export function patientToPlaneMat4(plane: MprPlaneMetadata): Mat4 {
  return invertMat4(planeToPatientMat4(plane));
}

export function physicalSpacingAlong(
  direction: Vec3,
  volume: Pick<VolumeGeometry, 'xAxis' | 'yAxis' | 'normal' | 'spacingMm'>,
): number {
  const x = dot(direction, volume.xAxis) / volume.spacingMm[0];
  const y = dot(direction, volume.yAxis) / volume.spacingMm[1];
  const z = dot(direction, volume.normal) / volume.spacingMm[2];
  const length = Math.hypot(x, y, z);
  if (!Number.isFinite(length) || length < 1e-12) return Number.POSITIVE_INFINITY;
  return 1 / length;
}

export interface PlaneProjection {
  xMin: number;
  xMax: number;
  yMin: number;
  yMax: number;
  zMin: number;
  zMax: number;
}

export function projectBoundsOnPlane(
  boundsMin: Vec3,
  boundsMax: Vec3,
  xAxis: Vec3,
  yAxis: Vec3,
  normal: Vec3,
): PlaneProjection {
  const projections = { xMin: Infinity, xMax: -Infinity, yMin: Infinity, yMax: -Infinity, zMin: Infinity, zMax: -Infinity };
  for (const x of [boundsMin[0], boundsMax[0]]) {
    for (const y of [boundsMin[1], boundsMax[1]]) {
      for (const z of [boundsMin[2], boundsMax[2]]) {
        const relative = [x, y, z] as Vec3;
        const px = dot(relative, xAxis);
        const py = dot(relative, yAxis);
        const pz = dot(relative, normal);
        projections.xMin = Math.min(projections.xMin, px);
        projections.xMax = Math.max(projections.xMax, px);
        projections.yMin = Math.min(projections.yMin, py);
        projections.yMax = Math.max(projections.yMax, py);
        projections.zMin = Math.min(projections.zMin, pz);
        projections.zMax = Math.max(projections.zMax, pz);
      }
    }
  }
  return projections;
}

export function computePlaneStackGeometry(
  volume: VolumeGeometry,
  boundsMin: Vec3,
  boundsMax: Vec3,
  xAxis: Vec3,
  yAxis: Vec3,
  normal: Vec3,
  maximumDimension = 1024,
): PlaneStackGeometry {
  const spacingX = physicalSpacingAlong(xAxis, volume);
  const spacingY = physicalSpacingAlong(yAxis, volume);
  const sliceSpacing = physicalSpacingAlong(normal, volume);
  const projection = projectBoundsOnPlane(boundsMin, boundsMax, xAxis, yAxis, normal);

  let cols = Math.max(1, Math.floor((projection.xMax - projection.xMin) / spacingX) + 1);
  let rows = Math.max(1, Math.floor((projection.yMax - projection.yMin) / spacingY) + 1);
  const sliceCount = Math.max(1, Math.floor((projection.zMax - projection.zMin) / sliceSpacing) + 1);

  // 避免输出尺寸过大：保持 physical FOV 不变，按比例增大输出 spacing。
  let finalSpacingX = spacingX;
  let finalSpacingY = spacingY;
  if (cols > maximumDimension) {
    const scale = cols / maximumDimension;
    finalSpacingX *= scale;
    cols = maximumDimension;
  }
  if (rows > maximumDimension) {
    const scale = rows / maximumDimension;
    finalSpacingY *= scale;
    rows = maximumDimension;
  }

  const origin: Vec3 = [
    projection.xMin * xAxis[0] + projection.yMin * yAxis[0] + projection.zMin * normal[0],
    projection.xMin * xAxis[1] + projection.yMin * yAxis[1] + projection.zMin * normal[1],
    projection.xMin * xAxis[2] + projection.yMin * yAxis[2] + projection.zMin * normal[2],
  ];
  return {
    rows,
    cols,
    sliceCount,
    spacingXmm: finalSpacingX,
    spacingYmm: finalSpacingY,
    sliceSpacingMm: sliceSpacing,
    origin,
  };
}

export function dot(left: Vec3, right: Vec3): number {
  return left[0] * right[0] + left[1] * right[1] + left[2] * right[2];
}

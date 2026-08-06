import type { MprMetadata, MprPlane, MprPlaneMetadata, Point } from './types';

export interface MaskLayer {
  data: Uint8Array;
  rows: number;
  cols: number;
  color: [number, number, number];
  opacity: number;
}

export interface MaskVolume {
  rows: number;
  cols: number;
  slices: number;
  sourceSlices: Map<number, Uint8Array>;
  revisions: Map<number, number>;
  syncStates: Map<number, 'synced' | 'pending' | 'error'>;
  generation: number;
}

export type MaskSliceSnapshot = Map<number, Uint8Array | null>;

export interface MaskStatistics {
  voxelCount: number;
  volumeMm3: number | null;
  maximumDiameterMm: number | null;
}

export function createMaskVolume(rows: number, cols: number, slices: number): MaskVolume {
  if (![rows, cols, slices].every((value) => Number.isInteger(value) && value > 0)) {
    throw new Error('Mask 体数据尺寸无效');
  }
  return {
    rows,
    cols,
    slices,
    sourceSlices: new Map(),
    revisions: new Map(),
    syncStates: new Map(),
    generation: 0,
  };
}

export function getMaskVolumeSlice(volume: MaskVolume, slice: number, create = false): Uint8Array | null {
  if (slice < 0 || slice >= volume.slices) return null;
  let data = volume.sourceSlices.get(slice);
  if (!data && create) {
    data = new Uint8Array(volume.rows * volume.cols);
    volume.sourceSlices.set(slice, data);
  }
  return data ?? null;
}

export function removeEmptyMaskVolumeSlice(volume: MaskVolume, slice: number): void {
  const data = volume.sourceSlices.get(slice);
  if (data && !data.some((value) => value !== 0)) volume.sourceSlices.delete(slice);
}

export function renderMaskPlane(
  volume: MaskVolume,
  metadata: MprMetadata,
  plane: MprPlane,
  sliceIndex: number,
): Uint8Array {
  const planeMetadata = metadata.planes.find((candidate) => candidate.plane === plane);
  if (!planeMetadata || sliceIndex < 0 || sliceIndex >= planeMetadata.slice_count) {
    return new Uint8Array();
  }
  const output = new Uint8Array(planeMetadata.rows * planeMetadata.cols);
  for (let row = 0; row < planeMetadata.rows; row += 1) {
    for (let col = 0; col < planeMetadata.cols; col += 1) {
      const source = planePixelToSourceVoxel(metadata, planeMetadata, sliceIndex, col, row);
      if (!source) continue;
      const sourceSlice = volume.sourceSlices.get(source.z);
      if (sourceSlice) output[row * planeMetadata.cols + col] = sourceSlice[source.y * volume.cols + source.x];
    }
  }
  return output;
}

export function paintMaskVolumePlane(
  volume: MaskVolume,
  metadata: MprMetadata,
  plane: MprPlane,
  sliceIndex: number,
  from: Point,
  to: Point,
  diameterMm: number,
  value: 0 | 1,
  beforeSlices: MaskSliceSnapshot = new Map(),
): Set<number> {
  const planeMetadata = metadata.planes.find((candidate) => candidate.plane === plane);
  if (!planeMetadata) return new Set();
  const radius = Math.max(0.25, Math.min(256, diameterMm / 2));
  const distanceMm = Math.hypot(to.x - from.x, to.y - from.y) * planeMetadata.pixel_spacing_mm;
  const steps = Math.max(1, Math.ceil(distanceMm / Math.max(0.25, radius * 0.4)));
  const changedSlices = new Set<number>();
  for (let step = 0; step <= steps; step += 1) {
    const amount = step / steps;
    const center = {
      x: from.x + (to.x - from.x) * amount,
      y: from.y + (to.y - from.y) * amount,
    };
    paintMaskVolumeDisc(
      volume,
      metadata,
      planeMetadata,
      sliceIndex,
      center,
      radius,
      value,
      changedSlices,
      beforeSlices,
    );
  }
  for (const sourceSlice of changedSlices) removeEmptyMaskVolumeSlice(volume, sourceSlice);
  if (changedSlices.size) volume.generation += 1;
  return changedSlices;
}

export function paintMaskSourcePlane(
  volume: MaskVolume,
  sourceSlice: number,
  from: Point,
  to: Point,
  diameter: number,
  spacing: { rowMm: number | null; colMm: number | null },
  value: 0 | 1,
  beforeSlices: MaskSliceSnapshot = new Map(),
): Set<number> {
  if (sourceSlice < 0 || sourceSlice >= volume.slices) return new Set();
  const rowMm = positiveOr(spacing.rowMm, 1);
  const colMm = positiveOr(spacing.colMm, 1);
  const radiusMm = Math.max(0.25, Math.min(256, diameter / 2));
  const distanceMm = Math.hypot((to.x - from.x) * colMm, (to.y - from.y) * rowMm);
  const steps = Math.max(1, Math.ceil(distanceMm / Math.max(0.25, radiusMm * 0.4)));
  const changed = new Set<number>();
  for (let step = 0; step <= steps; step += 1) {
    const amount = step / steps;
    paintMaskSourceDisc(
      volume,
      sourceSlice,
      {
        x: from.x + (to.x - from.x) * amount,
        y: from.y + (to.y - from.y) * amount,
      },
      radiusMm,
      rowMm,
      colMm,
      value,
      changed,
      beforeSlices,
    );
  }
  if (changed.size) {
    removeEmptyMaskVolumeSlice(volume, sourceSlice);
    volume.generation += 1;
  }
  return changed;
}

export function snapshotMaskSlices(
  volume: MaskVolume,
  slices: Iterable<number>,
): MaskSliceSnapshot {
  const snapshot: MaskSliceSnapshot = new Map();
  for (const slice of slices) snapshot.set(slice, volume.sourceSlices.get(slice)?.slice() ?? null);
  return snapshot;
}

export function restoreMaskSlices(volume: MaskVolume, snapshot: MaskSliceSnapshot): void {
  for (const [slice, data] of snapshot) {
    if (data?.some(Boolean)) volume.sourceSlices.set(slice, data.slice());
    else volume.sourceSlices.delete(slice);
  }
  if (snapshot.size) volume.generation += 1;
}

export function calculateMaskStatistics(
  volume: MaskVolume,
  spacing: [number, number, number] | null,
): MaskStatistics {
  let voxelCount = 0;
  let minimumX = volume.cols;
  let minimumY = volume.rows;
  let minimumZ = volume.slices;
  let maximumX = -1;
  let maximumY = -1;
  let maximumZ = -1;
  for (const [z, data] of volume.sourceSlices) {
    for (let offset = 0; offset < data.length; offset += 1) {
      if (!data[offset]) continue;
      const x = offset % volume.cols;
      const y = Math.floor(offset / volume.cols);
      voxelCount += 1;
      minimumX = Math.min(minimumX, x);
      minimumY = Math.min(minimumY, y);
      minimumZ = Math.min(minimumZ, z);
      maximumX = Math.max(maximumX, x);
      maximumY = Math.max(maximumY, y);
      maximumZ = Math.max(maximumZ, z);
    }
  }
  if (!spacing || !spacing.every((value) => Number.isFinite(value) && value > 0)) {
    return { voxelCount, volumeMm3: null, maximumDiameterMm: null };
  }
  const [colMm, rowMm, sliceMm] = spacing;
  const volumeMm3 = voxelCount * colMm * rowMm * sliceMm;
  // The first viewer release uses the physical diagonal of the occupied voxel
  // bounds as a stable, allocation-free 3D maximum extent.
  const maximumDiameterMm = voxelCount === 0 ? 0 : Math.hypot(
    (maximumX - minimumX) * colMm,
    (maximumY - minimumY) * rowMm,
    (maximumZ - minimumZ) * sliceMm,
  );
  return { voxelCount, volumeMm3, maximumDiameterMm };
}

export function createMask(rows: number, cols: number): Uint8Array {
  if (!Number.isInteger(rows) || !Number.isInteger(cols) || rows <= 0 || cols <= 0) {
    throw new Error('Mask 尺寸无效');
  }
  return new Uint8Array(rows * cols);
}

export function paintMaskStroke(
  mask: Uint8Array,
  rows: number,
  cols: number,
  from: Point,
  to: Point,
  radius: number,
  value: 0 | 1,
): void {
  validateMask(mask, rows, cols);
  const safeRadius = Math.max(0.5, Math.min(256, radius));
  const distance = Math.hypot(to.x - from.x, to.y - from.y);
  const steps = Math.max(1, Math.ceil(distance / Math.max(0.5, safeRadius * 0.45)));
  for (let step = 0; step <= steps; step += 1) {
    const amount = step / steps;
    paintDisc(
      mask,
      rows,
      cols,
      from.x + (to.x - from.x) * amount,
      from.y + (to.y - from.y) * amount,
      safeRadius,
      value,
    );
  }
}

export function encodeMaskRle(mask: Uint8Array): Uint8Array {
  if (!mask.length) throw new Error('Mask 不能为空');
  const runs: number[] = [];
  let expected = 0;
  let count = 0;
  for (const pixel of mask) {
    const value = pixel ? 1 : 0;
    if (value === expected) {
      count += 1;
    } else {
      runs.push(count);
      expected = value;
      count = 1;
    }
  }
  runs.push(count);
  const encoded = new Uint8Array(runs.length * 4);
  const view = new DataView(encoded.buffer);
  runs.forEach((run, index) => view.setUint32(index * 4, run, true));
  return encoded;
}

export function decodeMaskRle(encoded: Uint8Array, pixelCount: number): Uint8Array {
  if (!encoded.length || encoded.length % 4 !== 0 || pixelCount <= 0) {
    throw new Error('Mask RLE 无效');
  }
  const output = new Uint8Array(pixelCount);
  const view = new DataView(encoded.buffer, encoded.byteOffset, encoded.byteLength);
  let offset = 0;
  let value = 0;
  for (let index = 0; index < encoded.length; index += 4) {
    const run = view.getUint32(index, true);
    if (run === 0 && index !== 0) throw new Error('Mask RLE 包含空游程');
    if (offset + run > pixelCount) throw new Error('Mask RLE 超出图像范围');
    if (value === 1) output.fill(1, offset, offset + run);
    offset += run;
    value = value ? 0 : 1;
  }
  if (offset !== pixelCount) throw new Error('Mask RLE 像素数不匹配');
  return output;
}

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = '';
  const chunkSize = 0x8000;
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunkSize));
  }
  return btoa(binary);
}

export function base64ToBytes(value: string): Uint8Array {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

function paintDisc(
  mask: Uint8Array,
  rows: number,
  cols: number,
  centerX: number,
  centerY: number,
  radius: number,
  value: 0 | 1,
): void {
  const left = Math.max(0, Math.floor(centerX - radius));
  const right = Math.min(cols - 1, Math.ceil(centerX + radius));
  const top = Math.max(0, Math.floor(centerY - radius));
  const bottom = Math.min(rows - 1, Math.ceil(centerY + radius));
  const radiusSquared = radius * radius;
  for (let y = top; y <= bottom; y += 1) {
    for (let x = left; x <= right; x += 1) {
      const dx = x + 0.5 - centerX;
      const dy = y + 0.5 - centerY;
      if (dx * dx + dy * dy <= radiusSquared) mask[y * cols + x] = value;
    }
  }
}

function paintMaskVolumeDisc(
  volume: MaskVolume,
  metadata: MprMetadata,
  plane: MprPlaneMetadata,
  sliceIndex: number,
  center: Point,
  radius: number,
  value: 0 | 1,
  changedSlices: Set<number>,
  beforeSlices: MaskSliceSnapshot,
): void {
  const pixelRadius = radius / plane.pixel_spacing_mm;
  const left = Math.max(0, Math.floor(center.x - pixelRadius));
  const right = Math.min(plane.cols - 1, Math.ceil(center.x + pixelRadius));
  const top = Math.max(0, Math.floor(center.y - pixelRadius));
  const bottom = Math.min(plane.rows - 1, Math.ceil(center.y + pixelRadius));
  const radiusSquared = radius * radius;
  for (let row = top; row <= bottom; row += 1) {
    for (let col = left; col <= right; col += 1) {
      const dx = (col + 0.5 - center.x) * plane.pixel_spacing_mm;
      const dy = (row + 0.5 - center.y) * plane.pixel_spacing_mm;
      if (dx * dx + dy * dy > radiusSquared) continue;
      const source = planePixelToSourceVoxel(metadata, plane, sliceIndex, col, row);
      if (!source) continue;
      const sourceSlice = getMaskVolumeSlice(volume, source.z, value === 1);
      if (!sourceSlice) continue;
      const offset = source.y * volume.cols + source.x;
      if (sourceSlice[offset] !== value) {
        captureBeforeSlice(volume, source.z, beforeSlices);
        sourceSlice[offset] = value;
        changedSlices.add(source.z);
      }
    }
  }
}

function paintMaskSourceDisc(
  volume: MaskVolume,
  sourceSlice: number,
  center: Point,
  radiusMm: number,
  rowMm: number,
  colMm: number,
  value: 0 | 1,
  changedSlices: Set<number>,
  beforeSlices: MaskSliceSnapshot,
): void {
  const radiusX = radiusMm / colMm;
  const radiusY = radiusMm / rowMm;
  const left = Math.max(0, Math.floor(center.x - radiusX));
  const right = Math.min(volume.cols - 1, Math.ceil(center.x + radiusX));
  const top = Math.max(0, Math.floor(center.y - radiusY));
  const bottom = Math.min(volume.rows - 1, Math.ceil(center.y + radiusY));
  let data = volume.sourceSlices.get(sourceSlice);
  for (let row = top; row <= bottom; row += 1) {
    for (let col = left; col <= right; col += 1) {
      const dx = (col + 0.5 - center.x) * colMm;
      const dy = (row + 0.5 - center.y) * rowMm;
      if (dx * dx + dy * dy > radiusMm * radiusMm) continue;
      if (!data) {
        if (value === 0) continue;
        data = getMaskVolumeSlice(volume, sourceSlice, true)!;
      }
      const offset = row * volume.cols + col;
      if (data[offset] === value) continue;
      captureBeforeSlice(volume, sourceSlice, beforeSlices);
      data[offset] = value;
      changedSlices.add(sourceSlice);
    }
  }
}

function captureBeforeSlice(
  volume: MaskVolume,
  slice: number,
  beforeSlices: MaskSliceSnapshot,
): void {
  if (!beforeSlices.has(slice)) {
    beforeSlices.set(slice, volume.sourceSlices.get(slice)?.slice() ?? null);
  }
}

function planePixelToSourceVoxel(
  metadata: MprMetadata,
  plane: MprPlaneMetadata,
  sliceIndex: number,
  col: number,
  row: number,
): { x: number; y: number; z: number } | null {
  const patient = [
    plane.origin[0] + plane.x_axis[0] * col * plane.pixel_spacing_mm + plane.y_axis[0] * row * plane.pixel_spacing_mm + plane.normal[0] * sliceIndex * plane.slice_spacing_mm,
    plane.origin[1] + plane.x_axis[1] * col * plane.pixel_spacing_mm + plane.y_axis[1] * row * plane.pixel_spacing_mm + plane.normal[1] * sliceIndex * plane.slice_spacing_mm,
    plane.origin[2] + plane.x_axis[2] * col * plane.pixel_spacing_mm + plane.y_axis[2] * row * plane.pixel_spacing_mm + plane.normal[2] * sliceIndex * plane.slice_spacing_mm,
  ];
  const relative = [
    patient[0] - metadata.source_origin[0],
    patient[1] - metadata.source_origin[1],
    patient[2] - metadata.source_origin[2],
  ];
  const dot = (axis: [number, number, number]) => relative[0] * axis[0] + relative[1] * axis[1] + relative[2] * axis[2];
  const x = Math.round(dot(metadata.source_x_axis) / metadata.source_spacing_mm[0]);
  const y = Math.round(dot(metadata.source_y_axis) / metadata.source_spacing_mm[1]);
  const z = Math.round(dot(metadata.source_normal) / metadata.source_spacing_mm[2]);
  if (x < 0 || y < 0 || z < 0 || x >= metadata.dimensions[0] || y >= metadata.dimensions[1] || z >= metadata.dimensions[2]) return null;
  return { x, y, z };
}

function validateMask(mask: Uint8Array, rows: number, cols: number): void {
  if (rows <= 0 || cols <= 0 || mask.length !== rows * cols) throw new Error('Mask 尺寸不匹配');
}

function positiveOr(value: number | null, fallback: number): number {
  return value != null && Number.isFinite(value) && value > 0 ? value : fallback;
}

import { describe, expect, it } from 'vitest';
import {
  frameHasGeometry,
  seriesGeometryFrame,
  syncEligibility,
  syncFrameTargets,
  windowEquals,
} from './sync-controller';
import type { FrameMetadata } from './types';

/** 构造测试用 FrameMetadata：轴向切片（normal = Z），行= X、列= Y。 */
function frame(
  z: number,
  overrides: Partial<FrameMetadata> = {},
): FrameMetadata {
  return {
    logical_index: 0,
    frame_key: `k-${z}-${overrides.position === null ? 'no-position' : 'ok'}`,
    sop_instance_uid: null,
    source_frame: 1,
    instance_number: 1,
    rows: 512,
    cols: 512,
    bits_allocated: 16,
    pixel_format: 'gray16',
    photometric_interpretation: 'MONOCHROME2',
    cine_rate_fps: null,
    quantitative: { unit: 'HU', suvbw_factor: null, suvbw_status: null },
    laterality: null,
    view_position: null,
    patient_orientation: [],
    position: [0, 0, z],
    orientation: [1, 0, 0, 0, 1, 0],
    window_presets: [],
    spacing: {
      confidence: 'calibrated',
      source: null,
      description: '',
      row_mm: 0.5,
      col_mm: 0.5,
      column_over_row: 1,
    },
    ...overrides,
  };
}

describe('seriesGeometryFrame', () => {
  it('adapts FrameMetadata spacing fields to the geometry interface', () => {
    const adapted = seriesGeometryFrame(frame(3));
    expect(adapted.rows).toBe(512);
    expect(adapted.cols).toBe(512);
    expect(adapted.position).toEqual([0, 0, 3]);
    expect(adapted.orientation).toEqual([1, 0, 0, 0, 1, 0]);
    expect(adapted.rowSpacingMm).toBe(0.5);
    expect(adapted.colSpacingMm).toBe(0.5);
  });

  it('carries null spacings through', () => {
    const adapted = seriesGeometryFrame(frame(3, {
      spacing: { confidence: 'none', source: null, description: '', row_mm: null, col_mm: null, column_over_row: 1 },
    }));
    expect(adapted.rowSpacingMm).toBeNull();
    expect(adapted.colSpacingMm).toBeNull();
  });
});

describe('frameHasGeometry', () => {
  it('requires position, orientation and both spacings', () => {
    expect(frameHasGeometry(frame(0))).toBe(true);
    expect(frameHasGeometry(frame(0, { position: null }))).toBe(false);
    expect(frameHasGeometry(frame(0, { orientation: null }))).toBe(false);
    expect(
      frameHasGeometry(frame(0, {
        spacing: { confidence: 'none', source: null, description: '', row_mm: null, col_mm: 0.5, column_over_row: 1 },
      })),
    ).toBe(false);
    expect(
      frameHasGeometry(frame(0, {
        spacing: { confidence: 'none', source: null, description: '', row_mm: 0.5, col_mm: null, column_over_row: 1 },
      })),
    ).toBe(false);
  });
});

describe('syncEligibility', () => {
  it('is eligible when every frame carries geometry', () => {
    expect(syncEligibility([frame(0), frame(5), frame(10)])).toEqual({
      eligible: true,
      reason: null,
    });
  });

  it('reports missing geometry when any frame lacks it', () => {
    expect(syncEligibility([frame(0), frame(5, { position: null })])).toEqual({
      eligible: false,
      reason: '缺几何',
    });
  });

  it('reports an empty stack', () => {
    expect(syncEligibility([])).toEqual({ eligible: false, reason: '无帧' });
  });
});

describe('syncFrameTargets', () => {
  const memberFrames = [frame(0), frame(10), frame(20)];

  it('maps the source frame to the closest parallel target frame', () => {
    const targets = syncFrameTargets(
      frame(10.4),
      [{ paneIndex: 1, frames: memberFrames }],
      new Set(),
    );
    expect(targets).toEqual([{ paneIndex: 1, frameIndex: 1 }]);
  });

  it('returns a null frame index for non-parallel members', () => {
    const sagittal = frame(0, { orientation: [0, 1, 0, 0, 0, -1] });
    const targets = syncFrameTargets(
      frame(10),
      [{ paneIndex: 1, frames: [sagittal] }],
      new Set(),
    );
    expect(targets).toEqual([{ paneIndex: 1, frameIndex: null }]);
  });

  it('skips excluded members entirely', () => {
    const targets = syncFrameTargets(
      frame(10),
      [
        { paneIndex: 1, frames: memberFrames },
        { paneIndex: 2, frames: memberFrames },
      ],
      new Set([2]),
    );
    expect(targets).toEqual([{ paneIndex: 1, frameIndex: 1 }]);
  });

  it('returns a null frame index when the source itself lacks geometry', () => {
    const targets = syncFrameTargets(
      frame(0, { position: null }),
      [{ paneIndex: 1, frames: memberFrames }],
      new Set(),
    );
    expect(targets).toEqual([{ paneIndex: 1, frameIndex: null }]);
  });
});

describe('windowEquals', () => {
  it('compares center and width exactly', () => {
    expect(windowEquals({ center: 40, width: 400 }, { center: 40, width: 400 })).toBe(true);
    expect(windowEquals({ center: 40, width: 400 }, { center: 41, width: 400 })).toBe(false);
    expect(windowEquals({ center: 40, width: 400 }, { center: 40, width: 401 })).toBe(false);
  });
});

import { describe, expect, it } from 'vitest';
import {
  fitScale,
  imageToScreen,
  measurementValue,
  screenToImage,
  zoomAt,
} from './geometry';
import type { SpacingInfo } from './types';

const viewport = { width: 1000, height: 800 };
const image = { rows: 400, cols: 500, columnOverRow: 1 };

describe('viewer geometry', () => {
  it('fits an image without changing its aspect ratio', () => {
    expect(fitScale(viewport, image)).toBe(2);
    expect(fitScale(viewport, { ...image, columnOverRow: 2 })).toBe(1);
  });

  it('round-trips image and screen coordinates', () => {
    const view = { zoom: 1.7, panX: 28, panY: -13 };
    const original = { x: 133.25, y: 291.5 };
    const screen = imageToScreen(original, viewport, image, view);
    const restored = screenToImage(screen, viewport, image, view);
    expect(restored.x).toBeCloseTo(original.x, 8);
    expect(restored.y).toBeCloseTo(original.y, 8);
  });

  it('keeps the image point under the cursor fixed while zooming', () => {
    const cursor = { x: 720, y: 240 };
    const before = { zoom: 1, panX: 0, panY: 0 };
    const imagePoint = screenToImage(cursor, viewport, image, before);
    const after = zoomAt(before, cursor, 2.4, viewport, image);
    const screenAfter = imageToScreen(imagePoint, viewport, image, after);
    expect(screenAfter.x).toBeCloseTo(cursor.x, 8);
    expect(screenAfter.y).toBeCloseTo(cursor.y, 8);
  });

  it('uses DICOM row and column spacing in the correct directions', () => {
    const spacing: SpacingInfo = {
      confidence: 'calibrated',
      source: 'pixel-spacing',
      description: '',
      row_mm: 2,
      col_mm: 0.5,
      column_over_row: 0.25,
    };
    const result = measurementValue({ x: 0, y: 0 }, { x: 6, y: 4 }, spacing);
    expect(result.unit).toBe('mm');
    expect(result.value).toBeCloseTo(Math.hypot(3, 8));
  });

  it('falls back to pixel distance when physical spacing is unavailable', () => {
    const spacing: SpacingInfo = {
      confidence: 'none',
      source: null,
      description: '',
      row_mm: null,
      col_mm: null,
      column_over_row: 1,
    };
    expect(measurementValue({ x: 0, y: 0 }, { x: 3, y: 4 }, spacing)).toEqual({
      value: 5,
      unit: 'px',
    });
  });
});

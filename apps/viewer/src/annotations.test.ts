import { describe, expect, it } from 'vitest';
import {
  AnnotationHistory,
  angleDegrees,
  annotationHitTest,
  createAnnotation,
} from './annotations';
import type { Annotation } from './types';

describe('annotation geometry and history', () => {
  it('hit-tests rectangle borders and ellipse handles', () => {
    const rectangle: Annotation = {
      id: 'rectangle',
      kind: 'rectangle_roi',
      start: { x: 10, y: 20 },
      end: { x: 50, y: 60 },
    };
    expect(annotationHitTest(rectangle, { x: 30, y: 20 }, (point) => point)?.handle).toBeNull();
    expect(annotationHitTest(rectangle, { x: 10, y: 20 }, (point) => point)?.handle).toBe(0);
    expect(annotationHitTest(rectangle, { x: 30, y: 40 }, (point) => point)).toBeNull();
  });

  it('calculates the smaller angle between three points', () => {
    const angle: Annotation = {
      id: 'angle',
      kind: 'angle',
      start: { x: 1, y: 0 },
      vertex: { x: 0, y: 0 },
      end: { x: 0, y: 1 },
    };
    expect(angleDegrees(angle)).toBeCloseTo(90);
  });

  it('maintains bounded undo and redo snapshots', () => {
    const history = new AnnotationHistory(2);
    const first = createAnnotation('point_probe', { x: 1, y: 2 }, 'first');
    const second = createAnnotation('point_probe', { x: 3, y: 4 }, 'second');
    history.push({ key: 'frame', before: [], after: [first] });
    history.push({ key: 'frame', before: [first], after: [first, second] });
    const undone = history.undo();
    const redone = history.redo();
    expect(undone && 'key' in undone ? undone.before : null).toEqual([first]);
    expect(redone && 'key' in redone ? redone.after : null).toEqual([first, second]);
  });

  it('groups a series-wide clear into one undo item', () => {
    const history = new AnnotationHistory();
    const annotation = createAnnotation('point_probe', { x: 1, y: 2 }, 'probe');
    history.pushBatch([
      { key: 'frame-1', before: [annotation], after: [] },
      { key: 'frame-2', before: [annotation], after: [] },
    ]);
    const item = history.undo();
    expect(item && 'changes' in item ? item.changes : null).toHaveLength(2);
    expect(history.canUndo).toBe(false);
    expect(history.canRedo).toBe(true);
  });
});

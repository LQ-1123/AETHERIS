import { describe, expect, it } from 'vitest';
import { framePrefetchGroups, framePrefetchOrder } from './frame-prefetch';

describe('framePrefetchOrder', () => {
  it('prefetches every other frame with nearby forward frames first', () => {
    expect(framePrefetchOrder(6, 2)).toEqual([3, 1, 4, 0, 5]);
  });

  it('handles the first frame and a single-frame image', () => {
    expect(framePrefetchOrder(4, 0)).toEqual([1, 2, 3]);
    expect(framePrefetchOrder(1, 0)).toEqual([]);
  });

  it('keeps frames from the same DICOM source in one sequential group', () => {
    const sources = ['a', 'a', 'b', 'c', 'a', 'b'];
    expect(framePrefetchGroups(sources.length, 2, (index) => sources[index])).toEqual([
      [3],
      [1, 4, 0],
      [5],
    ]);
  });
});

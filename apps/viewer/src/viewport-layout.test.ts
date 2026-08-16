import { describe, expect, it } from 'vitest';
import { seriesGridLayout } from './viewport-layout';

describe('seriesGridLayout', () => {
  it('uses a single cell for zero or one series', () => {
    expect(seriesGridLayout(0)).toEqual({ columns: 1, rows: 1, slots: 1 });
    expect(seriesGridLayout(1)).toEqual({ columns: 1, rows: 1, slots: 1 });
  });

  it('uses a side-by-side layout for two series', () => {
    expect(seriesGridLayout(2)).toEqual({ columns: 2, rows: 1, slots: 2 });
  });

  it('uses a 2x2 grid for three or four series', () => {
    expect(seriesGridLayout(3)).toEqual({ columns: 2, rows: 2, slots: 4 });
    expect(seriesGridLayout(4)).toEqual({ columns: 2, rows: 2, slots: 4 });
  });

  it('uses a 3x2 grid for five or six series', () => {
    expect(seriesGridLayout(5)).toEqual({ columns: 3, rows: 2, slots: 6 });
    expect(seriesGridLayout(6)).toEqual({ columns: 3, rows: 2, slots: 6 });
  });

  it('caps at a 3x3 grid', () => {
    expect(seriesGridLayout(7)).toEqual({ columns: 3, rows: 3, slots: 9 });
    expect(seriesGridLayout(9)).toEqual({ columns: 3, rows: 3, slots: 9 });
    expect(seriesGridLayout(20)).toEqual({ columns: 3, rows: 3, slots: 9 });
  });
});

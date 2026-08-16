export const MAX_SERIES_PANES = 9;

export interface SeriesGridLayout {
  columns: number;
  rows: number;
  slots: number;
}

/**
 * 根据已打开序列数自动选择分屏布局。
 * 1 -> 1x1，2 -> 1x2，3-4 -> 2x2，5-6 -> 2x3（2 行 3 列），7-9 -> 3x3。
 */
export function seriesGridLayout(seriesCount: number): SeriesGridLayout {
  const count = Math.max(0, Math.floor(seriesCount));
  const slots = count <= 1
    ? 1
    : count === 2
      ? 2
      : count <= 4
        ? 4
        : count <= 6
          ? 6
          : 9;
  const columns = count <= 1 ? 1 : count === 2 ? 2 : count <= 4 ? 2 : 3;
  const rows = Math.max(1, Math.ceil(slots / columns));
  return { columns, rows, slots };
}

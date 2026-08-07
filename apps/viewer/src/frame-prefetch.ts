export function framePrefetchOrder(frameCount: number, current: number): number[] {
  const order: number[] = [];
  for (let distance = 1; order.length < Math.max(0, frameCount - 1); distance += 1) {
    const next = current + distance;
    const previous = current - distance;
    if (next < frameCount) order.push(next);
    if (previous >= 0) order.push(previous);
  }
  return order;
}

export function framePrefetchGroups(
  frameCount: number,
  current: number,
  sourceKey: (index: number) => string,
): number[][] {
  const groups = new Map<string, number[]>();
  for (const index of framePrefetchOrder(frameCount, current)) {
    const key = sourceKey(index);
    const group = groups.get(key);
    if (group) group.push(index);
    else groups.set(key, [index]);
  }
  return [...groups.values()];
}

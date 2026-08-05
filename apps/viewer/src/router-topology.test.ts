import { describe, expect, it } from 'vitest';
import { layoutTopology, type RouterTopologyNode } from './router-topology';

function node(id: string, side: RouterTopologyNode['side']): RouterTopologyNode {
  return { id, side, label: id, statusText: '在线', summary: id, status: 'online' };
}

describe('router topology layout', () => {
  it('places connected stations around the central PACS node', () => {
    const positioned = layoutTopology(1000, 320, [
      node('local', 'local'),
      node('top', 'inbound'),
      node('right', 'inbound'),
      node('bottom', 'inbound'),
      node('left', 'inbound'),
    ]);
    const local = positioned.find((entry) => entry.id === 'local')!;
    expect(positioned.find((entry) => entry.id === 'top')!.y).toBeLessThan(local.y);
    expect(positioned.find((entry) => entry.id === 'right')!.x).toBeGreaterThan(local.x);
    expect(positioned.find((entry) => entry.id === 'bottom')!.y).toBeGreaterThan(local.y);
    expect(positioned.find((entry) => entry.id === 'left')!.x).toBeLessThan(local.x);
  });

  it('keeps dense node sets inside the canvas and hides crowded labels', () => {
    const nodes = [node('local', 'local')];
    for (let index = 0; index < 25; index += 1) nodes.push(node(`peer-${index}`, 'inbound'));
    const peers = layoutTopology(1000, 320, nodes).filter((entry) => entry.side === 'inbound');
    expect(peers.every((entry) => !entry.showLabel)).toBe(true);
    expect(peers.every((entry) => entry.x >= 0 && entry.x <= 1000)).toBe(true);
    expect(peers.every((entry) => entry.y >= 0 && entry.y <= 320)).toBe(true);
  });
});

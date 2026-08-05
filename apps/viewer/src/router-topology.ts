export type TopologySide = 'inbound' | 'local' | 'outbound';
export type TopologyStatus = 'online' | 'recent' | 'offline' | 'unknown' | 'disabled';

export interface RouterTopologyNode {
  id: string;
  side: TopologySide;
  label: string;
  statusText: string;
  summary: string;
  status: TopologyStatus;
}

export interface PositionedTopologyNode extends RouterTopologyNode {
  x: number;
  y: number;
  radius: number;
  showLabel: boolean;
}

const MAX_LABELED_SITES = 12;

export function layoutTopology(
  width: number,
  height: number,
  nodes: RouterTopologyNode[],
): PositionedTopologyNode[] {
  const local = nodes.find((node) => node.side === 'local');
  const positioned: PositionedTopologyNode[] = local ? [{
    ...local,
    x: width / 2,
    y: height / 2,
    radius: 8,
    showLabel: true,
  }] : [];
  const sites = nodes.filter((node) => node.side !== 'local');
  const radiusX = Math.max(70, width * 0.29);
  const radiusY = Math.max(55, height * 0.3);
  positioned.push(...sites.map((node, index) => {
    const angle = -Math.PI / 2 + (Math.PI * 2 * index) / sites.length;
    return {
      ...node,
      x: width / 2 + Math.cos(angle) * radiusX,
      y: height / 2 + Math.sin(angle) * radiusY,
      radius: 5,
      showLabel: sites.length <= MAX_LABELED_SITES,
    };
  }));
  return positioned;
}

export class RouterTopologyCanvas {
  private nodes: RouterTopologyNode[] = [];
  private positioned: PositionedTopologyNode[] = [];
  private pinnedId = 'local';
  private hoverId: string | null = null;
  private frame: number | null = null;
  private readonly resizeObserver: ResizeObserver;

  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly detail: HTMLElement,
  ) {
    this.resizeObserver = new ResizeObserver(() => this.scheduleDraw());
    this.resizeObserver.observe(canvas);
    canvas.addEventListener('pointermove', (event) => this.pointerMove(event));
    canvas.addEventListener('pointerleave', () => this.setHover(null));
    canvas.addEventListener('click', () => {
      if (this.hoverId) this.pinnedId = this.hoverId;
      this.updateDetail();
    });
  }

  update(nodes: RouterTopologyNode[]): void {
    this.nodes = nodes;
    if (!nodes.some((node) => node.id === this.pinnedId)) this.pinnedId = 'local';
    this.canvas.setAttribute('aria-label', topologyAriaLabel(nodes));
    this.updateDetail();
    this.scheduleDraw();
  }

  private scheduleDraw(): void {
    if (this.frame != null) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = null;
      this.draw();
    });
  }

  private draw(): void {
    const bounds = this.canvas.getBoundingClientRect();
    if (bounds.width < 1 || bounds.height < 1) return;
    const ratio = Math.min(window.devicePixelRatio || 1, 2);
    const pixelWidth = Math.round(bounds.width * ratio);
    const pixelHeight = Math.round(bounds.height * ratio);
    if (this.canvas.width !== pixelWidth || this.canvas.height !== pixelHeight) {
      this.canvas.width = pixelWidth;
      this.canvas.height = pixelHeight;
    }
    const context = this.canvas.getContext('2d');
    if (!context) return;
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, bounds.width, bounds.height);
    this.positioned = layoutTopology(bounds.width, bounds.height, this.nodes);
    const local = this.positioned.find((node) => node.side === 'local');
    if (!local) return;

    context.font = '10px system-ui, sans-serif';
    context.fillStyle = '#879196';
    context.textAlign = 'left';
    context.fillText('已接入站点', 12, 18);
    context.textAlign = 'center';
    context.fillText('PACS', local.x, 18);

    for (const node of this.positioned) {
      if (node.side === 'local') continue;
      const source = node.side === 'inbound' ? node : local;
      const target = node.side === 'inbound' ? local : node;
      drawEdge(context, source, target, statusColor(node.status));
    }
    for (const node of this.positioned) this.drawNode(context, node);
  }

  private drawNode(context: CanvasRenderingContext2D, node: PositionedTopologyNode): void {
    const active = node.id === (this.hoverId ?? this.pinnedId);
    if (active) {
      context.beginPath();
      context.arc(node.x, node.y, node.radius + 5, 0, Math.PI * 2);
      context.strokeStyle = '#dce8ea';
      context.lineWidth = 1;
      context.stroke();
    }
    context.beginPath();
    context.arc(node.x, node.y, node.radius, 0, Math.PI * 2);
    context.fillStyle = statusColor(node.status);
    context.fill();
    if (!node.showLabel) return;
    context.font = node.side === 'local' ? '600 11px system-ui, sans-serif' : '10px system-ui, sans-serif';
    context.fillStyle = node.side === 'local' ? '#edf1f2' : '#c5cccf';
    context.textBaseline = 'middle';
    const centerX = this.canvas.clientWidth / 2;
    const centerY = this.canvas.clientHeight / 2;
    const deltaX = node.x - centerX;
    context.textAlign = node.side === 'local' || Math.abs(deltaX) < 24
      ? 'center'
      : deltaX < 0 ? 'right' : 'left';
    const x = node.side === 'local' || Math.abs(deltaX) < 24
      ? node.x
      : node.x + (deltaX < 0 ? -11 : 11);
    const y = node.side === 'local'
      ? node.y + 22
      : node.y < centerY ? node.y - 28 : node.y + 13;
    context.fillText(fitLabel(context, node.label, 150), x, y);
    context.font = '9px system-ui, sans-serif';
    context.fillStyle = statusColor(node.status);
    context.fillText(fitLabel(context, node.statusText, 150), x, y + 15);
  }

  private pointerMove(event: PointerEvent): void {
    const bounds = this.canvas.getBoundingClientRect();
    const x = event.clientX - bounds.left;
    const y = event.clientY - bounds.top;
    const hit = this.positioned.find((node) => Math.hypot(node.x - x, node.y - y) <= node.radius + 8);
    this.setHover(hit?.id ?? null);
  }

  private setHover(id: string | null): void {
    if (id === this.hoverId) return;
    this.hoverId = id;
    this.canvas.style.cursor = id ? 'pointer' : 'default';
    this.updateDetail();
    this.scheduleDraw();
  }

  private updateDetail(): void {
    const active = this.nodes.find((node) => node.id === (this.hoverId ?? this.pinnedId))
      ?? this.nodes.find((node) => node.side === 'local');
    this.detail.textContent = active?.summary ?? '暂无 DICOM 节点';
  }
}

function drawEdge(
  context: CanvasRenderingContext2D,
  source: PositionedTopologyNode,
  target: PositionedTopologyNode,
  color: string,
): void {
  context.beginPath();
  context.moveTo(source.x, source.y);
  context.lineTo(target.x, target.y);
  context.strokeStyle = `${color}66`;
  context.lineWidth = 1;
  context.stroke();
  const ratio = 0.7;
  const x = source.x + (target.x - source.x) * ratio;
  const y = source.y + (target.y - source.y) * ratio;
  const angle = Math.atan2(target.y - source.y, target.x - source.x);
  context.beginPath();
  context.moveTo(x + Math.cos(angle) * 4, y + Math.sin(angle) * 4);
  context.lineTo(x + Math.cos(angle + 2.5) * 4, y + Math.sin(angle + 2.5) * 4);
  context.lineTo(x + Math.cos(angle - 2.5) * 4, y + Math.sin(angle - 2.5) * 4);
  context.closePath();
  context.fillStyle = color;
  context.fill();
}

function fitLabel(context: CanvasRenderingContext2D, value: string, width: number): string {
  if (context.measureText(value).width <= width) return value;
  let end = value.length;
  while (end > 1 && context.measureText(`${value.slice(0, end)}...`).width > width) end -= 1;
  return `${value.slice(0, end)}...`;
}

function statusColor(status: TopologyStatus): string {
  return ({
    online: '#35bd79',
    recent: '#e2ad4f',
    offline: '#e05d68',
    unknown: '#8c969b',
    disabled: '#596164',
  })[status];
}

function topologyAriaLabel(nodes: RouterTopologyNode[]): string {
  const sites = nodes.filter((node) => node.side === 'inbound').length;
  return `DICOM 网络拓扑，${sites} 个已接入站点`;
}

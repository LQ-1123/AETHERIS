import {
  approveRouteDestination,
  deleteRouteDestination,
  getLocalDicomNode,
  listRouteDestinations,
  testRouteDestination,
} from './api';
import { RouterTopologyCanvas, type RouterTopologyNode } from './router-topology';
import type { LocalDicomNode, RouteDestination } from './types';

export class RouterPanel {
  private readonly dialog = element<HTMLDialogElement>('dicom-router-dialog');
  private readonly topology = new RouterTopologyCanvas(
    element<HTMLCanvasElement>('router-topology-canvas'),
    element<HTMLElement>('router-topology-detail'),
  );
  private localNode: LocalDicomNode | null = null;
  private destinations: RouteDestination[] = [];
  private busy = false;
  private pollingTimer: number | null = null;

  constructor(private readonly reportError: (message: string) => void) {
    element<HTMLButtonElement>('dicom-router-btn').addEventListener('click', () => void this.open());
    element<HTMLButtonElement>('router-close').addEventListener('click', () => this.dialog.close());
    element<HTMLButtonElement>('router-refresh').addEventListener('click', () => void this.refresh());
    this.dialog.addEventListener('close', () => this.stopPolling());
  }

  setAvailable(available: boolean): void {
    element<HTMLButtonElement>('dicom-router-btn').hidden = !available;
    if (!available) {
      this.stopPolling();
      if (this.dialog.open) this.dialog.close();
    }
  }

  async open(): Promise<void> {
    if (!this.dialog.open) this.dialog.showModal();
    this.startPolling();
    await this.refresh();
  }

  private async refresh(force = false): Promise<void> {
    if (this.busy && !force) return;
    const ownsBusy = !this.busy;
    if (ownsBusy) this.setBusy(true);
    try {
      const [localNode, destinations] = await Promise.all([
        getLocalDicomNode(),
        listRouteDestinations(),
      ]);
      const testable = destinations.filter(
        (entry) => entry.approval_status === 'approved' && entry.enabled,
      );
      const results = await Promise.allSettled(
        testable.map((entry) => testRouteDestination(entry.id)),
      );
      const failedTests = new Map<string, string>();
      results.forEach((result, index) => {
        if (result.status === 'rejected') {
          failedTests.set(testable[index].id, message(result.reason));
        }
      });
      const latest = await listRouteDestinations();
      this.localNode = localNode;
      this.destinations = latest.map((entry) => failedTests.has(entry.id)
        ? { ...entry, status: 'offline', last_error: failedTests.get(entry.id) ?? entry.last_error }
        : entry);
      this.render();
      this.clearError();
    } catch (error) {
      this.showError(message(error));
    } finally {
      if (ownsBusy) this.setBusy(false);
    }
  }

  private render(): void {
    const approved = this.destinations.filter((entry) => entry.approval_status === 'approved');
    const online = approved.filter((entry) => entry.enabled && entry.status === 'online');
    const pending = this.destinations.filter((entry) => entry.approval_status === 'pending');
    this.renderTopology(online);
    this.renderRequests(pending);
    text('router-summary', `${online.length} 个在线站点 · ${pending.length} 个待确认`);
    text('router-peer-count', `${online.length} 个在线站点`);
    text('router-request-count', `${pending.length} 个待确认`);
  }

  private renderTopology(online: RouteDestination[]): void {
    const nodes: RouterTopologyNode[] = [];
    if (this.localNode) {
      nodes.push({
        id: 'local',
        side: 'local',
        label: this.localNode.ae_title,
        statusText: `监听中 · ${this.localNode.listen_host}:${this.localNode.listen_port}`,
        status: 'online',
        summary: `${this.localNode.ae_title} · PACS 本机 · DIMSE ${this.localNode.listen_host}:${this.localNode.listen_port}`,
      });
    }
    for (const destination of online) {
      const endpoint = destinationEndpoint(destination);
      nodes.push({
        id: `site:${destination.id}`,
        side: 'inbound',
        label: destination.name,
        statusText: destinationStatusText(destination),
        status: destination.enabled ? destination.status : 'disabled',
        summary: `${destination.name} · ${destination.protocol.toUpperCase()} · ${endpoint} · ${destinationStatusText(destination)}${destination.last_error ? ` · ${destination.last_error}` : ''}`,
      });
    }
    this.topology.update(nodes);
  }

  private startPolling(): void {
    if (this.pollingTimer !== null) return;
    this.pollingTimer = window.setInterval(() => {
      if (this.dialog.open) void this.refresh();
    }, 5_000);
  }

  private stopPolling(): void {
    if (this.pollingTimer === null) return;
    window.clearInterval(this.pollingTimer);
    this.pollingTimer = null;
  }

  private renderRequests(pending: RouteDestination[]): void {
    const list = element<HTMLElement>('router-request-list');
    list.replaceChildren();
    if (!pending.length) {
      list.append(empty('暂无待确认的接入申请'));
      return;
    }
    for (const request of pending) {
      list.append(row(
        block(request.name, request.protocol.toUpperCase()),
        block(destinationEndpoint(request), request.called_ae_title ? `AE ${request.called_ae_title}` : 'DICOMweb 站点'),
        requestActions(
          actionButton('同意', 'approve', () => void this.approve(request)),
          actionButton('拒绝', 'reject', () => void this.reject(request)),
        ),
      ));
    }
  }

  private async approve(request: RouteDestination): Promise<void> {
    await this.perform(async () => {
      await approveRouteDestination(request.id);
      await this.refresh(true);
    });
  }

  private async reject(request: RouteDestination): Promise<void> {
    if (!window.confirm(`拒绝站点“${request.name}”的接入申请？`)) return;
    await this.perform(async () => {
      await deleteRouteDestination(request.id);
      await this.refresh(true);
    });
  }

  private async perform(operation: () => Promise<void>): Promise<void> {
    if (this.busy) return;
    this.setBusy(true);
    try {
      await operation();
      this.clearError();
    } catch (error) {
      this.showError(message(error));
      this.reportError(message(error));
    } finally {
      this.setBusy(false);
    }
  }

  private setBusy(value: boolean): void {
    this.busy = value;
    element<HTMLButtonElement>('router-refresh').disabled = value;
    for (const button of this.dialog.querySelectorAll<HTMLButtonElement>('button')) {
      if (button.id !== 'router-close') button.disabled = value;
    }
  }

  private showError(value: string): void {
    const node = element<HTMLElement>('router-error');
    node.textContent = value;
    node.hidden = false;
  }

  private clearError(): void {
    element<HTMLElement>('router-error').hidden = true;
  }
}

function destinationStatusText(destination: RouteDestination): string {
  if (!destination.enabled) return '已停用';
  if (destination.status === 'online') {
    return `在线${destination.last_latency_ms == null ? '' : ` · ${destination.last_latency_ms} ms`}`;
  }
  return ({ offline: '离线', unknown: '未检测' })[destination.status];
}

function destinationEndpoint(destination: RouteDestination): string {
  if (destination.protocol === 'dimse') {
    return `${destination.host ?? '--'}:${destination.port ?? '--'}`;
  }
  return destination.stow_url ?? '--';
}

function element<T extends HTMLElement = HTMLElement>(id: string): T {
  const value = document.getElementById(id);
  if (!value) throw new Error(`缺少界面元素 #${id}`);
  return value as T;
}

function text(id: string, value: string): void {
  element(id).textContent = value;
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function empty(value: string): HTMLElement {
  const node = document.createElement('div');
  node.className = 'empty-worklist-message';
  node.textContent = value;
  return node;
}

function block(main: string, detail: string): HTMLElement {
  const node = document.createElement('div');
  node.className = 'router-row-main';
  const strong = document.createElement('strong');
  strong.textContent = main;
  const small = document.createElement('small');
  small.textContent = detail;
  node.append(strong, small);
  return node;
}

function row(...children: Node[]): HTMLElement {
  const node = document.createElement('div');
  node.className = 'router-row router-request-row';
  node.append(...children);
  return node;
}

function requestActions(...children: Node[]): HTMLElement {
  const node = document.createElement('div');
  node.className = 'router-request-actions';
  node.append(...children);
  return node;
}

function actionButton(
  label: string,
  kind: 'approve' | 'reject',
  action: () => void,
): HTMLButtonElement {
  const button = document.createElement('button');
  button.type = 'button';
  button.className = 'router-request-action';
  button.dataset.kind = kind;
  button.textContent = label;
  button.addEventListener('click', action);
  return button;
}

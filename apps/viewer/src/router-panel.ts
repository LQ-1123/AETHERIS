import { Activity, Pencil, RotateCcw, Trash2, createIcons } from 'lucide';
import {
  deleteRouteDestination,
  deleteRouteRule,
  listObservedDicomPeers,
  listRouteDeliveries,
  listRouteDestinations,
  listRouteRules,
  replayRouteDelivery,
  saveRouteDestination,
  saveRouteRule,
  sendRouteScope,
  testRouteDestination,
} from './api';
import type {
  ObservedDicomPeer,
  RouteDelivery,
  RouteDestination,
  RouteDestinationInput,
  RouteProtocol,
  RouteRule,
  RouteRuleInput,
} from './types';

export class RouterPanel {
  private readonly dialog = element<HTMLDialogElement>('dicom-router-dialog');
  private peers: ObservedDicomPeer[] = [];
  private destinations: RouteDestination[] = [];
  private rules: RouteRule[] = [];
  private deliveries: RouteDelivery[] = [];
  private busy = false;

  constructor(private readonly reportError: (message: string) => void) {
    this.bind();
  }

  setAvailable(available: boolean): void {
    element<HTMLButtonElement>('dicom-router-btn').hidden = !available;
    if (!available && this.dialog.open) this.dialog.close();
  }

  async open(): Promise<void> {
    if (!this.dialog.open) this.dialog.showModal();
    await this.refresh();
  }

  private bind(): void {
    element<HTMLButtonElement>('dicom-router-btn').addEventListener('click', () => void this.open());
    element<HTMLButtonElement>('router-close').addEventListener('click', () => this.dialog.close());
    element<HTMLButtonElement>('router-refresh').addEventListener('click', () => void this.refresh());
    for (const button of document.querySelectorAll<HTMLButtonElement>('[data-router-tab]')) {
      button.addEventListener('click', () => this.selectTab(button.dataset.routerTab ?? 'destinations'));
    }
    element<HTMLSelectElement>('router-destination-protocol').addEventListener('change', () => this.updateProtocolFields());
    element<HTMLButtonElement>('router-destination-reset').addEventListener('click', () => this.resetDestination());
    element<HTMLFormElement>('router-destination-form').addEventListener('submit', (event) => {
      event.preventDefault();
      void this.submitDestination();
    });
    element<HTMLButtonElement>('router-rule-reset').addEventListener('click', () => this.resetRule());
    element<HTMLFormElement>('router-rule-form').addEventListener('submit', (event) => {
      event.preventDefault();
      void this.submitRule();
    });
    element<HTMLFormElement>('router-send-form').addEventListener('submit', (event) => {
      event.preventDefault();
      void this.submitSend();
    });
  }

  private selectTab(name: string): void {
    for (const button of document.querySelectorAll<HTMLButtonElement>('[data-router-tab]')) {
      button.setAttribute('aria-selected', String(button.dataset.routerTab === name));
    }
    for (const view of document.querySelectorAll<HTMLElement>('[data-router-view]')) {
      view.hidden = view.dataset.routerView !== name;
    }
  }

  private async refresh(): Promise<void> {
    if (this.busy) return;
    this.setBusy(true);
    try {
      [this.peers, this.destinations, this.rules, this.deliveries] = await Promise.all([
        listObservedDicomPeers(),
        listRouteDestinations(),
        listRouteRules(),
        listRouteDeliveries(),
      ]);
      this.render();
      this.clearError();
    } catch (error) {
      this.showError(message(error));
    } finally {
      this.setBusy(false);
    }
  }

  private render(): void {
    this.renderObservedPeers();
    this.renderDestinations();
    this.renderRules();
    this.renderDeliveries();
    this.renderDestinationOptions();
    const online = this.destinations.filter((entry) => entry.status === 'online').length;
    text('router-summary', `${this.peers.length} 台入站设备 · ${this.destinations.length} 个目的地 · ${online} 个在线`);
    createIcons({ icons: { Activity, Pencil, RotateCcw, Trash2 } });
  }

  private renderObservedPeers(): void {
    const list = element<HTMLElement>('router-peer-list');
    list.replaceChildren();
    text('router-peer-count', `${this.peers.length} 台`);
    if (!this.peers.length) return void list.append(empty('尚未发现入站 DIMSE 设备'));
    for (const peer of this.peers) {
      const status = document.createElement('span');
      status.className = 'router-status';
      status.dataset.status = peer.status === 'connected' ? 'online' : peer.status === 'offline' ? 'offline' : 'unknown';
      status.textContent = observedStatusLabel(peer.status);
      const destination = this.destinations.find((entry) => entry.protocol === 'dimse'
        && entry.called_ae_title === peer.calling_ae_title);
      list.append(row(
        block(peer.calling_ae_title, `来源 ${peer.remote_host}`),
        blockNode(status, `最近 ${formatDate(peer.last_seen_at)}`),
        block(`${peer.association_count} 次关联`, destination ? `已配置回传 · ${destination.name}` : '未配置回传'),
        actions(document.createElement('span')),
      ));
    }
  }

  private renderDestinations(): void {
    const list = element<HTMLElement>('router-destination-list');
    list.replaceChildren();
    if (!this.destinations.length) return void list.append(empty('尚未配置路由目的地'));
    for (const destination of this.destinations) {
      const endpoint = destination.protocol === 'dimse'
        ? `${destination.called_ae_title}@${destination.host}:${destination.port}`
        : destination.stow_url ?? '--';
      const status = document.createElement('span');
      status.className = 'router-status';
      status.dataset.status = destination.status;
      status.textContent = statusLabel(destination.status);
      const latency = destination.last_latency_ms == null ? '未检测' : `${destination.last_latency_ms} ms`;
      list.append(row(
        block(destination.name, `${destination.protocol.toUpperCase()}${destination.enabled ? '' : ' · 已停用'}`),
        block(endpoint, destination.last_error ?? '连接信息正常'),
        blockNode(status, latency),
        actions(
          iconButton('activity', '测试连接', () => void this.test(destination.id)),
          iconButton('pencil', '编辑设备', () => this.editDestination(destination)),
          iconButton('trash-2', '删除设备', () => void this.removeDestination(destination)),
        ),
      ));
    }
  }

  private renderRules(): void {
    const list = element<HTMLElement>('router-rule-list');
    list.replaceChildren();
    if (!this.rules.length) return void list.append(empty('尚未配置自动路由规则'));
    for (const rule of this.rules) {
      const conditions = [rule.source_ae_title && `AE ${rule.source_ae_title}`, rule.modality,
        rule.body_part_examined, rule.study_description, rule.series_description]
        .filter(Boolean).join(' · ') || '全部新增实例';
      list.append(row(
        block(rule.name, `优先级 ${rule.priority}${rule.enabled ? '' : ' · 已停用'}`),
        block(rule.destination_name, conditions),
        block(`${Object.keys(rule.tag_matches).length} 个 Tag 条件`, ''),
        actions(
          iconButton('pencil', '编辑规则', () => this.editRule(rule)),
          iconButton('trash-2', '删除规则', () => void this.removeRule(rule)),
        ),
      ));
    }
  }

  private renderDeliveries(): void {
    const list = element<HTMLElement>('router-delivery-list');
    list.replaceChildren();
    if (!this.deliveries.length) return void list.append(empty('尚无路由投递记录'));
    for (const delivery of this.deliveries) {
      const status = document.createElement('span');
      status.className = 'router-status';
      status.dataset.status = delivery.status === 'succeeded' ? 'online' : delivery.status === 'dead_letter' ? 'offline' : 'unknown';
      status.textContent = deliveryLabel(delivery.status);
      list.append(row(
        block(delivery.destination_name, formatDate(delivery.created_at)),
        block(delivery.sop_instance_uid, delivery.last_error ?? '无错误'),
        blockNode(status, `${delivery.attempts} 次尝试`),
        actions(delivery.status === 'dead_letter'
          ? iconButton('rotate-ccw', '重放死信', () => void this.replay(delivery.id))
          : document.createElement('span')),
      ));
    }
  }

  private renderDestinationOptions(): void {
    for (const id of ['router-rule-destination', 'router-send-destination']) {
      const select = element<HTMLSelectElement>(id);
      const selected = select.value;
      select.replaceChildren(...this.destinations.map((entry) => {
        const option = document.createElement('option');
        option.value = entry.id;
        option.textContent = `${entry.name} · ${entry.protocol.toUpperCase()}`;
        return option;
      }));
      if (this.destinations.some((entry) => entry.id === selected)) select.value = selected;
    }
  }

  private async test(id: string): Promise<void> {
    await this.perform(async () => { await testRouteDestination(id); await this.refresh(); });
  }

  private editDestination(value: RouteDestination): void {
    input('router-destination-id').value = value.id;
    input('router-destination-name').value = value.name;
    select('router-destination-protocol').value = value.protocol;
    checkbox('router-destination-enabled').checked = value.enabled;
    input('router-destination-host').value = value.host ?? '';
    input('router-destination-port').value = String(value.port ?? 104);
    input('router-destination-called-ae').value = value.called_ae_title ?? 'STORESCP';
    input('router-destination-calling-ae').value = value.calling_ae_title ?? 'REMOTE_PACS';
    checkbox('router-destination-tls').checked = value.use_tls;
    textarea('router-destination-dimse-ca').value = '';
    input('router-destination-stow-url').value = value.stow_url ?? '';
    input('router-destination-token').value = '';
    textarea('router-destination-ca').value = '';
    this.updateProtocolFields();
  }

  private resetDestination(): void {
    element<HTMLFormElement>('router-destination-form').reset();
    input('router-destination-id').value = '';
    select('router-destination-protocol').value = 'dimse';
    checkbox('router-destination-enabled').checked = true;
    input('router-destination-port').value = '104';
    input('router-destination-called-ae').value = 'STORESCP';
    input('router-destination-calling-ae').value = 'REMOTE_PACS';
    this.updateProtocolFields();
  }

  private updateProtocolFields(): void {
    const dimse = select('router-destination-protocol').value === 'dimse';
    element<HTMLElement>('router-dimse-fields').hidden = !dimse;
    element<HTMLElement>('router-stow-fields').hidden = dimse;
  }

  private async submitDestination(): Promise<void> {
    const protocol = select('router-destination-protocol').value as RouteProtocol;
    const value: RouteDestinationInput = {
      name: input('router-destination-name').value.trim(), protocol,
      enabled: checkbox('router-destination-enabled').checked,
    };
    if (protocol === 'dimse') {
      Object.assign(value, { host: input('router-destination-host').value.trim(),
        port: Number(input('router-destination-port').value), called_ae_title: input('router-destination-called-ae').value.trim(),
        calling_ae_title: input('router-destination-calling-ae').value.trim(), use_tls: checkbox('router-destination-tls').checked,
        ca_pem: textarea('router-destination-dimse-ca').value.trim() || undefined });
    } else {
      Object.assign(value, { stow_url: input('router-destination-stow-url').value.trim(),
        auth_token: input('router-destination-token').value.trim() || undefined,
        ca_pem: textarea('router-destination-ca').value.trim() || undefined });
    }
    await this.perform(async () => {
      await saveRouteDestination(value, input('router-destination-id').value || undefined);
      this.resetDestination(); await this.refresh();
    });
  }

  private async removeDestination(value: RouteDestination): Promise<void> {
    if (!window.confirm(`删除路由目的地“${value.name}”及其规则和投递记录？`)) return;
    await this.perform(async () => { await deleteRouteDestination(value.id); await this.refresh(); });
  }

  private editRule(value: RouteRule): void {
    input('router-rule-id').value = value.id; input('router-rule-name').value = value.name;
    select('router-rule-destination').value = value.destination_id; input('router-rule-priority').value = String(value.priority);
    checkbox('router-rule-enabled').checked = value.enabled; input('router-rule-source').value = value.source_ae_title ?? '';
    input('router-rule-modality').value = value.modality ?? ''; input('router-rule-body-part').value = value.body_part_examined ?? '';
    input('router-rule-study-description').value = value.study_description ?? '';
    input('router-rule-series-description').value = value.series_description ?? '';
    textarea('router-rule-tags').value = JSON.stringify(value.tag_matches, null, 2);
  }

  private resetRule(): void {
    element<HTMLFormElement>('router-rule-form').reset(); input('router-rule-id').value = '';
    input('router-rule-priority').value = '100'; checkbox('router-rule-enabled').checked = true;
    textarea('router-rule-tags').value = '{}';
  }

  private async submitRule(): Promise<void> {
    let tags: unknown;
    try { tags = JSON.parse(textarea('router-rule-tags').value || '{}'); }
    catch { return this.showError('指定 Tag 必须是合法 JSON'); }
    if (!tags || Array.isArray(tags) || typeof tags !== 'object') return this.showError('指定 Tag 必须是 JSON 对象');
    const optional = (id: string): string | undefined => input(id).value.trim() || undefined;
    const value: RouteRuleInput = { destination_id: select('router-rule-destination').value,
      name: input('router-rule-name').value.trim(), priority: Number(input('router-rule-priority').value),
      enabled: checkbox('router-rule-enabled').checked, source_ae_title: optional('router-rule-source'),
      modality: optional('router-rule-modality'), body_part_examined: optional('router-rule-body-part'),
      study_description: optional('router-rule-study-description'), series_description: optional('router-rule-series-description'),
      tag_matches: tags as Record<string, unknown> };
    await this.perform(async () => { await saveRouteRule(value, input('router-rule-id').value || undefined); this.resetRule(); await this.refresh(); });
  }

  private async removeRule(value: RouteRule): Promise<void> {
    if (!window.confirm(`删除路由规则“${value.name}”？`)) return;
    await this.perform(async () => { await deleteRouteRule(value.id); await this.refresh(); });
  }

  private async submitSend(): Promise<void> {
    await this.perform(async () => {
      const result = await sendRouteScope(select('router-send-destination').value,
        input('router-send-study').value.trim(), input('router-send-series').value.trim() || undefined);
      text('router-summary', `已排队 ${result.queued} 项 · 跳过 ${result.skipped_as_duplicate} 项`);
      await this.refresh();
    });
  }

  private async replay(id: string): Promise<void> {
    await this.perform(async () => { await replayRouteDelivery(id); await this.refresh(); });
  }

  private async perform(operation: () => Promise<void>): Promise<void> {
    if (this.busy) return;
    this.setBusy(true);
    try { await operation(); this.clearError(); }
    catch (error) { this.showError(message(error)); }
    finally { this.setBusy(false); }
  }

  private setBusy(value: boolean): void {
    this.busy = value;
    element<HTMLButtonElement>('router-refresh').disabled = value;
    for (const button of this.dialog.querySelectorAll<HTMLButtonElement>('button[type="submit"]')) button.disabled = value;
  }

  private showError(value: string): void {
    const error = element<HTMLElement>('router-error'); error.textContent = value; error.hidden = false;
    this.reportError(value);
  }
  private clearError(): void { element<HTMLElement>('router-error').hidden = true; }
}

function element<T extends HTMLElement>(id: string): T { const value = document.getElementById(id); if (!value) throw new Error(`缺少元素 #${id}`); return value as T; }
const input = (id: string): HTMLInputElement => element(id);
const select = (id: string): HTMLSelectElement => element(id);
const textarea = (id: string): HTMLTextAreaElement => element(id);
const checkbox = (id: string): HTMLInputElement => element(id);
function text(id: string, value: string): void { element(id).textContent = value; }
function message(error: unknown): string { return error instanceof Error ? error.message : String(error); }
function formatDate(value: string): string { const date = new Date(value); return Number.isNaN(date.getTime()) ? value : date.toLocaleString('zh-CN'); }
function statusLabel(status: RouteDestination['status']): string { return ({ online: '在线', offline: '离线', unknown: '未检测' })[status]; }
function observedStatusLabel(status: ObservedDicomPeer['status']): string { return ({ connected: '连接中', recent: '最近连接', offline: '历史设备' })[status]; }
function deliveryLabel(status: RouteDelivery['status']): string { return ({ queued: '等待', running: '发送中', succeeded: '成功', dead_letter: '死信' })[status]; }
function empty(value: string): HTMLElement { const node = document.createElement('div'); node.className = 'empty-worklist-message'; node.textContent = value; return node; }
function block(main: string, detail: string): HTMLElement { const node = document.createElement('div'); node.className = 'router-row-main'; const strong = document.createElement('strong'); strong.textContent = main; const small = document.createElement('small'); small.textContent = detail; node.append(strong, small); return node; }
function blockNode(main: Node, detail: string): HTMLElement { const node = document.createElement('div'); node.className = 'router-row-detail'; const small = document.createElement('small'); small.textContent = detail; node.append(main, small); return node; }
function row(...children: Node[]): HTMLElement { const node = document.createElement('div'); node.className = 'router-row'; node.append(...children); return node; }
function actions(...children: Node[]): HTMLElement { const node = document.createElement('div'); node.className = 'router-row-actions'; node.append(...children); return node; }
function iconButton(icon: string, title: string, action: () => void): HTMLButtonElement { const button = document.createElement('button'); button.type = 'button'; button.className = 'icon-button'; button.title = title; button.setAttribute('aria-label', title); button.innerHTML = `<i data-lucide="${icon}"></i>`; button.addEventListener('click', action); return button; }

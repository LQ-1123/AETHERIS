import {
  approveDevice,
  listDevices,
  listSeriesSources,
  listUserDeviceGrants,
  listUsers,
  registerDevice,
  replaceUserDeviceGrants,
  resolveSeriesSource,
  setDeviceStatus,
} from './api';
import type { AdminUser, DicomDevice, SeriesSourceEntry } from './types';

function element<T extends HTMLElement = HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (!found) throw new Error(`缺少元素 #${id}`);
  return found as T;
}

type AdminTab = 'devices' | 'sources' | 'grants';

const SOURCE_STATUS_LABELS: Record<string, string> = {
  legacy_unattributed: '历史未归属',
  needs_review: '待复核',
  trusted: '已归属',
};

/**
 * 管理员控制台：设备注册/批准、未归属序列来源归属、用户设备授权。
 * 仅 admin 可见（入口按钮由 app.ts 按角色控制显隐）。
 */
export class AdminConsole {
  private readonly dialog = element<HTMLDialogElement>('admin-console-dialog');
  private readonly error = element<HTMLElement>('admin-console-error');
  private readonly body = element<HTMLElement>('admin-console-body');

  private tab: AdminTab = 'devices';
  private devices: DicomDevice[] = [];
  private users: AdminUser[] = [];
  private busy = false;
  private sourcesOffset = 0;
  /** 渲染代际：每次渲染自增；过期渲染不得写 DOM（防快速点击时的交错渲染）。 */
  private renderGeneration = 0;

  constructor(private readonly reportError: (message: string) => void) {
    element<HTMLButtonElement>('admin-console-btn').addEventListener('click', () => void this.open());
    element<HTMLButtonElement>('admin-console-close').addEventListener('click', () => this.dialog.close());
    element<HTMLButtonElement>('admin-console-refresh').addEventListener('click', () => void this.refresh());
    for (const button of document.querySelectorAll<HTMLButtonElement>('[data-admin-tab]')) {
      button.addEventListener('click', () => {
        this.tab = button.dataset.adminTab as AdminTab;
        for (const candidate of document.querySelectorAll<HTMLButtonElement>('[data-admin-tab]')) {
          candidate.classList.toggle('active', candidate === button);
        }
        void this.refresh();
      });
    }
  }

  async open(): Promise<void> {
    if (!this.dialog.open) this.dialog.showModal();
    await this.refresh();
  }

  close(): void {
    if (this.dialog.open) this.dialog.close();
  }

  private showError(message: string): void {
    this.error.textContent = message;
    this.error.classList.remove('dialog-success');
    this.error.hidden = false;
    this.reportError(message);
  }

  private showSuccess(message: string): void {
    this.error.textContent = message;
    this.error.classList.add('dialog-success');
    this.error.hidden = false;
  }

  private clearError(): void {
    this.error.hidden = true;
  }

  async refresh(): Promise<void> {
    if (this.busy) return;
    this.busy = true;
    this.clearError();
    try {
      const [devices, users] = await Promise.all([listDevices(), listUsers()]);
      this.devices = devices;
      this.users = users;
      if (this.tab === 'devices') await this.renderDevices();
      else if (this.tab === 'sources') await this.renderSources();
      else await this.renderGrants();
    } catch (error) {
      this.showError(errorMessage(error));
    } finally {
      this.busy = false;
    }
  }

  private activeDevices(): DicomDevice[] {
    return this.devices.filter((device) => device.status === 'active');
  }

  private deviceOptions(selectedId: string | null, placeholder: string): HTMLSelectElement {
    const select = document.createElement('select');
    const placeholderOption = document.createElement('option');
    placeholderOption.value = '';
    placeholderOption.textContent = placeholder;
    select.append(placeholderOption);
    for (const device of this.activeDevices()) {
      const option = document.createElement('option');
      option.value = device.id;
      option.textContent = `${device.name}（${device.calling_ae_title}）`;
      if (device.id === selectedId) option.selected = true;
      select.append(option);
    }
    return select;
  }

  private async renderDevices(): Promise<void> {
    const generation = ++this.renderGeneration;
    const fragment = document.createDocumentFragment();

    const form = document.createElement('form');
    form.className = 'admin-device-form';
    const fields: Array<[string, string, string, string]> = [
      ['device-name', '名称', '如：CT 室西门子', 'text'],
      ['device-ae', 'AE Title', '如：CTROOM1', 'text'],
      ['device-ip', '来源 IP', '如：192.168.1.50', 'text'],
      ['device-modality', '模态提示（可选）', 'CT / MR / DR', 'text'],
    ];
    for (const [id, label, placeholder, type] of fields) {
      const wrapper = document.createElement('label');
      wrapper.textContent = label;
      const input = document.createElement('input');
      input.type = type;
      input.id = id;
      input.placeholder = placeholder;
      wrapper.append(input);
      form.append(wrapper);
    }
    const register = document.createElement('button');
    register.type = 'submit';
    register.className = 'command-button';
    register.textContent = '注册设备';
    form.append(register);
    form.addEventListener('submit', (event) => {
      event.preventDefault();
      void this.register();
    });
    fragment.append(form);

    const list = document.createElement('div');
    list.className = 'admin-list';
    if (this.devices.length === 0) {
      const empty = document.createElement('p');
      empty.className = 'admin-empty';
      empty.textContent = '暂无设备。设备可由 DIMSE 入站自动发现，或在上方手动注册。';
      list.append(empty);
    }
    for (const device of this.devices) {
      const row = document.createElement('div');
      row.className = 'admin-row';
      const info = document.createElement('div');
      info.className = 'admin-row-info';
      const title = document.createElement('strong');
      title.textContent = device.name;
      const meta = document.createElement('span');
      meta.textContent = [
        device.calling_ae_title,
        device.source_ip,
        device.modality_hint ?? '',
      ].filter(Boolean).join(' · ');
      info.append(title, meta);
      const status = document.createElement('span');
      status.className = 'admin-status';
      status.dataset.status = device.status;
      status.textContent = { pending: '待批准', active: '已启用', disabled: '已禁用' }[device.status] ?? device.status;
      const actions = document.createElement('div');
      actions.className = 'admin-row-actions';
      if (device.status === 'pending') {
        const approve = document.createElement('button');
        approve.type = 'button';
        approve.className = 'command-button';
        approve.textContent = '批准';
        approve.addEventListener('click', () => void this.approve(device));
        actions.append(approve);
      }
      const toggle = document.createElement('button');
      toggle.type = 'button';
      toggle.className = 'command-button';
      toggle.textContent = device.status === 'disabled' ? '启用' : '禁用';
      if (device.status === 'pending') toggle.disabled = true;
      else toggle.addEventListener('click', () => void this.toggleStatus(device));
      actions.append(toggle);
      row.append(info, status, actions);
      list.append(row);
    }
    fragment.append(list);
    if (generation !== this.renderGeneration) return;
    this.body.replaceChildren(fragment);
  }

  private async register(): Promise<void> {
    const name = element<HTMLInputElement>('device-name').value.trim();
    const ae = element<HTMLInputElement>('device-ae').value.trim();
    const ip = element<HTMLInputElement>('device-ip').value.trim();
    const modality = element<HTMLInputElement>('device-modality').value.trim() || null;
    if (!name || !ae || !ip) {
      this.showError('名称、AE Title 与来源 IP 不能为空');
      return;
    }
    this.clearError();
    try {
      await registerDevice(name, ae, ip, modality);
      await this.refresh();
    } catch (error) {
      this.showError(errorMessage(error));
    }
  }

  private async approve(device: DicomDevice): Promise<void> {
    this.clearError();
    try {
      await approveDevice(device.id, device.name, device.modality_hint);
      await this.refresh();
    } catch (error) {
      this.showError(errorMessage(error));
    }
  }

  private async toggleStatus(device: DicomDevice): Promise<void> {
    this.clearError();
    try {
      await setDeviceStatus(device.id, device.status === 'disabled' ? 'active' : 'disabled');
      await this.refresh();
    } catch (error) {
      this.showError(errorMessage(error));
    }
  }

  private async renderSources(): Promise<void> {
    const generation = ++this.renderGeneration;
    const fragment = document.createDocumentFragment();
    const activeDevices = this.activeDevices();
    if (activeDevices.length === 0) {
      const hint = document.createElement('p');
      hint.className = 'admin-empty';
      hint.textContent = '还没有已启用的设备。请先在「设备」页注册并批准设备，再来归属历史序列。';
      fragment.append(hint);
      if (generation !== this.renderGeneration) return;
      this.body.replaceChildren(fragment);
      return;
    }

    const bulk = document.createElement('div');
    bulk.className = 'admin-bulk';
    const bulkLabel = document.createElement('span');
    bulkLabel.textContent = '批量归属本页全部序列到：';
    const bulkSelect = this.deviceOptions(null, '选择设备');
    const bulkButton = document.createElement('button');
    bulkButton.type = 'button';
    bulkButton.className = 'command-button';
    bulkButton.textContent = '归属本页';
    bulkButton.addEventListener('click', () => void this.bulkResolve(bulkSelect.value));
    bulk.append(bulkLabel, bulkSelect, bulkButton);
    fragment.append(bulk);

    let entries: SeriesSourceEntry[] = [];
    try {
      entries = await listSeriesSources(true, 50, this.sourcesOffset);
    } catch (error) {
      this.showError(errorMessage(error));
      return;
    }
    const list = document.createElement('div');
    list.className = 'admin-list';
    if (entries.length === 0 && this.sourcesOffset === 0) {
      const empty = document.createElement('p');
      empty.className = 'admin-empty';
      empty.textContent = '没有待归属的序列。';
      list.append(empty);
    }
    for (const entry of entries) {
      const row = document.createElement('div');
      row.className = 'admin-row';
      const info = document.createElement('div');
      info.className = 'admin-row-info';
      const title = document.createElement('strong');
      title.textContent = entry.patient_name || entry.patient_id;
      const meta = document.createElement('span');
      meta.textContent = [
        entry.modality ?? '',
        entry.description ?? '',
        `${entry.instance_count} 帧`,
      ].filter(Boolean).join(' · ');
      info.append(title, meta);
      const status = document.createElement('span');
      status.className = 'admin-status';
      status.dataset.status = 'pending';
      status.textContent = SOURCE_STATUS_LABELS[entry.source_status] ?? entry.source_status;
      const actions = document.createElement('div');
      actions.className = 'admin-row-actions';
      const select = this.deviceOptions(null, '归属到设备');
      const resolve = document.createElement('button');
      resolve.type = 'button';
      resolve.className = 'command-button';
      resolve.textContent = '归属';
      resolve.addEventListener('click', () => void this.resolve(entry.series_uid, select.value));
      actions.append(select, resolve);
      row.append(info, status, actions);
      list.append(row);
    }
    fragment.append(list);

    const pager = document.createElement('div');
    pager.className = 'admin-pager';
    const previous = document.createElement('button');
    previous.type = 'button';
    previous.className = 'command-button';
    previous.textContent = '上一页';
    previous.disabled = this.sourcesOffset === 0;
    previous.addEventListener('click', () => {
      this.sourcesOffset = Math.max(0, this.sourcesOffset - 50);
      void this.renderSources();
    });
    const next = document.createElement('button');
    next.type = 'button';
    next.className = 'command-button';
    next.textContent = '下一页';
    next.disabled = entries.length < 50;
    next.addEventListener('click', () => {
      this.sourcesOffset += 50;
      void this.renderSources();
    });
    pager.append(previous, next);
    fragment.append(pager);
    if (generation !== this.renderGeneration) return;
    this.body.replaceChildren(fragment);
  }

  private async resolve(seriesUid: string, deviceId: string): Promise<void> {
    if (!deviceId) {
      this.showError('请先选择归属设备');
      return;
    }
    this.clearError();
    try {
      await resolveSeriesSource(seriesUid, deviceId);
      await this.renderSources();
    } catch (error) {
      this.showError(errorMessage(error));
    }
  }

  private async bulkResolve(deviceId: string): Promise<void> {
    if (!deviceId) {
      this.showError('请先选择归属设备');
      return;
    }
    if (!window.confirm('将本页全部待归属序列归属到该设备？序列来源将标记为可信。')) return;
    this.clearError();
    const entries = await listSeriesSources(true, 50, this.sourcesOffset);
    let succeeded = 0;
    const failures: string[] = [];
    for (const entry of entries) {
      try {
        await resolveSeriesSource(entry.series_uid, deviceId);
        succeeded += 1;
      } catch (error) {
        failures.push(`${entry.patient_name || entry.patient_id}: ${errorMessage(error)}`);
      }
    }
    if (failures.length > 0) {
      this.showError(`成功 ${succeeded} / ${entries.length}，失败：${failures.slice(0, 3).join('；')}`);
    }
    await this.renderSources();
  }

  private async renderGrants(): Promise<void> {
    const generation = ++this.renderGeneration;
    const fragment = document.createDocumentFragment();
    const activeDevices = this.activeDevices();
    const list = document.createElement('div');
    list.className = 'admin-list';
    if (this.users.length === 0) {
      const empty = document.createElement('p');
      empty.className = 'admin-empty';
      empty.textContent = '暂无用户。';
      list.append(empty);
    }
    for (const user of this.users) {
      const row = document.createElement('div');
      row.className = 'admin-row admin-grant-row';
      const info = document.createElement('div');
      info.className = 'admin-row-info';
      const title = document.createElement('strong');
      title.textContent = user.display_name || user.username;
      const meta = document.createElement('span');
      meta.textContent = `${user.username} · ${user.role}${user.is_active ? '' : ' · 已停用'}`;
      info.append(title, meta);
      row.append(info);

      const grantBox = document.createElement('div');
      grantBox.className = 'admin-grant-box';
      if (activeDevices.length === 0) {
        const hint = document.createElement('span');
        hint.textContent = '无已启用设备可授权';
        grantBox.append(hint);
      } else {
        let grants: string[] = [];
        try {
          grants = await listUserDeviceGrants(user.id);
        } catch (error) {
          this.showError(errorMessage(error));
        }
        const granted = new Set(grants);
        for (const device of activeDevices) {
          const label = document.createElement('label');
          const checkbox = document.createElement('input');
          checkbox.type = 'checkbox';
          checkbox.value = device.id;
          checkbox.checked = granted.has(device.id);
          label.append(checkbox, document.createTextNode(`${device.name}（${device.calling_ae_title}）`));
          grantBox.append(label);
        }
        const save = document.createElement('button');
        save.type = 'button';
        save.className = 'command-button';
        save.textContent = '保存授权';
        save.addEventListener('click', () => {
          const checked = Array.from(
            grantBox.querySelectorAll<HTMLInputElement>('input[type="checkbox"]:checked'),
          ).map((checkbox) => checkbox.value);
          void this.saveGrants(user, checked);
        });
        grantBox.append(save);
      }
      row.append(grantBox);
      list.append(row);
    }
    fragment.append(list);
    if (generation !== this.renderGeneration) return;
    this.body.replaceChildren(fragment);
  }

  private async saveGrants(user: AdminUser, deviceIds: string[]): Promise<void> {
    this.clearError();
    const label = user.display_name || user.username;
    if (deviceIds.length === 0) {
      const confirmed = window.confirm(
        `未勾选任何设备，保存将清空「${label}」的全部设备授权。确认继续？`,
      );
      if (!confirmed) return;
    }
    try {
      await replaceUserDeviceGrants(user.id, deviceIds);
      this.showSuccess(`已保存「${label}」的授权（${deviceIds.length} 台设备）。`);
      await this.renderGrants();
    } catch (error) {
      this.showError(errorMessage(error));
    }
  }
}

function errorMessage(error: unknown): string {
  if (typeof error === 'string') return error;
  if (error instanceof Error) return error.message;
  return String(error);
}

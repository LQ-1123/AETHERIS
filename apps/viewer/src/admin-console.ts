import {
  approveDevice,
  createUser,
  getInstitutionSettings,
  listDevices,
  listPasswordResetRequests,
  listSeriesSources,
  listUserDeviceGrants,
  listUsers,
  listUserPermissions,
  registerDevice,
  replaceUserDeviceGrants,
  replaceUserPermissions,
  reviewPasswordResetRequest,
  resolveSeriesSource,
  setDeviceStatus,
  updateUser,
  updateInstitutionSettings,
  workloadReport,
} from './api';
import type { AdminUser, DicomDevice, PasswordResetRequest, SeriesSourceEntry, WorkloadRow } from './types';

function element<T extends HTMLElement = HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (!found) throw new Error(`缺少元素 #${id}`);
  return found as T;
}

type AdminTab = 'accounts' | 'password-resets' | 'devices' | 'sources' | 'grants' | 'workload' | 'settings';

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
  private passwordResetRequests: PasswordResetRequest[] = [];
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
      const [devices, users, passwordResetRequests] = await Promise.all([
        listDevices(),
        listUsers(),
        listPasswordResetRequests(),
      ]);
      this.devices = devices;
      this.users = users;
      this.passwordResetRequests = passwordResetRequests;
      if (this.tab === 'accounts') await this.renderAccounts();
      else if (this.tab === 'password-resets') this.renderPasswordResetRequests();
      else if (this.tab === 'devices') await this.renderDevices();
      else if (this.tab === 'sources') await this.renderSources();
      else if (this.tab === 'grants') await this.renderGrants();
      else if (this.tab === 'workload') await this.renderWorkload();
      else await this.renderSettings();
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

  private async renderAccounts(): Promise<void> {
    const generation = ++this.renderGeneration;
    const fragment = document.createDocumentFragment();
    const form = document.createElement('form');
    form.className = 'admin-account-form';
    const username = accountInput('账号', '小写字母、数字、. _ -', 'text');
    username.input.autocomplete = 'off';
    const displayName = accountInput('姓名', '医生姓名', 'text');
    const password = accountInput('临时密码', '至少 12 位，不能包含用户名', 'password');
    password.input.autocomplete = 'new-password';
    const roleLabel = document.createElement('label');
    roleLabel.append(document.createTextNode('角色'));
    const role = roleSelect('radiologist');
    roleLabel.append(role);
    const create = document.createElement('button');
    create.type = 'submit';
    create.className = 'command-button';
    create.textContent = '创建账号';
    form.append(username.label, displayName.label, roleLabel, password.label, create);
    form.addEventListener('submit', (event) => {
      event.preventDefault();
      void this.createAccount(
        username.input.value,
        displayName.input.value,
        role.value,
        password.input.value,
      );
    });
    fragment.append(form);

    const permissionRows = await Promise.all(this.users.map(async (user) => ({
      user,
      permissions: await listUserPermissions(user.id),
    })));
    const list = document.createElement('div');
    list.className = 'admin-list admin-account-list';
    for (const { user, permissions } of permissionRows) {
      const row = document.createElement('div');
      row.className = 'admin-row admin-account-row';
      const info = document.createElement('div');
      info.className = 'admin-row-info';
      const title = document.createElement('strong');
      title.textContent = user.display_name || user.username;
      const meta = document.createElement('span');
      meta.textContent = `${user.username} · ${user.is_active ? '已启用' : '已停用'}${user.must_change_password ? ' · 待首次改密' : ''}`;
      info.append(title, meta);

      const controls = document.createElement('div');
      controls.className = 'admin-account-controls';
      const display = document.createElement('input');
      display.type = 'text';
      display.value = user.display_name ?? '';
      display.placeholder = '姓名';
      display.setAttribute('aria-label', `${user.username} 姓名`);
      const selectedRole = roleSelect(user.role);
      selectedRole.setAttribute('aria-label', `${user.username} 角色`);
      const reviewLabel = document.createElement('label');
      reviewLabel.className = 'admin-review-permission';
      const review = document.createElement('input');
      review.type = 'checkbox';
      review.checked = permissions.includes('review_report');
      review.disabled = !['admin', 'radiologist'].includes(user.role);
      reviewLabel.append(review, document.createTextNode('可审核报告'));
      const save = actionButton('保存', () => void this.saveAccount(user, display.value, selectedRole.value, review.checked));
      const toggle = actionButton(user.is_active ? '停用' : '启用', () => void this.toggleAccount(user));
      controls.append(display, selectedRole, reviewLabel, save, toggle);
      row.append(info, controls);
      list.append(row);
    }
    if (!this.users.length) {
      const empty = document.createElement('p'); empty.className = 'admin-empty'; empty.textContent = '暂无账号。'; list.append(empty);
    }
    fragment.append(list);
    if (generation !== this.renderGeneration) return;
    this.body.replaceChildren(fragment);
  }

  private async createAccount(
    username: string,
    displayName: string,
    role: string,
    temporaryPassword: string,
  ): Promise<void> {
    try {
      await createUser({
        username: username.trim(),
        displayName: displayName.trim() || null,
        role,
        temporaryPassword,
      });
      this.showSuccess(`账号 ${username.trim()} 已创建，首次登录必须修改密码。`);
      this.users = await listUsers();
      await this.renderAccounts();
    } catch (error) { this.showError(errorMessage(error)); }
  }

  private async saveAccount(
    user: AdminUser,
    displayName: string,
    role: string,
    reviewReport: boolean,
  ): Promise<void> {
    try {
      await updateUser(user.id, { displayName: displayName.trim() || null, role });
      const canReview = ['admin', 'radiologist'].includes(role) && reviewReport;
      await replaceUserPermissions(user.id, canReview ? ['review_report'] : []);
      this.showSuccess(`已保存 ${user.username} 的角色与审核权限。`);
      this.users = await listUsers();
      await this.renderAccounts();
    } catch (error) { this.showError(errorMessage(error)); }
  }

  private async toggleAccount(user: AdminUser): Promise<void> {
    if (user.is_active && !window.confirm(`确认停用账号 ${user.username}？其会话将被吊销。`)) return;
    try {
      await updateUser(user.id, { isActive: !user.is_active });
      this.showSuccess(`账号 ${user.username} 已${user.is_active ? '停用' : '启用'}。`);
      this.users = await listUsers();
      await this.renderAccounts();
    } catch (error) { this.showError(errorMessage(error)); }
  }

  private renderPasswordResetRequests(): void {
    const generation = ++this.renderGeneration;
    const fragment = document.createDocumentFragment();
    const intro = document.createElement('p');
    intro.className = 'admin-section-note';
    intro.textContent = '用户在登录页提交希望使用的新密码。管理员只能批准或拒绝，无法查看或修改密码内容。';
    fragment.append(intro);
    const list = document.createElement('div');
    list.className = 'admin-list password-reset-list';
    for (const request of this.passwordResetRequests) {
      const row = document.createElement('div');
      row.className = 'admin-row';
      const info = document.createElement('div');
      info.className = 'admin-row-info';
      const title = document.createElement('strong');
      title.textContent = request.display_name || request.username;
      const meta = document.createElement('span');
      meta.textContent = `${request.username} · 申请于 ${formatDateTime(request.requested_at)}`;
      info.append(title, meta);
      const status = document.createElement('span');
      status.className = 'admin-status';
      status.dataset.status = 'pending';
      status.textContent = '待审核';
      const actions = document.createElement('div');
      actions.className = 'admin-row-actions';
      const reject = actionButton('拒绝', () => void this.reviewPasswordReset(request, false));
      const approve = actionButton('批准重置', () => void this.reviewPasswordReset(request, true));
      approve.classList.add('primary');
      actions.append(reject, approve);
      row.append(info, status, actions);
      list.append(row);
    }
    if (!this.passwordResetRequests.length) {
      const empty = document.createElement('p');
      empty.className = 'admin-empty';
      empty.textContent = '暂无待审核的密码重置申请。';
      list.append(empty);
    }
    fragment.append(list);
    if (generation !== this.renderGeneration) return;
    this.body.replaceChildren(fragment);
  }

  private async reviewPasswordReset(request: PasswordResetRequest, approve: boolean): Promise<void> {
    const action = approve ? '批准' : '拒绝';
    if (!window.confirm(`${action} ${request.username} 的密码重置申请？`)) return;
    try {
      await reviewPasswordResetRequest(request.id, approve);
      this.showSuccess(approve
        ? `已批准 ${request.username} 的申请，新密码现已生效。`
        : `已拒绝 ${request.username} 的申请，原密码保持不变。`);
      this.passwordResetRequests = await listPasswordResetRequests();
      this.renderPasswordResetRequests();
    } catch (error) { this.showError(errorMessage(error)); }
  }

  private async renderWorkload(dateFrom?: string, dateTo?: string): Promise<void> {
    const generation = ++this.renderGeneration;
    const today = localDate(new Date());
    const monthStart = `${today.slice(0, 8)}01`;
    const from = dateFrom ?? monthStart;
    const to = dateTo ?? today;
    const fragment = document.createDocumentFragment();
    const filters = document.createElement('form');
    filters.className = 'admin-workload-filters';
    const fromField = dateField('起始日期', from);
    const toField = dateField('结束日期', to);
    const query = actionButton('查询', () => void this.renderWorkload(fromField.input.value, toField.input.value));
    query.type = 'submit';
    filters.append(fromField.label, toField.label, query);
    filters.addEventListener('submit', (event) => {
      event.preventDefault();
      void this.renderWorkload(fromField.input.value, toField.input.value);
    });
    fragment.append(filters);

    let rows: WorkloadRow[];
    try {
      rows = await workloadReport(from, to);
    } catch (error) {
      this.showError(errorMessage(error));
      return;
    }
    const actions = document.createElement('div');
    actions.className = 'admin-workload-summary';
    const summary = document.createElement('span');
    summary.textContent = `${from} 至 ${to} · ${rows.length} 名员工`;
    const exportButton = actionButton('导出 CSV', () => exportWorkloadCsv(rows, from, to));
    actions.append(summary, exportButton);
    fragment.append(actions);

    const wrap = document.createElement('div');
    wrap.className = 'admin-workload-table-wrap';
    const table = document.createElement('table');
    table.className = 'admin-workload-table';
    table.innerHTML = `<thead><tr><th>人员</th><th>角色</th><th>草稿</th><th>待审核</th><th>审核中</th><th>已签发状态</th><th>签发版本</th><th>完成审核</th><th>被审核修改</th><th>申请单</th></tr></thead>`;
    const body = document.createElement('tbody');
    for (const row of rows) {
      const tr = document.createElement('tr');
      for (const value of [
        row.display_name || row.username,
        row.role === 'radiologist' ? '医生' : '技师',
        row.draft_reports,
        row.submitted_reports,
        row.under_review_reports,
        row.signed_status_reports,
        row.signed_reports,
        row.reviews_completed,
        row.reviewer_modifications,
        row.exam_requests_created,
      ]) {
        const td = document.createElement('td'); td.textContent = String(value); tr.append(td);
      }
      body.append(tr);
    }
    if (!rows.length) {
      const empty = document.createElement('tr');
      const td = document.createElement('td'); td.colSpan = 10; td.className = 'admin-empty'; td.textContent = '当前机构没有医生或技师账号。';
      empty.append(td); body.append(empty);
    }
    table.append(body); wrap.append(table); fragment.append(wrap);
    if (generation !== this.renderGeneration) return;
    this.body.replaceChildren(fragment);
  }

  private async renderSettings(): Promise<void> {
    const generation = ++this.renderGeneration;
    const settings = await getInstitutionSettings();
    const fragment = document.createDocumentFragment();
    const panel = document.createElement('section');
    panel.className = 'admin-settings-panel';
    const heading = document.createElement('div');
    heading.className = 'admin-settings-copy';
    const title = document.createElement('strong');
    title.textContent = '报告审核闭环';
    const description = document.createElement('span');
    description.textContent = '开启后，医生提交的报告必须经具备审核权限的医生审核通过才能签发。关闭时保留直接签发，适合单医生或演示环境。';
    heading.append(title, description);

    const switchLabel = document.createElement('label');
    switchLabel.className = 'admin-switch';
    const toggle = document.createElement('input');
    toggle.type = 'checkbox';
    toggle.checked = settings.review_required;
    toggle.setAttribute('role', 'switch');
    toggle.setAttribute('aria-label', '启用报告审核闭环');
    const track = document.createElement('span');
    track.className = 'admin-switch-track';
    const state = document.createElement('span');
    state.className = 'admin-switch-state';
    state.textContent = toggle.checked ? '已开启' : '已关闭';
    toggle.addEventListener('change', () => void this.saveReviewRequired(toggle, state));
    switchLabel.append(toggle, track, state);
    panel.append(heading, switchLabel);
    fragment.append(panel);
    if (generation !== this.renderGeneration) return;
    this.body.replaceChildren(fragment);
  }

  private async saveReviewRequired(toggle: HTMLInputElement, state: HTMLElement): Promise<void> {
    const previous = !toggle.checked;
    toggle.disabled = true;
    this.clearError();
    try {
      const settings = await updateInstitutionSettings(toggle.checked);
      toggle.checked = settings.review_required;
      state.textContent = settings.review_required ? '已开启' : '已关闭';
      this.showSuccess(`报告审核闭环已${settings.review_required ? '开启' : '关闭'}，新流程即时生效。`);
    } catch (error) {
      toggle.checked = previous;
      state.textContent = previous ? '已开启' : '已关闭';
      this.showError(errorMessage(error));
    } finally {
      toggle.disabled = false;
    }
  }
}

function accountInput(labelText: string, placeholder: string, type: string) {
  const label = document.createElement('label');
  label.append(document.createTextNode(labelText));
  const input = document.createElement('input');
  input.type = type;
  input.placeholder = placeholder;
  input.required = labelText !== '姓名';
  label.append(input);
  return { label, input };
}

function roleSelect(selected: string): HTMLSelectElement {
  const select = document.createElement('select');
  const roles: Array<[string, string]> = [
    ['admin', '管理员'], ['radiologist', '放射科医师'], ['technician', '技师'], ['viewer', '只读'],
  ];
  for (const [value, label] of roles) {
    const option = document.createElement('option');
    option.value = value; option.textContent = label; option.selected = value === selected; select.append(option);
  }
  return select;
}

function actionButton(label: string, action: () => void): HTMLButtonElement {
  const button = document.createElement('button');
  button.type = 'button'; button.className = 'command-button'; button.textContent = label;
  button.addEventListener('click', action);
  return button;
}

function errorMessage(error: unknown): string {
  if (typeof error === 'string') return error;
  if (error instanceof Error) return error.message;
  return String(error);
}

function formatDateTime(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString('zh-CN', { hour12: false });
}

function dateField(labelText: string, value: string) {
  const label = document.createElement('label'); label.append(document.createTextNode(labelText));
  const input = document.createElement('input'); input.type = 'date'; input.value = value; input.required = true; label.append(input);
  return { label, input };
}

function localDate(date: Date): string {
  const offset = date.getTimezoneOffset() * 60_000;
  return new Date(date.getTime() - offset).toISOString().slice(0, 10);
}

function exportWorkloadCsv(rows: WorkloadRow[], from: string, to: string): void {
  const header = ['姓名', '账号', '角色', '草稿', '待审核', '审核中', '已签发状态', '签发版本', '完成审核', '被审核修改', '申请单'];
  const lines = [header, ...rows.map((row) => [
    row.display_name ?? '', row.username, row.role, row.draft_reports, row.submitted_reports,
    row.under_review_reports, row.signed_status_reports, row.signed_reports,
    row.reviews_completed, row.reviewer_modifications, row.exam_requests_created,
  ])].map((columns) => columns.map((value) => `"${String(value).replace(/"/g, '""')}"`).join(','));
  const blob = new Blob([`\uFEFF${lines.join('\r\n')}`], { type: 'text/csv;charset=utf-8' });
  const link = document.createElement('a'); link.href = URL.createObjectURL(blob); link.download = `工作量-${from}-${to}.csv`; link.click();
  URL.revokeObjectURL(link.href);
}

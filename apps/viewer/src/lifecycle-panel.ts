import {
  approvePurgeRequest,
  createLegalHold,
  createLifecyclePolicy,
  createPurgeRequest,
  deleteLifecyclePolicy,
  getLifecycleSummary,
  listLegalHolds,
  listLifecycleEvents,
  listLifecycleJobs,
  listLifecyclePolicies,
  listLifecycleStudies,
  listPurgeRequests,
  moveLifecycleStudy,
  previewLifecyclePolicy,
  rejectPurgeRequest,
  releaseLegalHold,
  restoreLifecycleStudy,
  runLifecyclePolicy,
  updateLifecyclePolicy,
} from './api';
import type {
  LegalHold,
  LifecycleEvent,
  LifecycleJob,
  LifecyclePolicy,
  LifecyclePolicyInput,
  LifecycleStudy,
  LifecycleSummary,
  PurgeRequest,
  StorageTier,
} from './types';

type PendingLifecycleAction =
  | { kind: 'hold'; study: LifecycleStudy }
  | { kind: 'purge'; study: LifecycleStudy }
  | { kind: 'approve-purge'; request: PurgeRequest };

export class LifecyclePanel {
  private readonly dialog = element<HTMLDialogElement>('lifecycle-dialog');
  private readonly actionDialog = element<HTMLDialogElement>('lifecycle-action-dialog');
  private summary: LifecycleSummary | null = null;
  private policies: LifecyclePolicy[] = [];
  private studies: LifecycleStudy[] = [];
  private holds: LegalHold[] = [];
  private purges: PurgeRequest[] = [];
  private events: LifecycleEvent[] = [];
  private jobs: LifecycleJob[] = [];
  private editingPolicyId: string | null = null;
  private busy = false;
  private timer: number | null = null;
  private noticeTimer: number | null = null;
  private pendingAction: PendingLifecycleAction | null = null;

  constructor(private readonly reportError: (message: string) => void) {
    element<HTMLButtonElement>('lifecycle-btn').addEventListener('click', () => void this.open());
    element<HTMLButtonElement>('lifecycle-close').addEventListener('click', () => this.dialog.close());
    element<HTMLButtonElement>('lifecycle-refresh').addEventListener('click', () => void this.refresh(true));
    element<HTMLButtonElement>('lifecycle-policy-add').addEventListener('click', () => this.showPolicyForm());
    element<HTMLButtonElement>('lifecycle-policy-cancel').addEventListener('click', () => this.hidePolicyForm());
    element<HTMLButtonElement>('lifecycle-action-close').addEventListener('click', () => this.closeActionDialog());
    element<HTMLButtonElement>('lifecycle-action-cancel').addEventListener('click', () => this.closeActionDialog());
    element<HTMLFormElement>('lifecycle-action-form').addEventListener('submit', (event) => {
      event.preventDefault();
      void this.submitAction();
    });
    element<HTMLFormElement>('lifecycle-policy-form').addEventListener('submit', (event) => {
      event.preventDefault();
      void this.savePolicy();
    });
    for (const tab of this.dialog.querySelectorAll<HTMLButtonElement>('[data-lifecycle-tab]')) {
      tab.addEventListener('click', () => this.selectTab(tab.dataset.lifecycleTab ?? 'storage'));
    }
    this.dialog.addEventListener('close', () => {
      this.stopPolling();
      this.notice('');
      this.closeActionDialog();
    });
    this.actionDialog.addEventListener('close', () => { this.pendingAction = null; });
  }

  setAvailable(available: boolean): void {
    element<HTMLButtonElement>('lifecycle-btn').hidden = !available;
    if (!available) {
      this.stopPolling();
      if (this.dialog.open) this.dialog.close();
    }
  }

  private async open(): Promise<void> {
    if (!this.dialog.open) this.dialog.showModal();
    this.notice('');
    this.startPolling();
    await this.refresh();
  }

  private async refresh(showNotice = false): Promise<void> {
    if (this.busy) return;
    this.setBusy(true);
    try {
      [this.summary, this.policies, this.studies, this.holds, this.purges, this.events, this.jobs] = await Promise.all([
        getLifecycleSummary(), listLifecyclePolicies(), listLifecycleStudies(), listLegalHolds(),
        listPurgeRequests(), listLifecycleEvents(), listLifecycleJobs(),
      ]);
      this.render();
      if (showNotice) this.notice('生命周期状态已刷新。');
    } catch (error) {
      this.fail(error);
    } finally {
      this.setBusy(false);
    }
  }

  private render(): void {
    this.renderSummary();
    this.renderStudies();
    this.renderPolicies();
    this.renderGovernance();
  }

  private renderSummary(): void {
    const value = this.summary;
    if (!value) return;
    metric('lifecycle-hot-value', value.hot_studies, value.hot_bytes);
    metric('lifecycle-cold-value', value.cold_studies, value.cold_bytes);
    metric('lifecycle-quarantine-value', value.quarantine_studies, value.quarantine_bytes);
    text('lifecycle-summary', `${value.active_legal_holds} 个 Legal Hold · ${value.pending_purge_requests} 个待处理清除`);
  }

  private renderStudies(): void {
    const list = element<HTMLElement>('lifecycle-study-list');
    list.replaceChildren();
    if (!this.studies.length) {
      list.append(empty('暂无 Study'));
      return;
    }
    const now = Date.now();
    const activeHolds = new Map(this.holds
      .filter((hold) => !hold.released_at && (!hold.expires_at || new Date(hold.expires_at).getTime() > now))
      .map((hold) => [hold.study_instance_uid, hold]));
    const activePurges = new Map<string, PurgeRequest>();
    for (const request of this.purges) {
      if (['pending', 'approved', 'paused_hold', 'executing'].includes(request.status)
        && !activePurges.has(request.study_instance_uid)) {
        activePurges.set(request.study_instance_uid, request);
      }
    }
    for (const study of this.studies) {
      const actions = document.createElement('div');
      actions.className = 'lifecycle-row-actions';
      if (study.storage_tier === 'hot') actions.append(button('转冷', () => this.move(study, 'cold')));
      if (study.storage_tier !== 'hot') actions.append(button('恢复', () => this.restore(study)));
      if (study.storage_tier !== 'quarantine' && !study.legal_hold) {
        actions.append(button('隔离', () => this.move(study, 'quarantine'), 'danger'));
      }
      const hold = activeHolds.get(study.study_instance_uid);
      actions.append(hold
        ? button('解除 Hold', () => this.releaseHold(hold), 'warning')
        : button('Legal Hold', () => this.hold(study)));
      if (study.storage_tier === 'quarantine' && !study.legal_hold) {
        const purge = activePurges.get(study.study_instance_uid);
        actions.append(purge
          ? button(purgeActionLabel(purge.status), () => undefined, 'danger', true)
          : button('申请清除', () => this.requestPurge(study), 'danger'));
      }
      const patient = primary(
        formatPersonName(study.patient_name) || '未提供姓名',
        `${study.patient_id || '未提供 Patient ID'} · ${study.modalities.join(' / ') || '--'} · ${study.study_date ?? '日期未知'}`,
      );
      patient.title = `StudyInstanceUID: ${study.study_instance_uid}`;
      list.append(dataRow(
        patient,
        primary(tierLabel(study.storage_tier), `${formatBytes(study.storage_bytes)} · ${study.last_accessed_at ? `访问 ${formatTime(study.last_accessed_at)}` : '未访问'}`),
        actions,
      ));
    }
  }

  private renderPolicies(): void {
    const list = element<HTMLElement>('lifecycle-policy-list');
    list.replaceChildren();
    if (!this.policies.length) {
      list.append(empty('暂无生命周期策略'));
      return;
    }
    for (const policy of this.policies) {
      const actions = document.createElement('div');
      actions.className = 'lifecycle-row-actions';
      actions.append(button('预演', () => this.preview(policy)));
      if (policy.enabled) actions.append(button('执行', () => this.run(policy), 'primary'));
      actions.append(button(policy.enabled ? '停用' : '启用', () => this.togglePolicy(policy)));
      actions.append(button('编辑', () => this.showPolicyForm(policy)));
      actions.append(button('删除', () => this.deletePolicy(policy), 'danger'));
      const matches = Number(policy.last_preview.matched_studies ?? 0);
      const preview = policy.last_preview_at
        ? `${matches} 个 Study · ${policy.preview_current ? '预演有效' : '需重新预演'}`
        : '尚未预演';
      list.append(dataRow(
        primary(policy.name, `${policy.modalities.join(' / ') || '全部模态'} · ${policy.target_tier === 'cold' ? '转冷' : '隔离'}`),
        primary(policy.enabled ? '已启用' : '已停用', preview),
        actions,
      ));
    }
  }

  private renderGovernance(): void {
    const purgeByJob = new Map(this.purges
      .filter((request) => request.job_id)
      .map((request) => [request.job_id as string, request]));
    const jobs = element<HTMLElement>('lifecycle-job-list');
    jobs.replaceChildren();
    if (!this.jobs.length) jobs.append(empty('暂无生命周期任务'));
    for (const job of this.jobs.slice(0, 20)) {
      const operation = job.payload.operation === 'purge' ? '物理清除' : '存储迁移';
      const purge = purgeByJob.get(job.id);
      const detail = job.status === 'paused' && purge?.grace_remaining_seconds != null
        ? `剩余宽限 ${formatDuration(purge.grace_remaining_seconds)} · ${formatTime(job.created_at)}`
        : `${job.progress_completed} / ${job.progress_total} · ${formatTime(job.created_at)}`;
      jobs.append(dataRow(
        primary(operation, String(job.payload.study_instance_uid ?? job.payload.policy_id ?? job.id)),
        primary(jobStatus(job.status), detail),
      ));
    }
    const purges = element<HTMLElement>('lifecycle-purge-list');
    purges.replaceChildren();
    if (!this.purges.length) purges.append(empty('暂无清除申请'));
    for (const request of this.purges) {
      const actions = document.createElement('div');
      actions.className = 'lifecycle-row-actions';
      if (request.status === 'pending') {
        actions.append(button('批准', () => this.approvePurge(request), 'danger'));
        actions.append(button('拒绝', () => this.rejectPurge(request)));
      }
      purges.append(dataRow(
        primary(request.study_instance_uid, request.reason),
        primary(
          purgeStatus(request.status),
          request.status === 'paused_hold' && request.grace_remaining_seconds != null
            ? `剩余宽限 ${formatDuration(request.grace_remaining_seconds)}`
            : request.grace_until ? `宽限至 ${formatTime(request.grace_until)}` : formatTime(request.requested_at),
        ),
        actions,
      ));
    }
    const events = element<HTMLElement>('lifecycle-event-list');
    events.replaceChildren();
    if (!this.events.length) events.append(empty('暂无生命周期审计记录'));
    for (const event of this.events) {
      events.append(dataRow(
        primary(event.study_instance_uid, eventLabel(event.action)),
        primary(formatTime(event.created_at), [event.from_tier, event.to_tier].filter(Boolean).map((tier) => tierLabel(tier as StorageTier)).join(' → ')),
      ));
    }
  }

  private selectTab(name: string): void {
    for (const tab of this.dialog.querySelectorAll<HTMLButtonElement>('[data-lifecycle-tab]')) {
      tab.classList.toggle('active', tab.dataset.lifecycleTab === name);
    }
    for (const panel of this.dialog.querySelectorAll<HTMLElement>('[data-lifecycle-panel]')) {
      panel.hidden = panel.dataset.lifecyclePanel !== name;
    }
  }

  private showPolicyForm(policy?: LifecyclePolicy): void {
    this.editingPolicyId = policy?.id ?? null;
    const form = element<HTMLFormElement>('lifecycle-policy-form');
    form.hidden = false;
    text('lifecycle-policy-form-title', policy ? '编辑策略' : '新建策略');
    input('lifecycle-policy-name').value = policy?.name ?? '';
    input('lifecycle-policy-modalities').value = policy?.modalities.join(', ') ?? '';
    input('lifecycle-policy-date').value = policy?.study_date_before ?? '';
    input('lifecycle-policy-access').value = policy?.last_accessed_before?.slice(0, 16) ?? '';
    input('lifecycle-policy-min-gb').value = policy?.minimum_study_bytes == null ? '' : String(policy.minimum_study_bytes / 1024 ** 3);
    input('lifecycle-policy-used').value = policy?.minimum_storage_used_percent?.toString() ?? '';
    input('lifecycle-policy-priority').value = policy?.priority.toString() ?? '100';
    element<HTMLSelectElement>('lifecycle-policy-target').value = policy?.target_tier ?? 'cold';
    element<HTMLTextAreaElement>('lifecycle-policy-tags').value = JSON.stringify(policy?.tag_matches ?? {}, null, 2);
    input('lifecycle-policy-name').focus();
  }

  private hidePolicyForm(): void {
    this.editingPolicyId = null;
    element<HTMLFormElement>('lifecycle-policy-form').hidden = true;
  }

  private async savePolicy(): Promise<void> {
    let tags: Record<string, unknown>;
    try {
      const value: unknown = JSON.parse(element<HTMLTextAreaElement>('lifecycle-policy-tags').value || '{}');
      if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Tag 条件必须是 JSON 对象');
      tags = value as Record<string, unknown>;
    } catch (error) {
      this.fail(error);
      return;
    }
    const modalities = input('lifecycle-policy-modalities').value.split(',').map((value) => value.trim().toUpperCase()).filter(Boolean);
    const gb = optionalNumber('lifecycle-policy-min-gb');
    const used = optionalNumber('lifecycle-policy-used');
    const accessed = input('lifecycle-policy-access').value;
    const value: LifecyclePolicyInput = {
      name: input('lifecycle-policy-name').value.trim(),
      priority: Number(input('lifecycle-policy-priority').value),
      enabled: false,
      target_tier: element<HTMLSelectElement>('lifecycle-policy-target').value as 'cold' | 'quarantine',
      modalities,
      study_date_before: input('lifecycle-policy-date').value || undefined,
      last_accessed_before: accessed ? new Date(accessed).toISOString() : undefined,
      tag_matches: tags,
      minimum_study_bytes: gb == null ? undefined : Math.round(gb * 1024 ** 3),
      minimum_storage_used_percent: used ?? undefined,
    };
    await this.perform(async () => {
      if (this.editingPolicyId) await updateLifecyclePolicy(this.editingPolicyId, value);
      else await createLifecyclePolicy(value);
      this.hidePolicyForm();
      await this.reload();
      this.notice('策略已保存，请先预演再启用。');
    });
  }

  private async preview(policy: LifecyclePolicy): Promise<void> {
    await this.perform(async () => {
      const result = await previewLifecyclePolicy(policy.id);
      await this.reload();
      this.notice(`预演命中 ${Number(result.matched_studies ?? 0)} 个 Study，预计 ${formatBytes(Number(result.matched_bytes ?? 0))}。`);
    });
  }

  private async togglePolicy(policy: LifecyclePolicy): Promise<void> {
    await this.perform(async () => {
      await updateLifecyclePolicy(policy.id, policyInput(policy, !policy.enabled));
      await this.reload();
      this.notice(`策略“${policy.name}”已${policy.enabled ? '停用' : '启用'}。`);
    });
  }

  private async run(policy: LifecyclePolicy): Promise<void> {
    if (!window.confirm(`执行策略“${policy.name}”？`)) return;
    await this.perform(async () => {
      await runLifecyclePolicy(policy.id);
      await this.reload();
      this.notice('生命周期任务已进入后台队列。');
    });
  }

  private async deletePolicy(policy: LifecyclePolicy): Promise<void> {
    if (!window.confirm(`删除策略“${policy.name}”？`)) return;
    await this.perform(async () => {
      await deleteLifecyclePolicy(policy.id);
      await this.reload();
      this.notice(`策略“${policy.name}”已删除。`);
    });
  }

  private async move(study: LifecycleStudy, tier: 'cold' | 'quarantine'): Promise<void> {
    if (tier === 'quarantine' && !window.confirm(`将 Study ${study.study_instance_uid} 移入隔离区？`)) return;
    await this.perform(async () => {
      await moveLifecycleStudy(study.study_instance_uid, tier);
      await this.reload();
      this.notice(`${tier === 'cold' ? '转冷' : '隔离'}任务已提交，请在“审批审计”中查看进度。`);
    });
  }

  private async restore(study: LifecycleStudy): Promise<void> {
    await this.perform(async () => {
      await restoreLifecycleStudy(study.study_instance_uid);
      await this.reload();
      this.notice('恢复任务已提交，请在“审批审计”中查看进度。');
    });
  }

  private hold(study: LifecycleStudy): void {
    this.openActionDialog({ kind: 'hold', study });
  }

  private async releaseHold(hold: LegalHold): Promise<void> {
    if (!window.confirm(`解除 Study ${hold.study_instance_uid} 的 Legal Hold？`)) return;
    await this.perform(async () => {
      await releaseLegalHold(hold.id);
      await this.reload();
      this.notice('Legal Hold 已解除。');
    });
  }

  private requestPurge(study: LifecycleStudy): void {
    this.openActionDialog({ kind: 'purge', study });
  }

  private approvePurge(request: PurgeRequest): void {
    this.openActionDialog({ kind: 'approve-purge', request });
  }

  private async rejectPurge(request: PurgeRequest): Promise<void> {
    if (!window.confirm(`拒绝清除 Study ${request.study_instance_uid}？`)) return;
    await this.perform(async () => {
      await rejectPurgeRequest(request.id);
      await this.reload();
      this.notice('清除申请已拒绝。');
    });
  }

  private openActionDialog(action: PendingLifecycleAction): void {
    this.pendingAction = action;
    const reasonField = element<HTMLElement>('lifecycle-action-reason-field');
    const reason = element<HTMLTextAreaElement>('lifecycle-action-reason');
    const hoursField = element<HTMLElement>('lifecycle-action-hours-field');
    const hours = element<HTMLInputElement>('lifecycle-action-hours');
    const isApproval = action.kind === 'approve-purge';
    reasonField.hidden = isApproval;
    reason.disabled = isApproval;
    reason.value = '';
    hoursField.hidden = !isApproval;
    hours.disabled = !isApproval;
    hours.value = '168';
    text('lifecycle-action-title', action.kind === 'hold' ? '设置 Legal Hold' : action.kind === 'purge' ? '申请清除' : '批准清除');
    text('lifecycle-action-target', action.kind === 'approve-purge'
      ? action.request.study_instance_uid
      : `${formatPersonName(action.study.patient_name) || '未提供姓名'} · ${action.study.patient_id || '未提供 Patient ID'}`);
    text('lifecycle-action-reason-label', action.kind === 'hold' ? 'Hold 原因' : '清除原因');
    text('lifecycle-action-submit-label', action.kind === 'hold' ? '设置' : action.kind === 'purge' ? '提交申请' : '批准');
    element<HTMLElement>('lifecycle-action-error').hidden = true;
    if (!this.actionDialog.open) this.actionDialog.showModal();
    window.requestAnimationFrame(() => { (isApproval ? hours : reason).focus(); });
  }

  private closeActionDialog(): void {
    if (this.actionDialog.open) this.actionDialog.close();
    this.pendingAction = null;
  }

  private async submitAction(): Promise<void> {
    const action = this.pendingAction;
    if (!action) return;
    const reason = element<HTMLTextAreaElement>('lifecycle-action-reason').value.trim();
    const hours = Number(element<HTMLInputElement>('lifecycle-action-hours').value);
    if (action.kind !== 'approve-purge' && reason.length < 2) {
      this.showActionError('原因至少需要 2 个字符。');
      return;
    }
    if (action.kind === 'approve-purge' && (!Number.isInteger(hours) || hours < 0 || hours > 8760)) {
      this.showActionError('宽限期必须是 0 至 8760 之间的整数。');
      return;
    }
    this.closeActionDialog();
    await this.perform(async () => {
      if (action.kind === 'hold') {
        await createLegalHold(action.study.study_instance_uid, reason);
        await this.reload();
        this.notice('Legal Hold 已设置。');
      } else if (action.kind === 'purge') {
        await createPurgeRequest(action.study.study_instance_uid, reason);
        await this.reload();
        this.notice('清除申请已提交，等待审批。');
      } else {
        await approvePurgeRequest(action.request.id, hours);
        await this.reload();
        this.notice(hours === 0 ? '清除申请已批准，已进入执行流程。' : `清除申请已批准，将在 ${hours} 小时宽限期后执行。`);
      }
    });
  }

  private showActionError(value: string): void {
    const node = element<HTMLElement>('lifecycle-action-error');
    node.textContent = value;
    node.hidden = false;
  }

  private async perform(operation: () => Promise<void>): Promise<void> {
    if (this.busy) return;
    this.setBusy(true);
    try { await operation(); } catch (error) { this.fail(error); } finally { this.setBusy(false); }
  }

  private async reload(): Promise<void> {
    [this.summary, this.policies, this.studies, this.holds, this.purges, this.events, this.jobs] = await Promise.all([
      getLifecycleSummary(), listLifecyclePolicies(), listLifecycleStudies(), listLegalHolds(),
      listPurgeRequests(), listLifecycleEvents(), listLifecycleJobs(),
    ]);
    this.render();
  }

  private setBusy(value: boolean): void {
    this.busy = value;
    for (const button of this.dialog.querySelectorAll<HTMLButtonElement>('button')) {
      if (button.id !== 'lifecycle-close') button.disabled = value;
    }
  }

  private fail(error: unknown): void {
    const value = error instanceof Error ? error.message : String(error);
    this.notice(value, true);
    this.reportError(value);
  }

  private notice(value: string, error = false): void {
    if (this.noticeTimer !== null) {
      window.clearTimeout(this.noticeTimer);
      this.noticeTimer = null;
    }
    const node = element<HTMLElement>('lifecycle-notice');
    node.textContent = value;
    node.hidden = !value;
    node.dataset.kind = error ? 'error' : 'info';
    if (value && !error) {
      this.noticeTimer = window.setTimeout(() => {
        node.hidden = true;
        node.textContent = '';
        this.noticeTimer = null;
      }, 6_000);
    }
  }

  private startPolling(): void {
    if (this.timer !== null) return;
    this.timer = window.setInterval(() => { if (this.dialog.open) void this.refresh(); }, 5_000);
  }

  private stopPolling(): void {
    if (this.timer === null) return;
    window.clearInterval(this.timer);
    this.timer = null;
  }
}

function policyInput(policy: LifecyclePolicy, enabled: boolean): LifecyclePolicyInput {
  return {
    name: policy.name, priority: policy.priority, enabled, target_tier: policy.target_tier,
    modalities: policy.modalities, study_date_before: policy.study_date_before ?? undefined,
    last_accessed_before: policy.last_accessed_before ?? undefined, tag_matches: policy.tag_matches,
    minimum_study_bytes: policy.minimum_study_bytes ?? undefined,
    minimum_storage_used_percent: policy.minimum_storage_used_percent ?? undefined,
  };
}

function dataRow(...children: Node[]): HTMLElement {
  const row = document.createElement('div');
  row.className = 'lifecycle-row';
  row.append(...children);
  return row;
}

function primary(main: string, detail: string): HTMLElement {
  const node = document.createElement('div');
  node.className = 'lifecycle-row-main';
  const strong = document.createElement('strong');
  strong.textContent = main;
  const small = document.createElement('small');
  small.textContent = detail;
  node.append(strong, small);
  return node;
}

function button(label: string, action: () => void, kind = '', disabled = false): HTMLButtonElement {
  const node = document.createElement('button');
  node.type = 'button';
  node.className = 'lifecycle-action';
  if (kind) node.dataset.kind = kind;
  node.textContent = label;
  node.disabled = disabled;
  node.addEventListener('click', action);
  return node;
}

function empty(value: string): HTMLElement {
  const node = document.createElement('div');
  node.className = 'empty-worklist-message';
  node.textContent = value;
  return node;
}

function metric(id: string, studies: number, bytes: number): void {
  text(id, `${studies} 个 · ${formatBytes(bytes)}`);
}

function text(id: string, value: string): void { element(id).textContent = value; }
function input(id: string): HTMLInputElement { return element<HTMLInputElement>(id); }
function optionalNumber(id: string): number | null {
  const value = input(id).value.trim();
  return value ? Number(value) : null;
}
function element<T extends HTMLElement = HTMLElement>(id: string): T {
  const value = document.getElementById(id);
  if (!value) throw new Error(`缺少界面元素 #${id}`);
  return value as T;
}
function tierLabel(tier: StorageTier): string {
  return ({ hot: '热存储', cold: '冷存储', quarantine: '隔离区' })[tier];
}
function formatPersonName(value: string | null): string {
  return value?.split('=')[0].split('^').filter(Boolean).join(' ') ?? '';
}
function purgeStatus(status: PurgeRequest['status']): string {
  return ({ pending: '待审批', approved: '宽限期', paused_hold: '因 Legal Hold 暂停', executing: '清除中', completed: '已清除', rejected: '已拒绝', cancelled: '已取消', failed: '失败' })[status];
}
function purgeActionLabel(status: PurgeRequest['status']): string {
  return ({ pending: '已申请', approved: '已批准', paused_hold: 'Hold 暂停', executing: '清除中' } as Partial<Record<PurgeRequest['status'], string>>)[status]
    ?? '申请清除';
}
function eventLabel(action: string): string {
  return ({ move_to_cold: '转入冷存储', restore_to_hot: '恢复到热存储', quarantine: '移入隔离区',
    purge_requested: '申请清除', purge_approved: '批准清除', purge_paused_hold: '因 Legal Hold 暂停清除',
    purge_resumed_hold: '解除 Hold，恢复宽限期', purge_rejected: '拒绝清除', purged: '物理清除',
    legal_hold_created: '设置 Legal Hold', legal_hold_released: '解除 Legal Hold' } as Record<string, string>)[action] ?? action;
}
function jobStatus(status: LifecycleJob['status']): string {
  return ({ queued: '等待中', running: '执行中', paused: '因 Legal Hold 暂停', succeeded: '已完成', failed: '失败', cancelled: '已取消' })[status];
}
function formatDuration(totalSeconds: number): string {
  const seconds = Math.max(0, Math.round(totalSeconds));
  const days = Math.floor(seconds / 86_400);
  const hours = Math.floor((seconds % 86_400) / 3_600);
  const minutes = Math.floor((seconds % 3_600) / 60);
  if (days) return `${days} 天 ${hours} 小时`;
  if (hours) return `${hours} 小时 ${minutes} 分钟`;
  if (minutes) return `${minutes} 分钟`;
  return `${seconds} 秒`;
}
function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let size = value / 1024;
  let unit = units[0];
  for (let index = 1; index < units.length && size >= 1024; index += 1) { size /= 1024; unit = units[index]; }
  return `${size.toFixed(size >= 10 ? 1 : 2)} ${unit}`;
}
function formatTime(value: string): string { return new Date(value).toLocaleString('zh-CN', { hour12: false }); }

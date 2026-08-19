import {
  bindExamRequest,
  createExamRequest,
  createExamRequestForStudy,
  listExamRequests,
  listExamRequestStudyCandidates,
  updateExamRequest,
} from './api';
import type {
  ExamRequest,
  ExamRequestInput,
  ExamRequestStudyCandidate,
  ExistingStudyExamRequestInput,
  QueueStudyRow,
} from './types';

const required = <T extends HTMLElement>(id: string): T => {
  const found = document.getElementById(id);
  if (!found) throw new Error(`缺少申请单页面元素: ${id}`);
  return found as T;
};

const STATUS_LABEL: Record<string, string> = {
  pending: '待执行', executed: '已执行', completed: '已完成',
};

export interface ExamRequestPageOptions {
  canReturnToViewer: () => boolean;
  canCreateForStudy: () => boolean;
  beforeOpen?: () => void;
  onClose?: () => void;
  onStudyRequestCreated?: () => void;
}

export class ExamRequestPage {
  private readonly root = required<HTMLElement>('exam-request-page');
  private readonly workspace = required<HTMLElement>('workspace');
  private readonly queuePage = required<HTMLElement>('queue-page');
  private readonly navigator = document.getElementById('series-navigator');
  private readonly body = required<HTMLElement>('exam-request-body');
  private readonly status = required<HTMLElement>('exam-request-page-status');
  private readonly count = required<HTMLElement>('exam-request-count');
  private readonly filter = required<HTMLSelectElement>('exam-request-status-filter');
  private readonly editor = required<HTMLDialogElement>('exam-request-editor-dialog');
  private readonly bindDialog = required<HTMLDialogElement>('exam-request-bind-dialog');
  private editing: ExamRequest | null = null;
  private binding: ExamRequest | null = null;
  private existingStudy: QueueStudyRow | null = null;
  private loading = false;
  private opened = false;

  constructor(private readonly options: ExamRequestPageOptions) {
    required<HTMLButtonElement>('exam-request-btn').addEventListener('click', () => this.open());
    required<HTMLButtonElement>('exam-request-back').addEventListener('click', () => this.close());
    required<HTMLButtonElement>('exam-request-refresh').addEventListener('click', () => void this.load());
    required<HTMLButtonElement>('exam-request-new').addEventListener('click', () => this.openEditor());
    required<HTMLFormElement>('exam-request-filters').addEventListener('submit', (event) => {
      event.preventDefault(); void this.load();
    });
    required<HTMLButtonElement>('exam-request-editor-close').addEventListener('click', () => this.editor.close());
    required<HTMLButtonElement>('exam-request-editor-cancel').addEventListener('click', () => this.editor.close());
    required<HTMLFormElement>('exam-request-editor-form').addEventListener('submit', (event) => {
      event.preventDefault(); void this.save();
    });
    required<HTMLButtonElement>('exam-request-bind-close').addEventListener('click', () => this.bindDialog.close());
    required<HTMLFormElement>('exam-request-bind-search').addEventListener('submit', (event) => {
      event.preventDefault(); void this.loadCandidates();
    });
  }

  open(): void {
    this.options.beforeOpen?.();
    this.opened = true;
    this.queuePage.hidden = true;
    this.workspace.hidden = true;
    this.root.hidden = false;
    if (this.navigator) this.navigator.hidden = true;
    const back = required<HTMLButtonElement>('exam-request-back');
    back.hidden = false;
    back.title = this.options.canReturnToViewer() ? '返回阅片' : '返回患者队列';
    back.setAttribute('aria-label', back.title);
    void this.load();
  }

  openForStudy(study: QueueStudyRow): void {
    if (!this.options.canCreateForStudy()) return;
    this.open();
    this.openEditor(null, study);
  }

  close(): void {
    this.opened = false;
    this.root.hidden = true;
    this.workspace.hidden = false;
    if (this.navigator) this.navigator.hidden = false;
    this.options.onClose?.();
    required<HTMLButtonElement>('exam-request-btn').focus();
  }

  isOpen(): boolean { return this.opened; }

  private async load(): Promise<void> {
    if (this.loading) return;
    this.loading = true;
    this.status.textContent = '正在加载申请单...';
    try {
      const rows = await listExamRequests(this.filter.value);
      this.render(rows);
      this.status.textContent = rows.length ? '' : '没有匹配的申请单';
    } catch (error) {
      this.body.replaceChildren();
      this.status.textContent = errorMessage(error);
    } finally {
      this.loading = false;
    }
  }

  private render(rows: ExamRequest[]): void {
    this.body.replaceChildren(...rows.map((request) => this.renderRow(request)));
    this.count.textContent = `${rows.length} 张申请单`;
  }

  private renderRow(request: ExamRequest): HTMLTableRowElement {
    const row = document.createElement('tr');
    row.dataset.requestId = request.id;
    row.append(
      cell(request.patient_name, request.patient_id),
      cell(`${request.modality} · ${request.body_part}`, request.request_type),
      cell(request.clinical_indication),
      cell(request.scheduled_at ? formatDateTime(request.scheduled_at) : '未预约', `申请 ${formatDateTime(request.requested_at)}`),
    );
    const status = document.createElement('td');
    const badge = document.createElement('span');
    badge.className = 'exam-request-status';
    badge.dataset.status = request.status;
    badge.textContent = STATUS_LABEL[request.status] ?? request.status;
    status.append(badge);
    row.append(status, cell(request.study_uid ? `${request.study_date ?? '--'} · ${request.study_description ?? '检查'}` : '尚未绑定'));
    const actions = document.createElement('td');
    actions.className = 'queue-action-cell';
    if (request.status === 'pending') {
      actions.append(
        action('编辑', () => this.openEditor(request)),
        action('绑定检查', () => void this.openBind(request), true),
      );
    } else {
      const done = document.createElement('span');
      done.className = 'exam-request-action-note';
      done.textContent = request.status === 'completed' ? '已闭环' : '等待报告';
      actions.append(done);
    }
    row.append(actions);
    return row;
  }

  private openEditor(request: ExamRequest | null = null, existingStudy: QueueStudyRow | null = null): void {
    this.editing = request;
    this.existingStudy = existingStudy;
    const fromStudy = existingStudy !== null;
    required('exam-request-editor-title').textContent = fromStudy
      ? '为已入库检查开具申请单'
      : request ? '编辑申请单' : '新建申请单';
    required('exam-request-editor-subtitle').textContent = fromStudy
      ? '患者信息与关联检查由服务器校验'
      : '患者信息与检查请求';
    required<HTMLButtonElement>('exam-request-editor-submit').textContent = fromStudy
      ? '保存并关联检查'
      : request ? '保存修改' : '保存申请单';

    value('exam-request-patient-id', existingStudy?.patient_id ?? request?.patient_id ?? '');
    value('exam-request-patient-name', existingStudy?.patient_name ?? existingStudy?.patient_id ?? request?.patient_name ?? '');
    value('exam-request-birth-date', existingStudy?.patient_birth_date ?? request?.patient_birth_date ?? '');
    value('exam-request-sex', existingStudy?.patient_sex ?? request?.patient_sex ?? '');
    value('exam-request-modality', existingStudy?.modalities[0]?.toUpperCase() ?? request?.modality ?? 'CT');
    value('exam-request-body-part', existingStudy?.body_parts.join(' / ') ?? request?.body_part ?? '');
    value('exam-request-type', request?.request_type ?? '平扫');
    value('exam-request-scheduled-at', toLocalInput(request?.scheduled_at));
    value('exam-request-indication', request?.clinical_indication ?? '');
    this.setPatientInputsReadOnly(fromStudy);
    const context = required<HTMLElement>('exam-request-existing-study-context');
    context.hidden = !fromStudy;
    context.textContent = fromStudy
      ? `关联检查：${existingStudy.study_uid} · ${existingStudy.study_date ?? '日期未记录'} · ${existingStudy.description ?? '检查描述未记录'}`
      : '';
    required('exam-request-editor-error').hidden = true;
    this.editor.showModal();
    required<HTMLInputElement>(fromStudy ? 'exam-request-body-part' : 'exam-request-patient-id').focus();
  }

  private setPatientInputsReadOnly(readOnly: boolean): void {
    required<HTMLInputElement>('exam-request-patient-id').readOnly = readOnly;
    required<HTMLInputElement>('exam-request-patient-name').readOnly = readOnly;
    required<HTMLInputElement>('exam-request-birth-date').readOnly = readOnly;
    required<HTMLSelectElement>('exam-request-sex').disabled = readOnly;
  }

  private input(): ExamRequestInput {
    const scheduled = inputValue('exam-request-scheduled-at');
    return {
      patientId: inputValue('exam-request-patient-id').trim(),
      patientName: inputValue('exam-request-patient-name').trim(),
      patientBirthDate: inputValue('exam-request-birth-date') || null,
      patientSex: inputValue('exam-request-sex') || null,
      modality: inputValue('exam-request-modality'),
      bodyPart: inputValue('exam-request-body-part').trim(),
      requestType: inputValue('exam-request-type'),
      clinicalIndication: inputValue('exam-request-indication').trim(),
      scheduledAt: scheduled ? new Date(scheduled).toISOString() : null,
    };
  }

  private existingStudyInput(): ExistingStudyExamRequestInput {
    const input = this.input();
    return {
      modality: input.modality,
      bodyPart: input.bodyPart,
      requestType: input.requestType,
      clinicalIndication: input.clinicalIndication,
      scheduledAt: input.scheduledAt,
    };
  }

  private async save(): Promise<void> {
    const error = required('exam-request-editor-error');
    error.hidden = true;
    try {
      const input = this.input();
      const existingStudy = this.existingStudy;
      if (existingStudy) {
        await createExamRequestForStudy(existingStudy.study_uid, this.existingStudyInput());
      } else if (this.editing) {
        await updateExamRequest(this.editing.id, this.editing.revision, input);
      } else {
        await createExamRequest(input);
      }
      this.editor.close();
      await this.load();
      if (existingStudy) this.options.onStudyRequestCreated?.();
    } catch (reason) {
      error.textContent = errorMessage(reason); error.hidden = false;
    }
  }

  private async openBind(request: ExamRequest): Promise<void> {
    this.binding = request;
    required('exam-request-bind-subtitle').textContent = `${request.patient_name} · ${request.patient_id} · ${request.modality} ${request.body_part}`;
    value('exam-request-candidate-query', request.patient_id);
    required('exam-request-bind-error').hidden = true;
    this.bindDialog.showModal();
    await this.loadCandidates();
  }

  private async loadCandidates(): Promise<void> {
    const container = required('exam-request-candidates');
    const error = required('exam-request-bind-error');
    error.hidden = true;
    container.textContent = '正在查找已入库检查...';
    try {
      const candidates = await listExamRequestStudyCandidates(inputValue('exam-request-candidate-query'));
      container.replaceChildren(...candidates.map((candidate) => this.candidate(candidate)));
      if (!candidates.length) container.textContent = '没有可绑定的检查，请调整搜索条件。';
    } catch (reason) {
      container.replaceChildren(); error.textContent = errorMessage(reason); error.hidden = false;
    }
  }

  private candidate(candidate: ExamRequestStudyCandidate): HTMLElement {
    const row = document.createElement('article');
    row.className = 'exam-request-candidate';
    const info = document.createElement('div');
    const title = document.createElement('strong');
    title.textContent = candidate.patient_name || candidate.patient_id;
    const meta = document.createElement('span');
    meta.textContent = [candidate.patient_id, candidate.study_date, candidate.modalities.join('/'), candidate.description].filter(Boolean).join(' · ');
    info.append(title, meta);
    row.append(info, action('确认绑定', () => void this.bind(candidate), true));
    return row;
  }

  private async bind(candidate: ExamRequestStudyCandidate): Promise<void> {
    if (!this.binding) return;
    if (!window.confirm(`确认将申请单绑定到 ${candidate.patient_name || candidate.patient_id} 的该次检查？`)) return;
    const error = required('exam-request-bind-error');
    try {
      await bindExamRequest(this.binding.id, candidate.study_uid, this.binding.revision);
      this.bindDialog.close();
      await this.load();
    } catch (reason) {
      error.textContent = errorMessage(reason); error.hidden = false;
    }
  }
}

function cell(primary: string, secondary?: string): HTMLTableCellElement {
  const td = document.createElement('td');
  const strong = document.createElement('strong'); strong.textContent = primary;
  td.append(strong);
  if (secondary) { const small = document.createElement('small'); small.textContent = secondary; td.append(small); }
  return td;
}

function action(label: string, handler: () => void, primary = false): HTMLButtonElement {
  const button = document.createElement('button');
  button.type = 'button'; button.className = `queue-edit-button${primary ? ' primary' : ''}`; button.textContent = label;
  button.addEventListener('click', handler); return button;
}

function value(id: string, content: string): void { required<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>(id).value = content; }
function inputValue(id: string): string { return required<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>(id).value; }
function formatDateTime(value: string): string { const date = new Date(value); return Number.isNaN(date.getTime()) ? value : date.toLocaleString('zh-CN', { hour12: false }); }
function toLocalInput(value?: string | null): string { if (!value) return ''; const date = new Date(value); const offset = date.getTimezoneOffset() * 60_000; return new Date(date.getTime() - offset).toISOString().slice(0, 16); }
function errorMessage(error: unknown): string { return typeof error === 'string' ? error : error instanceof Error ? error.message : String(error); }

import {
  beginReportAmendment,
  claimWorkItem,
  createReport,
  listReportTemplates,
  listReports,
  listReportVersions,
  releaseWorkItem,
  signReport,
  updateReportDraft,
  workItemForSeries,
} from './api';
import { htmlToText, plainToHtml, sanitizeReportHtml } from './rich-text';
import type {
  ClinicalWorkItem,
  DiagnosticReport,
  ReportTemplate,
  ReportVersion,
} from './types';

export interface ReportWorkspaceContext {
  studyUid: string;
  seriesUid: string;
  modality: string | null;
  patientName: string;
  patientId: string | null;
  patientSex: string | null;
  patientBirthDate: string | null;
  studyDate: string | null;
  studyDescription: string | null;
  seriesDescription: string | null;
  institutionName: string;
}

export interface ReportWorkspaceUser {
  id: number | null;
  role: string | null;
  displayName: string | null;
  username: string | null;
}

function el<T extends HTMLElement = HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (!found) throw new Error(`缺少元素 #${id}`);
  return found as T;
}

const STATUS_LABEL: Record<string, { text: string; status: string }> = {
  draft: { text: '未锁定 · 编辑中', status: 'draft' },
  signed: { text: '已锁定 · 已签发', status: 'signed' },
  amending: { text: '修订中', status: 'amending' },
};

/**
 * 全屏报告工作台：三栏 + 顶底布局，富文本所见/意见编辑、模板片段插入、
 * 领取/签发/修订与版本历史。复用 B2-1 的 API 与乐观锁语义。
 */
export class ReportWorkspace {
  private readonly workspace = el<HTMLElement>('report-workspace');
  private readonly status = el<HTMLElement>('report-status');
  private readonly versionsList = el<HTMLElement>('report-versions-list');
  private readonly findingsEditor = el<HTMLDivElement>('report-findings-editor');
  private readonly impressionEditor = el<HTMLDivElement>('report-impression-editor');
  private readonly positive = el<HTMLInputElement>('report-positive');
  private readonly templateTree = el<HTMLElement>('report-template-tree');

  private context: ReportWorkspaceContext | null = null;
  private reports: DiagnosticReport[] = [];
  private report: DiagnosticReport | null = null;
  private versions: ReportVersion[] = [];
  private templates: ReportTemplate[] = [];
  private workItem: ClinicalWorkItem | null = null;
  private busy = false;
  private lastFocusedEditor: 'findings' | 'impression' | null = null;

  constructor(
    private readonly reportError: (message: string) => void,
    private readonly getContext: () => ReportWorkspaceContext | null,
    private readonly getUser: () => ReportWorkspaceUser,
  ) {
    el<HTMLButtonElement>('report-back').addEventListener('click', () => this.onExit());
    el<HTMLButtonElement>('report-to-image').addEventListener('click', () => this.onExit());
    el<HTMLButtonElement>('report-save').addEventListener('click', () => void this.save());
    el<HTMLButtonElement>('report-sign').addEventListener('click', () => void this.sign());
    el<HTMLButtonElement>('report-amend').addEventListener('click', () => void this.amend());
    el<HTMLButtonElement>('report-claim-btn').addEventListener('click', () => void this.claim());
    el<HTMLButtonElement>('report-release-btn').addEventListener('click', () => void this.release());
    el<HTMLButtonElement>('report-create-btn').addEventListener('click', () => void this.create());
    this.positive.addEventListener('change', () => this.markDirty());

    for (const editor of [this.findingsEditor, this.impressionEditor]) {
      editor.addEventListener('focus', () => {
        this.lastFocusedEditor = editor.dataset.editor as 'findings' | 'impression';
      });
      editor.addEventListener('input', () => this.markDirty());
    }
    for (const button of document.querySelectorAll<HTMLButtonElement>('[data-rich]')) {
      button.addEventListener('click', () => {
        const command = button.dataset.rich as string;
        document.execCommand(command, false);
        this.markDirty();
        this.lastFocusedEditor ??= 'findings';
      });
    }
    document.addEventListener('keydown', (event) => this.onKeydown(event));
  }

  /** 退出工作台（由 app 切回阅片）。 */
  onExit: () => void = () => {};

  private markDirty(): void {
    el<HTMLButtonElement>('report-save').disabled = false;
  }

  private onKeydown(event: KeyboardEvent): void {
    if (this.workspace.hidden) return;
    const target = event.target as HTMLElement | null;
    if (target && target.closest('.report-editor')) {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's') {
        event.preventDefault();
        void this.save();
      } else if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') {
        event.preventDefault();
        void this.sign();
      }
    }
  }

  async open(): Promise<void> {
    const context = this.getContext();
    if (!context) return;
    this.context = context;
    await this.refresh();
  }

  close(): void {
    this.workspace.hidden = true;
  }

  get visible(): boolean {
    return !this.workspace.hidden;
  }

  private writable(): boolean {
    return this.getUser().role === 'radiologist';
  }

  private claimedByMe(): boolean {
    const item = this.workItem;
    const user = this.getUser();
    return !!item
      && (item.status === 'claimed' || item.status === 'reporting')
      && item.assignee_id === user.id;
  }

  async refresh(): Promise<void> {
    if (!this.context) return;
    if (this.busy) return;
    this.busy = true;
    try {
      const { studyUid, seriesUid, modality } = this.context;
      const [reports, templates, item] = await Promise.all([
        listReports(studyUid),
        listReportTemplates(modality ?? undefined),
        workItemForSeries(seriesUid).catch(() => null),
      ]);
      this.reports = reports;
      this.templates = templates;
      this.workItem = item;
      this.report = reports[0] ?? null;
      this.versions = [];
      if (this.report?.status === 'signed') {
        this.versions = await listReportVersions(this.report.id);
      }
      this.render();
    } catch (error) {
      this.reportError(errorMessage(error));
    } finally {
      this.busy = false;
    }
  }

  private render(): void {
    this.renderHeader();
    this.renderTemplates();
    const report = this.report;
    if (!report) {
      this.renderEmpty();
      return;
    }
    this.renderDocument(report);
    this.renderSignatures(report);
    this.renderVersions();
    this.renderStatus(report.status);
    this.renderWorkItemRow();
  }

  private renderHeader(): void {
    const context = this.context;
    if (!context) return;
    el('report-patient-title').textContent = context.patientName;
    el('report-head-patient').textContent = context.patientName;
    el('report-head-hospital').textContent = context.institutionName;
    el('rp-patient-id').textContent = context.patientId || '--';
    el('rp-patient-name').textContent = context.patientName;
    el('rp-patient-sex').textContent = context.patientSex || '--';
    el('rp-patient-age').textContent = ageFromBirthDate(context.patientBirthDate);
    el('rp-modality').textContent = context.modality || '--';
    el('rp-study-date').textContent = context.studyDate || '--';
    el('rp-series-desc').textContent = context.seriesDescription || context.modality || '--';
    el('rp-study-desc').textContent = context.studyDescription || '--';
  }

  private renderStatus(status: string): void {
    const mapping = STATUS_LABEL[status] ?? STATUS_LABEL.draft;
    this.status.textContent = mapping.text;
    this.status.dataset.status = mapping.status;
  }

  private renderWorkItemRow(): void {
    const row = el<HTMLElement>('report-workitem-row');
    const text = el<HTMLElement>('report-workitem-text');
    const claim = el<HTMLButtonElement>('report-claim-btn');
    const release = el<HTMLButtonElement>('report-release-btn');
    const create = el<HTMLButtonElement>('report-create-btn');
    const item = this.workItem;
    const user = this.getUser();
    const report = this.report;

    claim.hidden = true;
    release.hidden = true;
    create.hidden = true;
    row.hidden = false;

    if (!this.writable()) {
      row.hidden = true;
      return;
    }
    if (!item) {
      text.textContent = '该序列没有待诊任务，无法创建报告';
      return;
    }
    if (item.status === 'pending') {
      text.textContent = '待诊任务：领取后才能撰写报告';
      claim.hidden = false;
    } else if (item.assignee_id === user.id) {
      text.textContent = '已领取任务';
      release.hidden = report?.status === 'draft' || report?.status === 'amending';
      if (!report) create.hidden = false;
    } else {
      text.textContent = `已由${item.assignee_name ?? '他人'}领取`;
    }
  }

  private renderEmpty(): void {
    el<HTMLButtonElement>('report-save').hidden = true;
    el<HTMLButtonElement>('report-sign').hidden = true;
    el<HTMLButtonElement>('report-amend').hidden = true;
    this.findingsEditor.contentEditable = 'false';
    this.impressionEditor.contentEditable = 'false';
    this.findingsEditor.textContent = '';
    this.impressionEditor.textContent = '';
    this.positive.disabled = true;
    this.positive.checked = false;
    el('report-author').textContent = '--';
    el('report-updated-at').textContent = '--';
    el('report-reviewer').textContent = '--';
    el('report-signed-at').textContent = '--';
    this.versionsList.replaceChildren();
    this.renderStatus('draft');
    this.renderWorkItemRow();
  }

  private renderDocument(report: DiagnosticReport): void {
    const editable = report.status === 'draft' || report.status === 'amending';
    const findings = contentForEditor(report.findings, report.template_payload != null);
    const impression = contentForEditor(report.impression, report.template_payload != null);
    this.findingsEditor.innerHTML = sanitizeReportHtml(findings);
    this.impressionEditor.innerHTML = sanitizeReportHtml(impression);
    this.findingsEditor.contentEditable = editable ? 'true' : 'false';
    this.impressionEditor.contentEditable = editable ? 'true' : 'false';
    this.positive.disabled = !editable;
    this.positive.checked = report.is_positive;

    const save = el<HTMLButtonElement>('report-save');
    const sign = el<HTMLButtonElement>('report-sign');
    const amend = el<HTMLButtonElement>('report-amend');
    save.hidden = !editable;
    sign.hidden = !editable;
    amend.hidden = report.status !== 'signed';
    save.disabled = true;
  }

  private renderSignatures(report: DiagnosticReport): void {
    const user = this.getUser();
    const author = user.displayName || user.username || '--';
    el('report-author').textContent = author;
    el('report-updated-at').textContent = new Date(report.updated_at).toLocaleString();
    if (report.status === 'signed' && this.versions.length > 0) {
      const latest = this.versions[this.versions.length - 1];
      el('report-reviewer').textContent = author;
      el('report-signed-at').textContent = new Date(latest.signed_at).toLocaleString();
    } else {
      el('report-reviewer').textContent = '--';
      el('report-signed-at').textContent = '--';
    }
  }

  private renderVersions(): void {
    const list = this.versionsList;
    list.replaceChildren();
    if (this.versions.length === 0) {
      const item = document.createElement('li');
      item.className = 'report-version-empty';
      item.textContent = '暂无版本';
      list.append(item);
      return;
    }
    for (const version of this.versions) {
      const item = document.createElement('li');
      const title = document.createElement('div');
      title.className = 'report-version-title';
      title.textContent = `v${version.version_number} · ${new Date(version.signed_at).toLocaleString()}`;
      if (version.amendment_reason) {
        const reason = document.createElement('div');
        reason.className = 'report-version-reason';
        reason.textContent = `修订：${version.amendment_reason}`;
        item.append(title, reason);
      } else {
        item.append(title);
      }
      item.title = `阳性：${version.is_positive ? '是' : '否'}`;
      list.append(item);
    }
  }

  private renderTemplates(): void {
    const tree = this.templateTree;
    tree.replaceChildren();
    if (this.templates.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'report-template-empty';
      empty.textContent = '当前模态没有可用模板';
      tree.append(empty);
      return;
    }
    const byBody: Map<string, ReportTemplate[]> = new Map();
    for (const template of this.templates) {
      const key = template.body_part ?? '其他';
      const list = byBody.get(key) ?? [];
      list.push(template);
      byBody.set(key, list);
    }
    for (const [group, list] of byBody) {
      const heading = document.createElement('div');
      heading.className = 'report-template-group';
      heading.textContent = group;
      tree.append(heading);
      for (const template of list) {
        const button = document.createElement('button');
        button.type = 'button';
        button.className = 'report-template-item';
        button.textContent = template.name;
        button.title = '点击插入骨架片段';
        button.addEventListener('click', () => this.insertTemplate(template));
        tree.append(button);
      }
    }
  }

  private insertTemplate(template: ReportTemplate): void {
    const editor = this.lastFocusedEditor === 'impression'
      ? this.impressionEditor
      : this.findingsEditor;
    if (editor.contentEditable !== 'true') {
      this.reportError('当前报告不可编辑，需先新建或修订');
      return;
    }
    const skeleton = templateSkeleton(template);
    document.execCommand('insertText', false, skeleton);
    this.markDirty();
  }

  private async create(): Promise<void> {
    if (!this.context) return;
    if (!this.workItem) {
      this.reportError('该序列没有待诊任务，无法创建报告');
      return;
    }
    if (!this.claimedByMe()) {
      this.reportError('请先领取任务再创建报告');
      return;
    }
    try {
      const report = await createReport(this.context.studyUid, [this.context.seriesUid], null, false);
      this.report = report;
      this.reports = [report, ...this.reports.filter((entry) => entry.id !== report.id)];
      this.render();
    } catch (error) {
      this.reportError(errorMessage(error));
    }
  }

  private collectRichText(): { findings: string; impression: string; isPositive: boolean } {
    const findings = sanitizeReportHtml(this.findingsEditor.innerHTML);
    const impression = sanitizeReportHtml(this.impressionEditor.innerHTML);
    const isPositive = this.positive.checked;
    return { findings, impression, isPositive };
  }

  private async save(): Promise<void> {
    const report = this.report;
    if (!report || (report.status !== 'draft' && report.status !== 'amending')) return;
    const { findings, impression, isPositive } = this.collectRichText();
    if (!htmlToText(findings) || !htmlToText(impression)) {
      this.reportError('影像所见和意见不能为空');
      return;
    }
    const clearingPayload = report.template_payload != null;
    try {
      const updated = await updateReportDraft(
        report.id,
        report.revision,
        findings,
        impression,
        null,
        clearingPayload ? null : report.template_payload,
        isPositive,
        clearingPayload,
      );
      this.report = updated;
      el<HTMLButtonElement>('report-save').disabled = true;
      this.reportError(`草稿已保存（第 ${updated.revision} 版）`);
    } catch (error) {
      this.reportError(errorMessage(error));
      await this.refresh();
    }
  }

  private async sign(): Promise<void> {
    await this.save();
    const report = this.report;
    if (!report || (report.status !== 'draft' && report.status !== 'amending')) return;
    if (!window.confirm('确认签发？签发后报告不可直接修改，需发起修订。')) return;
    try {
      await signReport(report.id, report.revision);
      await this.refresh();
    } catch (error) {
      this.reportError(errorMessage(error));
      await this.refresh();
    }
  }

  private async amend(): Promise<void> {
    if (!this.report) return;
    const reason = window.prompt('修订原因（必填）');
    if (!reason?.trim()) {
      this.reportError('修订原因不能为空');
      return;
    }
    try {
      const amended = await beginReportAmendment(this.report.id, reason.trim());
      this.report = amended;
      this.render();
    } catch (error) {
      this.reportError(errorMessage(error));
      await this.refresh();
    }
  }

  private async claim(): Promise<void> {
    if (!this.workItem) return;
    try {
      await claimWorkItem(this.workItem.id, this.workItem.revision);
      await this.refresh();
    } catch (error) {
      this.reportError(errorMessage(error));
      await this.refresh();
    }
  }

  private async release(): Promise<void> {
    if (!this.workItem) return;
    try {
      await releaseWorkItem(this.workItem.id, this.workItem.revision);
      await this.refresh();
    } catch (error) {
      this.reportError(errorMessage(error));
      await this.refresh();
    }
  }
}

function contentForEditor(text: string, isStructured: boolean): string {
  if (isStructured) return plainToHtml(text);
  return text;
}

function ageFromBirthDate(birthDate: string | null): string {
  if (!birthDate) return '--';
  const digits = birthDate.replace(/[^0-9]/g, '');
  if (digits.length < 8) return '--';
  const year = Number(digits.slice(0, 4));
  const month = Number(digits.slice(4, 6)) - 1;
  const day = Number(digits.slice(6, 8));
  const birth = new Date(year, month, day);
  if (Number.isNaN(birth.getTime())) return '--';
  const now = new Date();
  let age = now.getFullYear() - birth.getFullYear();
  const beforeBirthday = now.getMonth() < birth.getMonth()
    || (now.getMonth() === birth.getMonth() && now.getDate() < birth.getDate());
  if (beforeBirthday) age -= 1;
  return age > 0 ? `${age} 岁` : '--';
}

function templateSkeleton(template: ReportTemplate): string {
  const lines: string[] = [];
  for (const section of template.structure.sections) {
    lines.push(`【${section.title}】`);
    for (const field of section.fields) {
      lines.push(`- ${field.label}：`);
    }
    lines.push('');
  }
  return lines.join('\n');
}

function errorMessage(error: unknown): string {
  if (typeof error === 'string') return error;
  if (error instanceof Error) return error.message;
  return String(error);
}

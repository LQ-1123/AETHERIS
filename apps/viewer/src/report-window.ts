import {
  beginReportAmendment,
  createReport,
  getReportContext,
  listenReportContext,
  listReportTemplates,
  listReports,
  listReportVersions,
  listStudySeries,
  signReport,
  updateReportDraft,
  type ReportWindowContext,
} from './api';
import { htmlToText, plainToHtml, sanitizeReportHtml } from './rich-text';
import type { DiagnosticReport, ReportTemplate, ReportVersion } from './types';

function el<T extends HTMLElement = HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (!found) throw new Error(`缺少元素 #${id}`);
  return found as T;
}

const STATUS_LABEL: Record<string, { text: string; status: string }> = {
  pending: { text: '待书写', status: 'pending' },
  writing: { text: '书写中', status: 'writing' },
  locked: { text: '已锁定', status: 'locked' },
  signed: { text: '已签发', status: 'signed' },
};

/** 报告独立小窗：紧凑单栏，双屏工作流下拖到第二块屏边看边写。 */
export class ReportWindow {
  private context: ReportWindowContext | null = null;
  private report: DiagnosticReport | null = null;
  private versions: ReportVersion[] = [];
  private templates: ReportTemplate[] = [];
  private busy = false;
  private lastEditor: 'findings' | 'impression' | null = null;

  private readonly findings = el<HTMLDivElement>('rw-findings');
  private readonly impression = el<HTMLDivElement>('rw-impression');
  private readonly positive = el<HTMLInputElement>('rw-positive');
  private readonly drawer = el<HTMLElement>('rw-drawer');

  constructor() {
    el<HTMLButtonElement>('rw-image').addEventListener('click', async () => {
      // 聚焦主窗（切回影像）
      const { WebviewWindow } = await import('@tauri-apps/api/webviewWindow');
      const main = await WebviewWindow.getByLabel('main');
      await main?.setFocus();
    });
    el<HTMLButtonElement>('rw-save').addEventListener('click', () => void this.save());
    el<HTMLButtonElement>('rw-sign').addEventListener('click', () => void this.sign());
    el<HTMLButtonElement>('rw-amend').addEventListener('click', () => void this.amend());
    el<HTMLButtonElement>('rw-create').addEventListener('click', () => void this.create());
    el<HTMLButtonElement>('rw-template-btn').addEventListener('click', () => this.openTemplates());
    el<HTMLButtonElement>('rw-versions-btn').addEventListener('click', () => this.openVersions());
    el<HTMLButtonElement>('rw-drawer-close').addEventListener('click', () => { this.drawer.hidden = true; });
    this.positive.addEventListener('change', () => el<HTMLButtonElement>('rw-save').disabled = false);

    for (const editor of [this.findings, this.impression]) {
      editor.addEventListener('focus', () => {
        this.lastEditor = editor.dataset.editor as 'findings' | 'impression';
      });
      editor.addEventListener('input', () => { el<HTMLButtonElement>('rw-save').disabled = false; });
    }
    for (const button of document.querySelectorAll<HTMLButtonElement>('[data-rich]')) {
      button.addEventListener('click', () => {
        document.execCommand(button.dataset.rich as string, false);
        el<HTMLButtonElement>('rw-save').disabled = false;
        this.lastEditor ??= 'findings';
      });
    }
    document.addEventListener('keydown', (event) => this.onKeydown(event));
  }

  async init(): Promise<void> {
    try {
      await listenReportContext((context) => void this.applyContext(context));
      const context = await getReportContext();
      if (context) await this.applyContext(context);
    } catch (error) {
      console.error('报告窗初始化失败', error);
      this.showError(`初始化失败：${errorMessage(error)}`);
    }
  }

  private async applyContext(context: ReportWindowContext): Promise<void> {
    this.context = context;
    el('rw-title').textContent = `诊断报告 · ${context.patientName}`;
    el('rw-patient-strip').textContent = [
      context.patientName,
      context.patientId,
      context.patientSex,
      context.patientBirthDate ? `${ageFromBirthDate(context.patientBirthDate)}` : null,
      context.modality,
      context.seriesDescription,
      context.studyDate,
    ].filter(Boolean).join(' · ');
    await this.refresh();
  }

  private onKeydown(event: KeyboardEvent): void {
    const target = event.target as HTMLElement | null;
    if (!target?.closest('.report-editor')) return;
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's') {
      event.preventDefault();
      void this.save();
    } else if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') {
      event.preventDefault();
      void this.sign();
    }
  }

  private writable(): boolean {
    return this.context?.user.role === 'radiologist';
  }

  async refresh(): Promise<void> {
    if (!this.context) return;
    if (this.busy) return;
    this.busy = true;
    try {
      const { studyUid, modality } = this.context;
      const [reports, templates] = await Promise.all([
        listReports(studyUid),
        listReportTemplates(modality ?? undefined),
      ]);
      this.templates = templates;
      this.report = reports[0] ?? null;
      this.versions = [];
      if (this.report?.status === 'signed') {
        this.versions = await listReportVersions(this.report.id);
      }
      this.render();
    } catch (error) {
      this.showError(errorMessage(error));
    } finally {
      this.busy = false;
    }
  }

  private showError(message: string): void {
    el('rw-workitem-text').textContent = message;
    el('rw-workitem').hidden = false;
  }

  private render(): void {
    this.renderStatus();
    const report = this.report;
    if (!report) {
      this.renderEmpty();
      return;
    }
    this.renderDocument(report);
    this.renderSignature(report);
  }

  private renderStatus(): void {
    const report = this.report;
    let key = 'pending';
    if (report) {
      if (report.status === 'signed') key = 'signed';
      else if (report.author_id === this.context?.user.id) key = 'writing';
      else key = 'locked';
    }
    const mapping = STATUS_LABEL[key];
    const badge = el('rw-status');
    badge.textContent = mapping.text;
    badge.dataset.status = mapping.status;
    // 新建按钮：待书写且可写时显示
    const showCreate = key === 'pending' && this.writable();
    el<HTMLButtonElement>('rw-create').hidden = !showCreate;
    const workItemRow = el('rw-workitem');
    if (showCreate) {
      workItemRow.hidden = false;
      el('rw-workitem-text').textContent = '待书写';
    } else {
      workItemRow.hidden = true;
    }
  }

  private renderEmpty(): void {
    this.findings.contentEditable = 'false';
    this.impression.contentEditable = 'false';
    this.findings.textContent = '';
    this.impression.textContent = '';
    this.positive.disabled = true;
    this.positive.checked = false;
    el('rw-author').textContent = '--';
    el('rw-reviewer').textContent = '--';
    el<HTMLButtonElement>('rw-save').hidden = true;
    el<HTMLButtonElement>('rw-sign').hidden = true;
    el<HTMLButtonElement>('rw-amend').hidden = true;
  }

  private renderDocument(report: DiagnosticReport): void {
    // 草稿即锁：只有作者本人能编辑 draft/amending。
    const editable = (report.status === 'draft' || report.status === 'amending')
      && report.author_id === this.context?.user.id;
    this.findings.innerHTML = sanitizeReportHtml(contentForEditor(report.findings, report.template_payload != null));
    this.impression.innerHTML = sanitizeReportHtml(contentForEditor(report.impression, report.template_payload != null));
    this.findings.contentEditable = editable ? 'true' : 'false';
    this.impression.contentEditable = editable ? 'true' : 'false';
    this.positive.disabled = !editable;
    this.positive.checked = report.is_positive;
    el<HTMLButtonElement>('rw-save').hidden = !editable;
    el<HTMLButtonElement>('rw-sign').hidden = !editable;
    el<HTMLButtonElement>('rw-amend').hidden = report.status !== 'signed';
    el<HTMLButtonElement>('rw-save').disabled = true;
  }

  private renderSignature(report: DiagnosticReport): void {
    const user = this.context?.user;
    const author = user?.displayName || user?.username || '--';
    el('rw-author').textContent = author;
    if (report.status === 'signed' && this.versions.length > 0) {
      el('rw-reviewer').textContent = author;
    } else {
      el('rw-reviewer').textContent = '--';
    }
  }

  private openTemplates(): void {
    const body = el('rw-drawer-body');
    el('rw-drawer-title').textContent = '报告模板';
    body.replaceChildren();
    if (this.templates.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'report-template-empty';
      empty.textContent = '当前模态没有可用模板';
      body.append(empty);
    } else {
      const byBody = new Map<string, ReportTemplate[]>();
      for (const template of this.templates) {
        const key = template.body_part ?? '其他';
        (byBody.get(key) ?? byBody.set(key, []).get(key)!).push(template);
      }
      for (const [group, list] of byBody) {
        const heading = document.createElement('div');
        heading.className = 'report-template-group';
        heading.textContent = group;
        body.append(heading);
        for (const template of list) {
          const button = document.createElement('button');
          button.type = 'button';
          button.className = 'report-template-item';
          button.textContent = template.name;
          button.addEventListener('click', () => {
            this.insertTemplate(template);
            this.drawer.hidden = true;
          });
          body.append(button);
        }
      }
    }
    this.drawer.hidden = false;
  }

  private openVersions(): void {
    const body = el('rw-drawer-body');
    el('rw-drawer-title').textContent = '修改记录';
    body.replaceChildren();
    if (this.versions.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'report-template-empty';
      empty.textContent = '暂无版本';
      body.append(empty);
    } else {
      for (const version of this.versions) {
        const item = document.createElement('div');
        item.className = 'report-version-title';
        item.textContent = `v${version.version_number} · ${new Date(version.signed_at).toLocaleString()}${version.is_positive ? ' · 阳性' : ''}`;
        body.append(item);
      }
    }
    this.drawer.hidden = false;
  }

  private insertTemplate(template: ReportTemplate): void {
    const editor = this.lastEditor === 'impression' ? this.impression : this.findings;
    if (editor.contentEditable !== 'true') {
      this.showError('当前报告不可编辑，需先新建或修订');
      return;
    }
    const lines: string[] = [];
    for (const section of template.structure.sections) {
      lines.push(`【${section.title}】`);
      for (const field of section.fields) lines.push(`- ${field.label}：`);
      lines.push('');
    }
    document.execCommand('insertText', false, lines.join('\n'));
    el<HTMLButtonElement>('rw-save').disabled = false;
  }

  private async create(): Promise<void> {
    if (!this.context) return;
    try {
      // 报告按检查一份：覆盖医生有访问权的全部序列（list_study_series 已按授权过滤）
      const series = await listStudySeries(this.context.studyUid);
      const seriesUids = series.map((entry) => entry.series_uid);
      const report = await createReport(this.context.studyUid, seriesUids, null, false);
      this.report = report;
      this.render();
    } catch (error) {
      this.showError(errorMessage(error));
    }
  }

  private collect(): { findings: string; impression: string; isPositive: boolean } {
    return {
      findings: sanitizeReportHtml(this.findings.innerHTML),
      impression: sanitizeReportHtml(this.impression.innerHTML),
      isPositive: this.positive.checked,
    };
  }

  private async save(): Promise<void> {
    const report = this.report;
    if (!report || (report.status !== 'draft' && report.status !== 'amending')) return;
    const { findings, impression, isPositive } = this.collect();
    if (!htmlToText(findings) || !htmlToText(impression)) {
      this.showError('影像所见和意见不能为空');
      return;
    }
    const clearing = report.template_payload != null;
    try {
      const updated = await updateReportDraft(
        report.id, report.revision, findings, impression, null,
        clearing ? null : report.template_payload, isPositive, clearing,
      );
      this.report = updated;
      el<HTMLButtonElement>('rw-save').disabled = true;
      this.showError(`草稿已保存（第 ${updated.revision} 版）`);
    } catch (error) {
      this.showError(errorMessage(error));
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
      this.showError(errorMessage(error));
      await this.refresh();
    }
  }

  private async amend(): Promise<void> {
    if (!this.report) return;
    const reason = window.prompt('修订原因（必填）');
    if (!reason?.trim()) { this.showError('修订原因不能为空'); return; }
    try {
      this.report = await beginReportAmendment(this.report.id, reason.trim());
      this.render();
    } catch (error) {
      this.showError(errorMessage(error));
      await this.refresh();
    }
  }
}

function contentForEditor(text: string, isStructured: boolean): string {
  return isStructured ? plainToHtml(text) : text;
}

function ageFromBirthDate(birthDate: string | null): string {
  if (!birthDate) return '';
  const digits = birthDate.replace(/[^0-9]/g, '');
  if (digits.length < 8) return '';
  const year = Number(digits.slice(0, 4));
  const month = Number(digits.slice(4, 6)) - 1;
  const day = Number(digits.slice(6, 8));
  const birth = new Date(year, month, day);
  if (Number.isNaN(birth.getTime())) return '';
  const now = new Date();
  let age = now.getFullYear() - birth.getFullYear();
  if (now.getMonth() < birth.getMonth() || (now.getMonth() === birth.getMonth() && now.getDate() < birth.getDate())) age -= 1;
  return age > 0 ? `${age} 岁` : '';
}

function errorMessage(error: unknown): string {
  if (typeof error === 'string') return error;
  if (error instanceof Error) return error.message;
  return String(error);
}

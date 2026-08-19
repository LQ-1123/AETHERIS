import {
  approveReport,
  beginReportAmendment,
  createReport,
  examRequestForStudy,
  getReportContext,
  listenReportContext,
  listReportTemplates,
  listReports,
  listReportReviewEvents,
  listReportVersions,
  listStudySeries,
  startReportReview,
  submitReport,
  updateReportDraft,
  type ReportWindowContext,
} from './api';
import { htmlToText, plainToHtml, sanitizeReportHtml } from './rich-text';
import type { DiagnosticReport, ExamRequest, ReportReviewEvent, ReportTemplate, ReportVersion } from './types';

function el<T extends HTMLElement = HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (!found) throw new Error(`缺少元素 #${id}`);
  return found as T;
}

const STATUS_LABEL: Record<string, { text: string; status: string }> = {
  pending: { text: '待书写', status: 'pending' },
  writing: { text: '书写中', status: 'writing' },
  locked: { text: '已锁定', status: 'locked' },
  submitted: { text: '待审核', status: 'submitted' },
  under_review: { text: '审核中', status: 'under_review' },
  signed: { text: '已签发', status: 'signed' },
};

/** 报告独立小窗：紧凑单栏，双屏工作流下拖到第二块屏边看边写。 */
export class ReportWindow {
  private context: ReportWindowContext | null = null;
  private report: DiagnosticReport | null = null;
  private versions: ReportVersion[] = [];
  private reviewEvents: ReportReviewEvent[] = [];
  private templates: ReportTemplate[] = [];
  private examRequest: ExamRequest | null = null;
  private busy = false;
  private lastEditor: 'findings' | 'impression' | null = null;
  private reviewEditing = false;

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
    el<HTMLButtonElement>('rw-submit').addEventListener('click', () => void this.submit());
    el<HTMLButtonElement>('rw-review-start').addEventListener('click', () => void this.startReview());
    el<HTMLButtonElement>('rw-approve').addEventListener('click', () => void this.approve(false));
    el<HTMLButtonElement>('rw-modify').addEventListener('click', () => void this.modifyOrApprove());
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
      const [reports, templates, examRequest] = await Promise.all([
        listReports(studyUid),
        listReportTemplates(modality ?? undefined),
        examRequestForStudy(studyUid),
      ]);
      this.templates = templates;
      this.examRequest = examRequest;
      this.report = reports[0] ?? null;
      this.versions = [];
      this.reviewEvents = [];
      if (this.report) {
        [this.versions, this.reviewEvents] = await Promise.all([
          this.report.status === 'signed' ? listReportVersions(this.report.id) : Promise.resolve([]),
          listReportReviewEvents(this.report.id),
        ]);
      }
      this.reviewEditing = false;
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
    this.renderClinicalContext();
    this.renderStatus();
    const report = this.report;
    if (!report) {
      this.renderEmpty();
      return;
    }
    this.renderDocument(report);
    this.renderSignature(report);
  }

  private renderClinicalContext(): void {
    const node = el('rw-clinical-context');
    const request = this.examRequest;
    node.hidden = !request;
    node.replaceChildren();
    if (!request) return;
    const heading = document.createElement('strong'); heading.textContent = '申请信息';
    const indication = document.createElement('span'); indication.textContent = request.clinical_indication;
    const meta = document.createElement('small'); meta.textContent = `${request.request_type} · ${request.modality} · ${request.body_part} · 申请人 ${request.requested_by_name}`;
    node.append(heading, indication, meta);
  }

  private renderStatus(): void {
    const report = this.report;
    let key = 'pending';
    if (report) {
      if (report.status === 'signed') key = 'signed';
      else if (report.status === 'submitted') key = 'submitted';
      else if (report.status === 'under_review') key = 'under_review';
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
    const sameAuthor = report?.author_id === this.context?.user.id;
    const reviewRole = this.context?.user.role === 'radiologist' || this.context?.user.role === 'admin';
    const canStart = report?.status === 'submitted' && report.can_review && !sameAuthor;
    // 未授予 review_report 时也显示入口，但置灰并给出明确原因，避免审核人误以为
    // “待审核”只是作者只读状态。真正的权限校验仍在后端完成。
    const reviewCandidate = report?.status === 'submitted' && reviewRole && !sameAuthor;
    const reviewStart = el<HTMLButtonElement>('rw-review-start');
    reviewStart.hidden = !(canStart || reviewCandidate);
    reviewStart.disabled = !canStart;
    reviewStart.title = canStart ? '开始审核' : '请管理员在账号管理中授予“可审核报告”权限';
    const workItemRow = el('rw-workitem');
    if (showCreate || canStart || reviewCandidate) {
      workItemRow.hidden = false;
      el('rw-workitem-text').textContent = showCreate
        ? '待书写'
        : canStart
          ? '报告已提交，可开始审核'
          : '当前账号未授予审核权限，请管理员在账号管理中勾选“可审核报告”';
    } else if (report?.status === 'submitted' || report?.status === 'under_review') {
      workItemRow.hidden = false;
      el('rw-workitem-text').textContent = report.status === 'submitted'
        ? sameAuthor ? '已提交送审，作者不能审核自己的报告' : '已提交送审，报告只读'
        : `审核中${report.reviewer_name ? ` · ${report.reviewer_name}` : ''}`;
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
    el<HTMLButtonElement>('rw-amend').hidden = true;
    el<HTMLButtonElement>('rw-submit').hidden = true;
    el<HTMLButtonElement>('rw-approve').hidden = true;
    el<HTMLButtonElement>('rw-modify').hidden = true;
    el('rw-review-comment-row').hidden = true;
  }

  private renderDocument(report: DiagnosticReport): void {
    // 草稿即锁：只有作者本人能编辑 draft/amending。
    const isAuthorDraft = (report.status === 'draft' || report.status === 'amending')
      && report.author_id === this.context?.user.id;
    const isAssignedReviewer = report.status === 'under_review'
      && report.reviewer_id === this.context?.user.id && report.can_review;
    const editable = isAuthorDraft || (isAssignedReviewer && this.reviewEditing);
    this.findings.innerHTML = sanitizeReportHtml(contentForEditor(report.findings, report.template_payload != null));
    this.impression.innerHTML = sanitizeReportHtml(contentForEditor(report.impression, report.template_payload != null));
    this.findings.contentEditable = editable ? 'true' : 'false';
    this.impression.contentEditable = editable ? 'true' : 'false';
    this.positive.disabled = !editable;
    this.positive.checked = report.is_positive;
    el<HTMLButtonElement>('rw-save').hidden = !editable;
    el<HTMLButtonElement>('rw-submit').hidden = !isAuthorDraft;
    el<HTMLButtonElement>('rw-amend').hidden = report.status !== 'signed';
    el<HTMLButtonElement>('rw-approve').hidden = !isAssignedReviewer || this.reviewEditing;
    const modify = el<HTMLButtonElement>('rw-modify');
    modify.hidden = !isAssignedReviewer;
    modify.textContent = this.reviewEditing ? '确认修改并签发' : '修改后签发';
    el('rw-review-comment-row').hidden = !isAssignedReviewer;
    if (!isAssignedReviewer) el<HTMLTextAreaElement>('rw-review-comment').value = report.review_comment ?? '';
    el<HTMLButtonElement>('rw-save').disabled = true;
  }

  private renderSignature(report: DiagnosticReport): void {
    el('rw-author').textContent = report.author_name || '--';
    el('rw-reviewer').textContent = report.reviewer_modified && report.reviewer_name
      ? `${report.reviewer_name}（已修改）`
      : report.reviewer_name ?? '--';
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
    if (this.versions.length === 0 && this.reviewEvents.length === 0) {
      const empty = document.createElement('div');
      empty.className = 'report-template-empty';
      empty.textContent = '暂无版本';
      body.append(empty);
    } else {
      const actionText: Record<string, string> = {
        submitted: '提交送审', review_started: '开始审核', reviewer_modified: '审核人修改',
        approved: '审核通过并签发', rejected: '退回',
      };
      for (const event of this.reviewEvents) {
        const item = document.createElement('div');
        item.className = 'report-timeline-item';
        const comment = event.comment ? ` · ${event.comment}` : '';
        item.textContent = `${actionText[event.action] ?? event.action} · ${event.actor_name} · ${new Date(event.created_at).toLocaleString()}${comment}`;
        body.append(item);
      }
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

  private async submit(): Promise<void> {
    await this.save();
    const report = this.report;
    if (!report || (report.status !== 'draft' && report.status !== 'amending')) return;
    if (!window.confirm('确认提交送审？提交后在审核完成前报告为只读。')) return;
    try {
      await submitReport(report.id, report.revision);
      await this.refresh();
    } catch (error) {
      this.showError(errorMessage(error));
      await this.refresh();
    }
  }

  private async startReview(): Promise<void> {
    const report = this.report;
    if (!report || report.status !== 'submitted') return;
    try {
      await startReportReview(report.id, report.revision);
      await this.refresh();
    } catch (error) {
      this.showError(errorMessage(error));
      await this.refresh();
    }
  }

  private async modifyOrApprove(): Promise<void> {
    if (!this.reviewEditing) {
      this.reviewEditing = true;
      if (this.report) this.renderDocument(this.report);
      this.findings.focus();
      return;
    }
    await this.approve(true);
  }

  private async approve(modified: boolean): Promise<void> {
    const report = this.report;
    if (!report || report.status !== 'under_review') return;
    const reviewComment = el<HTMLTextAreaElement>('rw-review-comment').value.trim() || null;
    const collected = modified ? this.collect() : null;
    if (modified && collected && (!htmlToText(collected.findings) || !htmlToText(collected.impression))) {
      this.showError('影像所见和意见不能为空');
      return;
    }
    const prompt = modified ? '确认以修改后的内容签发？原作者将记录一次审核修正。' : '确认报告无需修改并直接签发？';
    if (!window.confirm(prompt)) return;
    try {
      await approveReport(
        report.id,
        report.revision,
        modified,
        collected ? { findings: collected.findings, impression: collected.impression, recommendation: null } : null,
        reviewComment,
      );
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

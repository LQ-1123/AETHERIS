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
import { payloadFromTemplate, renderReportText, validatePayload } from './report-render';
import type {
  ChoiceValue,
  ClinicalWorkItem,
  DiagnosticReport,
  NumberValue,
  ReportTemplate,
  ReportVersion,
  StructuredPayload,
} from './types';

export interface ReportContext {
  studyUid: string;
  seriesUid: string;
  modality: string | null;
  patientName: string;
  seriesDescription: string | null;
}

function element<T extends HTMLElement = HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (!found) throw new Error(`缺少元素 #${id}`);
  return found as T;
}

/** 结构化报告面板：状态机 无报告 → 草稿 → 已签发 → 修订。 */
export class ReportPanel {
  private readonly dialog = element<HTMLDialogElement>('report-dialog');
  private readonly subtitle = element<HTMLElement>('report-subtitle');
  private readonly error = element<HTMLElement>('report-error');
  private readonly workItemRow = element<HTMLElement>('report-workitem-row');
  private readonly workItemText = element<HTMLElement>('report-workitem-text');
  private readonly claimButton = element<HTMLButtonElement>('report-claim-btn');
  private readonly releaseButton = element<HTMLButtonElement>('report-release-btn');
  private readonly body = element<HTMLElement>('report-body');

  private context: ReportContext | null = null;
  private reports: DiagnosticReport[] = [];
  private report: DiagnosticReport | null = null;
  private versions: ReportVersion[] = [];
  private templates: ReportTemplate[] = [];
  private workItem: ClinicalWorkItem | null = null;
  private busy = false;

  constructor(
    private readonly reportError: (message: string) => void,
    private readonly getContext: () => ReportContext | null,
    private readonly getCurrentUser: () => { id: number | null; role: string | null },
  ) {
    element<HTMLButtonElement>('report-panel-btn').addEventListener('click', () => void this.open());
    element<HTMLButtonElement>('report-close').addEventListener('click', () => this.dialog.close());
    element<HTMLButtonElement>('report-refresh').addEventListener('click', () => void this.refresh());
    this.claimButton.addEventListener('click', () => void this.claim());
    this.releaseButton.addEventListener('click', () => void this.release());
  }

  async open(): Promise<void> {
    const context = this.getContext();
    if (!context) return;
    this.context = context;
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
    this.error.classList.remove('dialog-success');
    this.error.hidden = true;
  }

  async refresh(): Promise<void> {
    if (!this.context) return;
    if (this.busy) return;
    this.busy = true;
    this.clearError();
    try {
      const { studyUid, seriesUid, modality, patientName, seriesDescription } = this.context;
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
      this.subtitle.textContent = [
        patientName,
        modality,
        seriesDescription,
      ].filter(Boolean).join(' · ');
      this.render();
    } catch (error) {
      this.showError(errorMessage(error));
    } finally {
      this.busy = false;
    }
  }

  private writable(): boolean {
    return this.getCurrentUser().role === 'radiologist';
  }

  private render(): void {
    this.renderWorkItemRow();
    const report = this.report;
    if (!report) {
      this.renderEmpty();
      return;
    }
    if (report.status === 'draft' || report.status === 'amending') {
      this.renderEditor(report);
    } else {
      this.renderSigned(report);
    }
  }

  private renderWorkItemRow(): void {
    const item = this.workItem;
    const user = this.getCurrentUser();
    if (!item || !this.writable()) {
      this.workItemRow.hidden = true;
      this.claimButton.hidden = true;
      this.releaseButton.hidden = true;
      return;
    }
    this.workItemRow.hidden = false;
    this.claimButton.hidden = item.status !== 'pending';
    this.releaseButton.hidden = !(
      (item.status === 'claimed' || item.status === 'reporting')
      && item.assignee_id === user.id
    );
    if (item.status === 'pending') this.workItemText.textContent = '待诊任务：可领取后撰写报告';
    else if (item.assignee_id === user.id) this.workItemText.textContent = `已领取${item.assignee_name ? `（${item.assignee_name}）` : ''}`;
    else this.workItemText.textContent = `已由${item.assignee_name ?? '他人'}领取`;
  }

  /** 当前序列的工作项是否已被我领取（创建报告的前置条件之一）。 */
  private claimedByMe(): boolean {
    const item = this.workItem;
    const user = this.getCurrentUser();
    return !!item
      && (item.status === 'claimed' || item.status === 'reporting')
      && item.assignee_id === user.id;
  }

  private renderEmpty(): void {
    this.body.replaceChildren();
    const section = document.createElement('section');
    section.className = 'report-empty';
    const hint = document.createElement('p');
    if (!this.writable()) {
      hint.textContent = '该检查还没有报告。需要医师角色才能创建。';
    } else if (!this.workItem) {
      hint.textContent = '该序列没有待诊任务，无法创建报告（任务在影像入库或来源归属时自动生成）。';
    } else if (!this.claimedByMe()) {
      hint.textContent = this.workItem.assignee_id != null
        ? `任务已由${this.workItem.assignee_name ?? '他人'}领取。`
        : '请先在顶部「领取任务」，领取成功后才能撰写报告。';
    } else {
      hint.textContent = '该检查还没有报告。选择一个模板开始撰写：';
    }
    section.append(hint);
    if (this.writable() && this.claimedByMe() && this.templates.length > 0) {
      const select = document.createElement('select');
      select.className = 'report-template-select';
      for (const template of this.templates) {
        const option = document.createElement('option');
        option.value = template.id;
        option.textContent = `${template.name}${template.builtin ? '（示例模板）' : ''}`;
        select.append(option);
      }
      const create = document.createElement('button');
      create.type = 'button';
      create.className = 'command-button';
      create.textContent = '新建报告';
      create.addEventListener('click', () => void this.create(select.value));
      section.append(select, create);
    } else if (this.writable()) {
      const noTemplates = document.createElement('p');
      noTemplates.textContent = `当前模态（${this.context?.modality ?? '未知'}）没有可用模板。`;
      section.append(noTemplates);
    }
    this.body.append(section);
  }

  private async create(templateId: string): Promise<void> {
    if (!this.context) return;
    this.clearError();
    if (!this.workItem) {
      this.showError('该序列没有待诊任务，无法创建报告');
      return;
    }
    if (!this.claimedByMe()) {
      this.showError('请先在顶部「领取任务」再创建报告');
      return;
    }
    const template = this.templates.find((candidate) => candidate.id === templateId);
    if (!template) return;
    try {
      const payload = payloadFromTemplate(template);
      const report = await createReport(this.context.studyUid, [this.context.seriesUid], payload);
      this.report = report;
      this.reports = [report, ...this.reports.filter((entry) => entry.id !== report.id)];
      this.renderEditor(report);
    } catch (error) {
      this.showError(errorMessage(error));
    }
  }

  private renderEditor(report: DiagnosticReport): void {
    this.body.replaceChildren();
    const payload = report.template_payload;
    const form = payload
      ? this.buildStructuredForm(payload)
      : this.buildPlainForm(report);
    const actions = document.createElement('div');
    actions.className = 'report-actions';
    const save = document.createElement('button');
    save.type = 'button';
    save.className = 'command-button';
    save.textContent = '保存草稿';
    save.addEventListener('click', () => void this.save(report, form));
    const sign = document.createElement('button');
    sign.type = 'button';
    sign.className = 'command-button';
    sign.textContent = '签发';
    sign.addEventListener('click', () => void this.sign(report, form));
    actions.append(save, sign);
    if (!this.writable()) {
      save.disabled = true;
      sign.disabled = true;
      (form.querySelectorAll('input, textarea') as NodeListOf<HTMLInputElement | HTMLTextAreaElement>)
        .forEach((control) => { control.disabled = true; });
    }
    if (report.status === 'amending') {
      const banner = document.createElement('p');
      banner.className = 'report-amending-banner';
      banner.textContent = '修订中：基于当前已签发版本编辑，签发后生成新版本。';
      this.body.append(banner);
    }
    this.body.append(form, actions);
  }

  private buildPlainForm(report: DiagnosticReport): HTMLFormElement {
    const form = document.createElement('form');
    form.className = 'report-form';
    for (const [key, label, value] of [
      ['plain-findings', '影像所见', report.findings],
      ['plain-impression', '诊断意见', report.impression],
      ['plain-recommendation', '建议', report.recommendation ?? ''],
    ] as const) {
      const wrapper = document.createElement('label');
      wrapper.textContent = label;
      const textarea = document.createElement('textarea');
      textarea.dataset.plain = key;
      textarea.value = value;
      wrapper.append(textarea);
      form.append(wrapper);
    }
    return form;
  }

  private buildStructuredForm(payload: StructuredPayload): HTMLFormElement {
    const form = document.createElement('form');
    form.className = 'report-form';
    for (const section of payload.structure.sections) {
      const fieldset = document.createElement('fieldset');
      const legend = document.createElement('legend');
      legend.textContent = section.title;
      fieldset.append(legend);
      for (const field of section.fields) {
        const key = `${section.id}.${field.id}`;
        if (field.kind === 'text') {
          const wrapper = document.createElement('label');
          wrapper.textContent = `${field.label}${field.required ? ' *' : ''}`;
          const textarea = document.createElement('textarea');
          textarea.dataset.key = key;
          textarea.dataset.kind = 'text';
          const value = payload.values[key];
          if (typeof value === 'string') textarea.value = value;
          wrapper.append(textarea);
          fieldset.append(wrapper);
        } else if (field.kind === 'number') {
          const wrapper = document.createElement('label');
          wrapper.textContent = `${field.label}${field.required ? ' *' : ''}${field.unit ? ` (${field.unit})` : ''}`;
          const input = document.createElement('input');
          input.type = 'number';
          input.step = 'any';
          input.dataset.key = key;
          input.dataset.kind = 'number';
          if (field.min != null) input.min = String(field.min);
          if (field.max != null) input.max = String(field.max);
          const value = payload.values[key] as NumberValue | undefined;
          if (value?.value != null) input.value = String(value.value);
          wrapper.append(input);
          fieldset.append(wrapper);
        } else {
          const wrapper = document.createElement('div');
          wrapper.className = 'report-choice';
          const label = document.createElement('span');
          label.className = 'report-choice-label';
          label.textContent = `${field.label}${field.required ? ' *' : ''}`;
          wrapper.append(label);
          const group = document.createElement('div');
          const expandsOption = (field.options ?? []).find((option) => option.expands);
          for (const option of field.options ?? []) {
            const optionWrapper = document.createElement('label');
            const radio = document.createElement('input');
            radio.type = 'radio';
            radio.name = key;
            radio.value = option.id;
            radio.dataset.key = key;
            radio.dataset.kind = 'choice';
            const current = payload.values[key] as ChoiceValue | undefined;
            if (current?.choice === option.id) radio.checked = true;
            optionWrapper.append(radio, document.createTextNode(option.label));
            group.append(optionWrapper);
          }
          wrapper.append(group);
          if (expandsOption) {
            const description = document.createElement('textarea');
            description.dataset.expandsFor = key;
            description.placeholder = '异常描述（选择「异常」后填写）';
            const current = payload.values[key] as ChoiceValue | undefined;
            const isExpanded = current?.choice === expandsOption.id;
            description.value = isExpanded ? (current?.description ?? '') : '';
            description.hidden = !isExpanded;
            for (const radio of group.querySelectorAll<HTMLInputElement>('input[type="radio"]')) {
              radio.addEventListener('change', () => {
                description.hidden = radio.checked && radio.value === expandsOption.id
                  ? false
                  : true;
                if (description.hidden) description.value = '';
              });
            }
            wrapper.append(description);
          }
          fieldset.append(wrapper);
        }
      }
      form.append(fieldset);
    }
    return form;
  }

  private collectStructuredValues(form: HTMLFormElement, payload: StructuredPayload): StructuredPayload {
    const values: Record<string, unknown> = {};
    for (const textarea of form.querySelectorAll<HTMLTextAreaElement>('textarea[data-kind="text"]')) {
      const key = textarea.dataset.key as string;
      values[key] = textarea.value;
    }
    for (const input of form.querySelectorAll<HTMLInputElement>('input[data-kind="number"]')) {
      const key = input.dataset.key as string;
      if (input.value.trim() !== '') values[key] = { value: Number(input.value) };
    }
    for (const section of payload.structure.sections) {
      for (const field of section.fields) {
        if (field.kind !== 'choice') continue;
        const key = `${section.id}.${field.id}`;
        const checked = form.querySelector<HTMLInputElement>(`input[type="radio"][name="${key}"]:checked`);
        if (!checked) continue;
        const description = form
          .querySelector<HTMLTextAreaElement>(`textarea[data-expands-for="${key}"]`)
          ?.value.trim();
        values[key] = description
          ? { choice: checked.value, description }
          : { choice: checked.value };
      }
    }
    return { ...payload, values };
  }

  private async save(report: DiagnosticReport, form: HTMLFormElement): Promise<void> {
    this.clearError();
    let payload: StructuredPayload | null = null;
    let findings: string;
    let impression: string;
    let recommendation: string | null = null;
    if (report.template_payload) {
      payload = this.collectStructuredValues(form, report.template_payload);
      const validation = validatePayload(payload);
      if (!validation.ok) {
        this.showError(`校验未通过：${validation.errors.slice(0, 4).join('；')}`);
        return;
      }
      const rendered = renderReportText(payload);
      findings = rendered.findings;
      impression = rendered.impression;
      recommendation = rendered.recommendation || null;
      if (!findings.trim() || !impression.trim()) {
        this.showError('影像所见和诊断意见不能为空');
        return;
      }
    } else {
      findings = form.querySelector<HTMLTextAreaElement>('textarea[data-plain="plain-findings"]')?.value ?? '';
      impression = form.querySelector<HTMLTextAreaElement>('textarea[data-plain="plain-impression"]')?.value ?? '';
      recommendation = form.querySelector<HTMLTextAreaElement>('textarea[data-plain="plain-recommendation"]')?.value || null;
      if (!findings.trim() || !impression.trim()) {
        this.showError('影像所见和诊断意见不能为空');
        return;
      }
    }
    try {
      const updated = await updateReportDraft(
        report.id,
        report.revision,
        findings,
        impression,
        recommendation,
        payload,
      );
      this.report = updated;
      this.showSuccess(`草稿已保存（第 ${updated.revision} 版）`);
      this.renderEditor(updated);
    } catch (error) {
      this.showError(errorMessage(error));
      await this.refresh();
    }
  }

  private async sign(report: DiagnosticReport, form: HTMLFormElement): Promise<void> {
    this.clearError();
    // 签发前先保存当前表单，保证三列文本由 payload 重渲染（I2）
    await this.save(report, form);
    const current = this.report;
    if (!current || (current.status !== 'draft' && current.status !== 'amending')) return;
    if (!window.confirm('确认签发？签发后报告不可直接修改，需发起修订。')) return;
    try {
      await signReport(current.id, current.revision);
      await this.refresh();
      this.showSuccess('报告已签发，版本历史已更新');
    } catch (error) {
      this.showError(errorMessage(error));
      await this.refresh();
    }
  }

  private renderSigned(report: DiagnosticReport): void {
    this.body.replaceChildren();
    const pre = document.createElement('pre');
    pre.className = 'report-text';
    pre.textContent = [report.findings, report.impression, report.recommendation]
      .filter(Boolean)
      .join('\n\n');
    this.body.append(pre);

    const actions = document.createElement('div');
    actions.className = 'report-actions';
    const reasonInput = document.createElement('input');
    reasonInput.type = 'text';
    reasonInput.placeholder = '修订原因（必填）';
    reasonInput.className = 'report-amend-reason';
    const amend = document.createElement('button');
    amend.type = 'button';
    amend.className = 'command-button';
    amend.textContent = '发起修订';
    amend.addEventListener('click', () => void this.amend(reasonInput.value));
    actions.append(reasonInput, amend);
    this.body.append(actions);

    const heading = document.createElement('div');
    heading.className = 'report-versions-heading';
    const strong = document.createElement('strong');
    strong.textContent = '版本历史';
    heading.append(strong);
    this.body.append(heading);
    const list = document.createElement('ol');
    list.className = 'report-versions';
    for (const version of this.versions) {
      const item = document.createElement('li');
      const title = document.createElement('summary');
      const detail = document.createElement('details');
      const label = [
        `v${version.version_number}`,
        new Date(version.signed_at).toLocaleString(),
        version.amendment_reason ? `修订：${version.amendment_reason}` : '首次签发',
      ].join(' · ');
      title.textContent = label;
      const content = document.createElement('pre');
      content.className = 'report-text';
      content.textContent = [version.findings, version.impression, version.recommendation]
        .filter(Boolean)
        .join('\n\n');
      detail.append(title, content);
      item.append(detail);
      list.append(item);
    }
    this.body.append(list);
  }

  private async amend(reason: string): Promise<void> {
    if (!this.report) return;
    if (!reason.trim()) {
      this.showError('修订原因不能为空');
      return;
    }
    this.clearError();
    try {
      const amended = await beginReportAmendment(this.report.id, reason.trim());
      this.report = amended;
      this.renderEditor(amended);
    } catch (error) {
      this.showError(errorMessage(error));
      await this.refresh();
    }
  }

  private async claim(): Promise<void> {
    if (!this.workItem) return;
    this.clearError();
    try {
      await claimWorkItem(this.workItem.id, this.workItem.revision);
      await this.refresh();
      this.showSuccess('已领取任务，可以开始撰写报告');
    } catch (error) {
      this.showError(errorMessage(error));
      await this.refresh();
    }
  }

  private async release(): Promise<void> {
    if (!this.workItem) return;
    this.clearError();
    try {
      await releaseWorkItem(this.workItem.id, this.workItem.revision);
      await this.refresh();
      this.showSuccess('已释放任务');
    } catch (error) {
      this.showError(errorMessage(error));
      await this.refresh();
    }
  }
}

function errorMessage(error: unknown): string {
  if (typeof error === 'string') return error;
  if (error instanceof Error) return error.message;
  return String(error);
}

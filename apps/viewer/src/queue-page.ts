import { listQueueStudies, listStudySeries, type QueueStudyFilters } from './api';
import type { QueueStudyRow, RemoteSeriesSummary } from './types';
import { Edit3, createIcons } from 'lucide';

const PAGE_SIZE = 50;

export interface QueuePageOptions {
  openStudy: (
    row: QueueStudyRow,
    seriesUid: string,
    series: RemoteSeriesSummary[],
  ) => Promise<boolean>;
  recommendSeries: (series: RemoteSeriesSummary[]) => RemoteSeriesSummary | null;
  canEditTags: () => boolean;
  editStudyTags: (row: QueueStudyRow) => Promise<void>;
  canCreateExamRequestForStudy: () => boolean;
  createExamRequestForStudy: (row: QueueStudyRow) => void;
  canReturnToViewer: () => boolean;
}

type QueueSort = NonNullable<QueueStudyFilters['sort']>;

const STATUS_LABELS: Record<string, string> = {
  pending: '待书写',
  writing: '书写中',
  locked: '已锁定',
  submitted: '待审核',
  under_review: '审核中',
  signed: '已签发',
};

const required = <T extends HTMLElement>(id: string): T => {
  const element = document.getElementById(id);
  if (!element) throw new Error(`缺少队列页面元素: ${id}`);
  return element as T;
};

const text = (value: string | null | undefined, fallback = '--'): string => {
  const trimmed = value?.trim();
  return trimmed ? trimmed : fallback;
};

const formatPersonName = (value: string | null): string =>
  value?.split('=')[0].split('^').filter(Boolean).join(' ') ?? '--';

const formatDate = (value: string | null): string => value ?? '--';

const formatTime = (value: string | null): string => {
  if (!value) return '';
  return value.slice(0, 8);
};

const statusLabel = (status: string): string => STATUS_LABELS[status] ?? status;

export class QueuePage {
  private readonly root = required<HTMLElement>('queue-page');
  private readonly workspace = required<HTMLElement>('workspace');
  private readonly seriesNavigator = document.getElementById('series-navigator');
  private readonly body = required<HTMLElement>('queue-body');
  private readonly status = required<HTMLElement>('queue-status');
  private readonly count = required<HTMLElement>('queue-count');
  private readonly pageLabel = required<HTMLElement>('queue-page-label');
  private readonly form = required<HTMLFormElement>('queue-filters');
  private readonly query = required<HTMLInputElement>('queue-query');
  private readonly dateFrom = required<HTMLInputElement>('queue-date-from');
  private readonly dateTo = required<HTMLInputElement>('queue-date-to');
  private readonly modality = required<HTMLSelectElement>('queue-modality');
  private readonly bodyPart = required<HTMLInputElement>('queue-body-part');
  private readonly reportStatus = required<HTMLSelectElement>('queue-report-status');
  private readonly institution = required<HTMLInputElement>('queue-institution');
  private readonly backButton = required<HTMLButtonElement>('queue-back');
  private readonly refreshButton = required<HTMLButtonElement>('queue-refresh');
  private readonly previousButton = required<HTMLButtonElement>('queue-previous');
  private readonly nextButton = required<HTMLButtonElement>('queue-next');
  private readonly openStudy: QueuePageOptions['openStudy'];
  private readonly recommendSeries: QueuePageOptions['recommendSeries'];
  private readonly canEditTags: QueuePageOptions['canEditTags'];
  private readonly editStudyTags: QueuePageOptions['editStudyTags'];
  private readonly canCreateExamRequestForStudy: QueuePageOptions['canCreateExamRequestForStudy'];
  private readonly createExamRequestForStudy: QueuePageOptions['createExamRequestForStudy'];
  private readonly canReturnToViewer: QueuePageOptions['canReturnToViewer'];
  private page = 0;
  private hasNext = false;
  private loading = false;
  private opening = false;
  private opened = false;
  private sort: QueueSort = 'study_date';
  private order: 'asc' | 'desc' = 'desc';
  private selectedStudyUid: string | null = null;

  constructor(options: QueuePageOptions) {
    this.openStudy = options.openStudy;
    this.recommendSeries = options.recommendSeries;
    this.canEditTags = options.canEditTags;
    this.editStudyTags = options.editStudyTags;
    this.canCreateExamRequestForStudy = options.canCreateExamRequestForStudy;
    this.createExamRequestForStudy = options.createExamRequestForStudy;
    this.canReturnToViewer = options.canReturnToViewer;
    this.form.addEventListener('submit', (event) => {
      event.preventDefault();
      this.page = 0;
      void this.load();
    });
    this.backButton.addEventListener('click', () => this.close());
    this.refreshButton.addEventListener('click', () => void this.load());
    this.previousButton.addEventListener('click', () => {
      if (this.page === 0 || this.loading) return;
      this.page -= 1;
      void this.load();
    });
    this.nextButton.addEventListener('click', () => {
      if (!this.hasNext || this.loading) return;
      this.page += 1;
      void this.load();
    });
    for (const header of this.root.querySelectorAll<HTMLButtonElement>('[data-queue-sort]')) {
      header.addEventListener('click', () => {
        const requested = header.dataset.queueSort as QueueSort | undefined;
        if (!requested) return;
        if (this.sort === requested) this.order = this.order === 'desc' ? 'asc' : 'desc';
        else {
          this.sort = requested;
          this.order = requested === 'study_date' ? 'desc' : 'asc';
        }
        this.page = 0;
        this.updateSortIndicators();
        void this.load();
      });
    }
  }

  open(): void {
    this.opened = true;
    this.root.hidden = false;
    this.workspace.hidden = true;
    if (this.seriesNavigator) this.seriesNavigator.hidden = true;
    this.updateSortIndicators();
    this.backButton.hidden = !this.canReturnToViewer();
    (this.backButton.hidden ? this.refreshButton : this.backButton).focus();
    void this.load();
  }

  close(): void {
    const wasOpen = this.opened;
    this.opened = false;
    this.root.hidden = true;
    this.workspace.hidden = false;
    if (this.seriesNavigator) this.seriesNavigator.hidden = false;
    if (wasOpen) document.getElementById('queue-btn')?.focus();
  }

  isOpen(): boolean {
    return this.opened;
  }

  refresh(): void {
    if (this.opened) void this.load();
  }

  private filters(): QueueStudyFilters {
    return {
      query: this.query.value.trim(),
      modality: this.modality.value,
      bodyPart: this.bodyPart.value.trim(),
      reportStatus: this.reportStatus.value,
      institution: this.institution.value.trim(),
      dateFrom: this.dateFrom.value,
      dateTo: this.dateTo.value,
      sort: this.sort,
      order: this.order,
    };
  }

  private async load(): Promise<void> {
    if (this.loading) return;
    this.loading = true;
    this.refreshButton.disabled = true;
    this.previousButton.disabled = true;
    this.nextButton.disabled = true;
    this.root.setAttribute('aria-busy', 'true');
    this.status.textContent = '正在加载检查...';
    try {
      const rows = await listQueueStudies(this.filters(), PAGE_SIZE + 1, this.page * PAGE_SIZE);
      this.hasNext = rows.length > PAGE_SIZE;
      this.render(rows.slice(0, PAGE_SIZE));
      this.status.textContent = rows.length === 0 ? '没有匹配的检查' : '';
    } catch (error) {
      this.body.replaceChildren();
      this.count.textContent = '';
      this.status.textContent = error instanceof Error ? error.message : String(error);
      this.hasNext = false;
    } finally {
      this.loading = false;
      this.refreshButton.disabled = false;
      this.previousButton.disabled = this.page === 0;
      this.nextButton.disabled = !this.hasNext;
      this.pageLabel.textContent = `第 ${this.page + 1} 页`;
      if (!this.opening) this.root.removeAttribute('aria-busy');
    }
  }

  private render(rows: QueueStudyRow[]): void {
    this.body.replaceChildren(...rows.map((row) => this.renderRow(row)));
    this.count.textContent = rows.length ? `本页 ${rows.length} 项` : '本页 0 项';
    this.pageLabel.textContent = `第 ${this.page + 1} 页`;
    this.previousButton.disabled = this.page === 0;
    this.nextButton.disabled = !this.hasNext;
    createIcons({ icons: { Edit3 } });
  }

  private renderRow(row: QueueStudyRow): HTMLTableRowElement {
    const element = document.createElement('tr');
    element.dataset.studyUid = row.study_uid;
    element.tabIndex = 0;
    if (row.study_uid === this.selectedStudyUid) element.classList.add('is-selected');
    element.addEventListener('click', () => {
      this.selectedStudyUid = row.study_uid;
      for (const item of this.body.querySelectorAll('tr')) item.classList.remove('is-selected');
      element.classList.add('is-selected');
    });
    element.addEventListener('keydown', (event) => {
      if (event.key === 'Enter') {
        event.preventDefault();
        void this.openRow(row);
      }
    });
    element.addEventListener('dblclick', (event) => {
      event.preventDefault();
      void this.openRow(row);
    });

    const patient = document.createElement('td');
    patient.className = 'queue-patient-cell';
    const patientName = document.createElement('strong');
    patientName.textContent = formatPersonName(row.patient_name);
    const patientId = document.createElement('small');
    patientId.textContent = text(row.patient_id);
    patient.append(patientName, patientId);

    const date = document.createElement('td');
    date.className = 'queue-date-cell';
    const dateText = document.createElement('strong');
    dateText.textContent = formatDate(row.study_date);
    const timeText = document.createElement('small');
    timeText.textContent = formatTime(row.study_time);
    date.append(dateText, timeText);

    const modality = document.createElement('td');
    modality.className = 'queue-modality-cell';
    for (const value of row.modalities.length ? row.modalities : ['--']) {
      const badge = document.createElement('span');
      badge.className = 'queue-modality-badge';
      badge.textContent = value;
      modality.append(badge);
    }

    const study = document.createElement('td');
    study.className = 'queue-study-cell';
    const bodyParts = document.createElement('strong');
    bodyParts.textContent = row.body_parts.length ? row.body_parts.join(' / ') : '部位未记录';
    const description = document.createElement('small');
    description.textContent = text(row.description, '检查描述未记录');
    study.append(bodyParts, description);
    if (row.has_exam_request) {
      const requestBadge = document.createElement('span');
      requestBadge.className = 'queue-exam-request-badge';
      requestBadge.textContent = '有申请单';
      study.append(requestBadge);
    }

    const status = document.createElement('td');
    const statusBadge = document.createElement('span');
    statusBadge.className = 'worklist-report-badge';
    statusBadge.dataset.status = row.report_status;
    statusBadge.textContent = statusLabel(row.report_status);
    status.append(statusBadge);

    const institution = document.createElement('td');
    institution.className = 'queue-institution-cell';
    institution.textContent = text(row.institution_name, '来源未记录');

    const seriesCount = document.createElement('td');
    seriesCount.className = 'queue-count-cell';
    seriesCount.textContent = String(row.series_count);

    const actions = document.createElement('td');
    actions.className = 'queue-action-cell';
    if (row.report_status === 'submitted' || row.report_status === 'under_review') {
      const review = document.createElement('button');
      review.type = 'button';
      review.className = 'queue-edit-button queue-review-button';
      review.textContent = row.report_status === 'submitted' ? '去审核' : '查看审核';
      review.addEventListener('click', (event) => {
        event.stopPropagation();
        void this.openRow(row);
      });
      actions.append(review);
    }
    if (this.canCreateExamRequestForStudy() && !row.has_exam_request) {
      const request = document.createElement('button');
      request.type = 'button';
      request.className = 'queue-edit-button primary';
      request.textContent = '开申请单';
      request.addEventListener('click', (event) => {
        event.stopPropagation();
        this.createExamRequestForStudy(row);
      });
      actions.append(request);
    }
    if (this.canEditTags()) {
      const edit = document.createElement('button');
      edit.type = 'button';
      edit.className = 'queue-edit-button';
      edit.title = '编辑检查 DICOM 标签';
      edit.setAttribute('aria-label', edit.title);
      edit.innerHTML = '<i data-lucide="edit-3"></i><span>编辑标签</span>';
      edit.addEventListener('click', (event) => {
        event.stopPropagation();
        void this.editStudyTags(row);
      });
      actions.append(edit);
    }
    element.append(patient, date, modality, study, status, institution, seriesCount, actions);
    return element;
  }

  private updateSortIndicators(): void {
    for (const header of this.root.querySelectorAll<HTMLButtonElement>('[data-queue-sort]')) {
      const active = header.dataset.queueSort === this.sort;
      header.dataset.active = String(active);
      header.closest('th')?.setAttribute(
        'aria-sort',
        active ? (this.order === 'asc' ? 'ascending' : 'descending') : 'none',
      );
      const indicator = header.querySelector<HTMLElement>('.queue-sort-indicator');
      if (indicator) indicator.textContent = active ? (this.order === 'asc' ? '↑' : '↓') : '';
    }
  }

  private async openRow(row: QueueStudyRow): Promise<void> {
    if (this.loading || this.opening) return;
    this.opening = true;
    this.root.setAttribute('aria-busy', 'true');
    this.status.textContent = '正在准备检查...';
    try {
      const series = await listStudySeries(row.study_uid);
      if (!series.length) {
        this.status.textContent = '该检查没有可打开的序列';
        return;
      }
      const target = this.recommendSeries(series) ?? series[0];
      const opened = await this.openStudy(row, target.series_uid, series);
      if (opened) this.close();
      else this.status.textContent = '打开检查失败';
    } catch (error) {
      this.status.textContent = error instanceof Error ? error.message : String(error);
    } finally {
      this.opening = false;
      if (!this.loading) this.root.removeAttribute('aria-busy');
    }
  }
}

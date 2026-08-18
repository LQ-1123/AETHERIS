import { mockIPC, mockWindows } from '@tauri-apps/api/mocks';
import type { QueueStudyRow, RemoteSeriesSummary, SeriesMetadata, StudySummary } from './types';

type QueueFixture = QueueStudyRow & { authorized: boolean };

export interface QueueAcceptanceCall {
  command: string;
  args: Record<string, unknown>;
}

export interface QueueAcceptanceState {
  calls: QueueAcceptanceCall[];
  queueRequests: QueueAcceptanceCall[];
  openedSeries: { studyUid: string; seriesUid: string } | null;
}

declare global {
  interface Window {
    __queueAcceptance?: QueueAcceptanceState;
  }
}

const STATUSES: QueueStudyRow['report_status'][] = ['pending', 'writing', 'locked', 'signed'];
const MODALITIES = ['CT', 'MR', 'DX', 'US'];
const BODY_PARTS = ['CHEST', 'HEAD', 'ABDOMEN', 'PELVIS'];
const INSTITUTIONS = ['中心医院', '协作医院', '区域影像中心'];

function fixtureRows(): QueueFixture[] {
  const rows = Array.from({ length: 56 }, (_, index): QueueFixture => {
    const day = String(18 - (index % 18)).padStart(2, '0');
    return {
      patient_key: index + 1,
      study_uid: `1.2.840.10008.5.1.4.1.1.2.${index + 1}`,
      patient_id: `QUEUE-${String(index + 1).padStart(4, '0')}`,
      patient_name: `患者${String(index + 1).padStart(2, '0')}^测试`,
      patient_sex: index % 2 ? 'F' : 'M',
      patient_birth_date: '1980-01-01',
      study_date: `2026-08-${day}`,
      study_time: `${String(8 + (index % 10)).padStart(2, '0')}1200`,
      modalities: [MODALITIES[index % MODALITIES.length]],
      description: `${BODY_PARTS[index % BODY_PARTS.length]} 常规检查`,
      body_parts: [BODY_PARTS[index % BODY_PARTS.length]],
      report_status: STATUSES[index % STATUSES.length],
      institution_name: INSTITUTIONS[index % INSTITUTIONS.length],
      series_count: 2 + (index % 3),
      authorized: index !== 55,
    };
  });
  rows[0] = {
    ...rows[0],
    patient_id: 'QUEUE-SPECIAL',
    patient_name: '张^明',
    study_date: '2026-08-18',
    study_time: '071500',
    modalities: ['CT'],
    description: 'CHEST 薄层增强检查',
    body_parts: ['CHEST'],
    report_status: 'pending',
    institution_name: '中心医院',
  };
  rows[55] = { ...rows[55], patient_id: 'UNAUTHORIZED', patient_name: '不应显示^设备' };
  return rows;
}

function remoteStudies(patientKey: number): StudySummary[] {
  const row = fixtureRows().find((entry) => entry.patient_key === patientKey) ?? fixtureRows()[0];
  return [
    {
      study_uid: row.study_uid,
      study_date: row.study_date,
      study_time: row.study_time,
      accession_number: `ACC-${patientKey}`,
      study_id: `STUDY-${patientKey}`,
      description: row.description,
      referring_physician: '测试医生',
      modalities: row.modalities,
      series_count: row.series_count,
      instance_count: 84,
      report_status: row.report_status,
    },
    {
      study_uid: `${row.study_uid}.99`,
      study_date: '2025-11-06',
      study_time: '091500',
      accession_number: `OLD-${patientKey}`,
      study_id: `OLD-STUDY-${patientKey}`,
      description: '既往复查',
      referring_physician: '历史医生',
      modalities: ['CT'],
      series_count: 2,
      instance_count: 72,
      report_status: 'signed',
    },
  ];
}

function arg(args: Record<string, unknown>, camel: string, snake: string): string {
  const value = args[camel] ?? args[snake];
  return typeof value === 'string' ? value : '';
}

function sortRows(rows: QueueFixture[], sort: string, order: string): QueueFixture[] {
  const sorted = [...rows].sort((left, right) => {
    const leftValue = sort === 'patient_name'
      ? left.patient_name ?? ''
      : sort === 'modality'
        ? left.modalities[0] ?? ''
        : sort === 'report_status'
          ? left.report_status
          : sort === 'institution'
            ? left.institution_name ?? ''
            : `${left.study_date ?? ''} ${left.study_time ?? ''}`;
    const rightValue = sort === 'patient_name'
      ? right.patient_name ?? ''
      : sort === 'modality'
        ? right.modalities[0] ?? ''
        : sort === 'report_status'
          ? right.report_status
          : sort === 'institution'
            ? right.institution_name ?? ''
            : `${right.study_date ?? ''} ${right.study_time ?? ''}`;
    return leftValue.localeCompare(rightValue, 'zh-CN') || left.study_uid.localeCompare(right.study_uid);
  });
  return order === 'desc' ? sorted.reverse() : sorted;
}

function filteredRows(args: Record<string, unknown>): QueueFixture[] {
  const query = arg(args, 'query', 'query').trim().toLocaleLowerCase();
  const modality = arg(args, 'modality', 'modality').trim();
  const bodyPart = arg(args, 'bodyPart', 'body_part').trim();
  const reportStatus = arg(args, 'reportStatus', 'report_status').trim();
  const institution = arg(args, 'institution', 'institution').trim();
  const dateFrom = arg(args, 'dateFrom', 'date_from').trim();
  const dateTo = arg(args, 'dateTo', 'date_to').trim();
  const rows = fixtureRows().filter((row) => {
    if (!row.authorized) return false;
    const searchable = `${row.patient_id} ${row.patient_name?.replace(/\^/g, ' ') ?? ''}`.toLocaleLowerCase();
    return (!query || searchable.includes(query))
      && (!modality || row.modalities.includes(modality))
      && (!bodyPart || row.body_parts.includes(bodyPart))
      && (!reportStatus || row.report_status === reportStatus)
      && (!institution || row.institution_name === institution)
      && (!dateFrom || (row.study_date ?? '') >= dateFrom)
      && (!dateTo || (row.study_date ?? '') <= dateTo);
  });
  return sortRows(rows, arg(args, 'sort', 'sort') || 'study_date', arg(args, 'order', 'order') || 'desc');
}

function remoteSeries(studyUid: string): RemoteSeriesSummary[] {
  return [
    {
      series_uid: `${studyUid}.scout`,
      series_number: 1,
      modality: 'CT',
      description: 'Scout localizer',
      body_part_examined: 'CHEST',
      protocol_name: null,
      instance_count: 20,
    },
    {
      series_uid: `${studyUid}.axial`,
      series_number: 2,
      modality: 'CT',
      description: 'Original thin axial',
      body_part_examined: 'CHEST',
      protocol_name: 'Routine',
      instance_count: 64,
    },
  ];
}

function remoteMetadata(studyUid: string, seriesUid: string): SeriesMetadata {
  return {
    handle: 7,
    patient: {
      patient_name: '张^明',
      patient_id: 'QUEUE-SPECIAL',
      patient_sex: 'M',
      patient_birth_date: '1980-01-01',
      study_date: '2026-08-18',
      accession_number: null,
      modality: 'CT',
      study_description: 'CHEST 薄层增强检查',
      series_description: 'Original thin axial',
    },
    study_uid: studyUid,
    series_uid: seriesUid,
    active_stack: 0,
    image_stacks: [{ index: 0, label: 'Default', frame_count: 1, rows: 2, cols: 2 }],
    frames: [{
      logical_index: 0,
      frame_key: '0',
      sop_instance_uid: `${seriesUid}.instance`,
      source_frame: 0,
      instance_number: 1,
      rows: 2,
      cols: 2,
      bits_allocated: 8,
      pixel_format: 'rgb8',
      photometric_interpretation: 'RGB',
      cine_rate_fps: null,
      quantitative: { unit: null, suvbw_factor: null, suvbw_status: null },
      laterality: null,
      view_position: null,
      patient_orientation: [],
      position: null,
      orientation: null,
      window_presets: [{ center: 127, width: 255, explanation: null, function: 'LINEAR' }],
      spacing: {
        confidence: 'none',
        source: null,
        description: 'mock',
        row_mm: null,
        col_mm: null,
        column_over_row: 1,
      },
    }],
    warnings: [],
  };
}

export function installQueueAcceptanceMock(): void {
  const state: QueueAcceptanceState = { calls: [], queueRequests: [], openedSeries: null };
  window.__queueAcceptance = state;
  mockWindows('main');
  const invokeHandler = async (command: string, payload?: Record<string, unknown>): Promise<unknown> => {
    const args = (payload ?? {}) as Record<string, unknown>;
    const call = { command, args };
    state.calls.push(call);
    if (command === 'remote_login') {
      return {
        id: 1,
        username: 'admin',
        display_name: '验收管理员',
        role: 'admin',
        institution_id: 1,
        institution_name: '中心医院',
      };
    }
    if (command === 'list_queue_studies') {
      state.queueRequests.push(call);
      const rows = filteredRows(args);
      const limit = Number(args.limit ?? 50);
      const offset = Number(args.offset ?? 0);
      return rows.slice(offset, offset + limit);
    }
    if (command === 'list_patient_studies') return remoteStudies(Number(args.patientId ?? 0));
    if (command === 'list_study_series') return remoteSeries(String(args.studyUid ?? ''));
    if (command === 'open_remote_series') {
      const studyUid = String(args.studyUid ?? '');
      const seriesUid = String(args.seriesUid ?? '');
      state.openedSeries = { studyUid, seriesUid };
      return remoteMetadata(studyUid, seriesUid);
    }
    if (command === 'local_stack_info') return null;
    if (command === 'transform_schema') {
      return {
        manual_tags: [
          { keyword: 'AccessionNumber', tag: '00080050', vr: 'SH', scope: 'study', actions: ['replace', 'empty'] },
          { keyword: 'StudyID', tag: '00200010', vr: 'SH', scope: 'study', actions: ['replace', 'empty'] },
          { keyword: 'StudyDescription', tag: '00081030', vr: 'LO', scope: 'study', actions: ['replace', 'empty'] },
          { keyword: 'ReferringPhysicianName', tag: '00080090', vr: 'PN', scope: 'study', actions: ['replace', 'empty'] },
        ],
      };
    }
    if (command === 'list_window_presets' || command === 'list_shared_annotations') return [];
    if (command === 'list_segmentation_projects' || command === 'list_segmentation_segments') return [];
    if (command === 'list_segmentation_masks' || command === 'list_segmentation_volume') return [];
    if (command === 'list_patients' || command === 'list_report_templates') return [];
    if (command === 'plugin:event|listen') return 1;
    if (command === 'plugin:event|unlisten' || command === 'close_series') return null;
    return null;
  };
  mockIPC(invokeHandler as Parameters<typeof mockIPC>[0]);
  const originalFetch = window.fetch.bind(window);
  window.fetch = ((input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input.toString();
    if (url.startsWith('pacs-frame://')) {
      return Promise.resolve(new Response(new Uint8Array([
        240, 80, 80, 80, 200, 240, 80, 80, 240, 240, 200, 80,
      ]), { status: 200, headers: { 'Content-Type': 'application/octet-stream' } }));
    }
    return originalFetch(input, init);
  }) as typeof window.fetch;
}

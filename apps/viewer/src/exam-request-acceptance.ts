import { mockIPC, mockWindows } from '@tauri-apps/api/mocks';
import type { ExamRequest } from './types';

declare global {
  interface Window {
    __examRequestAcceptance?: {
      calls: Array<{ command: string; args: Record<string, unknown> }>;
    };
  }
}

export function installExamRequestAcceptanceMock(): void {
  const state = {
    calls: [] as Array<{ command: string; args: Record<string, unknown> }>,
    requests: fixtures(),
    queueStudies: queueFixtures(),
    boundStudyUids: new Set<string>(),
  };
  window.__examRequestAcceptance = state;
  mockWindows('main');
  mockIPC((async (command: string, payload?: Record<string, unknown>) => {
    const args = payload ?? {};
    state.calls.push({ command, args });
    if (command === 'local_stack_info') return null;
    if (command === 'plugin:event|listen' || command === 'plugin:event|unlisten') return null;
    if (command === 'remote_login') {
      const users: Record<string, { id: number; display_name: string; role: string }> = {
        'admin.wang': { id: 1, display_name: '王管理员', role: 'admin' },
        'doctor.li': { id: 5, display_name: '李医生', role: 'radiologist' },
        'doctor.zhang': { id: 6, display_name: '张医生', role: 'radiologist' },
        'tech.zhao': { id: 7, display_name: '赵技师', role: 'technician' },
      };
      const user = users[String(args.username)] ?? { id: 8, display_name: '只读用户', role: 'viewer' };
      return { ...user, username: args.username, institution_id: 1, institution_name: '中心医院' };
    }
    if (command === 'remote_logout') return null;
    if (command === 'list_window_presets') return [];
    if (command === 'list_queue_studies') return state.queueStudies.filter((row) => !args.reportStatus || row.report_status === args.reportStatus);
    if (command === 'transform_schema') return { manual_tags: [] };
    if (command === 'list_exam_requests') {
      const status = args.status as string | null;
      return status ? state.requests.filter((entry) => entry.status === status) : state.requests;
    }
    if (command === 'create_exam_request') {
      const created: ExamRequest = {
        id: 'req-new', patient_id: String(args.patientId), patient_name: String(args.patientName),
        patient_birth_date: args.patientBirthDate as string | null, patient_sex: args.patientSex as string | null,
        modality: String(args.modality), body_part: String(args.bodyPart), request_type: String(args.requestType),
        clinical_indication: String(args.clinicalIndication), requested_by_id: 7, requested_by_name: '赵技师',
        requested_at: '2026-08-19T09:10:00+08:00', scheduled_at: args.scheduledAt as string | null,
        status: 'pending', study_uid: null, study_date: null, study_description: null, revision: 1,
        created_at: '2026-08-19T09:10:00+08:00', updated_at: '2026-08-19T09:10:00+08:00',
      };
      state.requests.unshift(created); return created;
    }
    if (command === 'create_exam_request_for_study') {
      const study = state.queueStudies.find((row) => row.study_uid === args.studyUid);
      if (!study) throw new Error('mock study not found');
      const created: ExamRequest = {
        id: 'req-existing-study', patient_id: study.patient_id, patient_name: study.patient_name ?? study.patient_id,
        patient_birth_date: study.patient_birth_date, patient_sex: study.patient_sex,
        modality: String(args.modality), body_part: String(args.bodyPart), request_type: String(args.requestType),
        clinical_indication: String(args.clinicalIndication), requested_by_id: 7, requested_by_name: '赵技师',
        requested_at: '2026-08-19T09:20:00+08:00', scheduled_at: args.scheduledAt as string | null,
        status: 'executed', study_uid: study.study_uid, study_date: study.study_date, study_description: study.description,
        revision: 2, created_at: '2026-08-19T09:20:00+08:00', updated_at: '2026-08-19T09:20:00+08:00',
      };
      study.has_exam_request = true;
      state.requests.unshift(created);
      return created;
    }
    if (command === 'update_exam_request') {
      const item = state.requests.find((entry) => entry.id === args.requestId);
      if (!item || item.status !== 'pending') throw new Error('申请单已执行、版本已变化或不存在');
      item.patient_id = String(args.patientId);
      item.patient_name = String(args.patientName);
      item.patient_birth_date = args.patientBirthDate as string | null;
      item.patient_sex = args.patientSex as string | null;
      item.modality = String(args.modality);
      item.body_part = String(args.bodyPart);
      item.request_type = String(args.requestType);
      item.clinical_indication = String(args.clinicalIndication);
      item.scheduled_at = args.scheduledAt as string | null;
      item.revision += 1;
      item.updated_at = '2026-08-19T09:15:00+08:00';
      return item;
    }
    if (command === 'list_exam_request_study_candidates') {
      const candidate = {
        study_uid: '1.2.840.113619.2.55.3.604688123.20260819.9',
        patient_id: 'P-20260819-016',
        patient_name: '林海',
        study_date: '2026-08-19',
        modalities: ['MR'],
        description: '上腹部 MR 平扫加增强',
      };
      return state.boundStudyUids.has(candidate.study_uid) ? [] : [candidate];
    }
    if (command === 'bind_exam_request') {
      const item = state.requests.find((entry) => entry.id === args.requestId)!;
      const studyUid = String(args.studyUid);
      if (state.boundStudyUids.has(studyUid)) throw new Error('该检查已绑定其他申请单');
      state.boundStudyUids.add(studyUid);
      item.status = 'executed';
      item.study_uid = studyUid;
      item.study_date = '2026-08-19';
      item.study_description = '上腹部 MR 平扫加增强';
      item.revision += 1;
      return item;
    }
    if (command === 'list_devices' || command === 'list_password_reset_requests') return [];
    if (command === 'list_users') return [
      { id: 1, username: 'admin.wang', display_name: '王管理员', role: 'admin', is_active: true, must_change_password: false, last_login_at: null, created_at: '2026-01-01T00:00:00Z' },
      { id: 5, username: 'doctor.li', display_name: '李医生', role: 'radiologist', is_active: true, must_change_password: false, last_login_at: null, created_at: '2026-01-01T00:00:00Z' },
      { id: 7, username: 'tech.zhao', display_name: '赵技师', role: 'technician', is_active: true, must_change_password: false, last_login_at: null, created_at: '2026-01-01T00:00:00Z' },
    ];
    if (command === 'list_user_permissions' || command === 'list_user_device_grants') return [];
    if (command === 'workload_report' && String(args.dateFrom).startsWith('2030-')) return [];
    if (command === 'workload_report') return [
      { user_id: 5, username: 'doctor.li', display_name: '李医生', role: 'radiologist', draft_reports: 3, submitted_reports: 2, under_review_reports: 1, signed_status_reports: 18, signed_reports: 21, reviews_completed: 12, reviewer_modifications: 2, exam_requests_created: 0 },
      { user_id: 7, username: 'tech.zhao', display_name: '赵技师', role: 'technician', draft_reports: 0, submitted_reports: 0, under_review_reports: 0, signed_status_reports: 0, signed_reports: 0, reviews_completed: 0, reviewer_modifications: 0, exam_requests_created: 34 },
    ];
    throw new Error(`申请单验收未模拟命令: ${command}`);
  }) as Parameters<typeof mockIPC>[0]);
}

function fixtures(): ExamRequest[] {
  return [
    { id: 'req-1', patient_id: 'P-20260819-008', patient_name: '陈晓华', patient_birth_date: '1972-04-16', patient_sex: 'F', modality: 'CT', body_part: '胸部', request_type: '增强', clinical_indication: '间断胸痛一周，伴活动后气促，排除肺栓塞。', requested_by_id: 7, requested_by_name: '赵技师', requested_at: '2026-08-19T08:42:00+08:00', scheduled_at: '2026-08-19T10:30:00+08:00', status: 'pending', study_uid: null, study_date: null, study_description: null, revision: 1, created_at: '2026-08-19T08:42:00+08:00', updated_at: '2026-08-19T08:42:00+08:00' },
    { id: 'req-2', patient_id: 'P-20260819-003', patient_name: '周建国', patient_birth_date: '1965-11-02', patient_sex: 'M', modality: 'MR', body_part: '头颅', request_type: '平扫', clinical_indication: '反复头晕伴左侧肢体麻木两天。', requested_by_id: 7, requested_by_name: '赵技师', requested_at: '2026-08-19T08:08:00+08:00', scheduled_at: null, status: 'executed', study_uid: '1.2.840.113619.2.55.3.604688123.2', study_date: '2026-08-19', study_description: '头颅 MR 平扫', revision: 2, created_at: '2026-08-19T08:08:00+08:00', updated_at: '2026-08-19T08:55:00+08:00' },
    { id: 'req-3', patient_id: 'P-20260818-021', patient_name: '孙梅', patient_birth_date: '1981-02-18', patient_sex: 'F', modality: 'DX', body_part: '右膝', request_type: '平扫', clinical_indication: '扭伤后右膝疼痛、活动受限。', requested_by_id: 7, requested_by_name: '赵技师', requested_at: '2026-08-18T15:20:00+08:00', scheduled_at: null, status: 'completed', study_uid: '1.2.840.113619.2.55.3.604688123.3', study_date: '2026-08-18', study_description: '右膝关节正侧位', revision: 3, created_at: '2026-08-18T15:20:00+08:00', updated_at: '2026-08-18T17:12:00+08:00' },
  ];
}

function queueFixtures() {
  return [
    {
      patient_key: 101,
      study_uid: '1.2.840.113619.2.55.3.604688123.20260819.1',
      patient_id: 'P-20260819-008',
      patient_name: '陈晓华',
      patient_sex: 'F',
      patient_birth_date: '1972-04-16',
      study_date: '2026-08-19',
      study_time: '090000',
      modalities: ['CT'],
      description: '胸部薄层 CT',
      body_parts: ['胸部'],
      report_status: 'pending' as const,
      has_exam_request: false,
      institution_name: '中心医院',
      series_count: 2,
    },
    {
      patient_key: 102,
      study_uid: '1.2.840.113619.2.55.3.604688123.20260819.2',
      patient_id: 'P-20260819-003',
      patient_name: '周建国',
      patient_sex: 'M',
      patient_birth_date: '1965-11-02',
      study_date: '2026-08-19',
      study_time: '083000',
      modalities: ['MR'],
      description: '头颅 MR 平扫',
      body_parts: ['头颅'],
      report_status: 'writing' as const,
      has_exam_request: true,
      institution_name: '中心医院',
      series_count: 4,
    },
  ];
}

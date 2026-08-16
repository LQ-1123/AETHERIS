import type {
  AdminUser,
  AiModelDescriptor,
  AiPluginConfiguration,
  AiSegmentationResult,
  AiCatalog,
  ClinicalWorkItem,
  DiagnosticReport,
  DicomDevice,
  DicomRevision,
  ImportTransferResponse,
  PatientSummary,
  MprMetadata,
  MprPlane,
  MprProjectionMode,
  ReportTemplate,
  ReportVersion,
  RoiStatistics,
  RemoteSeriesSummary,
  RemoteUser,
  CreatedSegmentationProject,
  SegmentationMaskRecord,
  SegmentationProject,
  SegmentationSegment,
  LocalDicomNode,
  LegalHold,
  LifecycleEvent,
  LifecycleJob,
  LifecyclePolicy,
  LifecyclePolicyInput,
  LifecycleStudy,
  LifecycleSummary,
  RoutableSeries,
  ObservedDicomPeer,
  RouteDelivery,
  RouteDestination,
  RouteDestinationInput,
  RouteRule,
  RouteRuleInput,
  PurgeRequest,
  SeriesMetadata,
  SeriesSourceEntry,
  SharedAnnotationRecord,
  StructuredPayload,
  StudySummary,
  TagRuleInput,
  TransformJob,
  TransformPreviewResponse,
  TransformSchema,
  TransformTargetType,
  VoiFunction,
  UserWindowPreset,
} from './types';

type InvokeFn = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

let invokeCache: InvokeFn | null = null;

async function getInvoke(): Promise<InvokeFn> {
  if (!invokeCache) {
    const module = await import('@tauri-apps/api/core');
    invokeCache = module.invoke as InvokeFn;
  }
  return invokeCache;
}

export async function chooseDicomFiles(): Promise<string[] | null> {
  const { open } = await import('@tauri-apps/plugin-dialog');
  const selected = await open({
    multiple: true,
    directory: false,
    filters: [{ name: 'DICOM', extensions: ['dcm', 'dicom', '*'] }],
  });
  if (!selected) return null;
  return Array.isArray(selected) ? selected : [selected];
}

export async function chooseImportFiles(): Promise<string[] | null> {
  const { open } = await import('@tauri-apps/plugin-dialog');
  const selected = await open({ multiple: true, directory: false,
    filters: [{ name: 'DICOM 或归档', extensions: ['dcm', 'dicom', 'zip', 'rar', '*'] }] });
  if (!selected) return null;
  return Array.isArray(selected) ? selected : [selected];
}

export async function chooseImportFolder(): Promise<string[] | null> {
  const { open } = await import('@tauri-apps/plugin-dialog');
  const selected = await open({ multiple: false, directory: true });
  return typeof selected === 'string' ? [selected] : null;
}

export async function importToPacs(paths: string[]): Promise<ImportTransferResponse> {
  const invoke = await getInvoke(); return invoke<ImportTransferResponse>('import_to_pacs', { paths });
}

export async function exportFromPacs(studyUid: string, seriesUid?: string): Promise<Record<string, unknown> | null> {
  const { save } = await import('@tauri-apps/plugin-dialog');
  const destination = await save({ defaultPath: `${seriesUid ? `series-${seriesUid}` : `study-${studyUid}`}.zip`,
    filters: [{ name: 'ZIP', extensions: ['zip'] }] });
  if (!destination) return null;
  const invoke = await getInvoke();
  return invoke('export_from_pacs', { studyUid, seriesUid, destination });
}

export async function cancelTransfer(kind: 'imports' | 'exports'): Promise<void> {
  const invoke = await getInvoke(); await invoke('cancel_transfer', { kind });
}

async function routerGet<T>(path: string): Promise<T> {
  const invoke = await getInvoke();
  return invoke<T>('router_get', { path });
}

async function routerWrite<T>(method: 'POST' | 'PUT', path: string, body: unknown): Promise<T> {
  const invoke = await getInvoke();
  return invoke<T>('router_write', { method, path, body });
}

async function routerDelete(path: string): Promise<void> {
  const invoke = await getInvoke();
  await invoke('router_delete', { path });
}

export const listRouteDestinations = (): Promise<RouteDestination[]> => routerGet('destinations');
export const getLocalDicomNode = (): Promise<LocalDicomNode> => routerGet('node');
export const listObservedDicomPeers = (): Promise<ObservedDicomPeer[]> => routerGet('peers?limit=200');
export const listRoutableSeries = (): Promise<RoutableSeries[]> => routerGet('series?limit=300');
export const saveRouteDestination = (input: RouteDestinationInput, id?: string): Promise<RouteDestination> =>
  routerWrite(id ? 'PUT' : 'POST', id ? `destinations/${id}` : 'destinations', input);
export const deleteRouteDestination = (id: string): Promise<void> => routerDelete(`destinations/${id}`);
export const testRouteDestination = (id: string): Promise<RouteDestination> =>
  routerWrite('POST', `destinations/${id}/test`, {});
export const approveRouteDestination = (id: string): Promise<RouteDestination> =>
  routerWrite('POST', `destinations/${id}/approve`, {});
export const listRouteRules = (): Promise<RouteRule[]> => routerGet('rules');
export const saveRouteRule = (input: RouteRuleInput, id?: string): Promise<RouteRule> =>
  routerWrite(id ? 'PUT' : 'POST', id ? `rules/${id}` : 'rules', input);
export const deleteRouteRule = (id: string): Promise<void> => routerDelete(`rules/${id}`);
export const listRouteDeliveries = (): Promise<RouteDelivery[]> => routerGet('deliveries?limit=200');
export const replayRouteDelivery = (id: string): Promise<{ job_id: string }> =>
  routerWrite('POST', `deliveries/${id}/replay`, {});
export const sendRouteScope = (
  destinationId: string,
  studyInstanceUid: string,
  seriesInstanceUid?: string,
): Promise<{ queued: number; skipped_as_duplicate: number; job_ids: string[] }> =>
  routerWrite('POST', 'send', {
    destination_id: destinationId,
    study_instance_uid: studyInstanceUid,
    series_instance_uid: seriesInstanceUid || null,
  });

async function lifecycleGet<T>(path: string): Promise<T> {
  const invoke = await getInvoke();
  return invoke<T>('lifecycle_get', { path });
}

async function lifecycleWrite<T>(method: 'POST' | 'PUT', path: string, body: unknown = {}): Promise<T> {
  const invoke = await getInvoke();
  return invoke<T>('lifecycle_write', { method, path, body });
}

async function lifecycleDelete(path: string): Promise<void> {
  const invoke = await getInvoke();
  await invoke('lifecycle_delete', { path });
}

export const getLifecycleSummary = (): Promise<LifecycleSummary> => lifecycleGet('summary');
export const listLifecycleJobs = (): Promise<LifecycleJob[]> => lifecycleGet('jobs');
export const listLifecyclePolicies = (): Promise<LifecyclePolicy[]> => lifecycleGet('policies');
export const createLifecyclePolicy = (input: LifecyclePolicyInput): Promise<LifecyclePolicy> =>
  lifecycleWrite('POST', 'policies', input);
export const updateLifecyclePolicy = (id: string, input: LifecyclePolicyInput): Promise<LifecyclePolicy> =>
  lifecycleWrite('PUT', `policies/${id}`, input);
export const deleteLifecyclePolicy = (id: string): Promise<void> => lifecycleDelete(`policies/${id}`);
export const previewLifecyclePolicy = (id: string): Promise<Record<string, unknown>> =>
  lifecycleWrite('POST', `policies/${id}/preview`);
export const runLifecyclePolicy = (id: string): Promise<Record<string, unknown>> =>
  lifecycleWrite('POST', `policies/${id}/run`);
export const listLifecycleStudies = (): Promise<LifecycleStudy[]> => lifecycleGet('studies');
export const moveLifecycleStudy = (studyUid: string, targetTier: 'cold' | 'quarantine'): Promise<Record<string, unknown>> =>
  lifecycleWrite('POST', `studies/${encodeURIComponent(studyUid)}/move`, { target_tier: targetTier });
export const restoreLifecycleStudy = (studyUid: string): Promise<Record<string, unknown>> =>
  lifecycleWrite('POST', `studies/${encodeURIComponent(studyUid)}/restore`);
export const listLegalHolds = (): Promise<LegalHold[]> => lifecycleGet('holds');
export const createLegalHold = (studyUid: string, reason: string): Promise<LegalHold> =>
  lifecycleWrite('POST', `studies/${encodeURIComponent(studyUid)}/holds`, { reason, expires_at: null });
export const releaseLegalHold = (id: string): Promise<void> => lifecycleDelete(`holds/${id}`);
export const listPurgeRequests = (): Promise<PurgeRequest[]> => lifecycleGet('purge-requests');
export const createPurgeRequest = (studyUid: string, reason: string): Promise<PurgeRequest> =>
  lifecycleWrite('POST', 'purge-requests', { study_instance_uid: studyUid, reason });
export const approvePurgeRequest = (id: string, graceHours: number): Promise<PurgeRequest> =>
  lifecycleWrite('POST', `purge-requests/${id}/approve`, { grace_hours: graceHours });
export const rejectPurgeRequest = (id: string): Promise<PurgeRequest> =>
  lifecycleWrite('POST', `purge-requests/${id}/reject`);
export const listLifecycleEvents = (): Promise<LifecycleEvent[]> => lifecycleGet('events');

export async function chooseCaCertificate(): Promise<string | null> {
  const { open } = await import('@tauri-apps/plugin-dialog');
  const selected = await open({
    multiple: false,
    directory: false,
    filters: [{ name: 'CA certificate', extensions: ['crt', 'pem', 'cer'] }],
  });
  return typeof selected === 'string' ? selected : null;
}

export async function openSeries(paths: string[]): Promise<SeriesMetadata> {
  const invoke = await getInvoke();
  return invoke<SeriesMetadata>('open_series', { paths });
}

export async function listAiModels(): Promise<AiModelDescriptor[]> {
  const invoke = await getInvoke();
  return invoke<AiModelDescriptor[]>('list_ai_models');
}

export async function listAiCatalog(): Promise<AiCatalog> {
  const invoke = await getInvoke();
  return invoke<AiCatalog>('list_ai_catalog');
}

export async function refreshAiPlugins(): Promise<AiCatalog> {
  const invoke = await getInvoke();
  return invoke<AiCatalog>('refresh_ai_plugins');
}

export async function chooseAiPluginFolder(): Promise<string | null> {
  const { open } = await import('@tauri-apps/plugin-dialog');
  const selected = await open({ multiple: false, directory: true });
  return typeof selected === 'string' ? selected : null;
}

export async function checkAiPlugin(name: string, path: string): Promise<AiCatalog> {
  const invoke = await getInvoke();
  return invoke<AiCatalog>('check_ai_plugin', { name, path });
}

export async function addAiPlugin(name: string, path: string): Promise<AiCatalog> {
  const invoke = await getInvoke();
  return invoke<AiCatalog>('add_ai_plugin', { name, path });
}

export async function listAiPluginConfigurations(): Promise<AiPluginConfiguration[]> {
  const invoke = await getInvoke();
  return invoke<AiPluginConfiguration[]>('list_ai_plugin_configurations');
}

export async function runAiSegmentation(
  handle: number,
  stackIndex: number,
  modelId: string,
): Promise<AiSegmentationResult> {
  const invoke = await getInvoke();
  return invoke<AiSegmentationResult>('run_ai_segmentation', { handle, stackIndex, modelId });
}

export async function cancelAiSegmentation(): Promise<boolean> {
  const invoke = await getInvoke();
  return invoke<boolean>('cancel_ai_segmentation');
}

export async function closeSeries(handle: number): Promise<void> {
  const invoke = await getInvoke();
  await invoke('close_series', { handle });
}

export async function selectImageStack(
  handle: number,
  stackIndex: number,
): Promise<SeriesMetadata> {
  const invoke = await getInvoke();
  return invoke<SeriesMetadata>('select_image_stack', { handle, stackIndex });
}

export async function remoteLogin(
  serverUrl: string,
  caCertPath: string,
  username: string,
  password: string,
): Promise<RemoteUser> {
  const invoke = await getInvoke();
  return invoke<RemoteUser>('remote_login', { serverUrl, caCertPath, username, password });
}

export async function remoteLogout(): Promise<void> {
  const invoke = await getInvoke();
  await invoke('remote_logout');
}

export async function listWindowPresets(): Promise<UserWindowPreset[]> {
  const invoke = await getInvoke();
  return invoke<UserWindowPreset[]>('list_window_presets');
}

export async function listReportTemplates(modality?: string): Promise<ReportTemplate[]> {
  const invoke = await getInvoke();
  return invoke<ReportTemplate[]>('list_report_templates', { modality: modality ?? null });
}

export async function listReports(studyUid: string): Promise<DiagnosticReport[]> {
  const invoke = await getInvoke();
  return invoke<DiagnosticReport[]>('list_reports', { studyUid });
}

export async function createReport(
  studyUid: string,
  seriesUids: string[],
  payload: StructuredPayload | null,
): Promise<DiagnosticReport> {
  const invoke = await getInvoke();
  return invoke<DiagnosticReport>('create_report', {
    studyUid,
    seriesUids,
    templatePayload: payload ?? null,
  });
}

export async function updateReportDraft(
  reportId: string,
  revision: number,
  findings: string,
  impression: string,
  recommendation: string | null,
  payload: StructuredPayload | null,
): Promise<DiagnosticReport> {
  const invoke = await getInvoke();
  return invoke<DiagnosticReport>('update_report_draft', {
    reportId,
    revision,
    findings,
    impression,
    recommendation,
    templatePayload: payload ?? null,
  });
}

export async function signReport(reportId: string, revision: number): Promise<void> {
  const invoke = await getInvoke();
  await invoke('sign_report', { reportId, revision });
}

export async function beginReportAmendment(
  reportId: string,
  reason: string,
): Promise<DiagnosticReport> {
  const invoke = await getInvoke();
  return invoke<DiagnosticReport>('begin_report_amendment', { reportId, reason });
}

export async function listReportVersions(reportId: string): Promise<ReportVersion[]> {
  const invoke = await getInvoke();
  return invoke<ReportVersion[]>('list_report_versions', { reportId });
}

export async function listWorklist(status?: string): Promise<ClinicalWorkItem[]> {
  const invoke = await getInvoke();
  return invoke<ClinicalWorkItem[]>('list_worklist', { status: status ?? null });
}

export async function workItemForSeries(seriesUid: string): Promise<ClinicalWorkItem> {
  const invoke = await getInvoke();
  return invoke<ClinicalWorkItem>('work_item_for_series', { seriesUid });
}

export async function claimWorkItem(workId: string, revision: number): Promise<void> {
  const invoke = await getInvoke();
  await invoke('claim_work_item', { workId, revision });
}

export async function releaseWorkItem(workId: string, revision: number): Promise<void> {
  const invoke = await getInvoke();
  await invoke('release_work_item', { workId, revision });
}

export async function registerDevice(
  name: string,
  callingAeTitle: string,
  sourceIp: string,
  modalityHint: string | null,
): Promise<DicomDevice> {
  const invoke = await getInvoke();
  return invoke<DicomDevice>('register_device', {
    name,
    callingAeTitle,
    sourceIp,
    modalityHint,
  });
}

export async function listDevices(status?: string): Promise<DicomDevice[]> {
  const invoke = await getInvoke();
  return invoke<DicomDevice[]>('list_devices', { status: status ?? null });
}

export async function approveDevice(
  deviceId: string,
  name: string,
  modalityHint: string | null,
): Promise<DicomDevice> {
  const invoke = await getInvoke();
  return invoke<DicomDevice>('approve_device', { deviceId, name, modalityHint });
}

export async function setDeviceStatus(deviceId: string, status: string): Promise<void> {
  const invoke = await getInvoke();
  await invoke('set_device_status', { deviceId, status });
}

export async function listSeriesSources(
  unattributed: boolean,
  limit: number,
  offset: number,
): Promise<SeriesSourceEntry[]> {
  const invoke = await getInvoke();
  return invoke<SeriesSourceEntry[]>('list_series_sources', { unattributed, limit, offset });
}

export async function resolveSeriesSource(seriesUid: string, deviceId: string): Promise<void> {
  const invoke = await getInvoke();
  await invoke('resolve_series_source', { seriesUid, deviceId });
}

export async function listUsers(): Promise<AdminUser[]> {
  const invoke = await getInvoke();
  return invoke<AdminUser[]>('list_users');
}

export async function listUserDeviceGrants(userId: number): Promise<string[]> {
  const invoke = await getInvoke();
  return invoke<string[]>('list_user_device_grants', { userId });
}

export async function replaceUserDeviceGrants(
  userId: number,
  deviceIds: string[],
): Promise<string[]> {
  const invoke = await getInvoke();
  return invoke<string[]>('replace_user_device_grants', { userId, deviceIds });
}

export async function createWindowPreset(
  modality: string,
  name: string,
  center: number,
  width: number,
  functionName: VoiFunction,
): Promise<UserWindowPreset> {
  const invoke = await getInvoke();
  return invoke<UserWindowPreset>('create_window_preset', {
    modality,
    name,
    center,
    width,
    function: functionName,
  });
}

export async function renameWindowPreset(presetId: number, name: string): Promise<UserWindowPreset> {
  const invoke = await getInvoke();
  return invoke<UserWindowPreset>('rename_window_preset', { presetId, name });
}

export async function deleteWindowPreset(presetId: number): Promise<void> {
  const invoke = await getInvoke();
  await invoke('delete_window_preset', { presetId });
}

export interface LocalModeInfo {
  server_url: string;
  ca_cert_path: string;
  username: string;
  password: string;
}

/** 获取本地完整栈（内嵌 PostgreSQL + pacsd）信息；非打包版返回 null。 */
export async function localStackInfo(): Promise<LocalModeInfo | null> {
  const invoke = await getInvoke();
  return invoke<LocalModeInfo | null>('local_stack_info');
}

export async function listPatients(
  query: string,
  limit: number,
  offset: number,
): Promise<PatientSummary[]> {
  const invoke = await getInvoke();
  return invoke<PatientSummary[]>('list_patients', { query, limit, offset });
}

export async function listPatientStudies(patientId: number): Promise<StudySummary[]> {
  const invoke = await getInvoke();
  return invoke<StudySummary[]>('list_patient_studies', { patientId });
}

export async function listStudySeries(studyUid: string): Promise<RemoteSeriesSummary[]> {
  const invoke = await getInvoke();
  return invoke<RemoteSeriesSummary[]>('list_study_series', { studyUid });
}

export async function listSharedAnnotations(
  studyUid: string,
  seriesUid: string,
  since?: string,
): Promise<SharedAnnotationRecord[]> {
  const invoke = await getInvoke();
  return invoke<SharedAnnotationRecord[]>('list_shared_annotations', { studyUid, seriesUid, since });
}

export async function createSharedAnnotation(
  studyUid: string,
  seriesUid: string,
  annotation: Record<string, unknown>,
): Promise<SharedAnnotationRecord> {
  const invoke = await getInvoke();
  return invoke<SharedAnnotationRecord>('create_shared_annotation', { studyUid, seriesUid, annotation });
}

export async function updateSharedAnnotation(
  studyUid: string,
  seriesUid: string,
  annotationId: string,
  expectedRevision: number,
  geometry: Record<string, unknown>,
  deleted: boolean,
): Promise<SharedAnnotationRecord> {
  const invoke = await getInvoke();
  return invoke<SharedAnnotationRecord>('update_shared_annotation', {
    studyUid,
    seriesUid,
    annotationId,
    expectedRevision,
    geometry,
    deleted,
  });
}

export async function listSegmentationProjects(
  studyUid: string,
  seriesUid: string,
): Promise<SegmentationProject[]> {
  const invoke = await getInvoke();
  return invoke<SegmentationProject[]>('list_segmentation_projects', { studyUid, seriesUid });
}

export async function createSegmentationProject(
  studyUid: string,
  seriesUid: string,
  input: Record<string, unknown>,
): Promise<CreatedSegmentationProject> {
  const invoke = await getInvoke();
  return invoke<CreatedSegmentationProject>('create_segmentation_project', { studyUid, seriesUid, input });
}

export async function deleteSegmentationProject(
  studyUid: string,
  seriesUid: string,
  projectId: string,
): Promise<void> {
  const invoke = await getInvoke();
  return invoke<void>('delete_segmentation_project', { studyUid, seriesUid, projectId });
}

export async function listSegmentationSegments(
  studyUid: string,
  seriesUid: string,
  projectId: string,
  tag?: string,
): Promise<SegmentationSegment[]> {
  const invoke = await getInvoke();
  return invoke<SegmentationSegment[]>('list_segmentation_segments', { studyUid, seriesUid, projectId, tag });
}

export async function updateSegmentationSegmentTags(
  studyUid: string,
  seriesUid: string,
  projectId: string,
  segmentId: string,
  tags: string[],
): Promise<SegmentationSegment> {
  const invoke = await getInvoke();
  return invoke<SegmentationSegment>('update_segmentation_segment_tags', {
    studyUid,
    seriesUid,
    projectId,
    segmentId,
    input: { tags },
  });
}

export async function listSegmentationMasks(
  studyUid: string,
  seriesUid: string,
  projectId: string,
  sopInstanceUid: string,
  frameNumber: number,
): Promise<SegmentationMaskRecord[]> {
  const invoke = await getInvoke();
  return invoke<SegmentationMaskRecord[]>('list_segmentation_masks', {
    studyUid,
    seriesUid,
    projectId,
    sopInstanceUid,
    frameNumber,
  });
}

export async function upsertSegmentationMask(
  studyUid: string,
  seriesUid: string,
  projectId: string,
  segmentId: string,
  input: Record<string, unknown>,
): Promise<SegmentationMaskRecord> {
  const invoke = await getInvoke();
  return invoke<SegmentationMaskRecord>('upsert_segmentation_mask', {
    studyUid,
    seriesUid,
    projectId,
    segmentId,
    input,
  });
}

export async function listSegmentationVolume(
  studyUid: string,
  seriesUid: string,
  projectId: string,
  segmentId: string,
): Promise<SegmentationMaskRecord[]> {
  const invoke = await getInvoke();
  return invoke<SegmentationMaskRecord[]>('list_segmentation_volume', {
    studyUid,
    seriesUid,
    projectId,
    segmentId,
  });
}

export async function upsertSegmentationMasks(
  studyUid: string,
  seriesUid: string,
  projectId: string,
  segmentId: string,
  updates: Array<Record<string, unknown>>,
): Promise<SegmentationMaskRecord[]> {
  const invoke = await getInvoke();
  return invoke<SegmentationMaskRecord[]>('upsert_segmentation_masks', {
    studyUid,
    seriesUid,
    projectId,
    segmentId,
    updates,
  });
}

export async function getTransformSchema(): Promise<TransformSchema> {
  const invoke = await getInvoke();
  return invoke<TransformSchema>('transform_schema');
}

export async function previewClinicalTransform(
  targetType: TransformTargetType,
  targetKey: string,
  rules: TagRuleInput[],
  reason: string,
): Promise<TransformPreviewResponse> {
  const invoke = await getInvoke();
  return invoke<TransformPreviewResponse>('preview_clinical_transform', {
    targetType,
    targetKey,
    rules,
    reason,
  });
}

export async function confirmTransform(
  jobId: string,
  confirmationToken: string,
): Promise<void> {
  const invoke = await getInvoke();
  await invoke('confirm_transform', { jobId, confirmationToken });
}

export async function listTransformJobs(): Promise<TransformJob[]> {
  const invoke = await getInvoke();
  return invoke<TransformJob[]>('transform_jobs');
}

export async function listInstanceRevisionsBySop(sopUid: string): Promise<DicomRevision[]> {
  const invoke = await getInvoke();
  return invoke<DicomRevision[]>('instance_revisions_by_sop', { sopUid });
}

export async function previewRollback(
  logicalId: string,
  versionId: number,
  reason: string,
): Promise<TransformPreviewResponse> {
  const invoke = await getInvoke();
  return invoke<TransformPreviewResponse>('preview_rollback', {
    logicalId,
    versionId,
    reason,
  });
}

export async function openRemoteSeries(
  studyUid: string,
  seriesUid: string,
): Promise<SeriesMetadata> {
  const invoke = await getInvoke();
  return invoke<SeriesMetadata>('open_remote_series', { studyUid, seriesUid });
}

export async function cancelRemoteDownload(): Promise<void> {
  const invoke = await getInvoke();
  await invoke('cancel_remote_download');
}

export async function buildLut(
  handle: number,
  stackIndex: number,
  frameIndex: number,
  windowCenter: number,
  windowWidth: number,
  voiFunction: VoiFunction,
): Promise<Uint8Array> {
  const invoke = await getInvoke();
  const result = await invoke<ArrayBuffer | Uint8Array | number[]>('build_lut', {
    handle,
    stackIndex,
    frameIndex,
    windowCenter,
    windowWidth,
    voiFunction,
  });
  if (result instanceof Uint8Array) return result;
  if (result instanceof ArrayBuffer) return new Uint8Array(result);
  return new Uint8Array(result);
}

export async function measureFrameRoi(
  handle: number,
  stackIndex: number,
  frameIndex: number,
  shape: 'point' | 'rectangle' | 'ellipse',
  start: [number, number],
  end: [number, number],
): Promise<RoiStatistics> {
  const invoke = await getInvoke();
  return invoke<RoiStatistics>('measure_frame_roi', {
    handle,
    stackIndex,
    frameIndex,
    shape,
    start,
    end,
  });
}

export async function measureMprRoi(
  handle: number,
  plane: MprPlane,
  sliceIndex: number,
  shape: 'point' | 'rectangle' | 'ellipse',
  start: [number, number],
  end: [number, number],
): Promise<RoiStatistics> {
  const invoke = await getInvoke();
  return invoke<RoiStatistics>('measure_mpr_roi', {
    handle,
    plane,
    sliceIndex,
    shape,
    start,
    end,
  });
}

export async function prepareMpr(handle: number, stackIndex: number): Promise<MprMetadata> {
  const invoke = await getInvoke();
  return invoke<MprMetadata>('prepare_mpr', { handle, stackIndex });
}

export async function renderMprSlice(
  handle: number,
  plane: MprPlane,
  sliceIndex: number,
  windowCenter: number,
  windowWidth: number,
  voiFunction: VoiFunction,
  projection: MprProjectionMode,
  slabThicknessMm: number,
): Promise<ArrayBuffer> {
  const invoke = await getInvoke();
  const result = await invoke<ArrayBuffer | Uint8Array | number[]>('render_mpr_slice', {
    request: {
      handle,
      plane,
      sliceIndex,
      windowCenter,
      windowWidth,
      voiFunction,
      projection,
      slabThicknessMm,
    },
  });
  if (result instanceof ArrayBuffer) return result;
  if (result instanceof Uint8Array) return result.buffer.slice(
    result.byteOffset,
    result.byteOffset + result.byteLength,
  ) as ArrayBuffer;
  return new Uint8Array(result).buffer;
}

export async function beginMprPrefetch(): Promise<number> {
  const invoke = await getInvoke();
  return invoke<number>('begin_mpr_prefetch');
}

export async function prefetchMprSlices(
  handle: number,
  generation: number,
  startSlices: [number, number, number],
  windowCenter: number,
  windowWidth: number,
  voiFunction: VoiFunction,
  projection: MprProjectionMode,
  slabThicknessMm: number,
): Promise<number> {
  const invoke = await getInvoke();
  return invoke<number>('prefetch_mpr_slices', {
    request: {
      handle,
      generation,
      startSlices,
      windowCenter,
      windowWidth,
      voiFunction,
      projection,
      slabThicknessMm,
    },
  });
}

export async function cancelMprPrefetch(): Promise<void> {
  const invoke = await getInvoke();
  await invoke('cancel_mpr_prefetch');
}

export async function closeMpr(handle: number): Promise<void> {
  const invoke = await getInvoke();
  await invoke('close_mpr', { handle });
}

export async function cancelMprBuild(): Promise<void> {
  const invoke = await getInvoke();
  await invoke('cancel_mpr_build');
}

export function getFrameUrl(handle: number, stack: number, frame: number): string {
  return `pacs-frame://localhost/${handle}/${stack}/${frame}`;
}

export function getVolumeUrl(handle: number): string {
  return `pacs-volume://localhost/${handle}`;
}

export async function loadVolume(handle: number, signal?: AbortSignal): Promise<ArrayBuffer> {
  const response = await fetch(getVolumeUrl(handle), { signal });
  if (!response.ok) {
    const detail = await response.text().catch(() => response.statusText);
    throw new Error(detail || '加载三维体数据失败');
  }
  return response.arrayBuffer();
}

export async function loadFrame(
  handle: number,
  stack: number,
  frame: number,
  signal?: AbortSignal,
): Promise<ArrayBuffer> {
  const response = await fetch(getFrameUrl(handle, stack, frame), { signal });
  if (!response.ok) {
    const detail = await response.text().catch(() => response.statusText);
    throw new Error(detail || `加载第 ${frame + 1} 帧失败`);
  }
  return response.arrayBuffer();
}

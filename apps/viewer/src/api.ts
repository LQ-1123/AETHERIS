import type {
  DicomRevision,
  ImportTransferResponse,
  PatientSummary,
  MprMetadata,
  MprPlane,
  RoiStatistics,
  RemoteSeriesSummary,
  RemoteUser,
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
  SharedAnnotationRecord,
  StudySummary,
  TagRuleInput,
  TransformJob,
  TransformPreviewResponse,
  TransformSchema,
  TransformTargetType,
  VoiFunction,
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
): Promise<ArrayBuffer> {
  const invoke = await getInvoke();
  const result = await invoke<ArrayBuffer | Uint8Array | number[]>('render_mpr_slice', {
    handle,
    plane,
    sliceIndex,
    windowCenter,
    windowWidth,
    voiFunction,
  });
  if (result instanceof ArrayBuffer) return result;
  if (result instanceof Uint8Array) return result.buffer.slice(
    result.byteOffset,
    result.byteOffset + result.byteLength,
  ) as ArrayBuffer;
  return new Uint8Array(result).buffer;
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

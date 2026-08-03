import type {
  PatientSummary,
  RemoteSeriesSummary,
  RemoteUser,
  SeriesMetadata,
  StudySummary,
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

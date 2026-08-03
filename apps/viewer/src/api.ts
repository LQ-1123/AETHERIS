import type { SeriesMetadata, VoiFunction } from './types';

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

export async function openSeries(paths: string[]): Promise<SeriesMetadata> {
  const invoke = await getInvoke();
  return invoke<SeriesMetadata>('open_series', { paths });
}

export async function closeSeries(handle: number): Promise<void> {
  const invoke = await getInvoke();
  await invoke('close_series', { handle });
}

export async function buildLut(
  handle: number,
  frameIndex: number,
  windowCenter: number,
  windowWidth: number,
  voiFunction: VoiFunction,
): Promise<Uint8Array> {
  const invoke = await getInvoke();
  const result = await invoke<ArrayBuffer | Uint8Array | number[]>('build_lut', {
    handle,
    frameIndex,
    windowCenter,
    windowWidth,
    voiFunction,
  });
  if (result instanceof Uint8Array) return result;
  if (result instanceof ArrayBuffer) return new Uint8Array(result);
  return new Uint8Array(result);
}

export function getFrameUrl(handle: number, frame: number): string {
  return `pacs-frame://localhost/${handle}/${frame}`;
}

export async function loadFrame(
  handle: number,
  frame: number,
  signal?: AbortSignal,
): Promise<ArrayBuffer> {
  const response = await fetch(getFrameUrl(handle, frame), { signal });
  if (!response.ok) {
    const detail = await response.text().catch(() => response.statusText);
    throw new Error(detail || `加载第 ${frame + 1} 帧失败`);
  }
  return response.arrayBuffer();
}

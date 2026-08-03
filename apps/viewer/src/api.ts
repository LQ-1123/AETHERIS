// 与 Tauri 后端通信的 API

import type { DisplayMetadata } from './types';

// Invoke 函数类型
type InvokeFn = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

// 延迟加载 Tauri API
let invokeCache: InvokeFn | null = null;
let openCache: any = null;

async function getInvoke(): Promise<InvokeFn> {
  if (!invokeCache) {
    const m = await import('@tauri-apps/api/core');
    console.log('Loaded @tauri-apps/api/core:', m);
    console.log('invoke function:', m.invoke);
    invokeCache = m.invoke as InvokeFn;
  }
  if (!invokeCache) {
    throw new Error('invoke is undefined after loading');
  }
  return invokeCache;
}

async function getOpen() {
  if (!openCache) {
    const m = await import('@tauri-apps/plugin-dialog');
    console.log('Loaded @tauri-apps/plugin-dialog:', m);
    console.log('open function:', m.open);
    openCache = m.open;
  }
  if (!openCache) {
    throw new Error('open is undefined after loading');
  }
  return openCache;
}

/**
 * 打开文件选择器并加载 DICOM 文件
 */
export async function openDicomFile(): Promise<DisplayMetadata | null> {
  const open = await getOpen();
  const invoke = await getInvoke();

  const selected = await open({
    multiple: false,
    filters: [{
      name: 'DICOM Files',
      extensions: ['dcm', 'dicom', '*']
    }]
  });

  if (!selected || selected === null) {
    return null;
  }

  const path = typeof selected === 'string' ? selected : (selected as any).path || selected[0];
  console.log('Opening DICOM file:', path);

  try {
    const metadata = await invoke<DisplayMetadata>('open_dicom', { path });
    console.log('Got metadata:', metadata);
    return metadata;
  } catch (e) {
    console.error('Failed to invoke open_dicom:', e);
    throw e;
  }
}

/**
 * 关闭实例
 */
export async function closeInstance(handle: number): Promise<void> {
  const invoke = await getInvoke();
  await invoke('close_instance', { handle });
}

/**
 * 生成查找表
 */
export async function buildLut(
  handle: number,
  windowCenter: number | null,
  windowWidth: number | null
): Promise<Uint8Array> {
  const invoke = await getInvoke();
  const result = await invoke<number[]>('build_lut', {
    handle,
    windowCenter,
    windowWidth
  });
  return new Uint8Array(result);
}

/**
 * 获取帧数据 URL
 *
 * 通过自定义协议 pacs-frame:// 获取帧的原始字节
 */
export function getFrameUrl(handle: number, frame: number): string {
  return `pacs-frame://localhost/${handle}/${frame}`;
}

/**
 * 加载帧数据
 */
export async function loadFrame(handle: number, frame: number): Promise<ArrayBuffer> {
  const url = getFrameUrl(handle, frame);
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`Failed to load frame ${frame}: ${response.statusText}`);
  }
  return await response.arrayBuffer();
}

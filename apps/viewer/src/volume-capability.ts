import type { VolumeRenderingMetadata } from './types';

export function volumeCapabilityReason(
  canvas: HTMLCanvasElement,
  metadata: VolumeRenderingMetadata,
): string | null {
  if (!metadata.available) return metadata.unavailable_reason ?? '当前体数据不支持 GPU 体渲染';
  const context = canvas.getContext('webgl2');
  if (!context) return '当前显卡或 WebView 不支持 WebGL2';
  const maximum = Number(context.getParameter(context.MAX_3D_TEXTURE_SIZE));
  const requested = Math.max(...metadata.dimensions);
  if (!Number.isFinite(maximum) || requested > maximum) {
    return `体纹理边长 ${requested} 超过当前 GPU 上限 ${maximum || '未知'}`;
  }
  return null;
}

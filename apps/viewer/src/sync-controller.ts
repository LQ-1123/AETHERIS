import { nearestParallelFrameIndex, type SeriesGeometryFrame } from './series-sync';
import type { FrameMetadata } from './types';

/**
 * 跨序列同步的纯逻辑层：把 series-sync 的几何函数接到 ViewerApp 的同步
 * 状态机上。不依赖 DOM、不发起帧请求，全部可单测。
 */

export interface SyncWindowLevel {
  center: number;
  width: number;
}

export interface SyncEligibility {
  eligible: boolean;
  reason: string | null;
}

export interface SyncMember {
  paneIndex: number;
  frames: FrameMetadata[];
}

export interface FrameSyncTarget {
  paneIndex: number;
  /** 目标帧号；null = 平面不平行或几何不可用，不传播该成员。 */
  frameIndex: number | null;
}

/** FrameMetadata → 几何接口适配：spacing.row_mm/col_mm 映射为行/列间距。 */
export function seriesGeometryFrame(frame: FrameMetadata): SeriesGeometryFrame {
  return {
    rows: frame.rows,
    cols: frame.cols,
    position: frame.position,
    orientation: frame.orientation,
    rowSpacingMm: frame.spacing.row_mm,
    colSpacingMm: frame.spacing.col_mm,
  };
}

/** 一帧是否携带患者空间几何与像素间距（同步与定位线的前置条件）。 */
export function frameHasGeometry(frame: FrameMetadata): boolean {
  return (
    frame.position !== null
    && frame.orientation !== null
    && frame.spacing.row_mm != null
    && frame.spacing.col_mm != null
  );
}

/** 判定一个序列能否参与同步：全部帧带几何才参与，绝不猜测。 */
export function syncEligibility(frames: FrameMetadata[]): SyncEligibility {
  if (frames.length === 0) return { eligible: false, reason: '无帧' };
  if (!frames.every(frameHasGeometry)) return { eligible: false, reason: '缺几何' };
  return { eligible: true, reason: null };
}

/**
 * 为组内成员计算同步目标帧：源帧按患者空间最近切片映射到各成员序列。
 * 被排除的成员不返回；映射失败的成员返回 frameIndex: null。
 */
export function syncFrameTargets(
  source: FrameMetadata,
  members: SyncMember[],
  excludedPaneIndexes: ReadonlySet<number>,
): FrameSyncTarget[] {
  const targets: FrameSyncTarget[] = [];
  for (const member of members) {
    if (excludedPaneIndexes.has(member.paneIndex)) continue;
    const frameIndex = nearestParallelFrameIndex(
      seriesGeometryFrame(source),
      member.frames.map(seriesGeometryFrame),
    );
    targets.push({ paneIndex: member.paneIndex, frameIndex });
  }
  return targets;
}

/** 窗宽窗位是否一致（传播短路判断用）。 */
export function windowEquals(left: SyncWindowLevel, right: SyncWindowLevel): boolean {
  return left.center === right.center && left.width === right.width;
}

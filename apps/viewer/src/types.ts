// Rust 后端返回的类型定义

export interface DisplayMetadata {
  handle: number;
  rows: number;
  cols: number;
  frame_count: number;
  bits_allocated: number;
  window_presets: WindowPreset[];
  spacing: SpacingInfo;
}

export interface WindowPreset {
  center: number;
  width: number;
  explanation: string | null;
  function: 'LINEAR' | 'LINEAR_EXACT' | 'SIGMOID';
}

export interface SpacingInfo {
  confidence: 'accurate' | 'detector' | 'none';
  row_mm: number | null;
  col_mm: number | null;
  aspect_ratio: number;
}

export interface ViewState {
  metadata: DisplayMetadata;
  currentFrame: number;
  windowCenter: number;
  windowWidth: number;
  zoom: number;
  panX: number;
  panY: number;
  lut: Uint8Array | null;
}

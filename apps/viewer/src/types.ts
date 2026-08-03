export type VoiFunction = 'LINEAR' | 'LINEAR_EXACT' | 'SIGMOID';
export type ToolMode = 'window' | 'pan' | 'length';

export interface PatientStudyInfo {
  patient_name: string | null;
  patient_id: string | null;
  study_date: string | null;
  accession_number: string | null;
  modality: string | null;
  study_description: string | null;
  series_description: string | null;
}

export interface SeriesMetadata {
  handle: number;
  patient: PatientStudyInfo;
  study_uid: string | null;
  series_uid: string | null;
  frames: FrameMetadata[];
  warnings: string[];
}

export interface FrameMetadata {
  logical_index: number;
  frame_key: string;
  sop_instance_uid: string | null;
  source_frame: number;
  instance_number: number | null;
  rows: number;
  cols: number;
  bits_allocated: 8 | 16;
  window_presets: WindowPreset[];
  spacing: SpacingInfo;
}

export interface WindowPreset {
  center: number;
  width: number;
  explanation: string | null;
  function: VoiFunction;
}

export interface SpacingInfo {
  confidence: 'calibrated' | 'detector' | 'none';
  source: string | null;
  description: string;
  row_mm: number | null;
  col_mm: number | null;
  column_over_row: number;
}

export interface Point {
  x: number;
  y: number;
}

export interface ViewTransform {
  zoom: number;
  panX: number;
  panY: number;
}

export interface LengthMeasurement {
  id: string;
  start: Point;
  end: Point;
}

export interface ViewState extends ViewTransform {
  metadata: SeriesMetadata;
  currentFrame: number;
  windowCenter: number;
  windowWidth: number;
  voiFunction: VoiFunction;
  lut: Uint8Array | null;
  tool: ToolMode;
}

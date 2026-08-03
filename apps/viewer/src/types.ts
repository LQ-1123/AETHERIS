export type VoiFunction = 'LINEAR' | 'LINEAR_EXACT' | 'SIGMOID';
export type ToolMode = 'window' | 'pan' | 'crosshair' | 'length';
export type ViewerMode = '2d' | 'mpr';
export type MprPlane = 'axial' | 'coronal' | 'sagittal';

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
  active_stack: number;
  image_stacks: ImageStackMetadata[];
  frames: FrameMetadata[];
  warnings: string[];
}

export interface ImageStackMetadata {
  index: number;
  label: string;
  frame_count: number;
  rows: number;
  cols: number;
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

export interface PatientPoint3D {
  x: number;
  y: number;
  z: number;
}

export interface MprPlaneMetadata {
  plane: MprPlane;
  rows: number;
  cols: number;
  slice_count: number;
  pixel_spacing_mm: number;
  slice_spacing_mm: number;
  origin: [number, number, number];
  x_axis: [number, number, number];
  y_axis: [number, number, number];
  normal: [number, number, number];
}

export interface MprMetadata {
  stack_index: number;
  dimensions: [number, number, number];
  source_spacing_mm: [number, number, number];
  patient_bounds_min: [number, number, number];
  patient_bounds_max: [number, number, number];
  initial_crosshair: [number, number, number];
  planes: MprPlaneMetadata[];
}

export interface MprViewportState extends ViewTransform {
  plane: MprPlane;
  sliceIndex: number;
}

export interface MprBuildProgress {
  completed: number;
  total: number;
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

export interface RemoteUser {
  id: number;
  username: string;
  display_name: string | null;
  role: string;
  institution_id: number;
}

export interface PatientSummary {
  id: number;
  patient_id: string;
  name: string | null;
  birth_date: string | null;
  sex: string | null;
  study_count: number;
  series_count: number;
  instance_count: number;
  latest_study_date: string | null;
}

export interface StudySummary {
  study_uid: string;
  study_date: string | null;
  study_time: string | null;
  accession_number: string | null;
  description: string | null;
  modalities: string[];
  series_count: number;
  instance_count: number;
}

export interface RemoteSeriesSummary {
  series_uid: string;
  series_number: number | null;
  modality: string | null;
  description: string | null;
  body_part_examined: string | null;
  instance_count: number;
}

export interface DownloadProgress {
  downloaded: number;
  total: number;
}

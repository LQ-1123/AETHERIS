export type VoiFunction = 'LINEAR' | 'LINEAR_EXACT' | 'SIGMOID';
export type AnnotationKind =
  | 'length'
  | 'arrow'
  | 'ellipse_roi'
  | 'rectangle_roi'
  | 'angle'
  | 'point_probe';
export type ToolMode = 'window' | 'pan' | 'crosshair' | AnnotationKind;
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
  rotation: 0 | 90 | 180 | 270;
  flipHorizontal: boolean;
  flipVertical: boolean;
  inverted: boolean;
}

export interface RoiStatistics {
  count: number;
  mean: number;
  standard_deviation: number;
  minimum: number;
  maximum: number;
  area: number | null;
  area_unit: 'mm2' | 'px2' | null;
  unit: string | null;
}

interface AnnotationBase {
  id: string;
  revision?: number;
  syncState?: 'synced' | 'pending' | 'error';
  measurementError?: string;
}

export interface LineAnnotation extends AnnotationBase {
  kind: 'length' | 'arrow';
  start: Point;
  end: Point;
}

export interface RoiAnnotation extends AnnotationBase {
  kind: 'ellipse_roi' | 'rectangle_roi';
  start: Point;
  end: Point;
  statistics?: RoiStatistics;
}

export interface PointProbeAnnotation extends AnnotationBase {
  kind: 'point_probe';
  point: Point;
  statistics?: RoiStatistics;
}

export interface AngleAnnotation extends AnnotationBase {
  kind: 'angle';
  start: Point;
  vertex: Point;
  end: Point;
}

export type Annotation =
  | LineAnnotation
  | RoiAnnotation
  | PointProbeAnnotation
  | AngleAnnotation;

export interface SharedAnnotationRecord {
  id: string;
  study_instance_uid: string;
  series_instance_uid: string;
  sop_instance_uid: string | null;
  frame_number: number | null;
  coordinate_space: 'image' | 'patient';
  mpr_plane: MprPlane | null;
  schema_version: number;
  kind: AnnotationKind;
  geometry: Record<string, unknown>;
  revision: number;
  created_by: number | null;
  modified_by: number | null;
  deleted_at: string | null;
  created_at: string;
  updated_at: string;
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

export type RouteProtocol = 'dimse' | 'stow';
export type RouteConnectionStatus = 'unknown' | 'online' | 'offline';

export interface LocalDicomNode {
  ae_title: string;
  listen_host: string;
  listen_port: number;
}

export interface RoutableSeries {
  patient_id: string;
  patient_name: string | null;
  study_instance_uid: string;
  study_description: string | null;
  study_date: string | null;
  series_instance_uid: string;
  series_number: number | null;
  series_description: string | null;
  modality: string | null;
  instance_count: number;
}

export interface ObservedDicomPeer {
  id: number;
  institution_id: number;
  calling_ae_title: string;
  remote_host: string;
  status: 'connected' | 'recent' | 'offline';
  active_associations: number;
  association_count: number;
  first_seen_at: string;
  last_seen_at: string;
  last_disconnected_at: string | null;
}

export interface RouteDestination {
  id: string;
  institution_id: number;
  name: string;
  protocol: RouteProtocol;
  enabled: boolean;
  approval_status: 'pending' | 'approved';
  approved_at: string | null;
  host: string | null;
  port: number | null;
  called_ae_title: string | null;
  calling_ae_title: string | null;
  use_tls: boolean;
  stow_url: string | null;
  has_auth_token: boolean;
  has_ca_certificate: boolean;
  status: RouteConnectionStatus;
  last_checked_at: string | null;
  last_success_at: string | null;
  last_latency_ms: number | null;
  last_error: string | null;
  created_at: string;
  updated_at: string;
}

export interface RouteDestinationInput {
  name: string;
  protocol: RouteProtocol;
  enabled: boolean;
  host?: string;
  port?: number;
  called_ae_title?: string;
  calling_ae_title?: string;
  use_tls?: boolean;
  stow_url?: string;
  auth_token?: string;
  ca_pem?: string;
}

export interface RouteRule {
  id: string;
  destination_id: string;
  destination_name: string;
  name: string;
  priority: number;
  enabled: boolean;
  source_ae_title: string | null;
  modality: string | null;
  body_part_examined: string | null;
  study_description: string | null;
  series_description: string | null;
  tag_matches: Record<string, unknown>;
}

export interface RouteRuleInput {
  destination_id: string;
  name: string;
  priority: number;
  enabled: boolean;
  source_ae_title?: string;
  modality?: string;
  body_part_examined?: string;
  study_description?: string;
  series_description?: string;
  tag_matches: Record<string, unknown>;
}

export interface RouteDelivery {
  id: string;
  destination_id: string;
  destination_name: string;
  rule_id: string | null;
  sop_instance_uid: string;
  job_id: string | null;
  status: 'queued' | 'running' | 'succeeded' | 'dead_letter';
  attempts: number;
  last_error: string | null;
  delivered_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface PatientSummary {
  id: number;
  patient_id: string;
  issuer_of_patient_id: string | null;
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
  study_id: string | null;
  description: string | null;
  referring_physician: string | null;
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
  protocol_name: string | null;
  instance_count: number;
}

export interface DownloadProgress {
  downloaded: number;
  total: number;
}

export interface TransferProgress {
  phase: 'upload' | 'processing';
  completed_bytes: number;
  total_bytes: number;
  completed_files: number;
  total_files: number;
  status: string | null;
}

export interface ImportTransferItem {
  item_key: string;
  status: 'pending' | 'running' | 'succeeded' | 'skipped' | 'conflict' | 'failed' | 'cancelled';
  input: { name?: string };
  result: {
    disposition?: 'created' | 'duplicate' | 'conflict' | 'invalid' | 'failed';
    sop_instance_uid?: string | null;
    error?: string | null;
  };
}

export interface ImportTransferResponse {
  job: {
    result?: {
      created?: number;
      duplicates?: number;
      conflicts?: number;
      invalid?: number;
      failed?: number;
    };
  };
  items?: ImportTransferItem[];
}

export type TransformTargetType = 'patient' | 'study' | 'series' | 'instance';
export type TransformScope = 'patient' | 'study' | 'series';

export interface ManualTagSpec {
  keyword: string;
  tag: string;
  vr: string;
  scope: TransformScope;
  actions: Array<'replace' | 'empty' | 'remove'>;
}

export interface TransformSchema {
  manual_tags: ManualTagSpec[];
}

export interface TagRuleInput {
  tag: string;
  action: 'replace' | 'empty' | 'remove';
  value?: string;
  recursive: false;
}

export interface TransformDiff {
  tag: string;
  keyword: string;
  old_value: string | null;
  new_value: string | null;
  action: string;
  affected_instances: number;
}

export interface TransformPreviewSummary {
  affected_instances: number;
  rule_target_instances: number;
  affected_studies: number;
  affected_series: number;
  uid_remaps: { studies: number; series: number; instances: number };
  changes: TransformDiff[];
  pixel_risk: 'safe' | 'review_required' | 'blocking' | 'unknown';
  pixel_risk_reasons: string[];
}

export interface TransformPreviewResponse {
  job_id: string;
  confirmation_token: string;
  confirmation_expires_at: string;
  preview: TransformPreviewSummary;
}

export interface TransformJob {
  id: string;
  mode: 'clinical_correction' | 'rollback';
  target: { target_type: TransformTargetType; key: string };
  status: 'previewed' | 'queued' | 'running' | 'succeeded' | 'failed' | 'blocked' | 'expired';
  reason: string;
  progress_completed: number;
  progress_total: number;
  pixel_risk: string;
  error_message: string | null;
  created_at: string;
  completed_at: string | null;
}

export interface DicomRevision {
  id: number;
  logical_instance_id: string;
  version_number: number;
  source_version_id: number | null;
  job_id: string | null;
  derivation_kind: 'original' | 'clinical_correction' | 'rollback';
  study_instance_uid: string;
  series_instance_uid: string;
  sop_instance_uid: string;
  storage_path: string;
  file_size: number;
  file_sha256_hex: string;
  metadata_snapshot: unknown;
  reason: string;
  created_at: string;
  is_current: boolean;
}

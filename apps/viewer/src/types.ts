export type VoiFunction = 'LINEAR' | 'LINEAR_EXACT' | 'SIGMOID';
export type AnnotationKind =
  | 'length'
  | 'arrow'
  | 'ellipse_roi'
  | 'rectangle_roi'
  | 'angle'
  | 'point_probe';
export type MaskTool = 'mask_brush' | 'mask_eraser';
export type ToolMode = 'window' | 'pan' | 'crosshair' | AnnotationKind | MaskTool;
export type ViewerMode = '2d' | 'mpr' | 'vr' | 'report';
export type MprPlane = 'axial' | 'coronal' | 'sagittal' | 'oblique';
export type MprProjectionMode = 'slice' | 'mip' | 'minip';
export type PixelFormat = 'gray8' | 'gray16' | 'rgb8';

export interface PatientStudyInfo {
  patient_name: string | null;
  patient_id: string | null;
  patient_sex: string | null;
  patient_birth_date: string | null;
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
  pixel_format: PixelFormat;
  photometric_interpretation: string;
  cine_rate_fps: number | null;
  quantitative: QuantitativeInfo;
  laterality: string | null;
  view_position: string | null;
  patient_orientation: string[];
  /** ImagePositionPatient（患者坐标系中的首个体素中心位置）。 */
  position: [number, number, number] | null;
  /** ImageOrientationPatient（行方向 3 分量 + 列方向 3 分量）。 */
  orientation:
    | [number, number, number, number, number, number]
    | null;
  window_presets: WindowPreset[];
  spacing: SpacingInfo;
}

export interface QuantitativeInfo {
  unit: string | null;
  suvbw_factor: number | null;
  suvbw_status: string | null;
}

export interface WindowPreset {
  center: number;
  width: number;
  explanation?: string | null;
  function: VoiFunction;
}

export interface UserWindowPreset extends WindowPreset {
  id: number;
  modality: string;
  name: string;
  created_at: string;
  updated_at: string;
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

export interface SegmentationProject {
  id: string;
  study_instance_uid: string;
  series_instance_uid: string;
  name: string;
  status: 'draft' | 'published' | 'archived';
  revision: number;
  created_by: number | null;
  modified_by: number | null;
  created_at: string;
  updated_at: string;
}

export interface SegmentationSegment {
  id: string;
  project_id: string;
  segment_number: number;
  label: string;
  description: string | null;
  color_r: number;
  color_g: number;
  color_b: number;
  algorithm_type: 'manual' | 'semiautomatic' | 'automatic';
  tags: string[];
  created_at: string;
  updated_at: string;
}

export interface SegmentationMaskRecord {
  segment_id: string;
  sop_instance_uid: string;
  frame_number: number;
  rows: number;
  cols: number;
  encoding: 'rle-v1';
  data_base64: string;
  revision: number;
  modified_by: number | null;
  updated_at: string;
}

export interface CreatedSegmentationProject {
  project: SegmentationProject;
  segment: SegmentationSegment;
}

export interface AiLabelDescriptor {
  id: string;
  display_name: string;
  color: [number, number, number];
  tags: string[];
}

export interface AiModelDescriptor {
  id: string;
  plugin_id: string;
  plugin_name: string;
  plugin_version: string;
  model_id: string;
  display_name: string;
  version: string;
  description: string;
  supported_modalities: string[];
  labels: AiLabelDescriptor[];
  estimated_peak_memory_mb: number;
  model_download_mb: number;
  device: string | null;
  available: boolean;
  unavailable_reason: string | null;
}

export interface AiPluginDescriptor {
  id: string;
  name: string;
  version: string;
  source: 'bundled' | 'user' | 'legacy';
  available: boolean;
  unavailable_reason: string | null;
}

export interface AiCatalog {
  plugins: AiPluginDescriptor[];
  models: AiModelDescriptor[];
}

export interface AiPluginConfiguration {
  id: string;
  name: string;
  version: string;
  path: string;
}

export interface AiMaskSlice {
  source_index: number;
  rows: number;
  cols: number;
  encoding: 'rle-v1';
  data_base64: string;
}

export interface AiSegmentResult {
  label: AiLabelDescriptor;
  voxel_count: number;
  masks: AiMaskSlice[];
}

export interface AiSegmentationResult {
  protocol_version: number;
  job_id: string;
  model_id: string;
  elapsed_ms: number;
  segments: AiSegmentResult[];
}

export interface AiSegmentationProgress {
  job_id: string;
  stage: string;
  completed: number;
  total: number;
  message: string;
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
  /** MPR 图像列方向（planeXAxis）的物理采样间距，缺省时回退到 pixel_spacing_mm。 */
  spacing_x_mm?: number;
  /** MPR 图像行方向（planeYAxis）的物理采样间距，缺省时回退到 pixel_spacing_mm。 */
  spacing_y_mm?: number;
  origin: [number, number, number];
  x_axis: [number, number, number];
  y_axis: [number, number, number];
  normal: [number, number, number];
}

export interface MprMetadata {
  stack_index: number;
  dimensions: [number, number, number];
  source_spacing_mm: [number, number, number];
  source_origin: [number, number, number];
  source_x_axis: [number, number, number];
  source_y_axis: [number, number, number];
  source_normal: [number, number, number];
  source_slices: MprSourceSlice[];
  patient_bounds_min: [number, number, number];
  patient_bounds_max: [number, number, number];
  initial_crosshair: [number, number, number];
  planes: MprPlaneMetadata[];
  volume_rendering: VolumeRenderingMetadata;
}

export interface VolumeRenderingMetadata {
  dimensions: [number, number, number];
  spacing_mm: [number, number, number];
  value_range: [number, number];
  byte_length: number;
  available: boolean;
  unavailable_reason: string | null;
}

export interface MprSourceSlice {
  frame_key: string;
  sop_instance_uid: string | null;
  frame_number: number;
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
  institution_name: string;
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

export type StorageTier = 'hot' | 'cold' | 'quarantine';

export interface LifecycleSummary {
  hot_studies: number;
  cold_studies: number;
  quarantine_studies: number;
  hot_bytes: number;
  cold_bytes: number;
  quarantine_bytes: number;
  active_legal_holds: number;
  pending_purge_requests: number;
}

export interface LifecyclePolicy {
  id: string;
  name: string;
  priority: number;
  enabled: boolean;
  target_tier: Exclude<StorageTier, 'hot'>;
  modalities: string[];
  study_date_before: string | null;
  last_accessed_before: string | null;
  tag_matches: Record<string, unknown>;
  minimum_study_bytes: number | null;
  minimum_storage_used_percent: number | null;
  preview_current: boolean;
  last_preview_at: string | null;
  last_preview: Record<string, unknown>;
  last_run_at: string | null;
}

export interface LifecyclePolicyInput {
  name: string;
  priority: number;
  enabled: boolean;
  target_tier: Exclude<StorageTier, 'hot'>;
  modalities: string[];
  study_date_before?: string;
  last_accessed_before?: string;
  tag_matches: Record<string, unknown>;
  minimum_study_bytes?: number;
  minimum_storage_used_percent?: number;
}

export interface LifecycleStudy {
  study_instance_uid: string;
  patient_name: string | null;
  patient_id: string;
  study_date: string | null;
  modalities: string[];
  storage_tier: StorageTier;
  last_accessed_at: string | null;
  storage_bytes: number;
  legal_hold: boolean;
}

export interface LegalHold {
  id: string;
  study_instance_uid: string;
  reason: string;
  expires_at: string | null;
  created_at: string;
  released_at: string | null;
}

export interface PurgeRequest {
  id: string;
  study_instance_uid: string;
  reason: string;
  status: 'pending' | 'approved' | 'paused_hold' | 'executing' | 'completed' | 'rejected' | 'cancelled' | 'failed';
  grace_until: string | null;
  grace_remaining_seconds: number | null;
  job_id: string | null;
  error_message: string | null;
  requested_at: string;
  approved_at: string | null;
  completed_at: string | null;
}

export interface LifecycleEvent {
  id: number;
  study_instance_uid: string;
  action: string;
  from_tier: StorageTier | null;
  to_tier: StorageTier | null;
  job_id: string | null;
  details: Record<string, unknown>;
  created_at: string;
}

export interface LifecycleJob {
  id: string;
  status: 'queued' | 'running' | 'paused' | 'succeeded' | 'failed' | 'cancelled';
  payload: Record<string, unknown>;
  result: Record<string, unknown>;
  progress_completed: number;
  progress_total: number;
  attempts: number;
  error_message: string | null;
  created_at: string;
  completed_at: string | null;
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
  pending_studies: number;
  writing_studies: number;
  locked_studies: number;
  signed_studies: number;
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
  report_status: string;
}

export interface QueueStudyRow {
  patient_key: number;
  study_uid: string;
  patient_id: string;
  patient_name: string | null;
  patient_sex: string | null;
  patient_birth_date: string | null;
  study_date: string | null;
  study_time: string | null;
  modalities: string[];
  description: string | null;
  body_parts: string[];
  report_status: 'pending' | 'writing' | 'locked' | 'submitted' | 'under_review' | 'signed';
  has_exam_request: boolean;
  institution_name: string | null;
  series_count: number;
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

// ---- 结构化报告（B2）----

export type ReportSectionId = 'findings' | 'impression' | 'recommendation';

export interface TemplateChoiceOption {
  id: string;
  label: string;
  /** 选中后展开一个描述文本域。 */
  expands?: boolean;
}

export interface TemplateField {
  id: string;
  kind: 'text' | 'choice' | 'number';
  label: string;
  required?: boolean;
  unit?: string;
  min?: number;
  max?: number;
  options?: TemplateChoiceOption[];
}

export interface TemplateSection {
  /** 固定枚举（I5），同时决定该章节渲染进哪一列。 */
  id: ReportSectionId;
  title: string;
  fields: TemplateField[];
}

export interface ReportTemplateStructure {
  schema_version: 1;
  sections: TemplateSection[];
}

export interface ReportTemplate {
  id: string;
  name: string;
  modality: string;
  body_part: string | null;
  version: number;
  structure: ReportTemplateStructure;
  builtin: boolean;
}

/** 自包含快照（I1）：渲染/编辑报告只依赖它，不查模板表。 */
export interface StructuredPayload {
  template_id: string;
  template_version: number;
  structure: ReportTemplateStructure;
  values: Record<string, unknown>;
}

export interface ChoiceValue {
  choice: string;
  description?: string;
}

export interface NumberValue {
  value: number;
}

export interface DiagnosticReport {
  id: string;
  study_uid: string;
  author_id: number;
  author_name: string;
  reviewer_id: number | null;
  reviewer_name: string | null;
  status: 'draft' | 'submitted' | 'under_review' | 'signed' | 'amending';
  findings: string;
  impression: string;
  recommendation: string | null;
  revision: number;
  access_incomplete: boolean;
  is_positive: boolean;
  template_payload: StructuredPayload | null;
  submitted_at: string | null;
  reviewed_at: string | null;
  review_comment: string | null;
  reviewer_modified: boolean;
  review_required: boolean;
  can_review: boolean;
  created_at: string;
  updated_at: string;
}

export interface ReportVersion {
  id: string;
  report_id: string;
  version_number: number;
  findings: string;
  impression: string;
  recommendation: string | null;
  covered_series_uids: string[];
  access_incomplete: boolean;
  is_positive: boolean;
  amendment_reason: string | null;
  signed_by: number;
  signed_at: string;
  reviewed_by: number | null;
  reviewed_at: string | null;
}

export interface ReportReviewEvent {
  id: number;
  report_id: string;
  actor_id: number;
  actor_name: string;
  action: 'submitted' | 'review_started' | 'reviewer_modified' | 'approved' | 'rejected';
  comment: string | null;
  created_at: string;
}

export interface ClinicalWorkItem {
  id: string;
  series_uid: string;
  study_uid: string;
  status: 'pending' | 'claimed' | 'reporting' | 'completed';
  assignee_id: number | null;
  assignee_name: string | null;
  revision: number;
  patient_name: string | null;
  modality: string | null;
  series_description: string | null;
  study_date: string | null;
}

export type ExamRequestStatus = 'pending' | 'executed' | 'completed';

export interface ExamRequest {
  id: string;
  patient_id: string;
  patient_name: string;
  patient_birth_date: string | null;
  patient_sex: string | null;
  modality: string;
  body_part: string;
  request_type: string;
  clinical_indication: string;
  requested_by_id: number;
  requested_by_name: string;
  requested_at: string;
  scheduled_at: string | null;
  status: ExamRequestStatus;
  study_uid: string | null;
  study_date: string | null;
  study_description: string | null;
  revision: number;
  created_at: string;
  updated_at: string;
}

export interface ExamRequestInput {
  patientId: string;
  patientName: string;
  patientBirthDate: string | null;
  patientSex: string | null;
  modality: string;
  bodyPart: string;
  requestType: string;
  clinicalIndication: string;
  scheduledAt: string | null;
}

/** 为已入库检查开具申请单时可编辑的字段。
 *
 * 患者快照和目标检查由服务端根据 Study UID 读取，不能由客户端覆盖。
 */
export interface ExistingStudyExamRequestInput {
  modality: string;
  bodyPart: string;
  requestType: string;
  clinicalIndication: string;
  scheduledAt: string | null;
}

export interface ExamRequestStudyCandidate {
  study_uid: string;
  patient_id: string;
  patient_name: string | null;
  study_date: string | null;
  modalities: string[];
  description: string | null;
}

export interface WorkloadRow {
  user_id: number;
  username: string;
  display_name: string | null;
  role: 'radiologist' | 'technician';
  draft_reports: number;
  submitted_reports: number;
  under_review_reports: number;
  signed_status_reports: number;
  signed_reports: number;
  reviews_completed: number;
  reviewer_modifications: number;
  exam_requests_created: number;
}

// ---- 管理员控制台（A1）----

export interface DicomDevice {
  id: string;
  name: string;
  calling_ae_title: string;
  source_ip: string;
  modality_hint: string | null;
  status: 'pending' | 'active' | 'disabled';
  approved_at: string | null;
}

export interface SeriesSourceEntry {
  series_uid: string;
  study_uid: string;
  patient_id: string;
  patient_name: string | null;
  modality: string | null;
  description: string | null;
  instance_count: number;
  source_status: string;
  device_name: string | null;
}

export interface AdminUser {
  id: number;
  username: string;
  display_name: string | null;
  role: string;
  is_active: boolean;
  must_change_password: boolean;
  last_login_at: string | null;
  created_at: string;
}

export interface PasswordResetRequest {
  id: number;
  user_id: number;
  username: string;
  display_name: string | null;
  status: 'pending' | 'approved' | 'rejected';
  requested_at: string;
  reviewed_by: number | null;
  reviewer_name: string | null;
  reviewed_at: string | null;
}

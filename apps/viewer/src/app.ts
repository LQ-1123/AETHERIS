import {
  addAiPlugin,
  beginMprPrefetch,
  buildLut,
  cancelAiSegmentation,
  cancelMprBuild,
  cancelMprPrefetch,
  cancelRemoteDownload,
  cancelTransfer,
  chooseCaCertificate,
  chooseAiPluginFolder,
  chooseDicomFiles,
  closeSeries,
  closeMpr,
  checkAiPlugin,
  confirmTransform,
  createWindowPreset,
  createSegmentationProject,
  deleteWindowPreset,
  deleteSegmentationProject,
  exportFromPacs,
  createSharedAnnotation,
  getTransformSchema,
  listInstanceRevisionsBySop,
  listAiCatalog,
  listAiPluginConfigurations,
  listPatientStudies,
  listPatients,
  listRouteDestinations,
  listSegmentationProjects,
  listSegmentationSegments,
  listSegmentationVolume,
  listStudySeries,
  listSharedAnnotations,
  listTransformJobs,
  listWindowPresets,
  loadFrame,
  loadVolume,
  measureFrameRoi,
  measureMprRoi,
  openRemoteSeries,
  openSeries,
  prepareMpr,
  prefetchMprSlices,
  previewClinicalTransform,
  previewRollback,
  remoteLogin,
  remoteLogout,
  requestPasswordReset,
  renameWindowPreset,
  renderMprSlice,
  runAiSegmentation,
  refreshAiPlugins,
  selectImageStack,
  sendRouteScope,
  updateSharedAnnotation,
  updateSegmentationSegmentTags,
  upsertSegmentationMasks,
  localStackInfo,
  openReportWindow,
  updateReportContext,
  type ReportWindowContext,
} from './api';
import { Download, Edit3, Share2, createIcons } from 'lucide';
import {
  AnnotationHistory,
  annotationHitTest,
  annotationPoints,
  cloneAnnotations,
  createAnnotation,
  translateAnnotation,
  updateAnnotationPoint,
  type AnnotationHit,
} from './annotations';
import { clampToImage, zoomAt } from './geometry';
import { GpuMprRenderer } from './gpu-mpr-renderer';
import { framePrefetchGroups } from './frame-prefetch';
import {
  base64ToBytes,
  bytesToBase64,
  calculateMaskStatistics,
  createMaskVolume,
  decodeMaskRle,
  encodeMaskRle,
  paintMaskSourcePlane,
  paintMaskVolumePlane,
  renderMaskPlane,
  restoreMaskSlices,
  snapshotMaskSlices,
  type MaskLayer,
  type MaskSliceSnapshot,
  type MaskVolume,
} from './masks';
import { ByteLruCache } from './lru';
import { LifecyclePanel } from './lifecycle-panel';
import {
  applyMat4,
  computePlaneStackGeometry,
  patientToPlaneMat4,
  planeToPatientMat4,
} from './patient-space';
import { imageGeometry, Renderer, type CrossReferenceLine } from './renderer';
import { RequestVersion } from './request-version';
import { RouterPanel } from './router-panel';
import { QueuePage } from './queue-page';
import { ExamRequestPage } from './exam-request-page';

import { AdminConsole } from './admin-console';
import { volumeCapabilityReason } from './volume-capability';
import { MAX_SERIES_PANES, seriesGridLayout } from './viewport-layout';
import { crossReferenceSegment } from './series-sync';
import {
  frameHasGeometry,
  seriesGeometryFrame,
  syncEligibility,
  syncFrameTargets,
  windowEquals,
  type SyncWindowLevel,
} from './sync-controller';
import {
  normalizedModality,
  parseWindowPresetSelection,
  userPresetsForModality,
  windowPresetMatchesState,
} from './window-presets';
import type { VolumeRenderer, VolumePreset, VolumeQuality } from './volume-renderer';

/** 扫描定位线配色：按序列 pane 索引取色，与多窗格序列体系保持区分度。 */
const SYNC_LINE_COLORS = [
  '#45d4e3',
  '#ffd166',
  '#ff7a9c',
  '#8ef29a',
  '#c792ea',
  '#ffab70',
  '#7fd6ff',
  '#f48fb1',
  '#a3f7bf',
];
import type {
  AiLabelDescriptor,
  AiModelDescriptor,
  AiPluginDescriptor,
  AiPluginConfiguration,
  AiSegmentationProgress,
  AiSegmentationResult,
  Annotation,
  AnnotationKind,
  DicomRevision,
  FrameMetadata,
  DownloadProgress,
  MprBuildProgress,
  MprMetadata,
  MprPlane,
  MprPlaneMetadata,
  MprProjectionMode,
  MprViewportState,
  MaskTool,
  PatientSummary,
  PatientPoint3D,
  Point,
  QueueStudyRow,
  RemoteSeriesSummary,
  RemoteUser,
  SegmentationProject,
  SegmentationSegment,
  SeriesMetadata,
  SharedAnnotationRecord,
  StudySummary,
  TransferProgress,
  TagRuleInput,
  ToolMode,
  TransformJob,
  TransformPreviewResponse,
  TransformSchema,
  TransformScope,
  TransformTargetType,
  UserWindowPreset,
  ViewerMode,
  ViewState,
  WindowPreset,
} from './types';

type DragState =
  | {
      kind: 'window';
      pointerId: number;
      start: Point;
      center: number;
      width: number;
    }
  | {
      kind: 'pan';
      pointerId: number;
      start: Point;
      panX: number;
      panY: number;
    }
  | { kind: 'annotation-create'; pointerId: number }
  | {
      kind: 'mask-paint';
      pointerId: number;
      segmentId: string;
      sourceSlice: number;
      previous: Point;
      before: MaskSliceSnapshot;
      changedSlices: Set<number>;
      value: 0 | 1;
    }
  | {
      kind: 'annotation-edit';
      pointerId: number;
      key: string;
      annotationId: string;
      handle: number | null;
      startImage: Point;
      original: Annotation;
      before: Annotation[];
    };

type MprDragState =
  | {
      kind: 'window';
      plane: MprPlane;
      pointerId: number;
      start: Point;
      center: number;
      width: number;
    }
  | {
      kind: 'pan';
      plane: MprPlane;
      pointerId: number;
      start: Point;
      panX: number;
      panY: number;
    }
  | { kind: 'crosshair' | 'annotation-create'; plane: MprPlane; pointerId: number }
  | { kind: 'oblique-rotate'; plane: MprPlane; pointerId: number; center: Point; lastAngle: number }
  | {
      kind: 'mask-paint';
      plane: MprPlane;
      pointerId: number;
      segmentId: string;
      previous: Point;
      before: MaskSliceSnapshot;
      changedSlices: Set<number>;
      value: 0 | 1;
    }
  | {
      kind: 'annotation-edit';
      plane: MprPlane;
      pointerId: number;
      key: string;
      annotationId: string;
      handle: number | null;
      startImage: Point;
      original: Annotation;
      before: Annotation[];
    };

interface ObliquePlaneState {
  normal: [number, number, number];
  xAxis: [number, number, number];
  yAxis: [number, number, number];
}

interface MprSession {
  metadata: MprMetadata;
  crosshair: PatientPoint3D;
  mainPlane: MprPlane;
  activePlane: MprPlane;
  obliquePlanes: Record<MprPlane, ObliquePlaneState>;
  viewports: Record<MprPlane, MprViewportState>;
}

interface TagEditorContext {
  targetType: TransformTargetType;
  targetKey: string;
  scope: TransformScope;
  title: string;
  values: Record<string, string | number | null | undefined>;
}

interface MaskHistoryEntry {
  segmentId: string;
  before: MaskSliceSnapshot;
  after: MaskSliceSnapshot;
}

const FRONTEND_CACHE_BYTES = 128 * 1024 * 1024;
const FRAME_CACHE_TTL_MS = 3 * 60 * 1000;
const FRAME_PREFETCH_CONCURRENCY = 3;
const PATIENT_PAGE_SIZE = 30;
const STANDARD_MPR_PLANES: readonly MprPlane[] = ['axial', 'coronal', 'sagittal'];
const MPR_PLANES: readonly MprPlane[] = ['axial', 'coronal', 'sagittal'];
const MPR_WHEEL_THRESHOLD = 30;
const TAG_LABELS: Record<string, string> = {
  PatientName: '患者姓名',
  PatientID: '患者 ID',
  IssuerOfPatientID: '患者 ID 发放机构',
  PatientBirthDate: '出生日期',
  PatientSex: '性别',
  AccessionNumber: '检查号',
  StudyID: '检查 ID',
  StudyDescription: '检查描述',
  ReferringPhysicianName: '申请医师',
  SeriesDescription: '序列描述',
  SeriesNumber: '序列号',
  BodyPartExamined: '检查部位',
  ProtocolName: '扫描协议',
};

export interface MprWheelAccumulator {
  x: number;
  y: number;
}

interface SeriesPane {
  index: number;
  element: HTMLElement;
  imageCanvas: HTMLCanvasElement;
  overlayCanvas: HTMLCanvasElement;
  renderer: Renderer;
  state: ViewState | null;
  measurements: Map<string, Annotation[]>;
  draft: Annotation | null;
  selectedMeasurementId: string | null;
  annotationHistory: AnnotationHistory;
  annotationsVisible: boolean;
  angleAwaitingEnd: boolean;
  remoteSeriesOpen: boolean;
  sharedAnnotationRecords: Map<string, SharedAnnotationRecord>;
  annotationSyncCursor: string | null;
  frameRequests: RequestVersion;
  lutRequests: RequestVersion;
  windowFrameRequest: number | null;
  wheelFrameDelta: number;
  syncExcludedReason: string | null;
  syncManualExcluded: boolean;
  syncBadge: HTMLElement;
  infoTitle: HTMLElement;
  infoMeta: HTMLElement;
  infoFrame: HTMLElement;
  closeButton: HTMLButtonElement;
  dropHint: HTMLElement;
}

interface SeriesDragPayload {
  studyUid: string;
  seriesUid: string;
}

interface SeriesPointerDrag {
  pointerId: number;
  startX: number;
  startY: number;
  started: boolean;
  payload: SeriesDragPayload;
}

const SERIES_DRAG_MIME = 'application/x-aetheris-series';
const SERIES_POINTER_DRAG_THRESHOLD = 6;
const WORKLIST_WIDTH_STORAGE_KEY = 'remote-pacs.worklist-width';
const WORKLIST_MIN_WIDTH = 280;
const WORKLIST_MAX_WIDTH = 520;
const DETAILS_WIDTH_STORAGE_KEY = 'remote-pacs.details-width';
const DETAILS_MIN_WIDTH = 240;
const DETAILS_MAX_WIDTH = 420;
const STATUS_BANNER_TIMEOUT_MS = 5_000;

export class App {
  private panes: SeriesPane[] = [];
  private activePaneIndex = 0;
  private mprRenderers: Record<MprPlane, Renderer>;
  private viewerMode: ViewerMode = '2d';
  private mpr: MprSession | null = null;
  private mprMeasurements = new Map<string, Annotation[]>();
  private mprDraft: { plane: MprPlane; measurement: Annotation } | null = null;
  private mprDrag: MprDragState | null = null;
  private mprRequests: Record<MprPlane, RequestVersion> = {
    axial: new RequestVersion(),
    coronal: new RequestVersion(),
    sagittal: new RequestVersion(),
    oblique: new RequestVersion(),
  };
  private mprRequestPending: Record<MprPlane, boolean> = {
    axial: false,
    coronal: false,
    sagittal: false,
    oblique: false,
  };
  private mprReloadQueued: Record<MprPlane, boolean> = {
    axial: false,
    coronal: false,
    sagittal: false,
    oblique: false,
  };
  private mprWheelDelta: Record<MprPlane, MprWheelAccumulator> = {
    axial: { x: 0, y: 0 },
    coronal: { x: 0, y: 0 },
    sagittal: { x: 0, y: 0 },
    oblique: { x: 0, y: 0 },
  };
  private mprWindowFrameRequest: number | null = null;
  private mprPrefetchTimer: number | null = null;
  private mprPrefetchRun = 0;
  private mprPrefetchActive = false;
  private mprPrefetchCancellation: Promise<void> = Promise.resolve();
  private mprBuildActive = false;
  private mprProjection: MprProjectionMode = 'slice';
  private mprSlabThicknessMm = 10;
  private mprObliqueMode = false;
  private mprObliqueVolumePromise: Promise<void> | null = null;
  private mprObliqueAbort: AbortController | null = null;
  private gpuMprRenderer: GpuMprRenderer | null = null;
  private volumeRenderer: VolumeRenderer | null = null;
  private volumeAbort: AbortController | null = null;
  private volumePreset: VolumePreset = 'soft_tissue';
  private volumeQuality: VolumeQuality = 'medium';
  private volumeWindowCenter = 0;
  private volumeWindowWidth = 1;
  private volumeWindowDrag: {
    pointerId: number;
    startX: number;
    startY: number;
    center: number;
    width: number;
  } | null = null;
  private frameCache = new ByteLruCache<string>(FRONTEND_CACHE_BYTES, FRAME_CACHE_TTL_MS);
  private pendingFrames = new Map<string, Promise<ArrayBuffer>>();
  private framePrefetchGeneration = 0;
  private framePrefetchKey: string | null = null;
  private framePrefetchCompletedAt = 0;
  private framePrefetchAbort: AbortController | null = null;
  private annotationSyncTimer: number | null = null;
  private annotationSyncActive = false;
  private annotationSyncQueues = new Map<string, Promise<void>>();
  private annotationSyncRetries = new Map<string, { generation: number; operation: () => Promise<void> }>();
  private annotationSyncGeneration = 0;
  private segmentationProjects: SegmentationProject[] = [];
  private segmentationSegments: SegmentationSegment[] = [];
  private segmentationSegment: SegmentationSegment | null = null;
  private maskVolumes = new Map<string, MaskVolume>();
  private maskUndoEntries: MaskHistoryEntry[] = [];
  private maskRedoEntries: MaskHistoryEntry[] = [];
  private maskDirtySlices = new Map<string, Set<number>>();
  private maskSyncingSegments = new Set<string>();
  private maskSyncErrors = new Set<string>();
  private maskWorkspacePromise: Promise<void> | null = null;
  private maskTagFilter = '';
  private maskMatchedSegmentIds: Set<string> | null = null;
  private maskTagQueryGeneration = 0;
  private maskTagSaving = false;
  private maskDeleting = false;
  private maskBrushRadius = 5;
  private maskOpacity = 0.38;
  private aiModels: AiModelDescriptor[] = [];
  private aiPlugins: AiPluginDescriptor[] = [];
  private aiSelectedModelId = '';
  private aiModelsLoaded = false;
  private aiModelsLoading = false;
  private aiRunning = false;
  private aiStatus = '';
  private drag: DragState | null = null;
  private cinePlaying = false;
  private cineSpeed = 1;
  private cineTimer: number | null = null;
  private statusBannerTimer: number | null = null;
  private busy = false;
  private remoteDownloadActive = false;
  private remoteUser: RemoteUser | null = null;
  private userWindowPresets: UserWindowPreset[] = [];
  private windowPresetDialogMode: 'create' | 'rename' = 'create';
  private windowPresetEditingId: number | null = null;
  private windowPresetBusy = false;
  private patients: PatientSummary[] = [];
  private patientContext: { key: number; patientId: string; name: string | null } | null = null;
  private patientPage = 0;
  private hasNextPatientPage = false;
  private expandedPatientId: number | null = null;
  private expandedStudyUid: string | null = null;
  private studies = new Map<number, StudySummary[]>();
  private series = new Map<string, RemoteSeriesSummary[]>();
  private worklistBusy = false;
  private transferActive = false;
  private transferKind: 'exports' | null = null;
  private transformSchema: TransformSchema | null = null;
  private tagEditorContext: TagEditorContext | null = null;
  private transformPreview: TransformPreviewResponse | null = null;
  private transformTaskTimer: number | null = null;
  private observedCompletedTransformJobs = new Set<string>();
  private revisions: DicomRevision[] = [];
  private selectedRollbackRevision: DicomRevision | null = null;
  private rollbackPreview: TransformPreviewResponse | null = null;
  private routerPanel: RouterPanel;
  private queuePage: QueuePage;
  private examRequestPage: ExamRequestPage;

  private adminConsole: AdminConsole;
  private lifecyclePanel: LifecyclePanel;
  private shareStudyUid: string | null = null;
  private syncScrollEnabled = true;
  private syncWindowEnabled = true;
  private syncPropagating = false;
  private worklistResize: { pointerId: number; startX: number; startWidth: number } | null = null;
  private worklistResizeFrame: number | null = null;
  private detailsResize: { pointerId: number; startX: number; startWidth: number } | null = null;
  private detailsResizeFrame: number | null = null;

  private get activePane(): SeriesPane {
    const pane = this.panes[this.activePaneIndex];
    if (pane === undefined) throw new Error('阅片窗格尚未初始化');
    return pane;
  }

  private get state(): ViewState | null {
    return this.panes[this.activePaneIndex]?.state ?? null;
  }

  private set state(value: ViewState | null) {
    const pane = this.panes[this.activePaneIndex];
    if (pane) pane.state = value;
    if (value) this.pushReportContext();
  }

  private get renderer(): Renderer {
    return this.activePane.renderer;
  }

  private get measurements(): Map<string, Annotation[]> {
    return this.activePane.measurements;
  }

  private set measurements(value: Map<string, Annotation[]>) {
    this.activePane.measurements = value;
  }

  private get draft(): Annotation | null {
    return this.activePane.draft;
  }

  private set draft(value: Annotation | null) {
    this.activePane.draft = value;
  }

  private get selectedMeasurementId(): string | null {
    return this.activePane.selectedMeasurementId;
  }

  private set selectedMeasurementId(value: string | null) {
    this.activePane.selectedMeasurementId = value;
  }

  private get annotationHistory(): AnnotationHistory {
    return this.activePane.annotationHistory;
  }

  private get annotationsVisible(): boolean {
    return this.activePane.annotationsVisible;
  }

  private set annotationsVisible(value: boolean) {
    this.activePane.annotationsVisible = value;
  }

  private get angleAwaitingEnd(): boolean {
    return this.activePane.angleAwaitingEnd;
  }

  private set angleAwaitingEnd(value: boolean) {
    this.activePane.angleAwaitingEnd = value;
  }

  private get remoteSeriesOpen(): boolean {
    return this.activePane.remoteSeriesOpen;
  }

  private set remoteSeriesOpen(value: boolean) {
    this.activePane.remoteSeriesOpen = value;
  }

  private get sharedAnnotationRecords(): Map<string, SharedAnnotationRecord> {
    return this.activePane.sharedAnnotationRecords;
  }

  private set sharedAnnotationRecords(value: Map<string, SharedAnnotationRecord>) {
    this.activePane.sharedAnnotationRecords = value;
  }

  private get annotationSyncCursor(): string | null {
    return this.activePane.annotationSyncCursor;
  }

  private set annotationSyncCursor(value: string | null) {
    this.activePane.annotationSyncCursor = value;
  }

  private get frameRequests(): RequestVersion {
    return this.activePane.frameRequests;
  }

  private get lutRequests(): RequestVersion {
    return this.activePane.lutRequests;
  }

  private get windowFrameRequest(): number | null {
    return this.activePane.windowFrameRequest;
  }

  private set windowFrameRequest(value: number | null) {
    this.activePane.windowFrameRequest = value;
  }

  private get wheelFrameDelta(): number {
    return this.activePane.wheelFrameDelta;
  }

  private set wheelFrameDelta(value: number) {
    this.activePane.wheelFrameDelta = value;
  }

  private get multiPane(): boolean {
    return this.panes.filter((pane) => pane.state != null).length > 1;
  }

  private viewport = requiredElement<HTMLElement>('viewport');
  private seriesGrid = requiredElement<HTMLElement>('series-grid');
  private overlayCanvas = requiredElement<HTMLCanvasElement>('overlay-canvas');
  private volumeCanvas = requiredElement<HTMLCanvasElement>('vr-canvas');
  private frameSlider = requiredElement<HTMLInputElement>('frame-slider');
  private cineSpeedSelect = requiredElement<HTMLSelectElement>('cine-speed');
  private presetSelect = requiredElement<HTMLSelectElement>('preset-select');
  private windowPresetDialog = requiredElement<HTMLDialogElement>('window-preset-dialog');
  private imageStackSelect = requiredElement<HTMLSelectElement>('image-stack-select');
  private mprSourceSelect = requiredElement<HTMLSelectElement>('mpr-source-select');
  private mprSlabThickness = requiredElement<HTMLInputElement>('mpr-slab-thickness');
  private errorBanner = requiredElement<HTMLElement>('error-banner');
  private tagEditorDialog = requiredElement<HTMLDialogElement>('tag-editor-dialog');
  private transformTasksDialog = requiredElement<HTMLDialogElement>('transform-tasks-dialog');
  private revisionHistoryDialog = requiredElement<HTMLDialogElement>('revision-history-dialog');

  constructor() {
    this.routerPanel = new RouterPanel((message) => this.showError(message));
    requiredElement<HTMLButtonElement>('report-panel-btn').addEventListener('click', () => {
      void this.openReportWindow();
    });
    this.adminConsole = new AdminConsole((message) => this.showError(message));
    this.lifecyclePanel = new LifecyclePanel((message) => this.showError(message));
    this.queuePage = new QueuePage({
      openStudy: (row, seriesUid, series) => this.openQueueStudy(row, seriesUid, series),
      recommendSeries: recommendMprSeries,
      canEditTags: () => this.canEditDicomTags(),
      editStudyTags: (row) => this.editQueueStudyTags(row),
      canCreateExamRequestForStudy: () => this.canManageExamRequests(),
      createExamRequestForStudy: (row) => this.examRequestPage.openForStudy(row),
      canReturnToViewer: () => this.panes.some((pane) => pane.state != null),
    });
    this.examRequestPage = new ExamRequestPage({
      canReturnToViewer: () => this.panes.some((pane) => pane.state != null),
      canCreateForStudy: () => this.canManageExamRequests(),
      beforeOpen: () => this.queuePage.close(),
      onStudyRequestCreated: () => this.queuePage.refresh(),
      onClose: () => {
        if (this.remoteUser && !this.panes.some((pane) => pane.state != null)) this.queuePage.open();
      },
    });
    this.initializePanes();
    this.mprRenderers = {
      axial: new Renderer(
        requiredElement<HTMLCanvasElement>('axial-image-canvas'),
        requiredElement<HTMLCanvasElement>('axial-overlay-canvas'),
      ),
      coronal: new Renderer(
        requiredElement<HTMLCanvasElement>('coronal-image-canvas'),
        requiredElement<HTMLCanvasElement>('coronal-overlay-canvas'),
      ),
      sagittal: new Renderer(
        requiredElement<HTMLCanvasElement>('sagittal-image-canvas'),
        requiredElement<HTMLCanvasElement>('sagittal-overlay-canvas'),
      ),
      oblique: new Renderer(
        document.createElement('canvas'),
        document.createElement('canvas'),
      ),
    };
    this.setupEventListeners();
    this.setupWorklistResizer();
    this.setupDetailsResizer();
    this.setupSeriesDropFallback();
    this.setupRemoteProgress();
    this.setupResizeObserver();
    this.restoreConnectionFields();
    this.updateUi();
    void this.autoLoginLocal();
  }

  private setupWorklistResizer(): void {
    const workspace = requiredElement<HTMLElement>('workspace');
    const resizer = requiredElement<HTMLElement>('worklist-resizer');
    const savedWidth = Number.parseInt(localStorage.getItem(WORKLIST_WIDTH_STORAGE_KEY) ?? '', 10);
    this.setWorklistWidth(Number.isFinite(savedWidth) ? savedWidth : 330);

    const stopResize = (event: PointerEvent): void => {
      if (!this.worklistResize || this.worklistResize.pointerId !== event.pointerId) return;
      if (resizer.hasPointerCapture(event.pointerId)) resizer.releasePointerCapture(event.pointerId);
      this.worklistResize = null;
      workspace.classList.remove('is-resizing');
      if (this.worklistResizeFrame !== null) {
        cancelAnimationFrame(this.worklistResizeFrame);
        this.worklistResizeFrame = null;
      }
      localStorage.setItem(WORKLIST_WIDTH_STORAGE_KEY, String(this.worklistWidth()));
      this.resizeViewport();
    };

    resizer.addEventListener('pointerdown', (event) => {
      if (event.button !== 0 || workspace.classList.contains('worklist-hidden')) return;
      const width = this.worklistWidth();
      this.worklistResize = { pointerId: event.pointerId, startX: event.clientX, startWidth: width };
      workspace.classList.add('is-resizing');
      resizer.setPointerCapture(event.pointerId);
      event.preventDefault();
    });
    resizer.addEventListener('pointermove', (event) => {
      const drag = this.worklistResize;
      if (!drag || drag.pointerId !== event.pointerId) return;
      const width = clampWorklistWidth(drag.startWidth + event.clientX - drag.startX);
      if (this.worklistResizeFrame !== null) cancelAnimationFrame(this.worklistResizeFrame);
      this.worklistResizeFrame = requestAnimationFrame(() => {
        this.setWorklistWidth(width);
        this.worklistResizeFrame = null;
      });
    });
    resizer.addEventListener('pointerup', stopResize);
    resizer.addEventListener('pointercancel', stopResize);
    resizer.addEventListener('keydown', (event) => {
      if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight' && event.key !== 'Home' && event.key !== 'End') return;
      const current = this.worklistWidth();
      const next = event.key === 'Home'
        ? WORKLIST_MIN_WIDTH
        : event.key === 'End'
          ? WORKLIST_MAX_WIDTH
          : current + (event.key === 'ArrowLeft' ? -16 : 16);
      this.setWorklistWidth(next);
      localStorage.setItem(WORKLIST_WIDTH_STORAGE_KEY, String(this.worklistWidth()));
      event.preventDefault();
    });
  }

  private setupDetailsResizer(): void {
    const workspace = requiredElement<HTMLElement>('workspace');
    const resizer = requiredElement<HTMLElement>('details-resizer');
    const savedWidth = Number.parseInt(localStorage.getItem(DETAILS_WIDTH_STORAGE_KEY) ?? '', 10);
    this.setDetailsWidth(Number.isFinite(savedWidth) ? savedWidth : 278);

    const stopResize = (event: PointerEvent): void => {
      if (!this.detailsResize || this.detailsResize.pointerId !== event.pointerId) return;
      if (resizer.hasPointerCapture(event.pointerId)) resizer.releasePointerCapture(event.pointerId);
      this.detailsResize = null;
      workspace.classList.remove('is-resizing-details');
      if (this.detailsResizeFrame !== null) {
        cancelAnimationFrame(this.detailsResizeFrame);
        this.detailsResizeFrame = null;
      }
      localStorage.setItem(DETAILS_WIDTH_STORAGE_KEY, String(this.detailsWidth()));
      this.resizeViewport();
    };

    resizer.addEventListener('pointerdown', (event) => {
      if (event.button !== 0 || workspace.classList.contains('details-hidden')) return;
      this.detailsResize = { pointerId: event.pointerId, startX: event.clientX, startWidth: this.detailsWidth() };
      workspace.classList.add('is-resizing-details');
      resizer.setPointerCapture(event.pointerId);
      event.preventDefault();
    });
    resizer.addEventListener('pointermove', (event) => {
      const drag = this.detailsResize;
      if (!drag || drag.pointerId !== event.pointerId) return;
      const width = clampDetailsWidth(drag.startWidth - (event.clientX - drag.startX));
      if (this.detailsResizeFrame !== null) cancelAnimationFrame(this.detailsResizeFrame);
      this.detailsResizeFrame = requestAnimationFrame(() => {
        this.setDetailsWidth(width);
        this.detailsResizeFrame = null;
      });
    });
    resizer.addEventListener('pointerup', stopResize);
    resizer.addEventListener('pointercancel', stopResize);
    resizer.addEventListener('keydown', (event) => {
      if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight' && event.key !== 'Home' && event.key !== 'End') return;
      const current = this.detailsWidth();
      const next = event.key === 'Home'
        ? DETAILS_MIN_WIDTH
        : event.key === 'End'
          ? DETAILS_MAX_WIDTH
          : current + (event.key === 'ArrowLeft' ? 16 : -16);
      this.setDetailsWidth(next);
      localStorage.setItem(DETAILS_WIDTH_STORAGE_KEY, String(this.detailsWidth()));
      event.preventDefault();
    });
  }

  private detailsWidth(): number {
    const workspace = requiredElement<HTMLElement>('workspace');
    const columns = getComputedStyle(workspace).gridTemplateColumns.split(' ');
    const value = Number.parseFloat(columns[columns.length - 1] ?? '');
    return Number.isFinite(value) ? clampDetailsWidth(value) : 278;
  }

  private setDetailsWidth(width: number): void {
    const workspace = requiredElement<HTMLElement>('workspace');
    const next = clampDetailsWidth(width);
    workspace.style.setProperty('--details-width', `${next}px`);
    requiredElement<HTMLElement>('details-resizer').setAttribute('aria-valuenow', String(next));
  }

  private worklistWidth(): number {
    const workspace = requiredElement<HTMLElement>('workspace');
    const value = Number.parseFloat(getComputedStyle(workspace).getPropertyValue('--worklist-width'));
    return Number.isFinite(value) ? clampWorklistWidth(value) : 330;
  }

  private setWorklistWidth(width: number): void {
    const workspace = requiredElement<HTMLElement>('workspace');
    const next = clampWorklistWidth(width);
    workspace.style.setProperty('--worklist-width', `${next}px`);
    requiredElement<HTMLElement>('worklist-resizer').setAttribute('aria-valuenow', String(next));
  }

  private initializePanes(): void {
    const element = requiredElement<HTMLElement>('series-pane-0');
    const pane = this.createSeriesPane(
      0,
      element,
      requiredElement<HTMLCanvasElement>('image-canvas'),
      this.overlayCanvas,
      element.querySelector<HTMLElement>('.pane-title') ?? document.createElement('strong'),
      element.querySelector<HTMLElement>('.pane-meta') ?? document.createElement('span'),
      element.querySelector<HTMLElement>('.pane-frame') ?? document.createElement('span'),
      element.querySelector<HTMLButtonElement>('.pane-close') ?? document.createElement('button'),
      element.querySelector<HTMLElement>('.pane-drop-hint') ?? document.createElement('div'),
    );
    this.panes = [pane];
    this.applyPaneLayout();
  }

  private createSeriesPane(
    index: number,
    element: HTMLElement,
    imageCanvas: HTMLCanvasElement,
    overlayCanvas: HTMLCanvasElement,
    infoTitle: HTMLElement,
    infoMeta: HTMLElement,
    infoFrame: HTMLElement,
    closeButton: HTMLButtonElement,
    dropHint: HTMLElement,
  ): SeriesPane {
    element.className = 'series-pane';
    if (index === 0) element.classList.add('active');
    element.dataset.paneIndex = String(index);
    element.setAttribute('aria-label', `阅片窗格 ${index + 1}`);
    imageCanvas.classList.add('pane-image-canvas');
    overlayCanvas.classList.add('pane-overlay-canvas');
    closeButton.type = 'button';
    closeButton.className = 'pane-close';
    closeButton.title = '关闭此窗格';
    closeButton.setAttribute('aria-label', closeButton.title);
    closeButton.textContent = '×';
    closeButton.hidden = false;
    closeButton.style.display = 'none';
    dropHint.className = 'pane-drop-hint';
    dropHint.textContent = '拖入序列';
    dropHint.hidden = false;
    const syncBadge = document.createElement('span');
    syncBadge.className = 'pane-sync-badge';
    syncBadge.hidden = true;
    syncBadge.title = '点击加入/退出联动同步';
    element.append(syncBadge);
    const pane: SeriesPane = {
      index,
      element,
      imageCanvas,
      overlayCanvas,
      renderer: new Renderer(imageCanvas, overlayCanvas),
      state: null,
      measurements: new Map(),
      draft: null,
      selectedMeasurementId: null,
      annotationHistory: new AnnotationHistory(),
      annotationsVisible: true,
      angleAwaitingEnd: false,
      remoteSeriesOpen: false,
      sharedAnnotationRecords: new Map(),
      annotationSyncCursor: null,
      frameRequests: new RequestVersion(),
      lutRequests: new RequestVersion(),
      windowFrameRequest: null,
      wheelFrameDelta: 0,
      syncExcludedReason: null,
      syncManualExcluded: false,
      syncBadge,
      infoTitle,
      infoMeta,
      infoFrame,
      closeButton,
      dropHint,
    };
    this.bindPaneEvents(pane);
    syncBadge.addEventListener('click', (event) => {
      event.stopPropagation();
      pane.syncManualExcluded = !pane.syncManualExcluded;
      this.refreshSyncBadges();
    });
    closeButton.addEventListener('click', (event) => {
      event.stopPropagation();
      void this.closePane(index);
    });
    element.addEventListener('click', () => {
      if (!this.busy && this.activePane !== pane) this.activatePane(this.paneIndex(pane));
    });
    return pane;
  }

  private appendEmptyPane(): SeriesPane {
    const index = this.panes.length;
    const element = document.createElement('section');
    const imageCanvas = document.createElement('canvas');
    const overlayCanvas = document.createElement('canvas');
    const infoTitle = document.createElement('strong');
    infoTitle.className = 'pane-title';
    const infoMeta = document.createElement('span');
    infoMeta.className = 'pane-meta';
    const infoFrame = document.createElement('span');
    infoFrame.className = 'pane-frame';
    const infoLeft = document.createElement('div');
    infoLeft.className = 'pane-info pane-info-top-left';
    infoLeft.append(infoTitle, infoMeta);
    const infoRight = document.createElement('div');
    infoRight.className = 'pane-info pane-info-top-right';
    infoRight.append(infoFrame);
    const closeButton = document.createElement('button');
    const dropHint = document.createElement('div');
    element.append(imageCanvas, overlayCanvas, infoLeft, infoRight, closeButton, dropHint);
    this.seriesGrid.append(element);
    const pane = this.createSeriesPane(
      index,
      element,
      imageCanvas,
      overlayCanvas,
      infoTitle,
      infoMeta,
      infoFrame,
      closeButton,
      dropHint,
    );
    this.panes.push(pane);
    return pane;
  }

  private bindPaneEvents(pane: SeriesPane): void {
    const canvas = pane.overlayCanvas;
    canvas.addEventListener('contextmenu', (event) => event.preventDefault());
    canvas.addEventListener('pointerdown', (event) => this.panePointerDown(pane, event));
    canvas.addEventListener('pointermove', (event) => this.pointerMove(event));
    canvas.addEventListener('pointerup', (event) => this.pointerUp(event));
    canvas.addEventListener('pointercancel', (event) => this.pointerUp(event));
    canvas.addEventListener('wheel', (event) => this.paneWheel(pane, event), { passive: false });
    pane.element.addEventListener('dragover', (event) => this.paneDragOver(pane, event));
    pane.element.addEventListener('dragleave', (event) => this.paneDragLeave(pane, event));
    pane.element.addEventListener('drop', (event) => void this.paneDrop(pane, event));
  }

  /** 当前活动窗格的检查上下文（报告小窗用）。 */
  private reportContext(): ReportWindowContext | null {
    const state = this.state;
    if (!state || !this.remoteSeriesOpen) return null;
    const patient = state.metadata.patient;
    return {
      studyUid: state.metadata.study_uid ?? '',
      seriesUid: state.metadata.series_uid ?? '',
      modality: patient.modality,
      patientName: formatPersonName(patient.patient_name) || patient.patient_id || '未提供',
      patientId: patient.patient_id,
      patientSex: patient.patient_sex,
      patientBirthDate: patient.patient_birth_date,
      studyDate: patient.study_date,
      studyDescription: patient.study_description,
      seriesDescription: patient.series_description,
      institutionName: this.remoteUser?.institution_name ?? '',
      user: {
        id: this.remoteUser?.id ?? null,
        role: this.remoteUser?.role ?? null,
        displayName: this.remoteUser?.display_name ?? null,
        username: this.remoteUser?.username ?? null,
      },
    };
  }

  private async openReportWindow(): Promise<void> {
    const context = this.reportContext();
    if (!context) return;
    await openReportWindow(context);
  }

  /** 序列切换/打开时把上下文推送给已打开的报告小窗。 */
  private pushReportContext(): void {
    const context = this.reportContext();
    if (context) void updateReportContext(context);
  }

  private bindSyncControls(): void {    const scrollButton = requiredElement<HTMLButtonElement>('sync-scroll-button');
    const windowButton = requiredElement<HTMLButtonElement>('sync-window-button');
    scrollButton.addEventListener('click', () => {
      this.syncScrollEnabled = !this.syncScrollEnabled;
      scrollButton.classList.toggle('active', this.syncScrollEnabled);
      scrollButton.setAttribute('aria-pressed', String(this.syncScrollEnabled));
      this.refreshSyncBadges();
      this.render();
    });
    windowButton.addEventListener('click', () => {
      this.syncWindowEnabled = !this.syncWindowEnabled;
      windowButton.classList.toggle('active', this.syncWindowEnabled);
      windowButton.setAttribute('aria-pressed', String(this.syncWindowEnabled));
    });
  }

  private setupSeriesDropFallback(): void {
    this.viewport.addEventListener('dragover', (event) => {
      if (!event.dataTransfer?.types.includes(SERIES_DRAG_MIME)) return;
      event.preventDefault();
      this.seriesDragActive = true;
      this.applyPaneLayout();
    });
    this.viewport.addEventListener('dragleave', (event) => {
      if (!event.dataTransfer?.types.includes(SERIES_DRAG_MIME)) return;
      if (event.relatedTarget instanceof Node && this.viewport.contains(event.relatedTarget)) return;
      this.seriesDragActive = false;
      this.applyPaneLayout();
    });
    this.viewport.addEventListener('drop', (event) => {
      const payload = this.seriesDragPayload(event);
      if (!payload) return;
      event.preventDefault();
      const target = event.target instanceof Element ? event.target.closest('.series-pane') : null;
      const targetPane = target ? this.panes.find((candidate) => candidate.element === target) : null;
      if (targetPane) {
        void this.paneDrop(targetPane, event);
        return;
      }
      const pane = this.panes.find((candidate) => candidate.state == null) ?? this.panes[0];
      if (pane) void this.paneDrop(pane, event);
    });
  }

  private panePointerDown(pane: SeriesPane, event: PointerEvent): void {
    if (this.activePane !== pane && !this.busy && !this.remoteDownloadActive) {
      this.activatePane(this.paneIndex(pane));
    }
    if (this.activePane !== pane || !this.state || this.busy) return;
    this.pointerDown(event);
  }

  private paneWheel(pane: SeriesPane, event: WheelEvent): void {
    if (this.activePane !== pane && !this.busy && !this.remoteDownloadActive) {
      this.activatePane(this.paneIndex(pane));
    }
    if (this.activePane !== pane) return;
    this.wheel(event);
  }

  private paneIndex(pane: SeriesPane): number {
    const index = this.panes.indexOf(pane);
    return index >= 0 ? index : pane.index;
  }

  private seriesPaneCount(): number {
    return this.panes.filter((pane) => pane.state != null).length;
  }

  private ensurePaneSlots(): void {
    const desired = Math.max(1, seriesGridLayout(this.seriesPaneCount()).slots);
    while (this.panes.length < desired) this.appendEmptyPane();
    while (this.panes.length > desired) {
      const last = this.panes[this.panes.length - 1];
      if (!last || last.state) break;
      last.element.remove();
      this.panes.pop();
    }
    this.applyPaneLayout();
    this.resizeViewport();
  }

  private applyPaneLayout(): void {
    const seriesCount = this.seriesPaneCount();
    const effectiveCount = Math.min(MAX_SERIES_PANES, Math.max(seriesCount, this.panes.length));
    const layout = seriesGridLayout(effectiveCount);
    const columns = layout.columns;
    const rows = Math.max(1, Math.ceil(this.panes.length / columns));
    this.seriesGrid.style.gridTemplateColumns = `repeat(${columns}, minmax(0, 1fr))`;
    this.seriesGrid.style.gridTemplateRows = `repeat(${rows}, minmax(0, 1fr))`;
    this.seriesGrid.classList.toggle('single-pane', this.panes.length === 1);
    this.viewport.classList.toggle('multi-pane', seriesCount > 1);
    this.panes.forEach((pane, index) => {
      pane.index = index;
      pane.element.dataset.paneIndex = String(index);
      pane.element.setAttribute('aria-label', `阅片窗格 ${index + 1}`);
      pane.element.classList.toggle('active', index === this.activePaneIndex);
      pane.element.classList.toggle('empty', pane.state == null);
    });
    const dropHint = requiredElement<HTMLElement>('series-drop-hint');
    dropHint.hidden = !this.seriesDragActive;
  }

  private seriesDragActive = false;
  private seriesPointerDrag: SeriesPointerDrag | null = null;
  private seriesClickSuppressedUntil = 0;

  private switchActivePane(index: number): SeriesPane | null {
    const target = this.panes[index];
    if (!target || index === this.activePaneIndex || this.busy || this.remoteDownloadActive) return null;
    this.stopCine();
    this.stopAnnotationSync();
    const previousPane = this.activePane;
    if (previousPane.windowFrameRequest != null) {
      cancelAnimationFrame(previousPane.windowFrameRequest);
      previousPane.windowFrameRequest = null;
    }
    if (this.viewerMode !== '2d' || this.mpr) this.leaveAdvancedViewModes();
    this.clearAdvancedWorkspaceState();
    this.activePaneIndex = index;
    this.viewerMode = '2d';
    this.updatePaneActivation();
    this.updatePaneLabels();
    return target;
  }

  private activatePane(index: number): void {
    const target = this.switchActivePane(index);
    if (!target) return;
    if (target.state) {
      target.renderer.clear();
      void this.loadCurrentFrame().then(() => {
        if (this.remoteSeriesOpen) {
          this.startAnnotationSync();
          void this.refreshSharedAnnotations();
        }
      }).catch((error) => this.showError(errorMessage(error)));
    } else {
      this.render();
    }
    this.updateUi();
    this.resizeViewport();
  }

  private leaveAdvancedViewModes(): void {
    this.stopMprPrefetch();
    this.disposeGpuMprRenderer();
    this.disposeVolumeRenderer();
    if (this.state && (this.state.tool === 'crosshair' || isMaskTool(this.state.tool))) {
      this.state.tool = 'window';
    }
    if (this.state) void closeMpr(this.state.metadata.handle).catch(() => undefined);
    this.mpr = null;
    this.mprMeasurements = new Map();
    this.mprDraft = null;
    this.mprDrag = null;
    this.mprProjection = 'slice';
    this.mprSlabThicknessMm = 10;
    this.invalidateMprRequests();
    this.viewerMode = '2d';
  }

  private clearAdvancedWorkspaceState(): void {
    this.segmentationProjects = [];
    this.segmentationSegments = [];
    this.segmentationSegment = null;
    this.maskVolumes = new Map();
    this.maskTagFilter = '';
    this.maskMatchedSegmentIds = null;
    this.maskTagQueryGeneration += 1;
    this.maskUndoEntries = [];
    this.maskRedoEntries = [];
    this.maskDirtySlices.clear();
    this.maskSyncingSegments.clear();
    this.maskSyncErrors.clear();
    this.maskWorkspacePromise = null;
    this.aiStatus = '';
    if (this.state && isMaskTool(this.state.tool)) this.state.tool = 'window';
  }

  private resetPaneState(pane: SeriesPane, closeHandle: boolean): void {
    const handle = pane.state?.metadata.handle ?? null;
    if (pane.windowFrameRequest != null) {
      cancelAnimationFrame(pane.windowFrameRequest);
      pane.windowFrameRequest = null;
    }
    pane.state = null;
    pane.measurements = new Map();
    pane.draft = null;
    pane.selectedMeasurementId = null;
    pane.annotationHistory.clear();
    pane.sharedAnnotationRecords = new Map();
    pane.annotationSyncCursor = null;
    pane.remoteSeriesOpen = false;
    pane.frameRequests.invalidate();
    pane.lutRequests.invalidate();
    pane.windowFrameRequest = null;
    pane.wheelFrameDelta = 0;
    pane.syncManualExcluded = false;
    pane.syncExcludedReason = null;
    pane.renderer.clear();
    if (closeHandle && handle) {
      void closeSeries(handle).catch((error) => this.showError(errorMessage(error)));
    }
  }

  private closePane(index: number): void {
    const pane = this.panes[index];
    if (!pane || this.busy || this.remoteDownloadActive) return;
    const wasActive = index === this.activePaneIndex;
    const activeBefore = this.activePane;
    this.stopCine();
    if (wasActive) this.stopAnnotationSync();
    if (wasActive && (this.viewerMode !== '2d' || this.mpr)) this.leaveAdvancedViewModes();
    if (wasActive) this.clearAdvancedWorkspaceState();
    this.resetPaneState(pane, true);

    // 最后一个窗格保留原始外壳，避免丢失全局 overlay 与固定 canvas。
    if (this.panes.length === 1) {
      this.activePaneIndex = 0;
      this.viewerMode = '2d';
      this.applyPaneLayout();
      this.updatePaneLabels();
      this.updateUi();
      this.render();
      return;
    }

    pane.element.remove();
    const remaining = this.panes.filter((candidate) => candidate !== pane);
    const filled = remaining.filter((candidate) => candidate.state != null);
    const empty = remaining.filter((candidate) => candidate.state == null);
    this.panes = [...filled, ...empty];
    if (this.panes.length === 0) this.appendEmptyPane();
    const desired = Math.max(1, seriesGridLayout(this.seriesPaneCount()).slots);
    while (this.panes.length < desired) this.appendEmptyPane();
    while (this.panes.length > desired) {
      const last = this.panes[this.panes.length - 1];
      if (!last || last.state) break;
      last.element.remove();
      this.panes.pop();
    }
    this.activePaneIndex = Math.max(0, this.panes.indexOf(activeBefore));
    if (wasActive) {
      const next = this.panes.find((candidate) => candidate.state != null) ?? this.panes[0];
      this.activePaneIndex = this.panes.indexOf(next ?? this.panes[0]);
      this.viewerMode = '2d';
      this.clearAdvancedWorkspaceState();
      if (this.state) {
        void this.loadCurrentFrame().then(() => {
          if (this.remoteSeriesOpen) {
            this.startAnnotationSync();
            void this.refreshSharedAnnotations();
          }
        }).catch((error) => this.showError(errorMessage(error)));
      }
    }
    this.applyPaneLayout();
    this.updatePaneLabels();
    this.resizeViewport();
    this.updateUi();
    this.render();
    if (this.state && !this.multiPane) {
      void this.loadSegmentationWorkspace()
        .then(() => this.updateUi())
        .catch((error) => this.showError(errorMessage(error)));
    }
  }

  private updatePaneActivation(): void {
    this.panes.forEach((pane, index) => {
      pane.element.classList.toggle('active', index === this.activePaneIndex);
      pane.element.classList.toggle('empty', pane.state == null);
    });
  }

  private updatePaneLabels(): void {
    for (const pane of this.panes) {
      const state = pane.state;
      const frame = state?.metadata.frames[state.currentFrame];
      if (!state || !frame) {
        pane.infoTitle.textContent = '未加载';
        pane.infoMeta.textContent = '拖入序列';
        pane.infoFrame.textContent = '0 / 0';
        pane.closeButton.style.display = 'none';
        continue;
      }
      const patient = state.metadata.patient;
      pane.infoTitle.textContent = formatPersonName(patient.patient_name) || '未提供';
      pane.infoMeta.textContent = [
        patient.modality,
        patient.series_description?.trim(),
        state.metadata.frames.length > 1 ? `${state.currentFrame + 1} / ${state.metadata.frames.length}` : '',
      ].filter(Boolean).join(' · ');
      pane.infoFrame.textContent = `${state.currentFrame + 1} / ${state.metadata.frames.length}`;
      pane.closeButton.style.display = this.seriesPaneCount() > 1 ? 'grid' : 'none';
    }
  }

  private paneDragOver(pane: SeriesPane, event: DragEvent): void {
    if (!event.dataTransfer?.types.includes(SERIES_DRAG_MIME)) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.dataTransfer) event.dataTransfer.dropEffect = event.altKey ? 'move' : 'copy';
    this.updateSeriesDropTarget(pane, event.altKey);
  }

  private paneDragLeave(pane: SeriesPane, event: DragEvent): void {
    if (!event.dataTransfer?.types.includes(SERIES_DRAG_MIME)) return;
    if (event.relatedTarget instanceof Node && pane.element.contains(event.relatedTarget)) return;
    pane.element.classList.remove('drop-target', 'drop-replace');
    if (![...this.panes].some((candidate) =>
      candidate.element.classList.contains('drop-target')
      || candidate.element.classList.contains('drop-replace')
    )) {
      this.seriesDragActive = false;
      this.applyPaneLayout();
    }
  }

  private async paneDrop(pane: SeriesPane, event: DragEvent): Promise<void> {
    const payload = this.seriesDragPayload(event);
    if (!payload) return;
    event.preventDefault();
    event.stopPropagation();
    await this.openDraggedSeries(pane, payload, event.altKey);
  }

  private seriesDragPayload(event: DragEvent): SeriesDragPayload | null {
    const raw = event.dataTransfer?.getData(SERIES_DRAG_MIME);
    if (!raw) return null;
    try {
      const value = JSON.parse(raw) as { studyUid?: unknown; seriesUid?: unknown };
      return typeof value.studyUid === 'string' && typeof value.seriesUid === 'string'
        ? { studyUid: value.studyUid, seriesUid: value.seriesUid }
        : null;
    } catch {
      return null;
    }
  }

  private updateSeriesDropTarget(pane: SeriesPane | null, altKey: boolean): void {
    this.seriesDragActive = true;
    for (const candidate of this.panes) {
      candidate.element.classList.toggle('drop-replace', candidate === pane && altKey && candidate.state != null);
      candidate.element.classList.toggle('drop-target', candidate === pane && (!altKey || candidate.state == null));
    }
    const hint = requiredElement<HTMLElement>('series-drop-hint').querySelector('span');
    if (hint) {
      hint.textContent = !pane
        ? '释放以添加序列并自动分屏'
        : pane.state == null
          ? '释放以在此窗格打开序列'
          : altKey
            ? '释放以替换此窗格的序列'
            : this.seriesPaneCount() >= MAX_SERIES_PANES
              ? '窗格已满，释放将替换此窗格'
              : '释放以添加序列并自动分屏';
    }
    this.applyPaneLayout();
  }

  private clearSeriesDropTargets(): void {
    for (const candidate of this.panes) {
      candidate.element.classList.remove('drop-target', 'drop-replace');
    }
    this.seriesDragActive = false;
    this.applyPaneLayout();
  }

  private async openDraggedSeries(
    pane: SeriesPane,
    payload: SeriesDragPayload,
    replaceRequested: boolean,
  ): Promise<void> {
    if (this.busy || this.remoteDownloadActive) {
      this.showError('当前正在加载序列，请稍后再拖入。');
      return;
    }
    this.clearSeriesDropTargets();
    const existingIndex = this.panes.findIndex((candidate) =>
      candidate.state?.metadata.series_uid === payload.seriesUid
      && candidate.state.metadata.study_uid === payload.studyUid
    );
    if (existingIndex >= 0) {
      this.activatePane(existingIndex);
      return;
    }
    const replace = replaceRequested && pane.state != null;
    if (replace || pane.state == null || this.seriesPaneCount() >= MAX_SERIES_PANES) {
      this.switchActivePane(this.paneIndex(pane));
      await this.openRemote(payload.studyUid, payload.seriesUid);
      return;
    }
    const count = this.seriesPaneCount();
    const desired = Math.max(count + 1, seriesGridLayout(count + 1).slots);
    while (this.panes.length < desired) this.appendEmptyPane();
    this.applyPaneLayout();
    this.resizeViewport();
    const emptyPane = this.panes.find((candidate) => candidate.state == null);
    if (emptyPane) {
      this.switchActivePane(this.paneIndex(emptyPane));
      await this.openRemote(payload.studyUid, payload.seriesUid);
    }
  }

  private beginSeriesPointerDrag(
    button: HTMLButtonElement,
    event: PointerEvent,
    payload: SeriesDragPayload,
  ): void {
    if (event.button !== 0 || this.busy || this.remoteDownloadActive) return;
    this.seriesPointerDrag = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      started: false,
      payload,
    };
    button.setPointerCapture(event.pointerId);
  }

  private moveSeriesPointerDrag(event: PointerEvent): void {
    const drag = this.seriesPointerDrag;
    if (!drag || drag.pointerId !== event.pointerId) return;
    if (!drag.started) {
      const distance = Math.hypot(event.clientX - drag.startX, event.clientY - drag.startY);
      if (distance < SERIES_POINTER_DRAG_THRESHOLD) return;
      drag.started = true;
      (event.currentTarget as HTMLElement | null)?.classList.add('dragging');
    }
    event.preventDefault();
    const viewportRect = this.viewport.getBoundingClientRect();
    const insideViewport = event.clientX >= viewportRect.left
      && event.clientX <= viewportRect.right
      && event.clientY >= viewportRect.top
      && event.clientY <= viewportRect.bottom;
    const pane = insideViewport ? this.seriesPaneAtPoint(event.clientX, event.clientY) : null;
    this.updateSeriesDropTarget(pane, event.altKey);
  }

  private finishSeriesPointerDrag(event: PointerEvent, cancelled: boolean): void {
    const drag = this.seriesPointerDrag;
    if (!drag || drag.pointerId !== event.pointerId) return;
    this.seriesPointerDrag = null;
    const button = event.currentTarget as HTMLElement | null;
    button?.classList.remove('dragging');
    if (button instanceof HTMLButtonElement && button.hasPointerCapture(event.pointerId)) {
      button.releasePointerCapture(event.pointerId);
    }
    if (cancelled) {
      this.clearSeriesDropTargets();
      return;
    }
    if (!drag.started) return;
    const viewportRect = this.viewport.getBoundingClientRect();
    const insideViewport = event.clientX >= viewportRect.left
      && event.clientX <= viewportRect.right
      && event.clientY >= viewportRect.top
      && event.clientY <= viewportRect.bottom;
    const pane = insideViewport
      ? (this.seriesPaneAtPoint(event.clientX, event.clientY) ?? this.panes.find((candidate) => candidate.state == null) ?? this.panes[0])
      : null;
    this.seriesClickSuppressedUntil = Date.now() + 350;
    if (pane) void this.openDraggedSeries(pane, drag.payload, event.altKey);
    else this.clearSeriesDropTargets();
  }

  private cancelSeriesPointerDrag(event: PointerEvent): void {
    this.finishSeriesPointerDrag(event, true);
  }

  private seriesPaneAtPoint(clientX: number, clientY: number): SeriesPane | null {
    const element = document.elementFromPoint(clientX, clientY);
    const paneElement = element instanceof Element ? element.closest('.series-pane') : null;
    return paneElement ? this.panes.find((candidate) => candidate.element === paneElement) ?? null : null;
  }

  async openFiles(): Promise<void> {
    try {
      const paths = await chooseDicomFiles();
      if (!paths?.length) return;
      await this.activateSeries(() => openSeries(paths), '正在解析序列...');
      this.hidePatientContext();
      if (this.queuePage.isOpen()) this.queuePage.close();
    } catch (error) {
      this.showError(errorMessage(error));
    }
  }

  private async activateSeries(
    loader: () => Promise<SeriesMetadata>,
    message: string,
    remoteDownload = false,
  ): Promise<void> {
    const previous = this.state;
    const previousMeasurements = this.measurements;
    const previousSelectedMeasurementId = this.selectedMeasurementId;
    const previousMode = this.viewerMode;
    const previousMpr = this.mpr;
    const previousMprMeasurements = this.mprMeasurements;
    const previousRemoteSeriesOpen = this.remoteSeriesOpen;
    const previousSharedRecords = new Map(this.sharedAnnotationRecords);
    const previousSyncCursor = this.annotationSyncCursor;
    const previousSegmentationProjects = this.segmentationProjects;
    const previousSegmentationSegments = this.segmentationSegments;
    const previousSegmentationSegment = this.segmentationSegment;
    const previousMaskVolumes = this.maskVolumes;
    const previousMaskTagFilter = this.maskTagFilter;
    const previousMaskMatchedSegmentIds = this.maskMatchedSegmentIds;
    let openedHandle: number | null = null;
    this.stopCine();
    this.remoteDownloadActive = remoteDownload;
    try {
      this.setBusy(true, message, remoteDownload);
      const metadata = await loader();
      openedHandle = metadata.handle;
      if (!metadata.frames.length) throw new Error('所选序列没有可显示的帧');
      const preset = metadata.frames[0].window_presets[0];
      if (!preset) throw new Error('影像没有可用的显示窗口');

      this.frameRequests.invalidate();
      this.lutRequests.invalidate();
      this.cancelFramePrefetch();
      this.frameCache.clear();
      this.pendingFrames.clear();
      this.measurements = new Map();
      this.mprMeasurements = new Map();
      this.stopAnnotationSync();
      this.sharedAnnotationRecords.clear();
      this.annotationSyncCursor = null;
      this.annotationHistory.clear();
      this.segmentationProjects = [];
      this.segmentationSegments = [];
      this.segmentationSegment = null;
      this.maskVolumes = new Map();
      this.maskTagFilter = '';
      this.maskMatchedSegmentIds = null;
      this.maskTagQueryGeneration += 1;
      this.maskUndoEntries = [];
      this.maskRedoEntries = [];
      this.maskDirtySlices.clear();
      this.maskSyncingSegments.clear();
      this.maskSyncErrors.clear();
      this.aiStatus = '';
      this.angleAwaitingEnd = false;
      this.draft = null;
      this.mprDraft = null;
      this.selectedMeasurementId = null;
      this.viewerMode = '2d';
      this.mprProjection = 'slice';
      this.mprSlabThicknessMm = 10;
      this.mpr = null;
      this.invalidateMprRequests();
      this.state = {
        metadata,
        currentFrame: 0,
        windowCenter: preset.center,
        windowWidth: preset.width,
        voiFunction: preset.function,
        zoom: 1,
        panX: 0,
        panY: 0,
        rotation: 0,
        flipHorizontal: false,
        flipVertical: false,
        inverted: false,
        lut: null,
        tool: metadata.frames[0].pixel_format === 'rgb8' ? 'pan' : 'window',
      };
      this.remoteSeriesOpen = remoteDownload;
      await this.loadCurrentFrame();
      await this.loadSegmentationWorkspace();
      if (remoteDownload) {
        await this.refreshSharedAnnotations();
        this.startAnnotationSync();
      }
      this.disposeGpuMprRenderer();
      this.disposeVolumeRenderer();
      if (previous) void closeSeries(previous.metadata.handle).catch(console.error);
      openedHandle = null;
      this.ensurePaneSlots();
      this.updatePaneLabels();
      this.showSeriesWarning();
    } catch (error) {
      if (openedHandle != null) {
        await closeSeries(openedHandle).catch(console.error);
        this.state = previous;
        this.measurements = previousMeasurements;
        this.mprMeasurements = previousMprMeasurements;
        this.selectedMeasurementId = previousSelectedMeasurementId;
        this.viewerMode = previousMode;
        this.mpr = previousMpr;
        this.remoteSeriesOpen = previousRemoteSeriesOpen;
        this.sharedAnnotationRecords = previousSharedRecords;
        this.annotationSyncCursor = previousSyncCursor;
        this.segmentationProjects = previousSegmentationProjects;
        this.segmentationSegments = previousSegmentationSegments;
        this.segmentationSegment = previousSegmentationSegment;
        this.maskVolumes = previousMaskVolumes;
        this.maskTagFilter = previousMaskTagFilter;
        this.maskMatchedSegmentIds = previousMaskMatchedSegmentIds;
        if (previousRemoteSeriesOpen) this.startAnnotationSync();
        this.cancelFramePrefetch();
        this.frameCache.clear();
        this.pendingFrames.clear();
        if (previous) {
          await this.loadCurrentFrame().catch(console.error);
        } else {
          this.renderer.clear();
        }
      }
      this.ensurePaneSlots();
      this.updatePaneLabels();
      throw error;
    } finally {
      this.remoteDownloadActive = false;
      this.setBusy(false);
      this.updateUi();
    }
  }

  async setFrame(requested: number): Promise<void> {
    if (!this.state) return;
    await this.setFrameOnPane(this.activePane, requested);
    await this.propagateFrameSync(this.activePane);
  }

  private async setFrameOnPane(pane: SeriesPane, requested: number): Promise<void> {
    const state = pane.state;
    if (!state) return;
    const next = Math.max(0, Math.min(requested, state.metadata.frames.length - 1));
    if (
      next === state.currentFrame
      && (state.lut || state.metadata.frames[next].pixel_format === 'rgb8')
    ) return;
    state.currentFrame = next;
    if (pane === this.activePane) {
      this.selectedMeasurementId = null;
      this.draft = null;
    }
    this.updateUi();
    try {
      if (pane === this.activePane) await this.loadCurrentFrame();
      else await this.loadFrameForPane(pane, next);
    } catch (error) {
      this.showError(errorMessage(error));
    }
  }

  private async propagateFrameSync(source: SeriesPane): Promise<void> {
    if (!this.syncScrollEnabled || this.syncPropagating || this.cinePlaying) return;
    if (this.panes.filter((pane) => pane.state != null).length < 2) return;
    const sourceState = source.state;
    if (!sourceState) return;
    const sourceFrame = sourceState.metadata.frames[sourceState.currentFrame];
    if (!sourceFrame || !frameHasGeometry(sourceFrame)) return;
    const members = this.panes
      .filter((pane) => pane !== source && pane.state != null && !pane.syncExcludedReason)
      .map((pane) => ({
        paneIndex: this.paneIndex(pane),
        frames: pane.state!.metadata.frames,
      }));
    const targets = syncFrameTargets(sourceFrame, members, new Set());
    this.syncPropagating = true;
    try {
      for (const target of targets) {
        if (target.frameIndex == null) continue;
        const pane = this.panes[target.paneIndex];
        const state = pane?.state;
        if (!state || state.currentFrame === target.frameIndex) continue;
        state.currentFrame = target.frameIndex;
        pane.draft = null;
        pane.selectedMeasurementId = null;
        await this.loadFrameForPane(pane, target.frameIndex);
      }
      this.updateUi();
    } finally {
      this.syncPropagating = false;
    }
  }

  private async propagateWindowSync(source: SeriesPane): Promise<void> {
    if (!this.syncWindowEnabled || this.syncPropagating) return;
    if (this.panes.filter((pane) => pane.state != null).length < 2) return;
    const sourceState = source.state;
    if (!sourceState) return;
    const sourceWindow: SyncWindowLevel = {
      center: sourceState.windowCenter,
      width: sourceState.windowWidth,
    };
    this.syncPropagating = true;
    try {
      for (const pane of this.panes) {
        if (pane === source || pane.state == null || pane.syncExcludedReason) continue;
        const state = pane.state;
        if (windowEquals(sourceWindow, { center: state.windowCenter, width: state.windowWidth })) continue;
        state.windowCenter = sourceWindow.center;
        state.windowWidth = sourceWindow.width;
        await this.refreshLutForPane(pane);
      }
      this.updateUi();
    } finally {
      this.syncPropagating = false;
    }
  }

  private async switchImageStack(requested: number): Promise<void> {
    if (!this.state || this.busy || requested === this.state.metadata.active_stack) return;
    const previous = this.state;
    const previousMode = this.viewerMode;
    const previousMpr = this.mpr;
    const previousMprMeasurements = this.mprMeasurements;
    const previousMaskVolumes = this.maskVolumes;
    let changed = false;
    this.stopCine();
    try {
      this.setBusy(true, '正在切换图像组...');
      const metadata = await selectImageStack(previous.metadata.handle, requested);
      if (!metadata.frames.length) throw new Error('所选图像组没有可显示的帧');
      const preset = metadata.frames[0].window_presets[0];
      if (!preset) throw new Error('所选图像组没有可用的显示窗口');

      this.frameRequests.invalidate();
      this.lutRequests.invalidate();
      this.cancelFramePrefetch();
      this.frameCache.clear();
      this.pendingFrames.clear();
      this.draft = null;
      this.mprDraft = null;
      this.selectedMeasurementId = null;
      this.viewerMode = '2d';
      this.mprProjection = 'slice';
      this.mprSlabThicknessMm = 10;
      this.mpr = null;
      this.mprMeasurements = new Map();
      this.maskVolumes = new Map();
      this.maskUndoEntries = [];
      this.maskRedoEntries = [];
      this.invalidateMprRequests();
      this.state = {
        metadata,
        currentFrame: 0,
        windowCenter: preset.center,
        windowWidth: preset.width,
        voiFunction: preset.function,
        zoom: 1,
        panX: 0,
        panY: 0,
        rotation: previous.rotation,
        flipHorizontal: previous.flipHorizontal,
        flipVertical: previous.flipVertical,
        inverted: previous.inverted,
        lut: null,
        tool: metadata.frames[0].pixel_format === 'rgb8'
          ? 'pan'
          : previous.tool === 'crosshair' ? 'window' : previous.tool,
      };
      changed = true;
      this.updateUi();
      await this.loadCurrentFrame();
      await this.loadSegmentationVolumes();
      this.disposeGpuMprRenderer();
      this.disposeVolumeRenderer();
      await closeMpr(previous.metadata.handle).catch(() => undefined);
      this.showSeriesWarning();
    } catch (error) {
      if (changed) {
        this.frameRequests.invalidate();
        this.lutRequests.invalidate();
        this.cancelFramePrefetch();
        this.frameCache.clear();
        this.pendingFrames.clear();
        this.state = previous;
        this.viewerMode = previousMode;
        this.mpr = previousMpr;
        this.mprMeasurements = previousMprMeasurements;
        this.maskVolumes = previousMaskVolumes;
        await this.loadCurrentFrame().catch(console.error);
      }
      this.showError(errorMessage(error));
    } finally {
      this.setBusy(false);
      this.updateUi();
    }
  }

  private async loadCurrentFrame(): Promise<void> {
    if (!this.state) return;
    await this.loadFrameForPane(this.activePane, this.state.currentFrame, {
      busy: true,
      statistics: true,
      prefetch: true,
    });
  }

  private async loadFrameForPane(
    pane: SeriesPane,
    frameIndex: number,
    options: { busy?: boolean; statistics?: boolean; prefetch?: boolean } = {},
  ): Promise<void> {
    const state = pane.state;
    if (!state) return;
    const generation = pane.frameRequests.next();
    const frame = state.metadata.frames[frameIndex];
    if (options.busy) this.setBusy(true, `正在加载第 ${frameIndex + 1} 帧...`);
    try {
      const isColor = frame.pixel_format === 'rgb8';
      const [buffer, lut] = await Promise.all([
        this.getFrameForPane(pane, frameIndex),
        isColor
          ? Promise.resolve<Uint8Array | null>(null)
          : buildLut(
            state.metadata.handle,
            state.metadata.active_stack,
            frameIndex,
            state.windowCenter,
            state.windowWidth,
            state.voiFunction,
          ),
      ]);
      if (
        !pane.frameRequests.isCurrent(generation)
        || state !== pane.state
        || state.currentFrame !== frameIndex
      ) return;
      state.lut = lut;
      pane.renderer.setFrame(buffer, frame);
      if (lut) pane.renderer.applyLut(lut);
      this.render();
      if (options.statistics && pane === this.activePane) this.ensureCurrentStatistics();
      if (options.prefetch && pane === this.activePane) this.prefetchFrames(frameIndex);
    } finally {
      if (pane.frameRequests.isCurrent(generation) && options.busy) this.setBusy(false);
      this.updateUi();
    }
  }

  private async refreshLut(): Promise<void> {
    if (!this.state) return;
    if (this.currentFrame().pixel_format === 'rgb8') return;
    await this.refreshLutForPane(this.activePane);
    await this.propagateWindowSync(this.activePane);
  }

  private async refreshLutForPane(pane: SeriesPane): Promise<void> {
    const state = pane.state;
    if (!state) return;
    if (state.metadata.frames[state.currentFrame]?.pixel_format === 'rgb8') return;
    const frameIndex = state.currentFrame;
    const generation = pane.lutRequests.next();
    try {
      const lut = await buildLut(
        state.metadata.handle,
        state.metadata.active_stack,
        frameIndex,
        state.windowCenter,
        state.windowWidth,
        state.voiFunction,
      );
      if (
        !pane.lutRequests.isCurrent(generation) ||
        state !== pane.state ||
        frameIndex !== state.currentFrame
      ) {
        return;
      }
      state.lut = lut;
      pane.renderer.applyLut(lut);
      this.render();
    } catch (error) {
      this.showError(errorMessage(error));
    }
  }

  private scheduleLutRefresh(): void {
    if (this.windowFrameRequest != null) return;
    this.windowFrameRequest = requestAnimationFrame(() => {
      this.windowFrameRequest = null;
      void this.refreshLut();
    });
  }

  private async setViewerMode(mode: ViewerMode): Promise<void> {
    if (!this.state || this.busy || mode === this.viewerMode) return;
    if (mode !== '2d' && this.multiPane) {
      this.showError('多窗格对比模式下仅支持 2D 阅片，请先关闭其他窗格再使用 MPR / VR。');
      return;
    }
    this.stopCine();
    const previousMode = this.viewerMode;
    if (mode === '2d') {
      this.stopMprPrefetch();
      this.disposeGpuMprRenderer();
      this.disposeVolumeRenderer();
      this.viewerMode = '2d';
      if (this.state.tool === 'crosshair') {
        this.state.tool = 'window';
      }
      this.updateUi();
      this.resizeViewport();
      this.render();
      return;
    }
    try {
      if (!this.canAttemptMpr()) {
        throw new Error('当前图像组不是可用于 MPR 的薄层断层序列');
      }
      if (!this.mpr || this.mpr.metadata.stack_index !== this.state.metadata.active_stack) {
        await this.prepareMprSession();
      }
      if (!this.mpr) throw new Error('MPR 体数据尚未准备完成');
      if (mode === 'vr') {
        this.stopMprPrefetch();
        this.disposeGpuMprRenderer();
        const reason = volumeCapabilityReason(this.volumeCanvas, this.mpr.metadata.volume_rendering);
        if (reason) throw new Error(`VR 已禁用：${reason}`);
        this.setBusy(true, '正在加载 GPU 体纹理...');
        this.volumeAbort?.abort();
        const abort = new AbortController();
        this.volumeAbort = abort;
        const data = await loadVolume(this.state.metadata.handle, abort.signal);
        if (abort.signal.aborted || !this.state || !this.mpr) return;
        const range = this.mpr.metadata.volume_rendering.value_range;
        this.volumeWindowCenter = this.state.windowCenter;
        this.volumeWindowWidth = this.state.windowWidth;
        if (this.state.metadata.patient.modality?.toUpperCase() === 'PT') {
          this.volumeWindowCenter = (range[0] + range[1]) / 2;
          this.volumeWindowWidth = Math.max(1, range[1] - range[0]);
          this.volumePreset = 'pet';
        } else {
          this.volumePreset = 'soft_tissue';
        }
        const { VolumeRenderer } = await import('./volume-renderer');
        this.volumeRenderer = new VolumeRenderer(
          this.volumeCanvas,
          data,
          this.mpr.metadata.volume_rendering,
          {
            windowCenter: this.volumeWindowCenter,
            windowWidth: this.volumeWindowWidth,
            preset: this.volumePreset,
            quality: this.volumeQuality,
          },
        );
        this.viewerMode = 'vr';
        this.state.tool = 'pan';
        this.updateUi();
        this.resizeViewport();
      } else {
        this.disposeVolumeRenderer();
        this.viewerMode = 'mpr';
        this.state.tool = 'crosshair';
        this.updateUi();
        this.resizeViewport();
        await this.refreshMprSlices();
        this.scheduleMprPrefetch(0);
        void this.enableGpuMprIfAvailable();
      }
    } catch (error) {
      this.disposeVolumeRenderer();
      this.viewerMode = previousMode;
      this.showError(errorMessage(error));
    } finally {
      this.volumeAbort = null;
      this.mprBuildActive = false;
      this.setBusy(false);
      this.updateUi();
    }
  }

  private disposeVolumeRenderer(): void {
    this.volumeAbort?.abort();
    this.volumeAbort = null;
    this.volumeRenderer?.dispose();
    this.volumeRenderer = null;
    this.volumeWindowDrag = null;
  }

  private disposeGpuMprRenderer(): void {
    if (this.mprObliqueAbort) this.mprObliqueAbort.abort();
    if (!this.mprObliqueVolumePromise) this.mprObliqueAbort = null;
    this.gpuMprRenderer?.dispose();
    this.gpuMprRenderer = null;
    this.mprObliqueMode = false;
    this.mprObliqueVolumePromise = null;
    this.clearMprRotateCursors();
    this.updateObliqueReadouts();
    if (this.mpr) {
      this.mpr.activePlane = 'axial';
      this.mpr.mainPlane = 'axial';
    }
  }

  private async ensureMaskGeometry(): Promise<void> {
    if (this.mpr || !this.canAttemptMpr()) return;
    await this.prepareMprSession();
  }

  private async prepareMprSession(): Promise<void> {
    if (!this.state || this.mpr?.metadata.stack_index === this.state.metadata.active_stack) return;
    this.mprBuildActive = true;
    this.setBusy(true, '正在构建三维体数据...', true);
    this.updateUi();
    try {
      const metadata = await prepareMpr(
        this.state.metadata.handle,
        this.state.metadata.active_stack,
      );
      this.alignMaskVolumesToMpr(metadata);
      this.mprMeasurements = new Map();
      this.mpr = this.createMprSession(metadata);
      this.applyAllSharedAnnotations();
    } finally {
      this.mprBuildActive = false;
      this.setBusy(false);
      this.updateUi();
    }
  }

  private alignMaskVolumesToMpr(metadata: MprMetadata): void {
    if (!this.state || !this.maskVolumes.size) return;
    const destinationByFrameKey = new Map(
      metadata.source_slices.map((source, index) => [source.frame_key, index]),
    );
    const aligned = new Map<string, MaskVolume>();
    for (const [segmentId, volume] of this.maskVolumes) {
      const next = createMaskVolume(metadata.dimensions[1], metadata.dimensions[0], metadata.dimensions[2]);
      for (let source = 0; source < volume.slices; source += 1) {
        const frameKey = this.state.metadata.frames[source]?.frame_key;
        const destination = frameKey == null ? undefined : destinationByFrameKey.get(frameKey);
        if (destination == null) continue;
        const data = volume.sourceSlices.get(source);
        if (data) next.sourceSlices.set(destination, data);
        const revision = volume.revisions.get(source);
        if (revision != null) next.revisions.set(destination, revision);
        const syncState = volume.syncStates.get(source);
        if (syncState) next.syncStates.set(destination, syncState);
      }
      next.generation = volume.generation;
      aligned.set(segmentId, next);
    }
    this.maskVolumes = aligned;
    this.maskUndoEntries = [];
    this.maskRedoEntries = [];
  }

  private createMprSession(metadata: MprMetadata): MprSession {
    const crosshair = point3(metadata.initial_crosshair);
    const standardViewport = (plane: MprPlane): MprViewportState => ({
      plane,
      sliceIndex: sliceForPatientPoint(crosshair, requirePlane(metadata, plane)),
      zoom: 1,
      panX: 0,
      panY: 0,
      rotation: 0,
      flipHorizontal: false,
      flipVertical: false,
      inverted: false,
    });
    const stateFor = (plane: MprPlane): ObliquePlaneState => {
      const standard = requirePlane(metadata, plane === 'oblique' ? 'axial' : plane);
      return {
        normal: standard.normal,
        xAxis: standard.x_axis,
        yAxis: standard.y_axis,
      };
    };
    return {
      metadata,
      crosshair,
      mainPlane: 'axial',
      activePlane: 'axial',
      obliquePlanes: {
        axial: stateFor('axial'),
        coronal: stateFor('coronal'),
        sagittal: stateFor('sagittal'),
        oblique: stateFor('oblique'),
      },
      viewports: {
        axial: standardViewport('axial'),
        coronal: standardViewport('coronal'),
        sagittal: standardViewport('sagittal'),
        oblique: {
          plane: 'oblique',
          sliceIndex: 0,
          zoom: 1,
          panX: 0,
          panY: 0,
          rotation: 0,
          flipHorizontal: false,
          flipVertical: false,
          inverted: false,
        },
      },
    };
  }

  private canAttemptMpr(): boolean {
    if (!this.state || this.state.metadata.frames.length < 3) return false;
    const frame = this.state.metadata.frames[0];
    if (frame.pixel_format === 'rgb8') return false;
    if (frame.spacing.row_mm == null || frame.spacing.col_mm == null) return false;
    const description = this.state.metadata.patient.series_description?.toLowerCase() ?? '';
    return !/(locali[sz]er|scout|定位|冠状|矢状|coronal|sagittal|\bmpr\b)/i.test(description);
  }

  private async refreshMprSlices(planes: readonly MprPlane[] = STANDARD_MPR_PLANES): Promise<void> {
    await Promise.all(planes.map((plane) => this.loadMprPlane(plane)));
  }

  private setMprProjection(mode: MprProjectionMode): void {
    if (!this.mpr || this.viewerMode !== 'mpr' || mode === this.mprProjection) return;
    this.stopMprPrefetch();
    this.mprProjection = mode;
    this.selectedMeasurementId = null;
    this.mprDraft = null;
    this.gpuMprRenderer?.setProjection(mode, this.mprSlabThicknessMm);
    if (this.mprObliqueMode) this.renderAllMprPlanes();
    this.updateUi();
    void this.refreshMprSlices().then(() => this.scheduleMprPrefetch(0));
  }

  private setMprSlabThickness(value: number): void {
    const next = Math.min(200, Math.max(0.5, value));
    if (!Number.isFinite(next) || next === this.mprSlabThicknessMm) return;
    this.mprSlabThicknessMm = next;
    setText('mpr-slab-value', `${next.toFixed(next < 10 ? 1 : 0)} mm`);
    this.gpuMprRenderer?.setProjection(this.mprProjection, next);
    if (this.mprObliqueMode) this.renderAllMprPlanes();
    if (this.mprProjection !== 'slice') {
      this.stopMprPrefetch();
      void this.refreshMprSlices().then(() => this.scheduleMprPrefetch(0));
    }
  }

  /** 进入 MPR 后自动尝试启用 GPU 三视图基准线 MPR；失败则回退到标准 CPU MPR。 */
  private async enableGpuMprIfAvailable(): Promise<void> {
    if (!this.state || !this.mpr || this.viewerMode !== 'mpr') return;
    if (this.gpuMprRenderer) {
      this.mprObliqueMode = true;
      this.updateUi();
      this.resizeViewport();
      this.renderAllMprPlanes();
      return;
    }
    try {
      await this.ensureMprObliqueRenderer();
      if (!this.state || !this.mpr || this.viewerMode !== 'mpr') return;
      if (!this.gpuMprRenderer) return;
      this.mprObliqueMode = true;
      this.updateUi();
      this.resizeViewport();
      this.renderAllMprPlanes();
    } catch (error) {
      console.warn('GPU Oblique MPR 不可用，使用标准 MPR', error);
    }
  }

  private async ensureMprObliqueRenderer(): Promise<void> {
    if (!this.state || !this.mpr) return;
    if (this.gpuMprRenderer) return;
    if (this.mprObliqueVolumePromise && !this.mprObliqueAbort?.signal.aborted) {
      return this.mprObliqueVolumePromise;
    }
    const state = this.state;
    const session = this.mpr;
    const abort = new AbortController();
    this.mprObliqueAbort?.abort();
    this.mprObliqueAbort = abort;
    const loading = (async () => {
      const reason = volumeCapabilityReason(this.volumeCanvas, session.metadata.volume_rendering);
      if (reason) throw new Error(`Oblique MPR 需要 GPU 体纹理：${reason}`);
      const data = await loadVolume(state.metadata.handle, abort.signal);
      if (abort.signal.aborted || this.state !== state || this.mpr !== session) return;
      this.gpuMprRenderer = new GpuMprRenderer(
        data,
        session.metadata,
        {
          windowCenter: state.windowCenter,
          windowWidth: state.windowWidth,
          inverted: false,
          projection: this.mprProjection,
          slabThicknessMm: this.mprSlabThicknessMm,
          voiFunction: state.voiFunction,
        },
      );
    })();
    this.mprObliqueVolumePromise = loading
      .catch((error: unknown) => {
        if (abort.signal.aborted) return;
        throw error;
      })
      .finally(() => {
        if (this.mprObliqueAbort === abort) {
          this.mprObliqueAbort = null;
          this.mprObliqueVolumePromise = null;
        }
      });
    return this.mprObliqueVolumePromise;
  }

  private resetObliqueToStandard(): void {
    if (!this.mpr) return;
    for (const plane of MPR_PLANES) {
      const standard = requirePlane(this.mpr.metadata, plane);
      this.mpr.obliquePlanes[plane] = {
        normal: standard.normal,
        xAxis: standard.x_axis,
        yAxis: standard.y_axis,
      };
    }
    for (const plane of MPR_PLANES) {
      this.mpr.viewports[plane].sliceIndex = sliceForPatientPoint(
        this.mpr.crosshair,
        this.mprPlaneMetadata(plane),
      );
    }
    this.updateUi();
    this.updateMprPositionUi();
    this.renderAllMprPlanes();
  }

  /** 旋转当前视图的基准线：当前视图保持不变，另外两个视图绕当前视图法线旋转。 */
  private rotateOblique(angleRadians: number): void {
    if (!this.mpr || !this.mprObliqueMode || Math.abs(angleRadians) < 1e-4) return;
    const plane = this.mpr.activePlane;
    if (plane === 'oblique') return;
    const axis = this.mpr.obliquePlanes[plane].normal;
    for (const other of MPR_PLANES) {
      if (other === plane) continue;
      const state = this.mpr.obliquePlanes[other];
      state.normal = normalizedArray(rotateVectorAroundAxis(state.normal, axis, angleRadians));
      state.xAxis = rotateVectorAroundAxis(state.xAxis, axis, angleRadians);
      state.yAxis = rotateVectorAroundAxis(state.yAxis, axis, angleRadians);
      state.yAxis = orthogonalizeArray(state.yAxis, state.normal);
      state.xAxis = normalizedArray(crossArray(state.normal, state.yAxis));
    }
    for (const item of MPR_PLANES) {
      this.mpr.viewports[item].sliceIndex = sliceForPatientPoint(
        this.mpr.crosshair,
        this.mprPlaneMetadata(item),
      );
    }
    this.renderAllMprPlanes();
    this.updateUi();
  }

  private renderAllMprPlanes(): void {
    for (const plane of MPR_PLANES) this.renderMprPlane(plane);
    this.updateObliqueReadouts();
  }

  private updateObliqueReadouts(): void {
    if (!this.mpr || !this.mprObliqueMode) {
      for (const plane of MPR_PLANES) {
        const element = document.getElementById(`${plane}-oblique-readout`);
        if (element) element.hidden = true;
      }
      return;
    }
    for (const plane of MPR_PLANES) {
      const element = requiredElement<HTMLElement>(`${plane}-oblique-readout`);
      const state = this.mpr.obliquePlanes[plane];
      const standard = requirePlane(this.mpr.metadata, plane);
      const cosine = Math.max(-1, Math.min(1, Math.abs(dotArray(state.normal, standard.normal))));
      const tiltDegrees = Math.acos(cosine) * 180 / Math.PI;
      const [x, y, z] = state.normal;
      const iop = [
        ...state.xAxis,
        ...state.yAxis,
      ].map((value) => value.toFixed(3)).join(', ');
      element.hidden = false;
      element.textContent = `偏转 ${tiltDegrees.toFixed(1)}° · n(${x.toFixed(3)}, ${y.toFixed(3)}, ${z.toFixed(3)})`;
      element.title = `DICOM Image Orientation (Patient): ${iop}`;
    }
  }

  private obliquePlaneMetadata(plane: MprPlane): MprPlaneMetadata {
    if (!this.mpr) throw new Error('MPR 尚未初始化');
    if (plane === 'oblique') throw new Error('Oblique 独立视口已移除');
    const state = this.mpr.obliquePlanes[plane];
    const metadata = this.mpr.metadata;
    const geometry = computePlaneStackGeometry(
      {
        origin: metadata.source_origin,
        xAxis: metadata.source_x_axis,
        yAxis: metadata.source_y_axis,
        normal: metadata.source_normal,
        spacingMm: metadata.source_spacing_mm,
        dimensions: metadata.dimensions,
      },
      metadata.patient_bounds_min,
      metadata.patient_bounds_max,
      state.xAxis,
      state.yAxis,
      state.normal,
    );
    return {
      plane,
      rows: geometry.rows,
      cols: geometry.cols,
      slice_count: geometry.sliceCount,
      pixel_spacing_mm: Math.min(geometry.spacingXmm, geometry.spacingYmm),
      slice_spacing_mm: geometry.sliceSpacingMm,
      spacing_x_mm: geometry.spacingXmm,
      spacing_y_mm: geometry.spacingYmm,
      origin: geometry.origin,
      x_axis: state.xAxis,
      y_axis: state.yAxis,
      normal: state.normal,
    };
  }

  private async loadMprPlane(plane: MprPlane): Promise<void> {
    if (!this.state || !this.mpr) return;
    if (plane === 'oblique' || this.mprObliqueMode) return;
    if (this.mprRequestPending[plane]) {
      this.mprReloadQueued[plane] = true;
      return;
    }
    this.mprRequestPending[plane] = true;
    const session = this.mpr;
    const state = this.state;
    const viewport = session.viewports[plane];
    const sliceIndex = viewport.sliceIndex;
    const windowCenter = state.windowCenter;
    const windowWidth = state.windowWidth;
    const voiFunction = state.voiFunction;
    const projection = this.mprProjection;
    const slabThicknessMm = this.mprSlabThicknessMm;
    const generation = this.mprRequests[plane].next();
    try {
      const buffer = await renderMprSlice(
        state.metadata.handle,
        plane,
        sliceIndex,
        windowCenter,
        windowWidth,
        voiFunction,
        projection,
        slabThicknessMm,
      );
      if (
        this.viewerMode !== 'mpr' ||
        this.mpr !== session ||
        this.state !== state ||
        !this.mprRequests[plane].isCurrent(generation) ||
        viewport.sliceIndex !== sliceIndex ||
        this.mprProjection !== projection ||
        this.mprSlabThicknessMm !== slabThicknessMm ||
        state.windowCenter !== windowCenter ||
        state.windowWidth !== windowWidth ||
        state.voiFunction !== voiFunction
      ) {
        return;
      }
      this.mprRenderers[plane].setGrayFrame(buffer, this.mprFrame(plane));
      this.renderMprPlane(plane);
      this.ensureCurrentStatistics(plane);
    } catch (error) {
      if (this.mprRequests[plane].isCurrent(generation)) this.showError(errorMessage(error));
    } finally {
      this.mprRequestPending[plane] = false;
      if (this.mprReloadQueued[plane]) {
        this.mprReloadQueued[plane] = false;
        void this.loadMprPlane(plane);
      }
    }
  }

  private scheduleMprRefresh(): void {
    this.cancelRunningMprPrefetch();
    this.scheduleMprPrefetch(400);
    if (this.mprObliqueMode) this.renderAllMprPlanes();
    if (this.mprWindowFrameRequest != null) return;
    this.mprWindowFrameRequest = requestAnimationFrame(() => {
      this.mprWindowFrameRequest = null;
      void this.refreshMprSlices();
    });
  }

  private changeMprSlice(plane: MprPlane, requested: number): void {
    if (!this.mpr) return;
    if (plane === 'oblique') return;
    const viewport = this.mpr.viewports[plane];
    if (this.mprObliqueMode) {
      const metadata = this.mprPlaneMetadata(plane);
      const next = Math.max(0, Math.min(requested, metadata.slice_count - 1));
      if (next === viewport.sliceIndex) return;
      const delta = (next - viewport.sliceIndex) * metadata.slice_spacing_mm;
      const crosshair = addPatientVector(this.mpr.crosshair, metadata.normal, delta);
      this.setMprCrosshair(crosshair);
      return;
    }
    const metadata = requirePlane(this.mpr.metadata, plane);
    const next = Math.max(0, Math.min(requested, metadata.slice_count - 1));
    if (next === viewport.sliceIndex) return;
    const delta = (next - viewport.sliceIndex) * metadata.slice_spacing_mm;
    const crosshair = addPatientVector(this.mpr.crosshair, metadata.normal, delta);
    this.setMprCrosshair(crosshair);
  }

  private setMprCrosshair(requested: PatientPoint3D): void {
    if (!this.mpr) return;
    const minimum = this.mpr.metadata.patient_bounds_min;
    const maximum = this.mpr.metadata.patient_bounds_max;
    const next: PatientPoint3D = {
      x: Math.max(minimum[0], Math.min(requested.x, maximum[0])),
      y: Math.max(minimum[1], Math.min(requested.y, maximum[1])),
      z: Math.max(minimum[2], Math.min(requested.z, maximum[2])),
    };
    const changed: MprPlane[] = [];
    for (const plane of MPR_PLANES) {
      const index = sliceForPatientPoint(next, this.mprPlaneMetadata(plane));
      if (index !== this.mpr.viewports[plane].sliceIndex) changed.push(plane);
      this.mpr.viewports[plane].sliceIndex = index;
    }
    this.mpr.crosshair = next;
    this.selectedMeasurementId = null;
    this.mprDraft = null;
    if (this.mprObliqueMode) {
      this.updateMprPositionUi();
      this.renderAllMprPlanes();
      return;
    }
    this.updateMprPositionUi();
    for (const plane of MPR_PLANES) this.renderMprOverlay(plane);
    if (changed.length) void this.refreshMprSlices(changed);
  }

  private mprPlaneMetadata(plane: MprPlane): MprPlaneMetadata {
    if (!this.mpr) throw new Error('MPR 尚未初始化');
    if (plane === 'oblique') throw new Error('Oblique 独立视口已移除');
    if (this.mprObliqueMode) return this.obliquePlaneMetadata(plane);
    return requirePlane(this.mpr.metadata, plane);
  }

  private mprSlicePlaneMetadata(plane: MprPlane): MprPlaneMetadata {
    if (!this.mpr) throw new Error('MPR 尚未初始化');
    const metadata = this.mprPlaneMetadata(plane);
    const sliceIndex = this.mpr.viewports[plane].sliceIndex;
    return {
      ...metadata,
      origin: addArray(
        metadata.origin,
        scaleArray(metadata.normal, sliceIndex * metadata.slice_spacing_mm),
      ),
    };
  }

  private mprFrame(plane: MprPlane): FrameMetadata {
    if (!this.state || !this.mpr) throw new Error('MPR 尚未初始化');
    const metadata = this.mprPlaneMetadata(plane);
    const sliceIndex = this.mpr.viewports[plane].sliceIndex;
    return {
      logical_index: sliceIndex,
      frame_key: `mpr:${plane}:${sliceIndex}`,
      sop_instance_uid: null,
      source_frame: sliceIndex + 1,
      instance_number: null,
      rows: metadata.rows,
      cols: metadata.cols,
      bits_allocated: 8,
      pixel_format: 'gray8',
      photometric_interpretation: 'MONOCHROME2',
      cine_rate_fps: null,
      quantitative: this.currentFrame().quantitative,
      laterality: null,
      view_position: null,
      patient_orientation: [],
      position: null,
      orientation: null,
      window_presets: this.currentFrame().window_presets,
      spacing: {
        confidence: 'calibrated',
        source: 'mpr-patient-space',
        description: `MPR 患者空间重采样 ${(metadata.spacing_x_mm ?? metadata.pixel_spacing_mm).toFixed(3)} x ${(metadata.spacing_y_mm ?? metadata.pixel_spacing_mm).toFixed(3)} mm`,
        row_mm: metadata.spacing_y_mm ?? metadata.pixel_spacing_mm,
        col_mm: metadata.spacing_x_mm ?? metadata.pixel_spacing_mm,
        column_over_row: (metadata.spacing_x_mm ?? metadata.pixel_spacing_mm)
          / (metadata.spacing_y_mm ?? metadata.pixel_spacing_mm),
      },
    };
  }

  private mprCrosshairImagePoint(plane: MprPlane): Point {
    if (!this.mpr) return { x: 0, y: 0 };
    const metadata = this.mprPlaneMetadata(plane);
    const viewport = this.mpr.viewports[plane];
    return mprImageForPatient(this.mpr.crosshair, viewport.sliceIndex, metadata);
  }

  private mprImagePointToPatient(plane: MprPlane, imagePoint: Point): PatientPoint3D {
    if (!this.mpr) throw new Error('MPR 尚未初始化');
    const metadata = this.mprPlaneMetadata(plane);
    const viewport = this.mpr.viewports[plane];
    return patientPointForMprImage(imagePoint, viewport.sliceIndex, metadata);
  }

  private invalidateMprRequests(): void {
    this.stopMprPrefetch();
    for (const plane of STANDARD_MPR_PLANES) {
      this.mprRequests[plane].invalidate();
      this.mprReloadQueued[plane] = false;
    }
  }

  private scheduleMprPrefetch(delay: number): void {
    if (!this.state || !this.mpr) return;
    if (this.mprPrefetchTimer != null) window.clearTimeout(this.mprPrefetchTimer);
    const state = this.state;
    const session = this.mpr;
    this.mprPrefetchTimer = window.setTimeout(async () => {
      this.mprPrefetchTimer = null;
      await this.mprPrefetchCancellation;
      if (this.state !== state || this.mpr !== session) return;
      const run = ++this.mprPrefetchRun;
      this.mprPrefetchActive = true;
      let backendGeneration: number;
      try {
        backendGeneration = await beginMprPrefetch();
      } catch (error) {
        if (this.mprPrefetchRun === run) this.mprPrefetchActive = false;
        console.warn('MPR 切片预计算未启动', error);
        return;
      }
      if (
        this.mprPrefetchRun !== run
        || this.state !== state
        || this.mpr !== session
      ) {
        this.mprPrefetchCancellation = cancelMprPrefetch().catch(console.error);
        return;
      }
      const startSlices = STANDARD_MPR_PLANES.map(
        (plane) => session.viewports[plane].sliceIndex,
      ) as [number, number, number];
      void prefetchMprSlices(
        state.metadata.handle,
        backendGeneration,
        startSlices,
        state.windowCenter,
        state.windowWidth,
        state.voiFunction,
        this.mprProjection,
        this.mprSlabThicknessMm,
      )
        .catch((error) => console.warn('MPR 切片预计算未完成', error))
        .finally(() => {
          if (this.mprPrefetchRun === run) this.mprPrefetchActive = false;
        });
    }, delay);
  }

  private stopMprPrefetch(): void {
    if (this.mprPrefetchTimer != null) window.clearTimeout(this.mprPrefetchTimer);
    this.mprPrefetchTimer = null;
    this.cancelRunningMprPrefetch();
  }

  private cancelRunningMprPrefetch(): void {
    if (!this.mprPrefetchActive) return;
    this.mprPrefetchRun += 1;
    this.mprPrefetchActive = false;
    this.mprPrefetchCancellation = cancelMprPrefetch().catch(console.error);
  }

  private async getFrame(index: number, signal?: AbortSignal): Promise<ArrayBuffer> {
    return this.getFrameForPane(this.activePane, index, signal);
  }

  private async getFrameForPane(
    pane: SeriesPane,
    index: number,
    signal?: AbortSignal,
  ): Promise<ArrayBuffer> {
    const state = pane.state;
    if (!state) throw new Error('没有已打开的序列');
    const handle = state.metadata.handle;
    const stack = state.metadata.active_stack;
    const requestKey = `${handle}:${stack}:${index}`;
    const cached = this.frameCache.get(requestKey);
    if (cached) return cached;
    const pending = this.pendingFrames.get(requestKey);
    if (pending) return pending;
    const request = loadFrame(handle, stack, index, signal)
      .then((buffer) => {
        this.frameCache.set(requestKey, buffer);
        return buffer;
      })
      .finally(() => {
        if (this.pendingFrames.get(requestKey) === request) {
          this.pendingFrames.delete(requestKey);
        }
      });
    this.pendingFrames.set(requestKey, request);
    return request;
  }

  private prefetchFrames(current: number): void {
    const state = this.state;
    if (!state || state.metadata.frames.length < 2) return;
    const key = `${state.metadata.handle}:${state.metadata.active_stack}`;
    if (this.framePrefetchKey === key) {
      if (this.framePrefetchAbort) return;
      if (Date.now() - this.framePrefetchCompletedAt < FRAME_CACHE_TTL_MS) return;
    }

    this.cancelFramePrefetch();
    const generation = this.framePrefetchGeneration;
    const controller = new AbortController();
    this.framePrefetchKey = key;
    this.framePrefetchAbort = controller;
    this.framePrefetchCompletedAt = 0;

    const groups = framePrefetchGroups(
      state.metadata.frames.length,
      current,
      (index) => frameSourceKey(state.metadata.frames[index]),
    );
    let groupCursor = 0;
    let failed = false;
    const worker = async (): Promise<void> => {
      while (!controller.signal.aborted && generation === this.framePrefetchGeneration) {
        const group = groups[groupCursor];
        groupCursor += 1;
        if (!group) return;
        for (const index of group) {
          try {
            await this.getFrame(index, controller.signal);
          } catch {
            if (!controller.signal.aborted) failed = true;
          }
        }
      }
    };
    const workerCount = Math.min(FRAME_PREFETCH_CONCURRENCY, groups.length);
    void Promise.all(Array.from({ length: workerCount }, worker)).then(async () => {
      if (
        controller.signal.aborted
        || generation !== this.framePrefetchGeneration
        || this.framePrefetchKey !== key
      ) {
        return;
      }
      this.framePrefetchAbort = null;
      this.framePrefetchCompletedAt = failed ? 0 : Date.now();
      await this.getFrame(current).catch(() => undefined);
    });
  }

  private cancelFramePrefetch(): void {
    this.framePrefetchGeneration += 1;
    this.framePrefetchAbort?.abort();
    this.framePrefetchAbort = null;
    this.framePrefetchKey = null;
    this.framePrefetchCompletedAt = 0;
  }

  private toggleCine(): void {
    if (!this.state || this.state.metadata.frames.length < 2 || this.viewerMode !== '2d') return;
    if (this.cinePlaying) {
      this.stopCine();
    } else {
      this.cinePlaying = true;
      this.scheduleCineFrame();
    }
    this.updateUi();
  }

  private stopCine(): void {
    this.cinePlaying = false;
    if (this.cineTimer != null) {
      window.clearTimeout(this.cineTimer);
      this.cineTimer = null;
    }
  }

  private scheduleCineFrame(): void {
    if (!this.cinePlaying || !this.state || this.viewerMode !== '2d') return;
    const fps = this.currentFrame().cine_rate_fps ?? 15;
    const delay = 1000 / Math.min(60, Math.max(1, fps * this.cineSpeed));
    this.cineTimer = window.setTimeout(async () => {
      this.cineTimer = null;
      if (!this.cinePlaying || !this.state) return;
      const next = (this.state.currentFrame + 1) % this.state.metadata.frames.length;
      await this.setFrame(next);
      this.scheduleCineFrame();
    }, delay);
  }

  private async setTool(tool: ToolMode): Promise<void> {
    if (!this.state) return;
    if (tool === 'crosshair' && this.viewerMode !== 'mpr') return;
    if (tool === 'window' && this.viewerMode === '2d' && this.currentFrame().pixel_format === 'rgb8') {
      return;
    }
    if (isMaskTool(tool)) {
      if (this.maskTagFilter && !this.visibleMaskSegments().length) {
        this.showError(`没有 Tag 为“${this.maskTagFilter}”的 Mask`);
        return;
      }
      try {
        await this.ensureSegmentationWorkspace();
        await this.ensureMaskGeometry();
      } catch (error) {
        this.showError(errorMessage(error));
        return;
      }
    }
    this.state.tool = tool;
    this.draft = null;
    this.mprDraft = null;
    this.selectedMeasurementId = null;
    this.angleAwaitingEnd = false;
    this.updateUi();
    this.render();
  }

  private async loadSegmentationWorkspace(): Promise<void> {
    const state = this.state;
    const studyUid = state?.metadata.study_uid;
    const seriesUid = state?.metadata.series_uid;
    if (!this.remoteSeriesOpen || !studyUid || !seriesUid) {
      const segment = localSegmentationSegment();
      this.segmentationProjects = [];
      this.segmentationSegments = [segment];
      this.segmentationSegment = segment;
      this.maskVolumes.set(segment.id, this.createEmptyMaskVolume());
      return;
    }
    const selectedId = this.segmentationSegment?.id;
    const projects = await listSegmentationProjects(studyUid, seriesUid);
    if (this.state !== state) return;
    const segmentGroups = await Promise.all(
      projects.map((project) => listSegmentationSegments(studyUid, seriesUid, project.id)),
    );
    if (this.state !== state) return;
    this.segmentationProjects = projects;
    this.segmentationSegments = segmentGroups.flat().sort(
      (left, right) => left.segment_number - right.segment_number || left.label.localeCompare(right.label),
    );
    this.segmentationSegment = this.segmentationSegments.find((segment) => segment.id === selectedId)
      ?? this.segmentationSegments[0]
      ?? null;
    await this.loadSegmentationVolumes();
  }

  private updateMaskSegmentOptions(): void {
    const select = requiredElement<HTMLSelectElement>('mask-segment-select');
    const segments = this.visibleMaskSegments();
    const signature = segments
      .map((segment) => `${segment.id}:${segment.segment_number}:${segment.label}:${segment.tags.join(',')}`)
      .join('|');
    if (select.dataset.signature !== signature) {
      select.replaceChildren();
      for (const segment of segments) {
        const option = document.createElement('option');
        option.value = segment.id;
        const tags = segment.tags.length ? ` [${segment.tags.join(', ')}]` : '';
        option.textContent = `${segment.segment_number}. ${segment.label}${tags}`;
        select.append(option);
      }
      if (!segments.length) {
        const option = document.createElement('option');
        option.value = '';
        option.textContent = this.maskTagFilter ? '无匹配 Mask' : 'Mask Segment';
        select.append(option);
      }
      select.dataset.signature = signature;
    }
    select.value = this.segmentationSegment?.id ?? '';
    select.disabled = !segments.length;
    const label = requiredElement<HTMLElement>('mask-menu-label');
    label.textContent = this.segmentationSegment?.label || 'Mask';
    const swatch = requiredElement<HTMLElement>('mask-color-swatch');
    const color = this.segmentationSegment ? maskSegmentColor(this.segmentationSegment) : [55, 213, 216];
    swatch.style.backgroundColor = `rgb(${color[0]} ${color[1]} ${color[2]})`;
    const tagInput = requiredElement<HTMLInputElement>('mask-tag-input');
    if (document.activeElement !== tagInput) {
      tagInput.value = this.segmentationSegment?.tags.join(', ') ?? '';
    }
    tagInput.disabled = !this.segmentationSegment || this.maskTagSaving;
    requiredElement<HTMLButtonElement>('mask-tag-save').disabled = !this.segmentationSegment
      || this.maskTagSaving;
    requiredElement<HTMLButtonElement>('mask-segment-delete').disabled = !this.segmentationSegment
      || this.maskDeleting
      || (this.segmentationSegment != null
        && this.maskSyncingSegments.has(this.segmentationSegment.id));
    this.updateMaskTagFilterOptions();
  }

  private visibleMaskSegments(): SegmentationSegment[] {
    if (!this.maskTagFilter) return this.segmentationSegments;
    if (this.maskMatchedSegmentIds) {
      return this.segmentationSegments.filter((segment) => this.maskMatchedSegmentIds!.has(segment.id));
    }
    return this.segmentationSegments.filter((segment) => segment.tags.includes(this.maskTagFilter));
  }

  private updateMaskTagFilterOptions(): void {
    const select = requiredElement<HTMLSelectElement>('mask-tag-filter');
    const tags = [...new Set(this.segmentationSegments.flatMap((segment) => segment.tags))]
      .sort((left, right) => left.localeCompare(right));
    if (this.maskTagFilter && !tags.includes(this.maskTagFilter)) tags.push(this.maskTagFilter);
    const signature = tags.join('|');
    if (select.dataset.signature !== signature) {
      select.replaceChildren(new Option('全部 Tag', ''));
      for (const tag of tags) select.add(new Option(tag, tag));
      select.dataset.signature = signature;
    }
    select.value = this.maskTagFilter;
  }

  private async loadAiModels(refresh = false): Promise<void> {
    if (this.aiModelsLoading || (this.aiModelsLoaded && !refresh)) return;
    this.aiModelsLoading = true;
    this.aiStatus = refresh ? '正在刷新本地 AI 插件...' : '正在检查本地 AI 插件...';
    this.updateAiControls();
    try {
      const catalog = refresh ? await refreshAiPlugins() : await listAiCatalog();
      this.aiPlugins = catalog.plugins;
      this.aiModels = catalog.models;
      this.aiModelsLoaded = true;
      const selected = this.aiModels.find((model) => model.id === this.aiSelectedModelId);
      this.aiSelectedModelId = selected?.id
        ?? this.aiModels.find((model) => model.available)?.id
        ?? this.aiModels[0]?.id
        ?? '';
      this.aiStatus = '';
    } catch (error) {
      this.aiPlugins = [];
      this.aiModels = [];
      this.aiModelsLoaded = false;
      console.warn('本地 AI 插件探测失败', error);
      this.aiStatus = '本地 AI 插件不可用';
    } finally {
      this.aiModelsLoading = false;
      this.updateAiControls();
    }
  }

  private async openAiPluginDialog(): Promise<void> {
    const dialog = requiredElement<HTMLDialogElement>('ai-plugin-dialog');
    requiredElement<HTMLInputElement>('ai-plugin-name').value = '';
    requiredElement<HTMLInputElement>('ai-plugin-path').value = '';
    this.invalidateAiPluginCheck();
    if (!dialog.open) dialog.showModal();
    requiredElement<HTMLInputElement>('ai-plugin-name').focus();
    const list = requiredElement<HTMLElement>('ai-plugin-installed-list');
    list.replaceChildren(Object.assign(document.createElement('p'), {
      className: 'ai-plugin-empty',
      textContent: '正在读取插件...',
    }));
    try {
      const [catalog, configurations] = await Promise.all([
        refreshAiPlugins(),
        listAiPluginConfigurations(),
      ]);
      this.aiPlugins = catalog.plugins;
      this.aiModels = catalog.models;
      this.aiModelsLoaded = true;
      this.renderInstalledAiPlugins(catalog.plugins, configurations);
    } catch (error) {
      list.replaceChildren(Object.assign(document.createElement('p'), {
        className: 'ai-plugin-empty ai-plugin-empty-error',
        textContent: `读取插件失败：${errorMessage(error)}`,
      }));
    }
  }

  private renderInstalledAiPlugins(
    plugins: AiPluginDescriptor[],
    configurations: AiPluginConfiguration[],
  ): void {
    const list = requiredElement<HTMLElement>('ai-plugin-installed-list');
    const count = requiredElement<HTMLElement>('ai-plugin-installed-count');
    const matchedConfigurations = new Set<AiPluginConfiguration>();
    const rows = plugins.map((plugin) => {
      const configuration = configurations.find((candidate) => (
        (candidate.id && candidate.id === plugin.id)
        || (!candidate.id && candidate.name === plugin.name)
      ));
      if (configuration) matchedConfigurations.add(configuration);
      return this.aiPluginRow(plugin, configuration);
    });
    for (const configuration of configurations) {
      if (matchedConfigurations.has(configuration)) continue;
      rows.push(this.aiPluginRow({
        id: configuration.id,
        name: configuration.name,
        version: configuration.version,
        source: 'user',
        available: false,
        unavailable_reason: '插件配置存在，但当前未能加载',
      }, configuration));
    }
    count.textContent = `${rows.length} 个`;
    if (rows.length) {
      list.replaceChildren(...rows);
    } else {
      list.replaceChildren(Object.assign(document.createElement('p'), {
        className: 'ai-plugin-empty',
        textContent: '尚未发现 AI 插件。',
      }));
    }
  }

  private aiPluginRow(
    plugin: AiPluginDescriptor,
    configuration?: AiPluginConfiguration,
  ): HTMLElement {
    const pluginModels = this.aiModels.filter((model) => model.plugin_id === plugin.id);
    const available = plugin.available
      && (pluginModels.length === 0 || pluginModels.some((model) => model.available));
    const unavailableReason = plugin.unavailable_reason
      ?? pluginModels.find((model) => !model.available)?.unavailable_reason;
    const row = document.createElement('article');
    row.className = 'ai-plugin-installed-row';
    const heading = document.createElement('div');
    heading.className = 'ai-plugin-installed-row-heading';
    const name = document.createElement('strong');
    name.textContent = plugin.name;
    const badge = document.createElement('span');
    badge.className = `ai-plugin-status-badge ${available ? 'is-available' : 'is-unavailable'}`;
    badge.textContent = available ? '可用' : '不可用';
    heading.append(name, badge);

    const metadata = document.createElement('p');
    const source = plugin.source === 'bundled' ? '内置插件' : plugin.source === 'legacy' ? '兼容 Worker' : '外部插件';
    const version = plugin.version ? ` · ${plugin.version}` : '';
    metadata.textContent = `${source}${version} · ${pluginModels.length} 个模型`;
    row.append(heading, metadata);
    if (configuration?.path) {
      const path = document.createElement('code');
      path.textContent = configuration.path;
      path.title = configuration.path;
      row.append(path);
    }
    if (!available && unavailableReason) {
      const reason = document.createElement('p');
      reason.className = 'ai-plugin-installed-error';
      reason.textContent = unavailableReason;
      row.append(reason);
    }
    return row;
  }

  private closeAiPluginDialog(): void {
    const dialog = requiredElement<HTMLDialogElement>('ai-plugin-dialog');
    if (dialog.open) dialog.close();
  }

  private invalidateAiPluginCheck(): void {
    const result = requiredElement<HTMLElement>('ai-plugin-check-result');
    result.textContent = '填写名称和路径后先检测插件。';
    result.dataset.state = '';
    result.dataset.signature = '';
    requiredElement<HTMLButtonElement>('ai-plugin-save').disabled = true;
  }

  private aiPluginFormValues(): { name: string; path: string; signature: string } {
    const name = requiredElement<HTMLInputElement>('ai-plugin-name').value.trim();
    const path = requiredElement<HTMLInputElement>('ai-plugin-path').value.trim();
    return { name, path, signature: `${name}\u0000${path}` };
  }

  private async detectAiPlugin(): Promise<void> {
    const { name, path, signature } = this.aiPluginFormValues();
    const result = requiredElement<HTMLElement>('ai-plugin-check-result');
    const check = requiredElement<HTMLButtonElement>('ai-plugin-check');
    const save = requiredElement<HTMLButtonElement>('ai-plugin-save');
    if (!name || !path) {
      result.textContent = '请填写插件名称并选择插件目录。';
      result.dataset.state = 'error';
      return;
    }
    check.disabled = true;
    save.disabled = true;
    result.textContent = '正在启动 Worker 并检测模型...';
    result.dataset.state = 'checking';
    try {
      const catalog = await checkAiPlugin(name, path);
      const plugin = catalog.plugins[0];
      if (!plugin?.available) throw new Error(plugin?.unavailable_reason ?? '插件不可用');
      const availableModels = catalog.models.filter((model) => model.available);
      if (!availableModels.length) {
        throw new Error(
          catalog.models.find((model) => model.unavailable_reason)?.unavailable_reason
          ?? '插件没有可用模型',
        );
      }
      result.textContent = `检测通过：${plugin.name} ${plugin.version}，${availableModels.length}/${catalog.models.length} 个模型可用。`;
      result.dataset.state = 'success';
      result.dataset.signature = signature;
      save.disabled = false;
    } catch (error) {
      result.textContent = `检测失败：${errorMessage(error)}`;
      result.dataset.state = 'error';
      result.dataset.signature = '';
    } finally {
      check.disabled = false;
    }
  }

  private async saveAiPlugin(): Promise<void> {
    const { name, path, signature } = this.aiPluginFormValues();
    const result = requiredElement<HTMLElement>('ai-plugin-check-result');
    const save = requiredElement<HTMLButtonElement>('ai-plugin-save');
    if (result.dataset.signature !== signature) {
      this.invalidateAiPluginCheck();
      result.textContent = '名称或路径已变化，请重新检测插件。';
      result.dataset.state = 'error';
      return;
    }
    save.disabled = true;
    result.textContent = '正在保存插件配置...';
    result.dataset.state = 'checking';
    try {
      const catalog = await addAiPlugin(name, path);
      this.aiPlugins = catalog.plugins;
      this.aiModels = catalog.models;
      this.aiModelsLoaded = true;
      this.aiSelectedModelId = this.aiModels.find((model) => model.available)?.id
        ?? this.aiModels[0]?.id
        ?? '';
      this.aiStatus = `已增加插件：${name}`;
      this.updateAiControls();
      this.closeAiPluginDialog();
    } catch (error) {
      result.textContent = `增加失败：${errorMessage(error)}`;
      result.dataset.state = 'error';
      save.disabled = false;
    }
  }

  private updateAiControls(): void {
    const select = requiredElement<HTMLSelectElement>('ai-model-select');
    const refresh = requiredElement<HTMLButtonElement>('ai-plugins-refresh');
    const run = requiredElement<HTMLButtonElement>('ai-segment-run');
    const status = requiredElement<HTMLElement>('ai-model-status');
    const previous = this.aiSelectedModelId;
    select.replaceChildren();
    refresh.disabled = this.aiModelsLoading || this.aiRunning || this.busy;
    if (!this.aiModels.length) {
      const option = document.createElement('option');
      option.value = '';
      option.textContent = this.aiModelsLoading ? '正在检查本地 AI...' : '没有可用模型';
      select.append(option);
      select.value = '';
      select.disabled = true;
      run.disabled = true;
      const unavailable = this.aiPlugins.find((plugin) => !plugin.available);
      status.textContent = this.aiStatus
        || unavailable?.unavailable_reason
        || '未发现本地 AI 插件';
      return;
    }
    for (const plugin of this.aiPlugins) {
      const group = document.createElement('optgroup');
      group.label = `${plugin.name} ${plugin.version}`.trim();
      const models = plugin.available
        ? this.aiModels.filter((model) => model.plugin_id === plugin.id)
        : [];
      if (!models.length) {
        const option = document.createElement('option');
        option.disabled = true;
        option.textContent = plugin.available ? '没有模型' : '插件不可用';
        group.append(option);
      }
      for (const model of models) {
        const option = document.createElement('option');
        option.value = model.id;
        option.disabled = !model.available;
        option.textContent = model.available
          ? model.display_name
          : `${model.display_name}（不可用）`;
        group.append(option);
      }
      select.append(group);
    }
    const selected = this.aiModels.find((model) => model.id === previous)
      ?? this.aiModels.find((model) => model.available)
      ?? this.aiModels[0];
    this.aiSelectedModelId = selected.id;
    select.value = selected.id;
    select.disabled = !this.state || this.aiModelsLoading || this.aiRunning;
    const modality = this.state?.metadata.patient.modality?.toUpperCase();
    const supportsSeries = Boolean(
      this.state
      && modality
      && selected.supported_modalities.some((value) => value.toUpperCase() === modality),
    );
    const canRun = Boolean(
      this.state
      && selected.available
      && supportsSeries
      && !this.busy
      && !this.aiRunning,
    );
    run.disabled = !canRun;
    if (this.aiStatus) {
      status.textContent = this.aiStatus;
    } else if (!selected.available) {
      status.textContent = selected.unavailable_reason ?? '本地 AI 插件不可用';
    } else if (!supportsSeries) {
      status.textContent = `当前序列不支持 ${selected.display_name}`;
    } else {
      const device = selected.device ? ` · ${selected.device}` : '';
      status.textContent = `${selected.plugin_name} · 权重约 ${selected.model_download_mb} MB · 峰值内存约 ${selected.estimated_peak_memory_mb} MB${device}`;
    }
  }

  private async runSelectedAiSegmentation(): Promise<void> {
    if (!this.state || this.aiRunning || this.busy) return;
    const model = this.aiModels.find((candidate) => candidate.id === this.aiSelectedModelId);
    if (!model?.available) {
      this.aiStatus = model?.unavailable_reason ?? '本地 AI Worker 不可用';
      this.updateAiControls();
      return;
    }
    const state = this.state;
    const modelName = model.display_name;
    this.aiRunning = true;
    this.aiStatus = `正在准备 ${modelName}...`;
    this.setBusy(true, this.aiStatus, true);
    this.updateAiControls();
    try {
      const result = await runAiSegmentation(
        state.metadata.handle,
        state.metadata.active_stack,
        model.id,
      );
      if (this.state !== state) return;
      const appliedSegments = await this.applyAiSegmentationResult(result, model);
      const voxelCount = result.segments.reduce((total, segment) => total + segment.voxel_count, 0);
      this.aiStatus = `已生成 ${appliedSegments} 个 AI Segment · ${voxelCount.toLocaleString()} voxel`;
    } catch (error) {
      const message = errorMessage(error);
      if (/取消/.test(message)) this.aiStatus = 'AI 分割已取消';
      else {
        this.aiStatus = message;
        this.showError(`AI 分割失败: ${message}`);
      }
    } finally {
      this.aiRunning = false;
      this.setBusy(false);
      this.updateAiControls();
      this.render();
      this.updateUi();
    }
  }

  private async applyAiSegmentationResult(
    result: AiSegmentationResult,
    model: AiModelDescriptor,
  ): Promise<number> {
    if (!this.state) throw new Error('没有已打开的序列');
    const frame = this.state.metadata.frames[0];
    const prepared = result.segments
      .filter((segment) => segment.voxel_count > 0)
      .map((segment) => {
        const volume = createMaskVolume(
          frame.rows,
          frame.cols,
          this.mpr?.metadata.dimensions[2] ?? this.state!.metadata.frames.length,
        );
        const changed = new Set<number>();
        for (const mask of segment.masks) {
          if (mask.rows !== volume.rows || mask.cols !== volume.cols) {
            throw new Error(`AI Segment ${segment.label.display_name} 尺寸不一致`);
          }
          const data = decodeMaskRle(
            base64ToBytes(mask.data_base64),
            volume.rows * volume.cols,
          );
          if (data.some(Boolean)) {
            volume.sourceSlices.set(mask.source_index, data);
            changed.add(mask.source_index);
          }
        }
        volume.generation = 1;
        return { segment, volume, changed };
      })
      .filter((entry) => entry.changed.size > 0);
    if (!prepared.length) throw new Error('AI 未识别到可显示的目标');

    const created: Array<{ segment: SegmentationSegment; volume: MaskVolume; changed: Set<number> }> = [];
    for (const entry of prepared) {
      const segment = await this.createAiSegment(entry.segment.label, model);
      created.push({ ...entry, segment });
    }
    for (const entry of created) {
      this.segmentationSegments.push(entry.segment);
      this.maskVolumes.set(entry.segment.id, entry.volume);
      this.queueMaskSync(entry.segment.id, entry.changed);
    }
    this.segmentationSegment = created[0].segment;
    this.maskTagFilter = '';
    this.maskMatchedSegmentIds = null;
    this.maskUndoEntries = [];
    this.maskRedoEntries = [];
    this.updateMaskSegmentOptions();
    return created.length;
  }

  private async createAiSegment(
    label: AiLabelDescriptor,
    model: AiModelDescriptor,
  ): Promise<SegmentationSegment> {
    if (!this.state) throw new Error('没有已打开的序列');
    const timestamp = new Date().toISOString();
    const description = `${model.display_name} ${model.version}`;
    if (!this.remoteSeriesOpen) {
      const id = `local-ai-segment-${makeId()}`;
      return {
        id,
        project_id: `local-ai-project-${makeId()}`,
        segment_number: this.segmentationSegments.length + 1,
        label: label.display_name,
        description,
        color_r: label.color[0],
        color_g: label.color[1],
        color_b: label.color[2],
        algorithm_type: 'automatic',
        tags: [...new Set(label.tags)],
        created_at: timestamp,
        updated_at: timestamp,
      };
    }
    const studyUid = this.state.metadata.study_uid;
    const seriesUid = this.state.metadata.series_uid;
    if (!studyUid || !seriesUid) throw new Error('当前序列缺少 DICOM UID');
    const created = await createSegmentationProject(studyUid, seriesUid, {
      id: makeId(),
      segment_id: makeId(),
      name: `AI ${label.display_name}`,
      segment_label: label.display_name,
      segment_description: description,
      color: label.color,
      algorithm_type: 'automatic',
      tags: [...new Set(label.tags)],
    });
    this.segmentationProjects.push(created.project);
    return created.segment;
  }

  private async applyMaskTagFilter(tag: string): Promise<void> {
    this.maskTagFilter = tag.trim();
    const generation = ++this.maskTagQueryGeneration;
    if (!this.maskTagFilter) {
      this.maskMatchedSegmentIds = null;
    } else if (!this.remoteSeriesOpen) {
      this.maskMatchedSegmentIds = new Set(
        this.segmentationSegments
          .filter((segment) => segment.tags.includes(this.maskTagFilter))
          .map((segment) => segment.id),
      );
    } else {
      const studyUid = this.state?.metadata.study_uid;
      const seriesUid = this.state?.metadata.series_uid;
      if (!studyUid || !seriesUid) return;
      try {
        const groups = await Promise.all(
          this.segmentationProjects.map((project) => listSegmentationSegments(
            studyUid,
            seriesUid,
            project.id,
            this.maskTagFilter,
          )),
        );
        if (generation !== this.maskTagQueryGeneration) return;
        this.maskMatchedSegmentIds = new Set(groups.flat().map((segment) => segment.id));
      } catch (error) {
        if (generation !== this.maskTagQueryGeneration) return;
        this.maskMatchedSegmentIds = new Set(
          this.segmentationSegments
            .filter((segment) => segment.tags.includes(this.maskTagFilter))
            .map((segment) => segment.id),
        );
        this.showError(`Mask Tag 查询失败: ${errorMessage(error)}`);
      }
    }
    const visible = this.visibleMaskSegments();
    if (!visible.some((segment) => segment.id === this.segmentationSegment?.id)) {
      this.segmentationSegment = visible[0] ?? null;
    }
    this.updateMaskSegmentOptions();
    this.updateAiControls();
    this.updateUi();
    this.render();
  }

  private async saveMaskTags(): Promise<void> {
    const selected = this.segmentationSegment;
    if (!selected || this.maskTagSaving) return;
    const values = requiredElement<HTMLInputElement>('mask-tag-input').value
      .split(/[,，]/)
      .map((tag) => tag.trim())
      .filter(Boolean);
    const tags = [...new Set(values)];
    if (tags.length > 16 || tags.some((tag) => [...tag].length > 40)) {
      this.showError('一个 Mask 最多设置 16 个 Tag，单个 Tag 最多 40 个字符');
      return;
    }
    this.maskTagSaving = true;
    this.updateMaskSegmentOptions();
    try {
      let updated: SegmentationSegment;
      if (this.remoteSeriesOpen) {
        const studyUid = this.state?.metadata.study_uid;
        const seriesUid = this.state?.metadata.series_uid;
        if (!studyUid || !seriesUid) throw new Error('当前序列缺少 DICOM UID');
        updated = await updateSegmentationSegmentTags(
          studyUid,
          seriesUid,
          selected.project_id,
          selected.id,
          tags,
        );
      } else {
        updated = { ...selected, tags, updated_at: new Date().toISOString() };
      }
      const index = this.segmentationSegments.findIndex((segment) => segment.id === updated.id);
      if (index >= 0) this.segmentationSegments[index] = updated;
      this.segmentationSegment = updated;
      if (this.maskTagFilter) await this.applyMaskTagFilter(this.maskTagFilter);
    } catch (error) {
      this.showError(`Mask Tag 保存失败: ${errorMessage(error)}`);
    } finally {
      this.maskTagSaving = false;
      this.updateMaskSegmentOptions();
      this.render();
    }
  }

  private async selectMaskSegment(segmentId: string): Promise<void> {
    const segment = this.segmentationSegments.find((candidate) => candidate.id === segmentId);
    if (!segment) return;
    this.segmentationSegment = segment;
    if (!this.maskVolumes.has(segment.id)) await this.loadSegmentationVolumes();
    this.updateMaskSegmentOptions();
    this.render();
    this.updateUi();
  }

  private async deleteSelectedSegmentation(): Promise<void> {
    const selected = this.segmentationSegment;
    if (!selected || this.maskDeleting) return;
    if (this.maskSyncingSegments.has(selected.id)) {
      this.showError('当前分割仍在保存，请稍后再删除');
      return;
    }
    if (!window.confirm(`删除分割“${selected.label}”及其全部 Mask 数据？此操作无法撤销。`)) {
      return;
    }
    this.maskDeleting = true;
    this.updateMaskSegmentOptions();
    try {
      if (this.remoteSeriesOpen) {
        const studyUid = this.state?.metadata.study_uid;
        const seriesUid = this.state?.metadata.series_uid;
        if (!studyUid || !seriesUid) throw new Error('当前序列缺少 DICOM UID');
        await deleteSegmentationProject(studyUid, seriesUid, selected.project_id);
      }
      const removedIds = new Set(
        this.segmentationSegments
          .filter((segment) => segment.project_id === selected.project_id)
          .map((segment) => segment.id),
      );
      this.segmentationProjects = this.segmentationProjects.filter(
        (project) => project.id !== selected.project_id,
      );
      this.segmentationSegments = this.segmentationSegments.filter(
        (segment) => !removedIds.has(segment.id),
      );
      for (const segmentId of removedIds) {
        this.maskVolumes.delete(segmentId);
        this.maskDirtySlices.delete(segmentId);
        this.maskSyncErrors.delete(segmentId);
        this.maskMatchedSegmentIds?.delete(segmentId);
      }
      this.maskUndoEntries = this.maskUndoEntries.filter((entry) => !removedIds.has(entry.segmentId));
      this.maskRedoEntries = this.maskRedoEntries.filter((entry) => !removedIds.has(entry.segmentId));
      this.segmentationSegment = this.visibleMaskSegments()[0] ?? null;
    } catch (error) {
      this.showError(`删除分割失败: ${errorMessage(error)}`);
    } finally {
      this.maskDeleting = false;
      this.updateMaskSegmentOptions();
      this.render();
      this.updateUi();
    }
  }

  private async ensureSegmentationWorkspace(): Promise<void> {
    if (this.segmentationSegment) return;
    if (this.maskWorkspacePromise) return this.maskWorkspacePromise;
    this.maskWorkspacePromise = (async () => {
      if (!this.remoteSeriesOpen) {
        const segment = localSegmentationSegment();
        this.segmentationSegments = [segment];
        this.segmentationSegment = segment;
        this.maskVolumes.set(segment.id, this.createEmptyMaskVolume());
        return;
      }
      const studyUid = this.state?.metadata.study_uid;
      const seriesUid = this.state?.metadata.series_uid;
      if (!studyUid || !seriesUid) throw new Error('当前序列缺少 DICOM UID，不能创建分割项目');
      const color: [number, number, number] = [55, 213, 216];
      const created = await createSegmentationProject(studyUid, seriesUid, {
        id: makeId(),
        segment_id: makeId(),
        name: '手工分割',
        segment_label: 'Segment 1',
        color,
      });
      this.segmentationProjects.push(created.project);
      this.segmentationSegments.push(created.segment);
      this.segmentationSegment = created.segment;
      this.maskVolumes.set(created.segment.id, this.createEmptyMaskVolume());
    })().finally(() => {
      this.maskWorkspacePromise = null;
      this.updateUi();
    });
    return this.maskWorkspacePromise;
  }

  private async loadSegmentationVolumes(): Promise<void> {
    if (!this.state) return;
    if (!this.remoteSeriesOpen) {
      for (const segment of this.segmentationSegments) {
        if (!this.maskVolumes.has(segment.id)) {
          this.maskVolumes.set(segment.id, this.createEmptyMaskVolume());
        }
      }
      return;
    }
    const studyUid = this.state.metadata.study_uid;
    const seriesUid = this.state.metadata.series_uid;
    if (!studyUid || !seriesUid) return;
    const state = this.state;
    const entries = await Promise.all(this.segmentationSegments.map(async (segment) => {
      const volume = this.createEmptyMaskVolume();
      const records = await listSegmentationVolume(studyUid, seriesUid, segment.project_id, segment.id);
      for (const record of records) {
        const sourceSlice = this.maskSourceIndex(record.sop_instance_uid, record.frame_number);
        if (sourceSlice == null) continue;
        if (record.rows !== volume.rows || record.cols !== volume.cols) {
          throw new Error(`Segment ${segment.label} 的 Mask 尺寸与来源序列不一致`);
        }
        const data = decodeMaskRle(base64ToBytes(record.data_base64), volume.rows * volume.cols);
        if (data.some(Boolean)) volume.sourceSlices.set(sourceSlice, data);
        volume.revisions.set(sourceSlice, record.revision);
        volume.syncStates.set(sourceSlice, 'synced');
      }
      return [segment.id, volume] as const;
    }));
    if (this.state !== state) return;
    this.maskVolumes = new Map(entries);
    this.render();
  }

  private createEmptyMaskVolume(): MaskVolume {
    if (!this.state) throw new Error('没有已打开的序列');
    const frame = this.state.metadata.frames[0];
    const slices = this.mpr?.metadata.dimensions[2] ?? this.state.metadata.frames.length;
    return createMaskVolume(frame.rows, frame.cols, slices);
  }

  private selectedMaskVolume(): MaskVolume | null {
    const segment = this.segmentationSegment;
    if (!segment) return null;
    let volume = this.maskVolumes.get(segment.id);
    if (!volume && this.state) {
      volume = this.createEmptyMaskVolume();
      this.maskVolumes.set(segment.id, volume);
    }
    return volume ?? null;
  }

  private maskSourceIndex(sopInstanceUid: string, frameNumber: number): number | null {
    const descriptors = this.mpr?.metadata.source_slices;
    if (descriptors) {
      const index = descriptors.findIndex(
        (source) => source.sop_instance_uid === sopInstanceUid && source.frame_number === frameNumber,
      );
      return index >= 0 ? index : null;
    }
    const index = this.state?.metadata.frames.findIndex(
      (frame) => frame.sop_instance_uid === sopInstanceUid && frame.source_frame === frameNumber,
    ) ?? -1;
    return index >= 0 ? index : null;
  }

  private currentMaskSourceIndex(): number {
    if (!this.state) return 0;
    const frame = this.currentFrame();
    const sourceIndex = this.mpr?.metadata.source_slices.findIndex(
      (source) => source.frame_key === frame.frame_key,
    ) ?? this.state.currentFrame;
    return sourceIndex >= 0 ? sourceIndex : this.state.currentFrame;
  }

  private currentMaskLayers(): MaskLayer[] {
    if (!this.state || this.viewerMode !== '2d') return [];
    const sourceSlice = this.currentMaskSourceIndex();
    const layers: MaskLayer[] = [];
    for (const segment of this.visibleMaskSegments()) {
      const volume = this.maskVolumes.get(segment.id);
      const data = volume?.sourceSlices.get(sourceSlice);
      if (!volume || !data?.some(Boolean)) continue;
      layers.push({
        data,
        rows: volume.rows,
        cols: volume.cols,
        color: maskSegmentColor(segment),
        opacity: this.maskOpacity,
      });
    }
    return layers;
  }

  private currentMprMaskLayers(plane: MprPlane): MaskLayer[] {
    if (!this.mpr || this.viewerMode !== 'mpr') return [];
    if (plane === 'oblique' || this.mprObliqueMode) return [];
    const metadata = requirePlane(this.mpr.metadata, plane);
    const sliceIndex = this.mpr.viewports[plane].sliceIndex;
    const layers: MaskLayer[] = [];
    for (const segment of this.visibleMaskSegments()) {
      const volume = this.maskVolumes.get(segment.id);
      if (!volume) continue;
      const data = renderMaskPlane(volume, this.mpr.metadata, plane, sliceIndex);
      if (!data.some(Boolean)) continue;
      layers.push({
        data,
        rows: metadata.rows,
        cols: metadata.cols,
        color: maskSegmentColor(segment),
        opacity: this.maskOpacity,
      });
    }
    return layers;
  }

  private recordMaskChange(
    segmentId: string,
    before: MaskSliceSnapshot,
    changedSlices: Set<number>,
  ): void {
    const volume = this.maskVolumes.get(segmentId);
    if (!volume || !changedSlices.size) return;
    const after = snapshotMaskSlices(volume, changedSlices);
    this.maskUndoEntries.push({ segmentId, before: cloneMaskSnapshot(before), after });
    if (this.maskUndoEntries.length > 100) this.maskUndoEntries.shift();
    this.maskRedoEntries = [];
    this.queueMaskSync(segmentId, changedSlices);
  }

  private queueMaskSync(segmentId: string, slices: Iterable<number>): void {
    if (!this.remoteSeriesOpen) return;
    const dirty = this.maskDirtySlices.get(segmentId) ?? new Set<number>();
    for (const slice of slices) dirty.add(slice);
    this.maskDirtySlices.set(segmentId, dirty);
    this.maskSyncErrors.delete(segmentId);
    void this.drainMaskSync(segmentId);
  }

  private async drainMaskSync(segmentId: string): Promise<void> {
    if (this.maskSyncingSegments.has(segmentId)) return;
    const segment = this.segmentationSegments.find((candidate) => candidate.id === segmentId);
    const volume = this.maskVolumes.get(segmentId);
    if (!this.remoteSeriesOpen || !this.state || !segment || !volume) return;
    const studyUid = this.state.metadata.study_uid;
    const seriesUid = this.state.metadata.series_uid;
    if (!studyUid || !seriesUid) return;
    const state = this.state;
    const mpr = this.mpr;
    let inFlightSlices: number[] = [];
    this.maskSyncingSegments.add(segmentId);
    try {
      while (this.maskDirtySlices.get(segmentId)?.size) {
        const dirty = this.maskDirtySlices.get(segmentId)!;
        const slices = [...dirty].sort((left, right) => left - right);
        dirty.clear();
        inFlightSlices = slices;
        const saved = snapshotMaskSlices(volume, slices);
        const updates = slices.map((slice) => {
          const source = this.maskSourceDescriptor(slice);
          if (!source?.sop_instance_uid) {
            throw new Error(`来源层 ${slice + 1} 缺少 SOPInstanceUID`);
          }
          volume.syncStates.set(slice, 'pending');
          const data = saved.get(slice) ?? new Uint8Array(volume.rows * volume.cols);
          return {
            sop_instance_uid: source.sop_instance_uid,
            frame_number: source.frame_number,
            rows: volume.rows,
            cols: volume.cols,
            encoding: 'rle-v1',
            data_base64: bytesToBase64(encodeMaskRle(data)),
            expected_revision: volume.revisions.get(slice) ?? 0,
          };
        });
        this.updateUi();
        const records = await upsertSegmentationMasks(
          studyUid,
          seriesUid,
          segment.project_id,
          segmentId,
          updates,
        );
        if (this.state !== state || this.mpr !== mpr) return;
        for (const record of records) {
          const slice = this.maskSourceIndex(record.sop_instance_uid, record.frame_number);
          if (slice == null) continue;
          volume.revisions.set(slice, record.revision);
          volume.syncStates.set(slice, 'synced');
        }
      }
    } catch (error) {
      if (this.state !== state || this.mpr !== mpr) return;
      for (const slice of [...inFlightSlices, ...(this.maskDirtySlices.get(segmentId) ?? [])]) {
        const dirty = this.maskDirtySlices.get(segmentId) ?? new Set<number>();
        dirty.add(slice);
        this.maskDirtySlices.set(segmentId, dirty);
        volume.syncStates.set(slice, 'error');
      }
      this.maskSyncErrors.add(segmentId);
      this.showError(`Mask 保存失败: ${errorMessage(error)}`);
    } finally {
      this.maskSyncingSegments.delete(segmentId);
      this.render();
      this.updateUi();
    }
  }

  private maskSourceDescriptor(slice: number): { sop_instance_uid: string | null; frame_number: number } | null {
    if (this.mpr) return this.mpr.metadata.source_slices[slice] ?? null;
    const frame = this.state?.metadata.frames[slice];
    return frame ? { sop_instance_uid: frame.sop_instance_uid, frame_number: frame.source_frame } : null;
  }

  private resetView(): void {
    if (!this.state) return;
    if (this.viewerMode === 'vr') {
      this.volumeRenderer?.resetView();
      return;
    }
    if (this.viewerMode === 'mpr' && this.mpr) {
      for (const plane of MPR_PLANES) {
        this.mpr.viewports[plane].zoom = 1;
        this.mpr.viewports[plane].panX = 0;
        this.mpr.viewports[plane].panY = 0;
        this.mpr.viewports[plane].rotation = 0;
        this.mpr.viewports[plane].flipHorizontal = false;
        this.mpr.viewports[plane].flipVertical = false;
        this.mpr.viewports[plane].inverted = false;
      }
      this.setMprCrosshair(point3(this.mpr.metadata.initial_crosshair));
      this.resetObliqueToStandard();
      return;
    }
    this.state.zoom = 1;
    this.state.panX = 0;
    this.state.panY = 0;
    this.state.rotation = 0;
    this.state.flipHorizontal = false;
    this.state.flipVertical = false;
    this.state.inverted = false;
    this.render();
    this.updateUi();
  }

  private applyPreset(value: string): void {
    if (!this.state) return;
    const preset = this.resolveWindowPreset(value);
    if (!preset) return;
    this.state.windowCenter = preset.center;
    this.state.windowWidth = preset.width;
    this.state.voiFunction = preset.function;
    this.updateUi();
    if (this.viewerMode === 'mpr') {
      this.stopMprPrefetch();
      if (this.mprObliqueMode) this.renderAllMprPlanes();
      void this.refreshMprSlices().then(() => this.scheduleMprPrefetch(0));
    } else {
      void this.refreshLut();
    }
  }

  private resolveWindowPreset(value: string): WindowPreset | undefined {
    const selection = parseWindowPresetSelection(value);
    if (!selection) return undefined;
    if (selection.source === 'dicom') return this.currentFrame()?.window_presets[selection.id];
    if (selection.source === 'user') {
      return this.userWindowPresets.find((preset) => preset.id === selection.id);
    }
    return undefined;
  }

  private currentModality(): string | null {
    return normalizedModality(this.state?.metadata.patient.modality);
  }

  private selectedUserWindowPreset(): UserWindowPreset | null {
    const selection = parseWindowPresetSelection(this.presetSelect.value);
    if (selection?.source !== 'user') return null;
    return this.userWindowPresets.find((preset) => preset.id === selection.id) ?? null;
  }

  private openCreateWindowPreset(): void {
    if (!this.state || !this.currentModality() || this.viewerMode === 'vr') return;
    const frame = this.currentFrame();
    if (frame.pixel_format === 'rgb8' && this.viewerMode === '2d') return;
    this.windowPresetDialogMode = 'create';
    this.windowPresetEditingId = null;
    setText('window-preset-dialog-title', '保存窗预设');
    setText(
      'window-preset-dialog-summary',
      `${this.currentModality()} · WL ${this.state.windowCenter.toFixed(0)} · WW ${this.state.windowWidth.toFixed(0)}`,
    );
    requiredElement<HTMLInputElement>('window-preset-name').value = '';
    requiredElement<HTMLButtonElement>('window-preset-submit').textContent = '保存';
    requiredElement<HTMLElement>('window-preset-error').hidden = true;
    if (!this.windowPresetDialog.open) this.windowPresetDialog.showModal();
    requiredElement<HTMLInputElement>('window-preset-name').focus();
  }

  private openRenameWindowPreset(): void {
    const preset = this.selectedUserWindowPreset();
    if (!preset) return;
    this.windowPresetDialogMode = 'rename';
    this.windowPresetEditingId = preset.id;
    setText('window-preset-dialog-title', '重命名窗预设');
    setText(
      'window-preset-dialog-summary',
      `${preset.modality} · WL ${preset.center.toFixed(0)} · WW ${preset.width.toFixed(0)}`,
    );
    requiredElement<HTMLInputElement>('window-preset-name').value = preset.name;
    requiredElement<HTMLButtonElement>('window-preset-submit').textContent = '重命名';
    requiredElement<HTMLElement>('window-preset-error').hidden = true;
    if (!this.windowPresetDialog.open) this.windowPresetDialog.showModal();
    const input = requiredElement<HTMLInputElement>('window-preset-name');
    input.focus();
    input.select();
  }

  private closeWindowPresetDialog(): void {
    if (this.windowPresetBusy) return;
    if (this.windowPresetDialog.open) this.windowPresetDialog.close();
    this.windowPresetEditingId = null;
  }

  private async submitWindowPreset(): Promise<void> {
    if (this.windowPresetBusy) return;
    const input = requiredElement<HTMLInputElement>('window-preset-name');
    const errorElement = requiredElement<HTMLElement>('window-preset-error');
    const submit = requiredElement<HTMLButtonElement>('window-preset-submit');
    const name = input.value.trim();
    if (!name || [...name].length > 64) {
      errorElement.textContent = '窗预设名称必须为 1 到 64 个字符';
      errorElement.hidden = false;
      return;
    }
    this.windowPresetBusy = true;
    submit.disabled = true;
    errorElement.hidden = true;
    try {
      let saved: UserWindowPreset;
      if (this.windowPresetDialogMode === 'create') {
        const modality = this.currentModality();
        if (!this.state || !modality) throw new Error('当前影像没有可用的模态信息');
        saved = await createWindowPreset(
          modality,
          name,
          this.state.windowCenter,
          this.state.windowWidth,
          this.state.voiFunction,
        );
        this.userWindowPresets.push(saved);
      } else {
        if (this.windowPresetEditingId === null) throw new Error('没有选中的个人窗预设');
        saved = await renameWindowPreset(this.windowPresetEditingId, name);
        const index = this.userWindowPresets.findIndex((preset) => preset.id === saved.id);
        if (index >= 0) this.userWindowPresets[index] = saved;
      }
      this.sortUserWindowPresets();
      this.windowPresetDialog.close();
      this.windowPresetEditingId = null;
      this.updateUi();
      this.presetSelect.value = `user:${saved.id}`;
      this.updateWindowPresetControls();
    } catch (error) {
      errorElement.textContent = errorMessage(error);
      errorElement.hidden = false;
    } finally {
      this.windowPresetBusy = false;
      submit.disabled = false;
    }
  }

  private async deleteSelectedWindowPreset(): Promise<void> {
    const preset = this.selectedUserWindowPreset();
    if (!preset || this.windowPresetBusy) return;
    if (!window.confirm(`确定删除个人窗预设“${preset.name}”吗？`)) return;
    this.windowPresetBusy = true;
    this.updateWindowPresetControls();
    try {
      await deleteWindowPreset(preset.id);
      this.userWindowPresets = this.userWindowPresets.filter((entry) => entry.id !== preset.id);
      this.updateUi();
    } catch (error) {
      this.showError(errorMessage(error));
    } finally {
      this.windowPresetBusy = false;
      this.updateWindowPresetControls();
    }
  }

  private sortUserWindowPresets(): void {
    this.userWindowPresets.sort((left, right) =>
      left.modality.localeCompare(right.modality)
      || left.name.localeCompare(right.name, 'zh-CN')
      || left.id - right.id,
    );
  }

  private async loadUserWindowPresets(): Promise<void> {
    try {
      this.userWindowPresets = await listWindowPresets();
      this.sortUserWindowPresets();
      if (this.state) this.updateUi();
    } catch (error) {
      this.userWindowPresets = [];
      this.showError(`个人窗预设加载失败：${errorMessage(error)}`);
    }
  }

  private applyVolumePreset(preset: VolumePreset): void {
    if (!this.state || !this.mpr || !this.volumeRenderer) return;
    this.volumePreset = preset;
    const range = this.mpr.metadata.volume_rendering.value_range;
    const settings: Partial<Record<VolumePreset, [number, number]>> = {
      soft_tissue: [40, 400],
      bone: [500, 2000],
      bone_color: [500, 1800],
      lung: [-600, 1500],
      pet: [(range[0] + range[1]) / 2, Math.max(1, range[1] - range[0])],
      grayscale: [this.state.windowCenter, this.state.windowWidth],
    };
    const [center, width] = settings[preset] ?? [this.volumeWindowCenter, this.volumeWindowWidth];
    this.volumeWindowCenter = center;
    this.volumeWindowWidth = width;
    this.volumeRenderer.setPreset(preset);
    this.volumeRenderer.setWindow(center, width);
    this.updateUi();
  }

  private vrWindowPointerDown(event: PointerEvent): void {
    if (this.viewerMode !== 'vr' || !this.volumeRenderer) return;
    if (event.button !== 0) return;
    // 阻止 OrbitControls 处理左键：左键留给窗宽窗位手势。
    event.preventDefault();
    event.stopImmediatePropagation();
    this.volumeCanvas.setPointerCapture(event.pointerId);
    this.volumeWindowDrag = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      center: this.volumeWindowCenter,
      width: this.volumeWindowWidth,
    };
  }

  private vrWindowPointerMove(event: PointerEvent): void {
    const drag = this.volumeWindowDrag;
    if (!drag || drag.pointerId !== event.pointerId || !this.volumeRenderer) return;
    const sensitivity = Math.max(1, drag.width / 512);
    const deltaX = event.clientX - drag.startX;
    const deltaY = event.clientY - drag.startY;
    this.volumeWindowCenter = drag.center + deltaX * sensitivity;
    this.volumeWindowWidth = Math.max(1, drag.width + deltaY * sensitivity * 2);
    this.volumeRenderer.setWindow(this.volumeWindowCenter, this.volumeWindowWidth);
    this.updateUi();
  }

  private vrWindowPointerUp(event: PointerEvent): void {
    if (this.volumeWindowDrag?.pointerId !== event.pointerId) return;
    if (this.volumeCanvas.hasPointerCapture(event.pointerId)) {
      this.volumeCanvas.releasePointerCapture(event.pointerId);
    }
    this.volumeWindowDrag = null;
  }


  private setupEventListeners(): void {
    requiredElement<HTMLFormElement>('login-form').addEventListener('submit', (event) => {
      event.preventDefault();
      void this.login();
    });
    requiredElement<HTMLButtonElement>('forgot-password-btn').addEventListener('click', () => {
      this.openPasswordReset();
    });
    requiredElement<HTMLButtonElement>('password-reset-close').addEventListener('click', () => {
      requiredElement<HTMLDialogElement>('password-reset-dialog').close();
    });
    requiredElement<HTMLButtonElement>('password-reset-cancel').addEventListener('click', () => {
      requiredElement<HTMLDialogElement>('password-reset-dialog').close();
    });
    requiredElement<HTMLFormElement>('password-reset-form').addEventListener('submit', (event) => {
      event.preventDefault();
      void this.submitPasswordReset();
    });
    requiredElement<HTMLButtonElement>('choose-ca-btn').addEventListener('click', () => {
      void this.chooseCertificate();
    });
    requiredElement<HTMLButtonElement>('logout-btn').addEventListener('click', () => {
      void this.logout();
    });
    requiredElement<HTMLButtonElement>('window-preset-save').addEventListener('click', () => {
      this.openCreateWindowPreset();
    });
    requiredElement<HTMLButtonElement>('window-preset-rename').addEventListener('click', () => {
      this.openRenameWindowPreset();
    });
    requiredElement<HTMLButtonElement>('window-preset-delete').addEventListener('click', () => {
      void this.deleteSelectedWindowPreset();
    });
    requiredElement<HTMLFormElement>('window-preset-form').addEventListener('submit', (event) => {
      event.preventDefault();
      void this.submitWindowPreset();
    });
    for (const id of ['window-preset-close', 'window-preset-cancel']) {
      requiredElement<HTMLButtonElement>(id).addEventListener('click', () => {
        this.closeWindowPresetDialog();
      });
    }
    requiredElement<HTMLFormElement>('study-share-form').addEventListener('submit', (event) => {
      event.preventDefault();
      void this.submitStudyShare();
    });
    for (const id of ['study-share-close', 'study-share-cancel']) {
      requiredElement<HTMLButtonElement>(id).addEventListener('click', () => this.closeStudyShare());
    }
    requiredElement<HTMLButtonElement>('queue-btn').addEventListener('click', () => {
      if (this.examRequestPage.isOpen()) this.examRequestPage.close();
      this.queuePage.open();
    });
    requiredElement<HTMLButtonElement>('refresh-worklist').addEventListener('click', () => {
      void this.refreshPatientContext();
    });
    const maskMenuButton = requiredElement<HTMLButtonElement>('mask-menu-button');
    const maskMenuPanel = requiredElement<HTMLElement>('mask-menu-panel');
    const positionMaskMenu = (): void => {
      const rect = maskMenuButton.getBoundingClientRect();
      const width = maskMenuPanel.offsetWidth || 262;
      maskMenuPanel.style.left = `${Math.max(8, Math.min(rect.left, window.innerWidth - width - 8))}px`;
    };
    const closeMaskMenu = (): void => {
      maskMenuPanel.hidden = true;
      maskMenuButton.setAttribute('aria-expanded', 'false');
    };
    const toolbarMenuButtons = [...document.querySelectorAll<HTMLButtonElement>('[data-toolbar-menu-button]')];
    const toolbarMenuPanel = (button: HTMLButtonElement): HTMLElement => {
      const panelId = button.getAttribute('aria-controls');
      if (!panelId) throw new Error('工具栏菜单缺少 aria-controls');
      return requiredElement<HTMLElement>(panelId);
    };
    const closeToolbarMenus = (): void => {
      for (const button of toolbarMenuButtons) {
        toolbarMenuPanel(button).hidden = true;
        button.setAttribute('aria-expanded', 'false');
      }
    };
    const positionToolbarMenu = (button: HTMLButtonElement, panel: HTMLElement): void => {
      const rect = button.getBoundingClientRect();
      const width = panel.offsetWidth || 196;
      const height = panel.offsetHeight;
      const requestedLeft = button.dataset.menuAlign === 'end' ? rect.right - width : rect.left;
      const left = Math.max(8, Math.min(requestedLeft, window.innerWidth - width - 8));
      const below = rect.bottom + 6;
      const top = below + height <= window.innerHeight - 8
        ? below
        : Math.max(8, rect.top - height - 6);
      panel.style.left = `${left}px`;
      panel.style.top = `${top}px`;
    };
    for (const button of toolbarMenuButtons) {
      const panel = toolbarMenuPanel(button);
      button.addEventListener('click', (event) => {
        event.stopPropagation();
        const opening = panel.hidden;
        closeToolbarMenus();
        closeMaskMenu();
        if (!opening) return;
        panel.hidden = false;
        button.setAttribute('aria-expanded', 'true');
        positionToolbarMenu(button, panel);
      });
      button.addEventListener('keydown', (event) => {
        if (event.key !== 'ArrowDown') return;
        event.preventDefault();
        if (panel.hidden) button.click();
        panel.querySelector<HTMLButtonElement>('button:not(:disabled):not([hidden])')?.focus();
      });
      panel.addEventListener('click', (event) => {
        event.stopPropagation();
        const item = (event.target as HTMLElement).closest<HTMLButtonElement>('button');
        if (item && !item.disabled) closeToolbarMenus();
      });
      panel.addEventListener('keydown', (event) => {
        if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
        const items = [...panel.querySelectorAll<HTMLButtonElement>('button:not(:disabled):not([hidden])')];
        if (!items.length) return;
        event.preventDefault();
        const current = items.indexOf(document.activeElement as HTMLButtonElement);
        const direction = event.key === 'ArrowDown' ? 1 : -1;
        const next = current < 0
          ? 0
          : (current + direction + items.length) % items.length;
        items[next].focus();
      });
    }
    maskMenuButton.addEventListener('click', (event) => {
      event.stopPropagation();
      closeToolbarMenus();
      maskMenuPanel.hidden = !maskMenuPanel.hidden;
      maskMenuButton.setAttribute('aria-expanded', String(!maskMenuPanel.hidden));
      if (!maskMenuPanel.hidden) {
        positionMaskMenu();
        this.updateMaskSegmentOptions();
        void this.loadAiModels();
      }
    });
    maskMenuPanel.addEventListener('click', (event) => event.stopPropagation());
    document.addEventListener('click', (event) => {
      if (!maskMenuPanel.contains(event.target as Node) && event.target !== maskMenuButton) closeMaskMenu();
      closeToolbarMenus();
    });
    document.addEventListener('keydown', (event) => {
      if (event.key !== 'Escape') return;
      const expanded = toolbarMenuButtons.find((button) => button.getAttribute('aria-expanded') === 'true');
      closeToolbarMenus();
      expanded?.focus();
    });
    window.addEventListener('resize', closeToolbarMenus);
    requiredElement<HTMLSelectElement>('mask-segment-select').addEventListener('change', (event) => {
      void this.selectMaskSegment((event.currentTarget as HTMLSelectElement).value);
    });
    requiredElement<HTMLButtonElement>('mask-segment-delete').addEventListener('click', () => {
      void this.deleteSelectedSegmentation();
    });
    requiredElement<HTMLFormElement>('mask-tag-form').addEventListener('submit', (event) => {
      event.preventDefault();
      void this.saveMaskTags();
    });
    requiredElement<HTMLSelectElement>('mask-tag-filter').addEventListener('change', (event) => {
      void this.applyMaskTagFilter((event.currentTarget as HTMLSelectElement).value);
    });
    requiredElement<HTMLSelectElement>('ai-model-select').addEventListener('change', (event) => {
      this.aiSelectedModelId = (event.currentTarget as HTMLSelectElement).value;
      this.aiStatus = '';
      this.updateAiControls();
    });
    requiredElement<HTMLButtonElement>('ai-segment-run').addEventListener('click', () => {
      void this.runSelectedAiSegmentation();
    });
    requiredElement<HTMLButtonElement>('ai-plugins-refresh').addEventListener('click', () => {
      void this.loadAiModels(true);
    });
    requiredElement<HTMLButtonElement>('ai-plugins-manage-btn').addEventListener('click', () => {
      closeToolbarMenus();
      void this.openAiPluginDialog();
    });
    for (const id of ['ai-plugin-close', 'ai-plugin-cancel']) {
      requiredElement<HTMLButtonElement>(id).addEventListener('click', () => this.closeAiPluginDialog());
    }
    for (const id of ['ai-plugin-name', 'ai-plugin-path']) {
      requiredElement<HTMLInputElement>(id).addEventListener('input', () => this.invalidateAiPluginCheck());
    }
    requiredElement<HTMLButtonElement>('ai-plugin-browse').addEventListener('click', async () => {
      const path = await chooseAiPluginFolder();
      if (!path) return;
      requiredElement<HTMLInputElement>('ai-plugin-path').value = path;
      this.invalidateAiPluginCheck();
    });
    requiredElement<HTMLButtonElement>('ai-plugin-check').addEventListener('click', () => {
      void this.detectAiPlugin();
    });
    requiredElement<HTMLFormElement>('ai-plugin-form').addEventListener('submit', (event) => {
      event.preventDefault();
      void this.saveAiPlugin();
    });
    requiredElement<HTMLButtonElement>('cancel-transfer').addEventListener('click', () => {
      if (this.transferKind) void cancelTransfer(this.transferKind).catch((error) => this.showError(errorMessage(error)));
    });
    requiredElement<HTMLFormElement>('patient-search').addEventListener('submit', (event) => {
      event.preventDefault();
      this.patientPage = 0;
      void this.loadPatients();
    });
    requiredElement<HTMLButtonElement>('patients-previous').addEventListener('click', () => {
      if (this.patientPage === 0) return;
      this.patientPage -= 1;
      void this.loadPatients();
    });
    requiredElement<HTMLButtonElement>('patients-next').addEventListener('click', () => {
      if (!this.hasNextPatientPage) return;
      this.patientPage += 1;
      void this.loadPatients();
    });
    requiredElement<HTMLFormElement>('tag-editor-form').addEventListener('submit', (event) => {
      event.preventDefault();
      void this.previewTagEdit();
    });
    for (const id of ['tag-editor-close', 'tag-editor-cancel']) {
      requiredElement<HTMLButtonElement>(id).addEventListener('click', () => {
        this.closeTagEditor();
      });
    }
    requiredElement<HTMLButtonElement>('tag-confirm-btn').addEventListener('click', () => {
      void this.confirmTagEdit();
    });
    requiredElement<HTMLButtonElement>('transform-tasks-btn').addEventListener('click', () => {
      void this.openTransformTasks();
    });
    requiredElement<HTMLButtonElement>('transform-tasks-refresh').addEventListener('click', () => {
      void this.loadTransformTasks();
    });
    requiredElement<HTMLButtonElement>('transform-tasks-close').addEventListener('click', () => {
      this.closeTransformTasks();
    });
    this.transformTasksDialog.addEventListener('close', () => this.stopTransformTaskPolling());
    requiredElement<HTMLButtonElement>('revision-history-btn').addEventListener('click', () => {
      void this.openRevisionHistory();
    });
    requiredElement<HTMLButtonElement>('revision-history-close').addEventListener('click', () => {
      this.closeRevisionHistory();
    });
    requiredElement<HTMLButtonElement>('rollback-cancel').addEventListener('click', () => {
      this.cancelRollback();
    });
    requiredElement<HTMLButtonElement>('rollback-preview-btn').addEventListener('click', () => {
      void this.previewSelectedRollback();
    });
    requiredElement<HTMLButtonElement>('rollback-confirm-btn').addEventListener('click', () => {
      void this.confirmSelectedRollback();
    });
    requiredElement<HTMLButtonElement>('cancel-download').addEventListener('click', () => {
      if (this.aiRunning) void cancelAiSegmentation();
      else if (this.mprBuildActive) void cancelMprBuild();
      else void cancelRemoteDownload();
      setText('loading-text', this.aiRunning ? '正在取消 AI 分割...' : '正在取消下载...');
      requiredElement<HTMLButtonElement>('cancel-download').disabled = true;
    });
    requiredElement<HTMLButtonElement>('open-btn').addEventListener('click', () => void this.openFiles());
    requiredElement<HTMLButtonElement>('empty-open-btn').addEventListener('click', () => void this.openFiles());
    requiredElement<HTMLButtonElement>('reset-btn').addEventListener('click', () => this.resetView());
    requiredElement<HTMLButtonElement>('undo-annotation').addEventListener('click', () => this.undoAnnotation());
    requiredElement<HTMLButtonElement>('redo-annotation').addEventListener('click', () => this.redoAnnotation());
    requiredElement<HTMLButtonElement>('toggle-annotations').addEventListener('click', () => this.toggleAnnotationVisibility());
    requiredElement<HTMLButtonElement>('retry-annotation-sync').addEventListener('click', () => this.retryAnnotationSync());
    requiredElement<HTMLButtonElement>('clear-current-annotations').addEventListener('click', () => this.clearAnnotations('current'));
    requiredElement<HTMLButtonElement>('clear-all-annotations').addEventListener('click', () => this.clearAnnotations('series'));
    requiredElement<HTMLButtonElement>('invert-btn').addEventListener('click', () => this.toggleInvert());
    requiredElement<HTMLButtonElement>('flip-horizontal-btn').addEventListener('click', () => this.flipView('horizontal'));
    requiredElement<HTMLButtonElement>('flip-vertical-btn').addEventListener('click', () => this.flipView('vertical'));
    requiredElement<HTMLButtonElement>('rotate-left-btn').addEventListener('click', () => this.rotateView(-1));
    requiredElement<HTMLButtonElement>('rotate-right-btn').addEventListener('click', () => this.rotateView(1));
    requiredElement<HTMLButtonElement>('previous-frame').addEventListener('click', () => {
      if (this.state) void this.setFrame(this.state.currentFrame - 1);
    });
    requiredElement<HTMLButtonElement>('next-frame').addEventListener('click', () => {
      if (this.state) void this.setFrame(this.state.currentFrame + 1);
    });
    requiredElement<HTMLButtonElement>('cine-toggle').addEventListener('click', () => {
      this.toggleCine();
    });
    this.cineSpeedSelect.addEventListener('change', () => {
      this.cineSpeed = Number(this.cineSpeedSelect.value) || 1;
      if (this.cinePlaying) {
        if (this.cineTimer != null) window.clearTimeout(this.cineTimer);
        this.cineTimer = null;
        this.scheduleCineFrame();
      }
      this.updateUi();
    });
    requiredElement<HTMLButtonElement>('error-close').addEventListener('click', () => {
      this.hideStatusBanner();
    });

    for (const button of document.querySelectorAll<HTMLButtonElement>('[data-tool]')) {
      button.addEventListener('click', () => void this.setTool(button.dataset.tool as ToolMode));
    }
    requiredElement<HTMLInputElement>('mask-brush-size').addEventListener('input', (event) => {
      this.maskBrushRadius = Number((event.currentTarget as HTMLInputElement).value);
      setText('mask-brush-size-value', `${this.maskBrushRadius} mm`);
    });
    requiredElement<HTMLInputElement>('mask-opacity').addEventListener('input', (event) => {
      this.maskOpacity = Number((event.currentTarget as HTMLInputElement).value) / 100;
      setText('mask-opacity-value', `${Math.round(this.maskOpacity * 100)}%`);
      this.render();
    });
    for (const button of document.querySelectorAll<HTMLButtonElement>('[data-view-mode]')) {
      button.addEventListener('click', () => {
        void this.setViewerMode(button.dataset.viewMode as ViewerMode);
      });
    }
    for (const button of document.querySelectorAll<HTMLButtonElement>('[data-mpr-projection]')) {
      button.addEventListener('click', () => {
        this.setMprProjection(button.dataset.mprProjection as MprProjectionMode);
      });
    }
    this.mprSlabThickness.addEventListener('input', () => {
      this.setMprSlabThickness(Number(this.mprSlabThickness.value));
    });
    requiredElement<HTMLSelectElement>('vr-preset').addEventListener('change', (event) => {
      this.applyVolumePreset((event.currentTarget as HTMLSelectElement).value as VolumePreset);
    });
    requiredElement<HTMLSelectElement>('vr-quality').addEventListener('change', (event) => {
      this.volumeQuality = (event.currentTarget as HTMLSelectElement).value as VolumeQuality;
      this.volumeRenderer?.setQuality(this.volumeQuality);
    });
    this.volumeCanvas.addEventListener('contextmenu', (event) => event.preventDefault());
    this.volumeCanvas.addEventListener('pointerdown', (event) => this.vrWindowPointerDown(event));
    this.volumeCanvas.addEventListener('pointermove', (event) => this.vrWindowPointerMove(event));
    this.volumeCanvas.addEventListener('pointerup', (event) => this.vrWindowPointerUp(event));
    this.volumeCanvas.addEventListener('pointercancel', (event) => this.vrWindowPointerUp(event));

    this.frameSlider.addEventListener('input', () => void this.setFrame(Number(this.frameSlider.value)));
    this.presetSelect.addEventListener('change', () => {
      this.applyPreset(this.presetSelect.value);
      this.updateWindowPresetControls();
    });
    this.bindSyncControls();
    this.imageStackSelect.addEventListener('change', () => {
      void this.switchImageStack(Number(this.imageStackSelect.value));
    });
    this.mprSourceSelect.addEventListener('change', () => {
      const studyUid = this.state?.metadata.study_uid;
      const seriesUid = this.mprSourceSelect.value;
      if (!studyUid || !seriesUid || seriesUid === this.state?.metadata.series_uid) return;
      void this.openRemote(studyUid, seriesUid);
    });

    for (const pane of document.querySelectorAll<HTMLElement>('.mpr-viewport')) {
      const plane = pane.dataset.plane as MprPlane;
      const canvas = pane.querySelector<HTMLCanvasElement>('canvas[id$="overlay-canvas"]');
      if (!canvas) continue;
      canvas.addEventListener('contextmenu', (event) => event.preventDefault());
      canvas.addEventListener('pointerdown', (event) => this.mprPointerDown(plane, canvas, event));
      canvas.addEventListener('pointermove', (event) => this.mprPointerMove(plane, canvas, event));
      canvas.addEventListener('pointerup', (event) => this.mprPointerUp(plane, canvas, event));
      canvas.addEventListener('pointercancel', (event) => this.mprPointerUp(plane, canvas, event));
      canvas.addEventListener('wheel', (event) => this.mprWheel(plane, canvas, event), {
        passive: false,
      });
      pane.addEventListener('dblclick', () => this.resetObliqueToStandard());
    }

    window.addEventListener('keydown', (event) => this.keyDown(event));
  }

  private restoreConnectionFields(): void {
    const savedUrl = localStorage.getItem('remote-pacs.server-url');
    const savedCa = localStorage.getItem('remote-pacs.ca-cert-path');
    if (savedUrl) requiredElement<HTMLInputElement>('server-url').value = savedUrl;
    if (savedCa) requiredElement<HTMLInputElement>('ca-cert-path').value = savedCa;
  }

  /** 打包版（内嵌本地服务）自动登录；非打包版静默跳过。 */
  private async autoLoginLocal(): Promise<void> {
    try {
      const info = await localStackInfo();
      if (!info) return;
      requiredElement<HTMLInputElement>('server-url').value = info.server_url;
      requiredElement<HTMLInputElement>('ca-cert-path').value = info.ca_cert_path;
      requiredElement<HTMLInputElement>('login-username').value = info.username;
      requiredElement<HTMLInputElement>('login-password').value = info.password;
      await this.login();
    } catch (error) {
      const loginError = requiredElement<HTMLElement>('login-error');
      loginError.textContent = `本地服务启动失败: ${errorMessage(error)}`;
      loginError.hidden = false;
    }
  }

  private async chooseCertificate(): Promise<void> {
    const selected = await chooseCaCertificate();
    if (selected) requiredElement<HTMLInputElement>('ca-cert-path').value = selected;
  }

  private openPasswordReset(): void {
    const dialog = requiredElement<HTMLDialogElement>('password-reset-dialog');
    requiredElement<HTMLInputElement>('password-reset-username').value =
      requiredElement<HTMLInputElement>('login-username').value.trim();
    requiredElement<HTMLInputElement>('password-reset-password').value = '';
    requiredElement<HTMLInputElement>('password-reset-confirm').value = '';
    requiredElement<HTMLElement>('password-reset-error').hidden = true;
    if (!dialog.open) dialog.showModal();
    requiredElement<HTMLInputElement>('password-reset-username').focus();
  }

  private async submitPasswordReset(): Promise<void> {
    const serverUrl = requiredElement<HTMLInputElement>('server-url').value.trim();
    const caCertPath = requiredElement<HTMLInputElement>('ca-cert-path').value.trim();
    const username = requiredElement<HTMLInputElement>('password-reset-username').value.trim();
    const password = requiredElement<HTMLInputElement>('password-reset-password').value;
    const confirmation = requiredElement<HTMLInputElement>('password-reset-confirm').value;
    const error = requiredElement<HTMLElement>('password-reset-error');
    const submit = requiredElement<HTMLButtonElement>('password-reset-submit');
    error.hidden = true;
    if (password !== confirmation) {
      error.textContent = '两次输入的新密码不一致。';
      error.hidden = false;
      return;
    }
    submit.disabled = true;
    try {
      await requestPasswordReset(serverUrl, caCertPath, username, password);
      requiredElement<HTMLDialogElement>('password-reset-dialog').close();
      const notice = requiredElement<HTMLElement>('login-notice');
      notice.textContent = '申请已提交。管理员审核通过后，即可使用新密码登录。';
      notice.hidden = false;
      requiredElement<HTMLInputElement>('login-username').value = username;
    } catch (submitError) {
      error.textContent = errorMessage(submitError);
      error.hidden = false;
    } finally {
      submit.disabled = false;
    }
  }

  private async login(): Promise<void> {
    const serverUrl = requiredElement<HTMLInputElement>('server-url').value.trim();
    const caCertPath = requiredElement<HTMLInputElement>('ca-cert-path').value.trim();
    const username = requiredElement<HTMLInputElement>('login-username').value.trim();
    const passwordInput = requiredElement<HTMLInputElement>('login-password');
    const loginButton = requiredElement<HTMLButtonElement>('login-btn');
    const loginError = requiredElement<HTMLElement>('login-error');
    loginButton.disabled = true;
    loginError.hidden = true;
    requiredElement<HTMLElement>('login-notice').hidden = true;
    try {
      const user = await remoteLogin(serverUrl, caCertPath, username, passwordInput.value);
      this.remoteUser = user;
      passwordInput.value = '';
      localStorage.setItem('remote-pacs.server-url', serverUrl);
      localStorage.setItem('remote-pacs.ca-cert-path', caCertPath);
      setText('current-user', user.display_name?.trim() || user.username);
      this.routerPanel.setAvailable(user.role === 'admin');
      this.lifecyclePanel.setAvailable(user.role === 'admin');
      // 角色相关入口不能等到打开检查后才刷新；首次登录成功时立即同步。
      this.updateUi();
      requiredElement<HTMLElement>('login-screen').hidden = true;
      requiredElement<HTMLElement>('app-shell').removeAttribute('aria-hidden');
      await this.loadUserWindowPresets();
      await this.initializeTransformTools();
      this.resizeViewport();
      this.queuePage.open();
    } catch (error) {
      loginError.textContent = errorMessage(error);
      loginError.hidden = false;
    } finally {
      loginButton.disabled = false;
    }
  }

  private closeAllPanes(): void {
    this.stopCine();
    this.stopAnnotationSync();
    if (this.viewerMode !== '2d' || this.mpr) this.leaveAdvancedViewModes();
    this.clearAdvancedWorkspaceState();
    for (const pane of this.panes) this.resetPaneState(pane, true);
    const first = this.panes[0];
    if (first) {
      for (const pane of this.panes.slice(1)) pane.element.remove();
      this.panes = [first];
      this.activePaneIndex = 0;
      this.viewerMode = '2d';
    }
    this.applyPaneLayout();
    this.updatePaneLabels();
    this.updateUi();
    this.render();
  }

  private async logout(): Promise<void> {
    try {
      await remoteLogout();
    } catch (error) {
      this.showError(errorMessage(error));
    } finally {
      this.queuePage.close();
      this.remoteUser = null;
      this.userWindowPresets = [];
      this.closeWindowPresetDialog();
      this.stopAnnotationSync();
      this.closeAllPanes();
      this.patients = [];
      this.hidePatientContext();
      this.studies.clear();
      this.series.clear();
      this.expandedPatientId = null;
      this.expandedStudyUid = null;
      this.transformSchema = null;
      this.tagEditorContext = null;
      this.transformPreview = null;
      this.observedCompletedTransformJobs.clear();
      this.closeTagEditor();
      this.closeTransformTasks();
      this.closeRevisionHistory();
      this.routerPanel.setAvailable(false);
      this.lifecyclePanel.setAvailable(false);
      requiredElement<HTMLButtonElement>('transform-tasks-btn').hidden = true;
      setText('worklist-status', '');
      this.renderPatients();
      requiredElement<HTMLElement>('login-screen').hidden = false;
      requiredElement<HTMLElement>('app-shell').setAttribute('aria-hidden', 'true');
    }
  }

  private setupRemoteProgress(): void {
    void import('@tauri-apps/api/event').then(({ listen }) =>
      listen<DownloadProgress>('remote-download-progress', ({ payload }) => {
        if (!this.remoteDownloadActive) return;
        setText('loading-text', `正在下载序列 ${payload.downloaded} / ${payload.total}`);
      }),
    );
    void import('@tauri-apps/api/event').then(({ listen }) =>
      listen<MprBuildProgress>('mpr-build-progress', ({ payload }) => {
        if (!this.mprBuildActive) return;
        setText('loading-text', `正在构建体数据 ${payload.completed} / ${payload.total}`);
      }),
    );
    void import('@tauri-apps/api/event').then(({ listen }) =>
      listen<TransferProgress>('transfer-progress', ({ payload }) => {
        if (!this.transferActive) return;
        const text = payload.phase === 'upload'
          ? `上传 ${formatBytes(payload.completed_bytes)} / ${formatBytes(payload.total_bytes)}`
          : `处理 ${payload.completed_files} / ${payload.total_files}`;
        setText('worklist-status', text);
      }),
    );
    void import('@tauri-apps/api/event').then(({ listen }) =>
      listen<AiSegmentationProgress>('ai-segmentation-progress', ({ payload }) => {
        if (!this.aiRunning) return;
        const suffix = payload.total > 1 ? ` ${payload.completed} / ${payload.total}` : '';
        const message = `${payload.message}${suffix}`;
        this.aiStatus = message;
        setText('loading-text', message);
        setText('ai-model-status', message);
      }),
    );
  }

  private async exportSelection(studyUid: string, seriesUid?: string): Promise<void> {
    if (this.transferActive) return;
    this.transferActive = true; this.transferKind = 'exports'; this.setWorklistBusy(true, '正在生成 ZIP...');
    try { const result = await exportFromPacs(studyUid, seriesUid); if (result) setText('worklist-status', 'ZIP 已保存'); }
    catch (error) { this.showError(errorMessage(error)); setText('worklist-status', errorMessage(error)); }
    finally { this.transferActive = false; this.transferKind = null; this.setWorklistBusy(false); }
  }

  private async loadPatients(): Promise<void> {
    if (!this.remoteUser || this.worklistBusy) return;
    this.setWorklistBusy(true, '正在加载病人...');
    try {
      const query = requiredElement<HTMLInputElement>('patient-query').value.trim();
      const offset = this.patientPage * PATIENT_PAGE_SIZE;
      const rows = await listPatients(query, PATIENT_PAGE_SIZE + 1, offset);
      this.hasNextPatientPage = rows.length > PATIENT_PAGE_SIZE;
      this.patients = rows.slice(0, PATIENT_PAGE_SIZE);
      this.studies.clear();
      this.series.clear();
      this.expandedPatientId = null;
      this.expandedStudyUid = null;
      this.renderPatients();
    } catch (error) {
      setText('worklist-status', errorMessage(error));
      this.showError(errorMessage(error));
    } finally {
      this.setWorklistBusy(false);
    }
  }

  private async togglePatient(patientId: number): Promise<void> {
    if (this.expandedPatientId === patientId) {
      this.expandedPatientId = null;
      this.expandedStudyUid = null;
      this.renderPatients();
      return;
    }
    this.expandedPatientId = patientId;
    this.expandedStudyUid = null;
    this.renderPatients();
    if (this.studies.has(patientId)) return;
    this.setWorklistBusy(true, '正在加载检查...');
    try {
      this.studies.set(patientId, await listPatientStudies(patientId));
      setText('worklist-status', '');
      this.renderPatients();
    } catch (error) {
      this.showError(errorMessage(error));
    } finally {
      this.setWorklistBusy(false);
    }
  }

  private async toggleStudy(studyUid: string): Promise<void> {
    if (this.expandedStudyUid === studyUid) {
      this.expandedStudyUid = null;
      this.renderActiveWorklist();
      return;
    }
    this.expandedStudyUid = studyUid;
    this.renderActiveWorklist();
    if (this.series.has(studyUid)) return;
    this.setWorklistBusy(true, '正在加载序列...');
    try {
      this.series.set(studyUid, await listStudySeries(studyUid));
      setText('worklist-status', '');
      this.renderActiveWorklist();
    } catch (error) {
      this.showError(errorMessage(error));
    } finally {
      this.setWorklistBusy(false);
    }
  }

  private async openRemote(studyUid: string, seriesUid: string): Promise<boolean> {
    try {
      await this.activateSeries(
        () => openRemoteSeries(studyUid, seriesUid),
        '正在准备远程序列...',
        true,
      );
      return true;
    } catch (error) {
      this.showError(errorMessage(error));
      return false;
    }
  }

  private async openQueueStudy(
    row: QueueStudyRow,
    seriesUid: string,
    series: RemoteSeriesSummary[],
  ): Promise<boolean> {
    const opened = await this.openRemote(row.study_uid, seriesUid);
    if (!opened) return false;

    this.patientContext = {
      key: row.patient_key,
      patientId: row.patient_id,
      name: row.patient_name,
    };
    this.expandedPatientId = row.patient_key;
    this.expandedStudyUid = row.study_uid;
    this.studies.delete(row.patient_key);
    this.series.set(row.study_uid, series);
    this.showPatientContext();
    this.renderPatientContext();
    await this.refreshPatientContext();
    return true;
  }

  private showPatientContext(): void {
    const workspace = requiredElement<HTMLElement>('workspace');
    const panel = requiredElement<HTMLElement>('worklist-panel');
    const resizer = requiredElement<HTMLElement>('worklist-resizer');
    panel.hidden = false;
    resizer.hidden = false;
    resizer.tabIndex = 0;
    workspace.classList.remove('worklist-hidden');
    setTimeout(() => this.resizeViewport(), 0);
  }

  private hidePatientContext(): void {
    this.patientContext = null;
    this.expandedPatientId = null;
    this.expandedStudyUid = null;
    const workspace = requiredElement<HTMLElement>('workspace');
    const panel = requiredElement<HTMLElement>('worklist-panel');
    const resizer = requiredElement<HTMLElement>('worklist-resizer');
    panel.hidden = true;
    resizer.hidden = true;
    resizer.tabIndex = -1;
    workspace.classList.add('worklist-hidden');
    requiredElement<HTMLElement>('patient-list').replaceChildren();
  }

  private async refreshPatientContext(): Promise<void> {
    const context = this.patientContext;
    if (!context || !this.remoteUser || this.worklistBusy) return;
    this.setWorklistBusy(true, '正在加载检查...');
    try {
      this.studies.set(context.key, await listPatientStudies(context.key));
      setText('worklist-status', '');
      this.renderPatientContext();
    } catch (error) {
      setText('worklist-status', errorMessage(error));
      this.showError(errorMessage(error));
      this.renderPatientContext();
    } finally {
      this.setWorklistBusy(false);
    }
  }

  private async editQueueStudyTags(row: QueueStudyRow): Promise<void> {
    if (!this.canEditDicomTags()) return;
    try {
      let studies = this.studies.get(row.patient_key);
      if (!studies) {
        studies = await listPatientStudies(row.patient_key);
        this.studies.set(row.patient_key, studies);
      }
      const study = studies.find((entry) => entry.study_uid === row.study_uid);
      if (!study) throw new Error('没有找到该检查的标签信息');
      await this.openTagEditor({
        targetType: 'study',
        targetKey: study.study_uid,
        scope: 'study',
        title: `${study.description?.trim() || '未命名检查'} · ${study.study_uid}`,
        values: {
          AccessionNumber: study.accession_number,
          StudyID: study.study_id,
          StudyDescription: study.description,
          ReferringPhysicianName: study.referring_physician,
        },
      });
    } catch (error) {
      this.showError(errorMessage(error));
    }
  }

  private canEditDicomTags(): boolean {
    return this.remoteUser?.role === 'admin' || this.remoteUser?.role === 'technician';
  }

  private canManageExamRequests(): boolean {
    return this.remoteUser?.role === 'admin' || this.remoteUser?.role === 'technician';
  }

  private async initializeTransformTools(): Promise<void> {
    const available = this.canEditDicomTags();
    requiredElement<HTMLButtonElement>('transform-tasks-btn').hidden = !available;
    if (!available) return;
    try {
      this.transformSchema = await getTransformSchema();
    } catch (error) {
      this.showError(errorMessage(error));
    }
  }

  private async openTagEditor(context: TagEditorContext): Promise<void> {
    if (!this.canEditDicomTags()) return;
    try {
      this.transformSchema ??= await getTransformSchema();
    } catch (error) {
      this.showError(errorMessage(error));
      return;
    }
    this.tagEditorContext = context;
    this.transformPreview = null;
    setText('tag-editor-title', `编辑${scopeLabel(context.scope)}标签`);
    setText('tag-editor-target', context.title);
    const fields = requiredElement<HTMLElement>('tag-editor-fields');
    fields.replaceChildren();
    for (const spec of this.transformSchema.manual_tags.filter((tag) => tag.scope === context.scope)) {
      const label = document.createElement('label');
      const caption = document.createElement('span');
      caption.textContent = TAG_LABELS[spec.keyword] ?? spec.keyword;
      const original = dicomInputValue(spec.keyword, context.values[spec.keyword]);
      let input: HTMLInputElement | HTMLSelectElement;
      if (spec.keyword === 'PatientSex') {
        const select = document.createElement('select');
        for (const [value, text] of [['', '未指定'], ['M', '男'], ['F', '女'], ['O', '其他']]) {
          const option = document.createElement('option');
          option.value = value;
          option.textContent = text;
          select.append(option);
        }
        input = select;
      } else {
        const text = document.createElement('input');
        text.type = spec.keyword === 'PatientBirthDate'
          ? 'date'
          : spec.keyword === 'SeriesNumber'
            ? 'number'
            : 'text';
        if (spec.keyword === 'SeriesNumber') text.step = '1';
        input = text;
      }
      input.dataset.tagKeyword = spec.keyword;
      input.dataset.original = original;
      input.value = editorInputValue(spec.keyword, original);
      input.addEventListener('input', () => this.invalidateTagPreview());
      label.append(caption, input);
      fields.append(label);
    }
    requiredElement<HTMLTextAreaElement>('tag-editor-reason').value = '';
    requiredElement<HTMLElement>('tag-editor-error').hidden = true;
    requiredElement<HTMLElement>('tag-preview').hidden = true;
    requiredElement<HTMLButtonElement>('tag-confirm-btn').hidden = true;
    requiredElement<HTMLButtonElement>('tag-preview-btn').hidden = false;
    if (!this.tagEditorDialog.open) this.tagEditorDialog.showModal();
  }

  private invalidateTagPreview(): void {
    this.transformPreview = null;
    requiredElement<HTMLElement>('tag-preview').hidden = true;
    requiredElement<HTMLButtonElement>('tag-confirm-btn').hidden = true;
    requiredElement<HTMLButtonElement>('tag-preview-btn').hidden = false;
  }

  private closeTagEditor(): void {
    this.tagEditorContext = null;
    this.transformPreview = null;
    if (this.tagEditorDialog.open) this.tagEditorDialog.close();
  }

  private collectTagRules(): TagRuleInput[] {
    const rules: TagRuleInput[] = [];
    for (const input of document.querySelectorAll<HTMLInputElement | HTMLSelectElement>(
      '#tag-editor-fields [data-tag-keyword]',
    )) {
      const keyword = input.dataset.tagKeyword;
      if (!keyword) continue;
      const original = input.dataset.original ?? '';
      const value = dicomInputValue(keyword, input.value);
      if (value === original) continue;
      rules.push(value === ''
        ? { tag: keyword, action: 'empty', recursive: false }
        : { tag: keyword, action: 'replace', value, recursive: false });
    }
    return rules;
  }

  private async previewTagEdit(): Promise<void> {
    const context = this.tagEditorContext;
    if (!context) return;
    const errorElement = requiredElement<HTMLElement>('tag-editor-error');
    errorElement.hidden = true;
    const rules = this.collectTagRules();
    const reason = requiredElement<HTMLTextAreaElement>('tag-editor-reason').value.trim();
    if (!rules.length) {
      errorElement.textContent = '没有发生标签变化';
      errorElement.hidden = false;
      return;
    }
    if (reason.length < 3) {
      errorElement.textContent = '变更原因至少 3 个字符';
      errorElement.hidden = false;
      return;
    }
    this.setTagEditorBusy(true);
    try {
      this.transformPreview = await previewClinicalTransform(
        context.targetType,
        context.targetKey,
        rules,
        reason,
      );
      this.renderTagPreview(this.transformPreview);
    } catch (error) {
      errorElement.textContent = errorMessage(error);
      errorElement.hidden = false;
    } finally {
      this.setTagEditorBusy(false);
    }
  }

  private renderTagPreview(response: TransformPreviewResponse): void {
    const preview = response.preview;
    const summary = requiredElement<HTMLElement>('tag-preview-summary');
    summary.replaceChildren();
    for (const text of [
      `${preview.affected_instances} 实例`,
      `${preview.affected_studies} 检查`,
      `${preview.affected_series} 序列`,
      `${preview.uid_remaps.instances} UID 重映射`,
    ]) {
      const item = document.createElement('span');
      item.textContent = text;
      summary.append(item);
    }
    const diffs = requiredElement<HTMLElement>('tag-preview-diffs');
    diffs.replaceChildren();
    for (const diff of preview.changes) {
      const row = document.createElement('div');
      row.className = 'tag-diff-row';
      const keyword = document.createElement('strong');
      keyword.textContent = TAG_LABELS[diff.keyword] ?? diff.keyword;
      const oldValue = document.createElement('span');
      oldValue.className = 'tag-diff-old';
      oldValue.textContent = displayTagValue(diff.old_value);
      const arrow = document.createElement('span');
      arrow.className = 'tag-diff-arrow';
      arrow.textContent = '→';
      const newValue = document.createElement('span');
      newValue.className = 'tag-diff-new';
      newValue.textContent = displayTagValue(diff.new_value);
      const count = document.createElement('span');
      count.className = 'tag-diff-count';
      count.textContent = `${diff.affected_instances} 项`;
      row.append(keyword, oldValue, arrow, newValue, count);
      diffs.append(row);
    }
    requiredElement<HTMLElement>('tag-preview').hidden = false;
    requiredElement<HTMLButtonElement>('tag-preview-btn').hidden = true;
    requiredElement<HTMLButtonElement>('tag-confirm-btn').hidden = false;
  }

  private setTagEditorBusy(busy: boolean): void {
    requiredElement<HTMLButtonElement>('tag-preview-btn').disabled = busy;
    requiredElement<HTMLButtonElement>('tag-confirm-btn').disabled = busy;
    requiredElement<HTMLButtonElement>('tag-editor-cancel').disabled = busy;
    requiredElement<HTMLButtonElement>('tag-editor-close').disabled = busy;
  }

  private async confirmTagEdit(): Promise<void> {
    const preview = this.transformPreview;
    if (!preview) return;
    const errorElement = requiredElement<HTMLElement>('tag-editor-error');
    this.setTagEditorBusy(true);
    try {
      await confirmTransform(preview.job_id, preview.confirmation_token);
      this.closeTagEditor();
      await this.openTransformTasks();
    } catch (error) {
      errorElement.textContent = errorMessage(error);
      errorElement.hidden = false;
    } finally {
      this.setTagEditorBusy(false);
    }
  }

  private async openTransformTasks(): Promise<void> {
    if (!this.canEditDicomTags()) return;
    if (!this.transformTasksDialog.open) this.transformTasksDialog.showModal();
    await this.loadTransformTasks();
  }

  private closeTransformTasks(): void {
    this.stopTransformTaskPolling();
    if (this.transformTasksDialog.open) this.transformTasksDialog.close();
  }

  private canViewDicomRevisions(): boolean {
    return ['admin', 'technician', 'radiologist'].includes(this.remoteUser?.role ?? '');
  }

  private async openRevisionHistory(): Promise<void> {
    if (!this.canViewDicomRevisions() || !this.remoteSeriesOpen || !this.state) return;
    const sopUid = this.currentFrame().sop_instance_uid;
    if (!sopUid) {
      this.showError('当前帧没有 SOP Instance UID');
      return;
    }
    this.revisionHistoryDialog.showModal();
    setText('revision-history-target', sopUid);
    const list = requiredElement<HTMLElement>('revision-history-list');
    list.replaceChildren(emptyWorklistMessage('正在读取修订历史...'));
    this.cancelRollback();
    try {
      this.revisions = await listInstanceRevisionsBySop(sopUid);
      this.renderRevisionHistory();
    } catch (error) {
      list.replaceChildren(emptyWorklistMessage(errorMessage(error)));
    }
  }

  private closeRevisionHistory(): void {
    this.revisions = [];
    this.cancelRollback();
    if (this.revisionHistoryDialog.open) this.revisionHistoryDialog.close();
  }

  private renderRevisionHistory(): void {
    const list = requiredElement<HTMLElement>('revision-history-list');
    list.replaceChildren();
    for (const revision of this.revisions) {
      const row = document.createElement('section');
      row.className = 'revision-row';
      const version = document.createElement('div');
      version.className = 'revision-version';
      const versionLabel = document.createElement('strong');
      versionLabel.textContent = `版本 ${revision.version_number}`;
      version.append(versionLabel);
      if (revision.is_current) {
        const current = document.createElement('small');
        current.textContent = '当前';
        version.append(current);
      }
      const detail = document.createElement('div');
      detail.className = 'revision-detail';
      const kind = document.createElement('span');
      kind.textContent = `${revisionKindLabel(revision.derivation_kind)} · ${revision.reason}`;
      const created = document.createElement('small');
      created.textContent = `${new Date(revision.created_at).toLocaleString('zh-CN')} · ${revision.file_sha256_hex.slice(0, 12)}`;
      detail.append(kind, created);
      row.append(version, detail);
      if (this.canEditDicomTags() && !revision.is_current) {
        const rollback = document.createElement('button');
        rollback.type = 'button';
        rollback.textContent = '回滚到此版本';
        rollback.addEventListener('click', () => this.selectRollbackRevision(revision));
        row.append(rollback);
      }
      list.append(row);
    }
    if (!this.revisions.length) list.append(emptyWorklistMessage('没有修订记录'));
  }

  private selectRollbackRevision(revision: DicomRevision): void {
    this.selectedRollbackRevision = revision;
    this.rollbackPreview = null;
    setText('rollback-target', `从版本 ${this.revisions.find((item) => item.is_current)?.version_number ?? '--'} 回滚到版本 ${revision.version_number}`);
    requiredElement<HTMLTextAreaElement>('rollback-reason').value = '';
    requiredElement<HTMLElement>('rollback-error').hidden = true;
    requiredElement<HTMLElement>('rollback-preview-summary').hidden = true;
    requiredElement<HTMLButtonElement>('rollback-preview-btn').hidden = false;
    requiredElement<HTMLButtonElement>('rollback-confirm-btn').hidden = true;
    requiredElement<HTMLElement>('rollback-panel').hidden = false;
  }

  private cancelRollback(): void {
    this.selectedRollbackRevision = null;
    this.rollbackPreview = null;
    requiredElement<HTMLElement>('rollback-panel').hidden = true;
  }

  private async previewSelectedRollback(): Promise<void> {
    const revision = this.selectedRollbackRevision;
    if (!revision) return;
    const reason = requiredElement<HTMLTextAreaElement>('rollback-reason').value.trim();
    const error = requiredElement<HTMLElement>('rollback-error');
    error.hidden = true;
    if (reason.length < 3) {
      error.textContent = '回滚原因至少 3 个字符';
      error.hidden = false;
      return;
    }
    this.setRollbackBusy(true);
    try {
      this.rollbackPreview = await previewRollback(
        revision.logical_instance_id,
        revision.id,
        reason,
      );
      const preview = this.rollbackPreview.preview;
      const summary = requiredElement<HTMLElement>('rollback-preview-summary');
      summary.replaceChildren();
      for (const text of [
        `${preview.affected_instances} 实例`,
        `${preview.affected_studies} 检查`,
        `${preview.affected_series} 序列`,
        `${preview.uid_remaps.instances} UID 重映射`,
      ]) {
        const item = document.createElement('span');
        item.textContent = text;
        summary.append(item);
      }
      summary.hidden = false;
      requiredElement<HTMLButtonElement>('rollback-preview-btn').hidden = true;
      requiredElement<HTMLButtonElement>('rollback-confirm-btn').hidden = false;
    } catch (cause) {
      error.textContent = errorMessage(cause);
      error.hidden = false;
    } finally {
      this.setRollbackBusy(false);
    }
  }

  private setRollbackBusy(busy: boolean): void {
    requiredElement<HTMLButtonElement>('rollback-preview-btn').disabled = busy;
    requiredElement<HTMLButtonElement>('rollback-confirm-btn').disabled = busy;
    requiredElement<HTMLButtonElement>('rollback-cancel').disabled = busy;
    requiredElement<HTMLButtonElement>('revision-history-close').disabled = busy;
  }

  private async confirmSelectedRollback(): Promise<void> {
    const preview = this.rollbackPreview;
    if (!preview) return;
    const error = requiredElement<HTMLElement>('rollback-error');
    this.setRollbackBusy(true);
    try {
      await confirmTransform(preview.job_id, preview.confirmation_token);
      this.closeRevisionHistory();
      await this.openTransformTasks();
    } catch (cause) {
      error.textContent = errorMessage(cause);
      error.hidden = false;
    } finally {
      this.setRollbackBusy(false);
    }
  }

  private stopTransformTaskPolling(): void {
    if (this.transformTaskTimer !== null) {
      window.clearTimeout(this.transformTaskTimer);
      this.transformTaskTimer = null;
    }
  }

  private async loadTransformTasks(): Promise<void> {
    this.stopTransformTaskPolling();
    const status = requiredElement<HTMLElement>('transform-tasks-status');
    status.textContent = '正在刷新';
    try {
      const jobs = await listTransformJobs();
      this.renderTransformTasks(jobs);
      const active = jobs.some((job) => job.status === 'queued' || job.status === 'running');
      const newlyCompleted = jobs.filter(
        (job) => job.status === 'succeeded' &&
          !this.observedCompletedTransformJobs.has(job.id),
      );
      for (const job of jobs) {
        if (job.status === 'succeeded') this.observedCompletedTransformJobs.add(job.id);
      }
      status.textContent = `${jobs.length} 项`;
      if (active && this.transformTasksDialog.open) {
        this.transformTaskTimer = window.setTimeout(() => void this.loadTransformTasks(), 1500);
      }
      if (newlyCompleted.length > 0) {
        this.studies.clear();
        this.series.clear();
        this.expandedStudyUid = null;
        this.queuePage.refresh();
      }
    } catch (error) {
      status.textContent = errorMessage(error);
    }
  }

  private renderTransformTasks(jobs: TransformJob[]): void {
    const container = requiredElement<HTMLElement>('transform-task-list');
    container.replaceChildren();
    if (!jobs.length) {
      container.append(emptyWorklistMessage('没有 DICOM 转换任务'));
      return;
    }
    for (const job of jobs) {
      const row = document.createElement('section');
      row.className = 'transform-task-row';
      row.dataset.status = job.status;
      const mode = document.createElement('strong');
      mode.textContent = transformModeLabel(job.mode);
      const detail = document.createElement('div');
      detail.className = 'transform-task-detail';
      const reason = document.createElement('span');
      reason.textContent = job.reason;
      const time = document.createElement('small');
      time.textContent = new Date(job.created_at).toLocaleString('zh-CN');
      detail.append(reason, time);
      if (job.error_message) {
        const failure = document.createElement('small');
        failure.textContent = job.error_message;
        detail.append(failure);
      }
      const state = document.createElement('div');
      state.className = 'transform-task-state';
      const stateLabel = document.createElement('span');
      stateLabel.textContent = transformStatusLabel(job.status);
      const progress = document.createElement('progress');
      progress.max = Math.max(1, job.progress_total);
      progress.value = Math.min(job.progress_completed, progress.max);
      state.append(stateLabel, progress);
      row.append(mode, detail, state);
      container.append(row);
    }
  }

  private appendTagEditButton(container: HTMLElement, context: TagEditorContext): void {
    if (!this.canEditDicomTags()) return;
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'worklist-edit-button';
    button.title = `编辑${scopeLabel(context.scope)}标签`;
    button.setAttribute('aria-label', button.title);
    button.innerHTML = '<i data-lucide="edit-3"></i>';
    button.addEventListener('click', (event) => {
      event.stopPropagation();
      void this.openTagEditor(context);
    });
    container.append(button);
  }

  private renderActiveWorklist(): void {
    if (this.patientContext) this.renderPatientContext();
    else this.renderPatients();
  }

  private renderPatientContext(): void {
    const container = requiredElement<HTMLElement>('patient-list');
    container.replaceChildren();
    const context = this.patientContext;
    if (!context) {
      setText('worklist-count', '未选择患者');
      return;
    }

    const studies = this.studies.get(context.key);
    const studyList = document.createElement('div');
    studyList.className = 'study-list patient-context-study-list';
    if (!studies) {
      studyList.append(emptyWorklistMessage('正在读取检查...'));
    } else if (!studies.length) {
      studyList.append(emptyWorklistMessage('没有可见检查'));
    } else {
      studyList.append(...studies.map((study) => this.renderStudyItem(study)));
    }
    container.append(studyList);
    const patientName = formatPersonName(context.name) || '未提供姓名';
    setText(
      'worklist-count',
      `${patientName} · ${context.patientId || '未提供 Patient ID'} · ${studies?.length ?? 0} 检查`,
    );
    createIcons({ icons: { Download, Edit3, Share2 } });
  }

  private renderStudyItem(study: StudySummary): HTMLElement {
    const studyItem = document.createElement('div');
    studyItem.className = 'study-item';
    const modality = study.modalities.join(' / ') || '未知模态';
    const studyTitle = study.description?.trim() || '未命名检查';
    const studyRow = worklistRow(
      studyTitle,
      `${formatApiDate(study.study_date) || '无日期'} · ${modality}`,
      `${study.series_count} 序列 · ${study.instance_count} 实例`,
      this.expandedStudyUid === study.study_uid,
    );
    studyRow.classList.add('study-row');
    studyRow.append(reportStatusBadge(
      studyReportStatusText(study.report_status),
      study.report_status,
    ));
    studyRow.addEventListener('click', () => void this.toggleStudy(study.study_uid));
    studyItem.append(studyRow);
    this.appendShareButton(studyItem, study.study_uid, studyTitle);
    this.appendExportButton(studyItem, study.study_uid);
    this.appendTagEditButton(studyItem, {
      targetType: 'study',
      targetKey: study.study_uid,
      scope: 'study',
      title: `${studyTitle} · ${study.study_uid}`,
      values: {
        AccessionNumber: study.accession_number,
        StudyID: study.study_id,
        StudyDescription: study.description,
        ReferringPhysicianName: study.referring_physician,
      },
    });

    if (this.expandedStudyUid !== study.study_uid) return studyItem;

    const series = this.series.get(study.study_uid);
    const seriesList = document.createElement('div');
    seriesList.className = 'series-list';
    if (!series) {
      seriesList.append(emptyWorklistMessage('正在读取序列...'));
    } else if (!series.length) {
      seriesList.append(emptyWorklistMessage('没有序列'));
    } else {
      const recommendedUid = recommendMprSeries(series)?.series_uid;
      for (const entry of series) {
        const seriesEntry = document.createElement('div');
        seriesEntry.className = 'series-entry';
        const seriesButton = document.createElement('button');
        seriesButton.type = 'button';
        seriesButton.className = 'series-row';
        const title = document.createElement('strong');
        title.textContent = entry.description?.trim() || `序列 ${entry.series_number ?? '--'}`;
        const detail = document.createElement('span');
        detail.textContent = `${entry.modality || '未知'} · ${entry.instance_count} 实例`;
        seriesButton.append(title, detail);
        if (entry.series_uid === recommendedUid) {
          seriesButton.classList.add('recommended');
          const recommendation = document.createElement('span');
          recommendation.className = 'series-recommendation';
          recommendation.textContent = 'MPR 推荐数据源';
          seriesButton.append(recommendation);
        }
        const seriesPayload: SeriesDragPayload = {
          studyUid: study.study_uid,
          seriesUid: entry.series_uid,
        };
        seriesButton.addEventListener('click', () => {
          if (Date.now() < this.seriesClickSuppressedUntil) return;
          void this.openRemote(study.study_uid, entry.series_uid);
        });
        seriesButton.addEventListener('pointerdown', (event) => {
          this.beginSeriesPointerDrag(seriesButton, event, seriesPayload);
        });
        seriesButton.addEventListener('pointermove', (event) => {
          this.moveSeriesPointerDrag(event);
        });
        seriesButton.addEventListener('pointerup', (event) => {
          this.finishSeriesPointerDrag(event, false);
        });
        seriesButton.addEventListener('pointercancel', (event) => {
          this.cancelSeriesPointerDrag(event);
        });
        seriesButton.addEventListener('lostpointercapture', (event) => {
          if (this.seriesPointerDrag?.pointerId === event.pointerId) {
            this.cancelSeriesPointerDrag(event);
          }
        });
        seriesEntry.append(seriesButton);
        this.appendExportButton(seriesEntry, study.study_uid, entry.series_uid);
        this.appendTagEditButton(seriesEntry, {
          targetType: 'series',
          targetKey: entry.series_uid,
          scope: 'series',
          title: `${entry.description?.trim() || `序列 ${entry.series_number ?? '--'}`} · ${entry.series_uid}`,
          values: {
            SeriesDescription: entry.description,
            SeriesNumber: entry.series_number,
            BodyPartExamined: entry.body_part_examined,
            ProtocolName: entry.protocol_name,
          },
        });
        seriesList.append(seriesEntry);
      }
    }
    studyItem.append(seriesList);
    return studyItem;
  }

  private renderPatients(): void {
    const container = requiredElement<HTMLElement>('patient-list');
    container.replaceChildren();
    for (const patient of this.patients) {
      const item = document.createElement('section');
      item.className = 'patient-item';
      const row = worklistRow(
        formatPersonName(patient.name) || '未提供姓名',
        patient.patient_id || '未提供 Patient ID',
        `${formatApiDate(patient.latest_study_date) || '无日期'} · ${patient.study_count} 检查 · ${patient.instance_count} 实例`,
        this.expandedPatientId === patient.id,
      );
      row.classList.add('patient-row');
      const patientStatus = patientReportStatus(patient);
      row.append(reportStatusBadge(patientStatus.text, patientStatus.key));
      row.addEventListener('click', () => void this.togglePatient(patient.id));
      item.append(row);
      this.appendTagEditButton(item, {
        targetType: 'patient',
        targetKey: String(patient.id),
        scope: 'patient',
        title: `${formatPersonName(patient.name) || '未提供姓名'} · ${patient.patient_id}`,
        values: {
          PatientName: patient.name,
          PatientID: patient.patient_id,
          IssuerOfPatientID: patient.issuer_of_patient_id,
          PatientBirthDate: patient.birth_date,
          PatientSex: patient.sex,
        },
      });
      if (this.expandedPatientId === patient.id) {
        const studies = this.studies.get(patient.id);
        const studyList = document.createElement('div');
        studyList.className = 'study-list';
        if (!studies) {
          studyList.append(emptyWorklistMessage('正在读取检查...'));
        } else if (!studies.length) {
          studyList.append(emptyWorklistMessage('没有检查'));
        } else {
          studyList.append(...studies.map((study) => this.renderStudyItem(study)));
        }
        item.append(studyList);
      }
      container.append(item);
    }
    if (!this.patients.length && !this.worklistBusy) {
      container.append(emptyWorklistMessage('没有匹配的病人'));
    }
    setText('worklist-count', `本页 ${this.patients.length} 位`);
    setText('patient-page', `第 ${this.patientPage + 1} 页`);
    requiredElement<HTMLButtonElement>('patients-previous').disabled = this.patientPage === 0;
    requiredElement<HTMLButtonElement>('patients-next').disabled = !this.hasNextPatientPage;
    createIcons({ icons: { Download, Edit3, Share2 } });
  }

  private appendShareButton(container: HTMLElement, studyUid: string, title: string): void {
    if (this.remoteUser?.role !== 'admin') return;
    const button = document.createElement('button'); button.type = 'button'; button.className = 'worklist-share-button';
    button.title = '分享此检查'; button.setAttribute('aria-label', button.title);
    button.innerHTML = '<i data-lucide="share-2"></i><span>分享</span>';
    button.addEventListener('click', (event) => {
      event.stopPropagation();
      void this.openStudyShare(studyUid, title);
    });
    container.append(button);
  }

  private async openStudyShare(studyUid: string, title: string): Promise<void> {
    this.shareStudyUid = studyUid;
    setText('study-share-description', title);
    requiredElement<HTMLElement>('study-share-result').hidden = true;
    requiredElement<HTMLElement>('study-share-error').hidden = true;
    const dialog = requiredElement<HTMLDialogElement>('study-share-dialog');
    if (!dialog.open) dialog.showModal();
    const select = requiredElement<HTMLSelectElement>('study-share-destination');
    const submit = requiredElement<HTMLButtonElement>('study-share-submit');
    const empty = requiredElement<HTMLElement>('study-share-empty');
    select.disabled = true;
    submit.disabled = true;
    select.replaceChildren();
    try {
      const destinations = (await listRouteDestinations()).filter(
        (entry) => entry.approval_status === 'approved' && entry.enabled,
      );
      select.replaceChildren(...destinations.map((entry) => {
        const option = document.createElement('option');
        option.value = entry.id;
        option.textContent = `${entry.name} · ${entry.status === 'online' ? '在线' : entry.status === 'offline' ? '离线' : '未检测'}`;
        return option;
      }));
      empty.hidden = destinations.length > 0;
      select.disabled = destinations.length === 0;
      submit.disabled = destinations.length === 0;
    } catch (error) {
      const node = requiredElement<HTMLElement>('study-share-error');
      node.textContent = errorMessage(error);
      node.hidden = false;
      empty.hidden = true;
    }
  }

  private async submitStudyShare(): Promise<void> {
    if (!this.shareStudyUid) return;
    const select = requiredElement<HTMLSelectElement>('study-share-destination');
    const submit = requiredElement<HTMLButtonElement>('study-share-submit');
    const error = requiredElement<HTMLElement>('study-share-error');
    const result = requiredElement<HTMLElement>('study-share-result');
    submit.disabled = true;
    error.hidden = true;
    result.hidden = true;
    try {
      const response = await sendRouteScope(select.value, this.shareStudyUid);
      result.textContent = `已发送到所选站点：${response.queued} 个实例进入队列，${response.skipped_as_duplicate} 个已发送实例被跳过`;
      result.hidden = false;
    } catch (reason) {
      error.textContent = errorMessage(reason);
      error.hidden = false;
    } finally {
      submit.disabled = false;
    }
  }

  private closeStudyShare(): void {
    this.shareStudyUid = null;
    const dialog = requiredElement<HTMLDialogElement>('study-share-dialog');
    if (dialog.open) dialog.close();
  }

  private appendExportButton(container: HTMLElement, studyUid: string, seriesUid?: string): void {
    const button = document.createElement('button'); button.type = 'button'; button.className = 'worklist-export-button';
    button.title = seriesUid ? '导出序列 ZIP' : '导出检查 ZIP'; button.setAttribute('aria-label', button.title);
    button.innerHTML = `<i data-lucide="download"></i><span>${seriesUid ? '导出序列' : '导出检查'}</span>`;
    button.addEventListener('click', (event) => { event.stopPropagation(); void this.exportSelection(studyUid, seriesUid); });
    container.append(button);
  }

  private setWorklistBusy(busy: boolean, message = ''): void {
    this.worklistBusy = busy;
    if (busy) setText('worklist-status', message);
    requiredElement<HTMLButtonElement>('refresh-worklist').disabled = busy;
    requiredElement<HTMLInputElement>('patient-query').disabled = busy;
    requiredElement<HTMLButtonElement>('refresh-worklist').classList.toggle('spinning', busy);
    requiredElement<HTMLButtonElement>('cancel-transfer').hidden = !this.transferActive;
  }

  private mprPointerDown(
    plane: MprPlane,
    canvas: HTMLCanvasElement,
    event: PointerEvent,
  ): void {
    if (!this.state || !this.mpr || this.viewerMode !== 'mpr' || this.busy) return;
    this.mpr.activePlane = plane;
    const point = eventPointFor(canvas, event);
    const viewport = this.mpr.viewports[plane];
    canvas.setPointerCapture(event.pointerId);
    if (event.button === 0 && this.mprObliqueMode && plane !== 'oblique' && this.crosshairLineHit(plane, point)) {
      event.preventDefault();
      const cross = this.rotatedCrosshairScreen(plane)!;
      this.mprDrag = {
        kind: 'oblique-rotate',
        plane,
        pointerId: event.pointerId,
        center: cross.center,
        lastAngle: Math.atan2(point.y - cross.center.y, point.x - cross.center.x),
      };
      requiredPlaneElement(plane).classList.remove('oblique-cursor-rotate');
      requiredPlaneElement(plane).classList.add('oblique-cursor-rotating');
    } else if (event.button === 1 || this.state.tool === 'pan') {
      event.preventDefault();
      this.mprDrag = {
        kind: 'pan',
        plane,
        pointerId: event.pointerId,
        start: point,
        panX: viewport.panX,
        panY: viewport.panY,
      };
    } else if (this.state.tool === 'window') {
      this.mprDrag = {
        kind: 'window',
        plane,
        pointerId: event.pointerId,
        start: point,
        center: this.state.windowCenter,
        width: this.state.windowWidth,
      };
    } else if (isMaskTool(this.state.tool)) {
      if (plane === 'oblique' || this.mprObliqueMode) {
        this.updateUi();
        return;
      }
      const segment = this.segmentationSegment;
      const volume = this.selectedMaskVolume();
      if (!segment || !volume) return;
      const frame = this.mprFrame(plane);
      const imagePoint = clampToImage(
        this.mprRenderers[plane].toImageFor(point, frame, viewport),
        imageGeometry(frame),
      );
      const before: MaskSliceSnapshot = new Map();
      const value = this.state.tool === 'mask_eraser' ? 0 : 1;
      const changedSlices = paintMaskVolumePlane(
        volume,
        this.mpr.metadata,
        plane,
        viewport.sliceIndex,
        imagePoint,
        imagePoint,
        this.maskBrushRadius,
        value,
        before,
      );
      this.mprDrag = {
        kind: 'mask-paint',
        plane,
        pointerId: event.pointerId,
        segmentId: segment.id,
        previous: imagePoint,
        before,
        changedSlices,
        value,
      };
    } else if (this.state.tool === 'crosshair') {
      this.mprDrag = { kind: 'crosshair', plane, pointerId: event.pointerId };
      this.moveMprCrosshairFromScreen(plane, point);
    } else {
      const hit = this.mprHitTest(plane, point);
      if (hit) {
        const key = this.mprMeasurementKey(plane);
        const frame = this.mprFrame(plane);
        const imagePoint = clampToImage(
          this.mprRenderers[plane].toImageFor(point, frame, viewport),
          imageGeometry(frame),
        );
        this.selectedMeasurementId = hit.annotation.id;
        this.mprDrag = {
          kind: 'annotation-edit',
          plane,
          pointerId: event.pointerId,
          key,
          annotationId: hit.annotation.id,
          handle: hit.handle,
          startImage: imagePoint,
          original: structuredClone(hit.annotation),
          before: cloneAnnotations(this.currentMprMeasurements(plane)),
        };
      } else {
        const frame = this.mprFrame(plane);
        const imagePoint = clampToImage(
          this.mprRenderers[plane].toImageFor(point, frame, viewport),
          imageGeometry(frame),
        );
        const tool = this.state.tool as AnnotationKind;
        if (
          tool === 'angle' && this.angleAwaitingEnd && this.mprDraft?.plane === plane &&
          this.mprDraft.measurement.kind === 'angle'
        ) {
          this.mprDraft.measurement.end = imagePoint;
        } else {
          this.mprDraft = { plane, measurement: createAnnotation(tool, imagePoint, makeId()) };
          this.angleAwaitingEnd = false;
        }
        this.selectedMeasurementId = null;
        this.mprDrag = { kind: 'annotation-create', plane, pointerId: event.pointerId };
      }
    }
    this.updateUi();
    this.render();
  }

  private mprPointerMove(
    plane: MprPlane,
    canvas: HTMLCanvasElement,
    event: PointerEvent,
  ): void {
    if (!this.state || !this.mpr || this.viewerMode !== 'mpr') return;
    const point = eventPointFor(canvas, event);
    this.updateMprRotateCursor(plane, point);
    if (
      !this.mprDrag ||
      this.mprDrag.plane !== plane ||
      this.mprDrag.pointerId !== event.pointerId
    ) {
      return;
    }
    const viewport = this.mpr.viewports[plane];
    if (this.mprDrag.kind === 'pan') {
      viewport.panX = this.mprDrag.panX + point.x - this.mprDrag.start.x;
      viewport.panY = this.mprDrag.panY + point.y - this.mprDrag.start.y;
      this.renderMprPlane(plane);
    } else if (this.mprDrag.kind === 'window') {
      const sensitivity = Math.max(1, this.mprDrag.width / 512);
      this.state.windowCenter = this.mprDrag.center + (point.x - this.mprDrag.start.x) * sensitivity;
      this.state.windowWidth = Math.max(
        1,
        this.mprDrag.width + (point.y - this.mprDrag.start.y) * sensitivity * 2,
      );
      this.state.voiFunction = 'LINEAR';
      setText(
        'window-readout',
        `WL ${this.state.windowCenter.toFixed(0)}  WW ${this.state.windowWidth.toFixed(0)}`,
      );
      this.scheduleMprRefresh();
    } else if (this.mprDrag.kind === 'oblique-rotate') {
      const drag = this.mprDrag;
      const angle = Math.atan2(point.y - drag.center.y, point.x - drag.center.x);
      const deltaAngle = angle - drag.lastAngle;
      drag.lastAngle = angle;
      if (Math.abs(deltaAngle) > 0.002) {
        // 屏幕坐标 y 轴向下，atan2 的正方向是视觉顺时针；
        // 患者空间的右手旋转需要取反，十字线才能跟着光标走。
        this.rotateOblique(-deltaAngle);
      }
    } else if (this.mprDrag.kind === 'crosshair') {
      this.moveMprCrosshairFromScreen(plane, point);
    } else if (this.mprDrag.kind === 'mask-paint') {
      const drag = this.mprDrag;
      const volume = this.maskVolumes.get(drag.segmentId);
      if (!volume) return;
      const frame = this.mprFrame(plane);
      const imagePoint = clampToImage(
        this.mprRenderers[plane].toImageFor(point, frame, viewport),
        imageGeometry(frame),
      );
      const changed = paintMaskVolumePlane(
        volume,
        this.mpr.metadata,
        plane,
        viewport.sliceIndex,
        drag.previous,
        imagePoint,
        this.maskBrushRadius,
        drag.value,
        drag.before,
      );
      for (const slice of changed) drag.changedSlices.add(slice);
      drag.previous = imagePoint;
      this.render();
    } else if (this.mprDrag.kind === 'annotation-edit') {
      const drag = this.mprDrag;
      const list = this.currentMprMeasurements(plane);
      const annotation = list.find((candidate) => candidate.id === drag.annotationId);
      if (!annotation) return;
      const frame = this.mprFrame(plane);
      const imagePoint = clampToImage(
        this.mprRenderers[plane].toImageFor(point, frame, viewport),
        imageGeometry(frame),
      );
      this.updateEditedAnnotation(annotation, drag, imagePoint, frame);
      this.renderMprPlane(plane);
    } else if (this.mprDraft?.plane === plane) {
      const frame = this.mprFrame(plane);
      const imagePoint = clampToImage(
        this.mprRenderers[plane].toImageFor(point, frame, viewport),
        imageGeometry(frame),
      );
      this.updateDraft(this.mprDraft.measurement, imagePoint);
      this.renderMprPlane(plane);
    }
  }

  private mprPointerUp(
    plane: MprPlane,
    canvas: HTMLCanvasElement,
    event: PointerEvent,
  ): void {
    if (!this.mpr || !this.mprDrag || this.mprDrag.pointerId !== event.pointerId) return;
    if (this.mprDrag.kind === 'mask-paint') {
      const drag = this.mprDrag;
      this.recordMaskChange(drag.segmentId, drag.before, drag.changedSlices);
    } else if (this.mprDrag.kind === 'annotation-create' && this.mprDraft?.plane === plane) {
      const frame = this.mprFrame(plane);
      const viewport = this.mpr.viewports[plane];
      const draft = this.mprDraft.measurement;
      const extent = this.annotationScreenExtent(draft, (value) =>
        this.mprRenderers[plane].toScreenFor(value, frame, viewport));
      if (draft.kind === 'angle' && !this.angleAwaitingEnd && extent >= 4) {
        this.angleAwaitingEnd = true;
      } else if (extent >= 4 || draft.kind === 'point_probe') {
        const list = this.currentMprMeasurements(plane);
        const before = cloneAnnotations(list);
        list.push(draft);
        const key = this.mprMeasurementKey(plane);
        this.mprMeasurements.set(key, list);
        this.recordAnnotationChange(key, before, list);
        this.selectedMeasurementId = draft.id;
        this.angleAwaitingEnd = false;
        void this.refreshAnnotationStatistics(draft, plane);
        this.mprDraft = null;
      } else {
        this.mprDraft = null;
        this.angleAwaitingEnd = false;
      }
    } else if (this.mprDrag.kind === 'annotation-edit') {
      const drag = this.mprDrag;
      const list = this.currentMprMeasurements(plane);
      this.recordAnnotationChange(drag.key, drag.before, list);
      const annotation = list.find((candidate) => candidate.id === drag.annotationId);
      if (annotation) void this.refreshAnnotationStatistics(annotation, plane);
    }
    this.mprDrag = null;
    if (canvas.hasPointerCapture(event.pointerId)) canvas.releasePointerCapture(event.pointerId);
    this.updateMprRotateCursor(plane, eventPointFor(canvas, event));
    this.render();
    this.updateUi();
  }

  private updateMprRotateCursor(plane: MprPlane, point: Point): void {
    const pane = requiredPlaneElement(plane);
    const hovering = this.mprObliqueMode && this.crosshairLineHit(plane, point);
    const rotating = this.mprDrag?.kind === 'oblique-rotate' && this.mprDrag.plane === plane;
    pane.classList.toggle('oblique-cursor-rotate', hovering && !rotating);
    pane.classList.toggle('oblique-cursor-rotating', rotating);
  }

  private clearMprRotateCursor(plane: MprPlane): void {
    const pane = requiredPlaneElement(plane);
    pane.classList.remove('oblique-cursor-rotate');
    pane.classList.remove('oblique-cursor-rotating');
  }

  private clearMprRotateCursors(): void {
    for (const plane of MPR_PLANES) this.clearMprRotateCursor(plane);
  }

  private mprWheel(plane: MprPlane, canvas: HTMLCanvasElement, event: WheelEvent): void {
    if (!this.mpr || this.viewerMode !== 'mpr') return;
    event.preventDefault();
    this.mpr.activePlane = plane;
    const viewport = this.mpr.viewports[plane];
    if (!event.ctrlKey) {
      const pageSize = Math.max(canvas.clientWidth, canvas.clientHeight, 1);
      const deltaX = normalizeWheelDelta(event.deltaX, event.deltaMode, pageSize);
      const deltaY = normalizeWheelDelta(event.deltaY, event.deltaMode, pageSize);
      const movement = consumeMprWheel(this.mprWheelDelta[plane], deltaX, deltaY, event.shiftKey);
      if (movement.x === 0 && movement.y === 0) return;
      if (this.mprObliqueMode) {
        const metadata = this.mprPlaneMetadata(plane);
        let crosshair = this.mpr.crosshair;
        if (movement.y !== 0) {
          crosshair = addPatientVector(
            crosshair,
            metadata.normal,
            movement.y * metadata.slice_spacing_mm,
          );
        }
        if (movement.x !== 0) {
          crosshair = addPatientVector(
            crosshair,
            metadata.x_axis,
            movement.x * (metadata.spacing_x_mm ?? metadata.pixel_spacing_mm),
          );
        }
        this.setMprCrosshair(crosshair);
        return;
      }
      const metadata = requirePlane(this.mpr.metadata, plane);
      let crosshair = this.mpr.crosshair;
      crosshair = addPatientVector(
        crosshair,
        metadata.x_axis,
        movement.x * metadata.pixel_spacing_mm,
      );
      crosshair = addPatientVector(
        crosshair,
        metadata.y_axis,
        movement.y * metadata.pixel_spacing_mm,
      );
      this.setMprCrosshair(crosshair);
      return;
    }
    this.mprWheelDelta[plane] = { x: 0, y: 0 };
    const frame = this.mprFrame(plane);
    const factor = Math.exp(-event.deltaY * 0.0015);
    const nextZoom = Math.max(0.1, Math.min(10, viewport.zoom * factor));
    const next = zoomAt(
      viewport,
      eventPointFor(canvas, event),
      nextZoom,
      this.mprRenderers[plane].getViewport(),
      imageGeometry(frame),
    );
    viewport.zoom = next.zoom;
    viewport.panX = next.panX;
    viewport.panY = next.panY;
    setText('zoom-readout', `${Math.round(viewport.zoom * 100)}%`);
    this.updateMprLayout();
    this.renderMprPlane(plane);
  }

  private moveMprCrosshairFromScreen(plane: MprPlane, screenPoint: Point): void {
    if (!this.mpr) return;
    const frame = this.mprFrame(plane);
    const viewport = this.mpr.viewports[plane];
    const imagePoint = clampToImage(
      this.mprRenderers[plane].toImageFor(screenPoint, frame, viewport),
      imageGeometry(frame),
    );
    this.setMprCrosshair(this.mprImagePointToPatient(plane, imagePoint));
  }

  private mprHitTest(plane: MprPlane, screenPoint: Point): AnnotationHit | null {
    if (!this.mpr) return null;
    const renderer = this.mprRenderers[plane];
    const frame = this.mprFrame(plane);
    const viewport = this.mpr.viewports[plane];
    for (const annotation of [...this.currentMprMeasurements(plane)].reverse()) {
      const hit = annotationHitTest(
        annotation,
        screenPoint,
        (value) => renderer.toScreenFor(value, frame, viewport),
      );
      if (hit) return { annotation, handle: hit.handle };
    }
    return null;
  }

  private pointerDown(event: PointerEvent): void {
    if (!this.state || this.busy) return;
    const point = this.eventPoint(event);
    const canvas = event.currentTarget instanceof HTMLCanvasElement
      ? event.currentTarget
      : this.overlayCanvas;
    canvas.setPointerCapture(event.pointerId);
    if (event.button === 1 || this.state.tool === 'pan') {
      this.drag = {
        kind: 'pan',
        pointerId: event.pointerId,
        start: point,
        panX: this.state.panX,
        panY: this.state.panY,
      };
      return;
    }
    if (this.state.tool === 'window') {
      if (this.currentFrame().pixel_format === 'rgb8') return;
      this.drag = {
        kind: 'window',
        pointerId: event.pointerId,
        start: point,
        center: this.state.windowCenter,
        width: this.state.windowWidth,
      };
      return;
    }

    if (isMaskTool(this.state.tool)) {
      const segment = this.segmentationSegment;
      const volume = this.selectedMaskVolume();
      if (!segment || !volume) return;
      const imagePoint = clampToImage(
        this.renderer.toImage(point, this.state),
        imageGeometry(this.currentFrame()),
      );
      const before: MaskSliceSnapshot = new Map();
      const sourceSlice = this.currentMaskSourceIndex();
      const value = this.state.tool === 'mask_eraser' ? 0 : 1;
      const changedSlices = paintMaskSourcePlane(
        volume,
        sourceSlice,
        imagePoint,
        imagePoint,
        this.maskBrushRadius,
        {
          rowMm: this.currentFrame().spacing.row_mm,
          colMm: this.currentFrame().spacing.col_mm,
          sliceMm: this.mpr?.metadata.source_spacing_mm[2] ?? null,
        },
        value,
        before,
      );
      this.drag = {
        kind: 'mask-paint',
        pointerId: event.pointerId,
        segmentId: segment.id,
        sourceSlice,
        previous: imagePoint,
        before,
        changedSlices,
        value,
      };
      this.render();
      return;
    }

    const hit = this.hitTest(point);
    if (hit) {
      const imagePoint = clampToImage(
        this.renderer.toImage(point, this.state),
        imageGeometry(this.currentFrame()),
      );
      const key = this.currentFrame().frame_key;
      this.selectedMeasurementId = hit.annotation.id;
      this.drag = {
        kind: 'annotation-edit',
        pointerId: event.pointerId,
        key,
        annotationId: hit.annotation.id,
        handle: hit.handle,
        startImage: imagePoint,
        original: structuredClone(hit.annotation),
        before: cloneAnnotations(this.currentMeasurements()),
      };
      this.render();
      return;
    }
    const imagePoint = clampToImage(this.renderer.toImage(point, this.state), imageGeometry(this.currentFrame()));
    const tool = this.state.tool as AnnotationKind;
    if (tool === 'angle' && this.angleAwaitingEnd && this.draft?.kind === 'angle') {
      this.draft.end = imagePoint;
    } else {
      this.draft = createAnnotation(tool, imagePoint, makeId());
      this.angleAwaitingEnd = false;
    }
    this.selectedMeasurementId = null;
    this.drag = { kind: 'annotation-create', pointerId: event.pointerId };
    this.render();
  }

  private pointerMove(event: PointerEvent): void {
    if (!this.state || !this.drag || this.drag.pointerId !== event.pointerId) return;
    const point = this.eventPoint(event);
    if (this.drag.kind === 'pan') {
      this.state.panX = this.drag.panX + point.x - this.drag.start.x;
      this.state.panY = this.drag.panY + point.y - this.drag.start.y;
      this.render();
      return;
    }
    if (this.drag.kind === 'window') {
      const sensitivity = Math.max(1, this.drag.width / 512);
      this.state.windowCenter = this.drag.center + (point.x - this.drag.start.x) * sensitivity;
      this.state.windowWidth = Math.max(
        1,
        this.drag.width + (point.y - this.drag.start.y) * sensitivity * 2,
      );
      this.state.voiFunction = 'LINEAR';
      this.updateUi();
      this.scheduleLutRefresh();
      return;
    }
    if (this.drag.kind === 'mask-paint') {
      const drag = this.drag;
      const volume = this.maskVolumes.get(drag.segmentId);
      if (!volume) return;
      const imagePoint = clampToImage(
        this.renderer.toImage(point, this.state),
        imageGeometry(this.currentFrame()),
      );
      const changed = paintMaskSourcePlane(
        volume,
        drag.sourceSlice,
        drag.previous,
        imagePoint,
        this.maskBrushRadius,
        {
          rowMm: this.currentFrame().spacing.row_mm,
          colMm: this.currentFrame().spacing.col_mm,
          sliceMm: this.mpr?.metadata.source_spacing_mm[2] ?? null,
        },
        drag.value,
        drag.before,
      );
      for (const slice of changed) drag.changedSlices.add(slice);
      drag.previous = imagePoint;
      this.render();
      return;
    }
    if (this.drag.kind === 'annotation-edit') {
      const drag = this.drag;
      const annotation = this.currentMeasurements().find(
        (candidate) => candidate.id === drag.annotationId,
      );
      if (!annotation) return;
      const imagePoint = clampToImage(
        this.renderer.toImage(point, this.state),
        imageGeometry(this.currentFrame()),
      );
      this.updateEditedAnnotation(annotation, drag, imagePoint, this.currentFrame());
      this.render();
    } else if (this.draft) {
      const imagePoint = clampToImage(
        this.renderer.toImage(point, this.state),
        imageGeometry(this.currentFrame()),
      );
      this.updateDraft(this.draft, imagePoint);
      this.render();
    }
  }

  private pointerUp(event: PointerEvent): void {
    if (!this.state || !this.drag || this.drag.pointerId !== event.pointerId) return;
    if (this.drag.kind === 'mask-paint') {
      const drag = this.drag;
      this.recordMaskChange(drag.segmentId, drag.before, drag.changedSlices);
    } else if (this.drag.kind === 'annotation-create' && this.draft) {
      const draft = this.draft;
      const extent = this.annotationScreenExtent(draft, (value) => this.renderer.toScreen(value, this.state!));
      if (draft.kind === 'angle' && !this.angleAwaitingEnd && extent >= 4) {
        this.angleAwaitingEnd = true;
      } else if (extent >= 4 || draft.kind === 'point_probe') {
        const list = this.currentMeasurements();
        const before = cloneAnnotations(list);
        list.push(draft);
        const key = this.currentFrame().frame_key;
        this.measurements.set(key, list);
        this.recordAnnotationChange(key, before, list);
        this.selectedMeasurementId = draft.id;
        this.angleAwaitingEnd = false;
        void this.refreshAnnotationStatistics(draft);
        this.draft = null;
      } else {
        this.draft = null;
        this.angleAwaitingEnd = false;
      }
    } else if (this.drag.kind === 'annotation-edit') {
      const drag = this.drag;
      const list = this.currentMeasurements();
      this.recordAnnotationChange(drag.key, drag.before, list);
      const annotation = list.find((candidate) => candidate.id === drag.annotationId);
      if (annotation) void this.refreshAnnotationStatistics(annotation);
    }
    this.drag = null;
    const canvas = event.currentTarget instanceof HTMLCanvasElement
      ? event.currentTarget
      : this.overlayCanvas;
    if (canvas.hasPointerCapture(event.pointerId)) {
      canvas.releasePointerCapture(event.pointerId);
    }
    this.render();
    this.updateUi();
  }

  private wheel(event: WheelEvent): void {
    if (!this.state) return;
    event.preventDefault();
    if (!event.ctrlKey) {
      this.wheelFrameDelta += event.deltaY;
      if (Math.abs(this.wheelFrameDelta) >= 30) {
        const direction = this.wheelFrameDelta > 0 ? 1 : -1;
        this.wheelFrameDelta = 0;
        void this.setFrame(this.state.currentFrame + direction);
      }
      return;
    }
    this.wheelFrameDelta = 0;
    const current = this.state;
    const factor = Math.exp(-event.deltaY * 0.0015);
    const nextZoom = Math.max(0.1, Math.min(10, current.zoom * factor));
    const next = zoomAt(
      current,
      this.eventPoint(event),
      nextZoom,
      this.renderer.getViewport(),
      imageGeometry(this.currentFrame()),
    );
    current.zoom = next.zoom;
    current.panX = next.panX;
    current.panY = next.panY;
    this.render();
    this.updateUi();
  }

  private keyDown(event: KeyboardEvent): void {
    const target = event.target as HTMLElement | null;
    if (target?.matches('input, select, textarea')) return;
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'o') {
      event.preventDefault();
      void this.openFiles();
    } else if (event.key === 'ArrowUp' || event.key === 'ArrowLeft') {
      event.preventDefault();
      if (this.mpr && this.viewerMode === 'mpr') {
        const plane = this.mpr.activePlane;
        this.changeMprSlice(plane, this.mpr.viewports[plane].sliceIndex - 1);
      } else if (this.state) void this.setFrame(this.state.currentFrame - 1);
    } else if (event.key === 'ArrowDown' || event.key === 'ArrowRight') {
      event.preventDefault();
      if (this.mpr && this.viewerMode === 'mpr') {
        const plane = this.mpr.activePlane;
        this.changeMprSlice(plane, this.mpr.viewports[plane].sliceIndex + 1);
      } else if (this.state) void this.setFrame(this.state.currentFrame + 1);
    } else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'z') {
      event.preventDefault();
      event.shiftKey ? this.redoAnnotation() : this.undoAnnotation();
    } else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'y') {
      event.preventDefault();
      this.redoAnnotation();
    } else if ((event.key === 'Delete' || event.key === 'Backspace') && this.selectedMeasurementId) {
      event.preventDefault();
      this.deleteSelectedAnnotation();
    } else if (event.key.toLowerCase() === 'v') {
      this.toggleAnnotationVisibility();
    } else if (event.key.toLowerCase() === 'i') {
      this.toggleInvert();
    } else if (event.key === ' ') {
      event.preventDefault();
      this.toggleCine();
    } else if (event.key === 'Escape') {
      this.draft = null;
      this.mprDraft = null;
      this.selectedMeasurementId = null;
      this.angleAwaitingEnd = false;
      this.render();
    }
  }

  private hitTest(screenPoint: Point): AnnotationHit | null {
    if (!this.state) return null;
    for (const annotation of [...this.currentMeasurements()].reverse()) {
      const hit = annotationHitTest(
        annotation,
        screenPoint,
        (value) => this.renderer.toScreen(value, this.state!),
      );
      if (hit) return { annotation, handle: hit.handle };
    }
    return null;
  }

  private updateDraft(annotation: Annotation, point: Point): void {
    if (annotation.kind === 'point_probe') annotation.point = point;
    else if (annotation.kind === 'angle') {
      if (this.angleAwaitingEnd) annotation.end = point;
      else annotation.start = point;
    } else annotation.end = point;
  }

  private updateEditedAnnotation(
    annotation: Annotation,
    drag: Extract<DragState | MprDragState, { kind: 'annotation-edit' }>,
    point: Point,
    frame: FrameMetadata,
  ): void {
    Object.assign(annotation, structuredClone(drag.original));
    if (drag.handle != null) {
      updateAnnotationPoint(annotation, drag.handle, point);
      return;
    }
    const points = annotationPoints(drag.original);
    const requested = {
      x: point.x - drag.startImage.x,
      y: point.y - drag.startImage.y,
    };
    const delta = {
      x: Math.max(-Math.min(...points.map((value) => value.x)), Math.min(
        requested.x,
        frame.cols - Math.max(...points.map((value) => value.x)),
      )),
      y: Math.max(-Math.min(...points.map((value) => value.y)), Math.min(
        requested.y,
        frame.rows - Math.max(...points.map((value) => value.y)),
      )),
    };
    translateAnnotation(annotation, delta);
  }

  private annotationScreenExtent(
    annotation: Annotation,
    toScreen: (point: Point) => Point,
  ): number {
    const points = annotationPoints(annotation).map(toScreen);
    if (points.length === 1) return 0;
    return Math.max(
      ...points.flatMap((left, index) =>
        points.slice(index + 1).map((right) => Math.hypot(right.x - left.x, right.y - left.y))),
    );
  }

  private recordAnnotationChange(key: string, before: Annotation[], after: Annotation[]): void {
    if (JSON.stringify(before) === JSON.stringify(after)) return;
    this.annotationHistory.push({ key, before, after: cloneAnnotations(after) });
    if (this.remoteSeriesOpen && !key.startsWith('mpr:oblique:')) {
      this.syncAnnotationDelta(key, before, after);
    }
  }

  private undoAnnotation(): void {
    if (isMaskTool(this.state?.tool)) {
      this.undoMask();
      return;
    }
    const entry = this.annotationHistory.undo();
    if (!entry) return;
    if ('changes' in entry) {
      for (const change of entry.changes) this.setAnnotationList(change.key, change.before, false);
      this.render();
      this.updateUi();
    } else this.setAnnotationList(entry.key, entry.before);
  }

  private redoAnnotation(): void {
    if (isMaskTool(this.state?.tool)) {
      this.redoMask();
      return;
    }
    const entry = this.annotationHistory.redo();
    if (!entry) return;
    if ('changes' in entry) {
      for (const change of entry.changes) this.setAnnotationList(change.key, change.after, false);
      this.render();
      this.updateUi();
    } else this.setAnnotationList(entry.key, entry.after);
  }

  private undoMask(): void {
    const entry = this.maskUndoEntries.pop();
    if (!entry) return;
    const volume = this.maskVolumes.get(entry.segmentId);
    if (!volume) return;
    restoreMaskSlices(volume, entry.before);
    this.maskRedoEntries.push({
      segmentId: entry.segmentId,
      before: cloneMaskSnapshot(entry.before),
      after: cloneMaskSnapshot(entry.after),
    });
    this.queueMaskSync(entry.segmentId, entry.before.keys());
    this.render();
    this.updateUi();
  }

  private redoMask(): void {
    const entry = this.maskRedoEntries.pop();
    if (!entry) return;
    const volume = this.maskVolumes.get(entry.segmentId);
    if (!volume) return;
    restoreMaskSlices(volume, entry.after);
    this.maskUndoEntries.push({
      segmentId: entry.segmentId,
      before: cloneMaskSnapshot(entry.before),
      after: cloneMaskSnapshot(entry.after),
    });
    this.queueMaskSync(entry.segmentId, entry.after.keys());
    this.render();
    this.updateUi();
  }

  private setAnnotationList(key: string, annotations: Annotation[], render = true): void {
    const target = key.startsWith('mpr:') ? this.mprMeasurements : this.measurements;
    const before = cloneAnnotations(target.get(key) ?? []);
    target.set(key, cloneAnnotations(annotations));
    if (this.remoteSeriesOpen) this.syncAnnotationDelta(key, before, annotations);
    this.selectedMeasurementId = null;
    if (render) {
      this.render();
      this.updateUi();
    }
  }

  private deleteSelectedAnnotation(): void {
    if (!this.selectedMeasurementId || !this.state) return;
    const mprPlane = this.mpr && this.viewerMode === 'mpr' ? this.mpr.activePlane : null;
    const key = mprPlane ? this.mprMeasurementKey(mprPlane) : this.currentFrame().frame_key;
    const list = mprPlane ? this.currentMprMeasurements(mprPlane) : this.currentMeasurements();
    const before = cloneAnnotations(list);
    const remaining = list.filter((annotation) => annotation.id !== this.selectedMeasurementId);
    (mprPlane ? this.mprMeasurements : this.measurements).set(key, remaining);
    this.recordAnnotationChange(key, before, remaining);
    this.selectedMeasurementId = null;
    this.render();
    this.updateUi();
  }

  private clearAnnotations(scope: 'current' | 'series'): void {
    if (!this.state) return;
    const message = scope === 'current' ? '清除当前图像上的全部标注？' : '清除整个序列中的全部标注？';
    if (!window.confirm(message)) return;
    if (scope === 'current') {
      const plane = this.mpr && this.viewerMode === 'mpr' ? this.mpr.activePlane : null;
      const key = plane ? this.mprMeasurementKey(plane) : this.currentFrame().frame_key;
      const target = plane ? this.mprMeasurements : this.measurements;
      const before = cloneAnnotations(target.get(key) ?? []);
      if (!before.length) return;
      target.set(key, []);
      this.recordAnnotationChange(key, before, []);
    } else {
      const target = this.viewerMode === 'mpr' ? this.mprMeasurements : this.measurements;
      const changes = [];
      for (const [key, list] of target) {
        if (!list.length) continue;
        const before = cloneAnnotations(list);
        target.set(key, []);
        changes.push({ key, before, after: [] });
        if (this.remoteSeriesOpen) this.syncAnnotationDelta(key, before, []);
      }
      this.annotationHistory.pushBatch(changes);
    }
    this.selectedMeasurementId = null;
    this.render();
    this.updateUi();
  }

  private toggleAnnotationVisibility(): void {
    this.annotationsVisible = !this.annotationsVisible;
    this.render();
    this.updateUi();
  }

  private activeViewTransform(): ViewState | MprViewportState | null {
    if (!this.state) return null;
    if (this.viewerMode === 'mpr' && this.mpr) return this.mpr.viewports[this.mpr.activePlane];
    return this.state;
  }

  private toggleInvert(): void {
    const view = this.activeViewTransform();
    if (!view) return;
    view.inverted = !view.inverted;
    this.render();
    this.updateUi();
  }

  private rotateView(direction: -1 | 1): void {
    const view = this.activeViewTransform();
    if (!view) return;
    view.rotation = ((view.rotation + direction * 90 + 360) % 360) as 0 | 90 | 180 | 270;
    view.panX = 0;
    view.panY = 0;
    this.render();
    this.updateUi();
  }

  private flipView(axis: 'horizontal' | 'vertical'): void {
    const view = this.activeViewTransform();
    if (!view) return;
    if (axis === 'horizontal') view.flipHorizontal = !view.flipHorizontal;
    else view.flipVertical = !view.flipVertical;
    this.render();
    this.updateUi();
  }

  private startAnnotationSync(): void {
    this.stopAnnotationSync();
    this.annotationSyncTimer = window.setInterval(() => {
      void this.refreshSharedAnnotations();
    }, 5000);
  }

  private stopAnnotationSync(): void {
    if (this.annotationSyncTimer != null) window.clearInterval(this.annotationSyncTimer);
    this.annotationSyncTimer = null;
    this.annotationSyncActive = false;
    this.annotationSyncQueues.clear();
    this.annotationSyncRetries.clear();
    this.annotationSyncGeneration += 1;
  }

  private async refreshSharedAnnotations(): Promise<void> {
    const studyUid = this.state?.metadata.study_uid;
    const seriesUid = this.state?.metadata.series_uid;
    if (!this.remoteSeriesOpen || !studyUid || !seriesUid || this.annotationSyncActive) return;
    const generation = this.annotationSyncGeneration;
    this.annotationSyncActive = true;
    try {
      const records = await listSharedAnnotations(
        studyUid,
        seriesUid,
        this.annotationSyncCursor ?? undefined,
      );
      if (generation !== this.annotationSyncGeneration) return;
      for (const record of records) {
        const known = this.sharedAnnotationRecords.get(record.id);
        if (known?.revision === record.revision && known.deleted_at === record.deleted_at) continue;
        this.sharedAnnotationRecords.set(record.id, record);
        this.applySharedAnnotation(record);
      }
      const latest = records.reduce<string | null>(
        (value, record) => value == null || record.updated_at > value ? record.updated_at : value,
        this.annotationSyncCursor,
      );
      this.annotationSyncCursor = latest;
      this.render();
      this.updateUi();
    } catch (error) {
      if (generation === this.annotationSyncGeneration) {
        this.showError(`共享标注刷新失败: ${errorMessage(error)}`);
      }
    } finally {
      if (generation === this.annotationSyncGeneration) this.annotationSyncActive = false;
    }
  }

  private applyAllSharedAnnotations(): void {
    for (const record of this.sharedAnnotationRecords.values()) this.applySharedAnnotation(record);
  }

  private applySharedAnnotation(record: SharedAnnotationRecord): void {
    const existing = this.findAnnotation(record.id);
    if (existing?.syncState === 'pending') return;
    this.removeAnnotationById(record.id);
    if (record.deleted_at) return;
    if (record.schema_version !== 1) {
      this.showError(`标注 ${record.id} 使用不支持的 schema_version=${record.schema_version}`);
      return;
    }
    const located = this.annotationFromRecord(record);
    if (!located) return;
    const target = located.key.startsWith('mpr:') ? this.mprMeasurements : this.measurements;
    const list = target.get(located.key) ?? [];
    list.push(located.annotation);
    target.set(located.key, list);
    if (this.isCurrentAnnotationKey(located.key)) {
      void this.refreshAnnotationStatistics(
        located.annotation,
        located.key.startsWith('mpr:') ? record.mpr_plane ?? undefined : undefined,
      );
    }
  }

  private annotationFromRecord(
    record: SharedAnnotationRecord,
  ): { key: string; annotation: Annotation } | null {
    let key: string;
    let mapper: (value: unknown) => Point | null;
    if (record.coordinate_space === 'image') {
      const frame = this.state?.metadata.frames.find(
        (candidate) => candidate.sop_instance_uid === record.sop_instance_uid
          && candidate.source_frame + 1 === record.frame_number,
      );
      if (!frame) return null;
      key = frame.frame_key;
      mapper = pointFromUnknown;
    } else {
      const plane = record.mpr_plane;
      if (!this.mpr || !plane) return null;
      const metadata = this.mprPlaneMetadata(plane);
      const first = firstGeometryPoint(record.geometry);
      const patient = patientPointFromUnknown(first);
      if (!patient) return null;
      const sliceIndex = sliceForPatientPoint(patient, metadata);
      key = `mpr:${plane}:${sliceIndex}`;
      mapper = (value) => {
        const point = patientPointFromUnknown(value);
        return point ? mprImageForPatient(point, sliceIndex, metadata) : null;
      };
    }
    const annotation = annotationFromGeometry(record.id, record.kind, record.geometry, mapper);
    if (!annotation) return null;
    annotation.revision = record.revision;
    annotation.syncState = 'synced';
    return { key, annotation };
  }

  private syncAnnotationDelta(key: string, before: Annotation[], after: Annotation[]): void {
    const generation = this.annotationSyncGeneration;
    const beforeById = new Map(before.map((annotation) => [annotation.id, annotation]));
    const afterById = new Map(after.map((annotation) => [annotation.id, annotation]));
    for (const annotation of after) {
      const previous = beforeById.get(annotation.id);
      if (!previous || JSON.stringify(annotationGeometry(previous)) !== JSON.stringify(annotationGeometry(annotation))) {
        annotation.syncState = 'pending';
        const snapshot = structuredClone(annotation);
        this.queueAnnotationSync(annotation.id, generation, async () => {
          await this.saveSharedAnnotation(key, snapshot, previous == null);
        });
      }
    }
    for (const annotation of before) {
      if (afterById.has(annotation.id)) continue;
      this.queueAnnotationSync(annotation.id, generation, async () => {
        await this.deleteSharedAnnotation(annotation.id);
      });
    }
  }

  private queueAnnotationSync(
    id: string,
    generation: number,
    operation: () => Promise<void>,
  ): void {
    const previous = this.annotationSyncQueues.get(id) ?? Promise.resolve();
    const next = previous
      .catch(() => undefined)
      .then(async () => {
        if (generation === this.annotationSyncGeneration) {
          await operation();
          this.annotationSyncRetries.delete(id);
        }
      })
      .catch((error) => {
        this.annotationSyncRetries.set(id, { generation, operation });
        const annotation = this.findAnnotation(id);
        if (annotation) annotation.syncState = 'error';
        const message = errorMessage(error);
        this.showError(`标注自动保存失败: ${message}`);
        if (message.includes('409')) void this.refreshSharedAnnotations();
        this.render();
        this.updateUi();
      })
      .finally(() => {
        if (this.annotationSyncQueues.get(id) === next) this.annotationSyncQueues.delete(id);
      });
    this.annotationSyncQueues.set(id, next);
  }

  private retryAnnotationSync(): void {
    const retries = [...this.annotationSyncRetries.entries()];
    this.annotationSyncRetries.clear();
    for (const [id, retry] of retries) {
      const annotation = this.findAnnotation(id);
      if (annotation) annotation.syncState = 'pending';
      this.queueAnnotationSync(id, retry.generation, retry.operation);
    }
    this.render();
    this.updateUi();
  }

  private async saveSharedAnnotation(
    key: string,
    annotation: Annotation,
    create: boolean,
  ): Promise<void> {
    const studyUid = this.state?.metadata.study_uid;
    const seriesUid = this.state?.metadata.series_uid;
    if (!studyUid || !seriesUid) return;
    const payload = this.sharedAnnotationPayload(key, annotation);
    let record: SharedAnnotationRecord;
    const known = this.sharedAnnotationRecords.get(annotation.id);
    if (create && !known) {
      record = await createSharedAnnotation(studyUid, seriesUid, payload);
    } else {
      const revision = known?.revision ?? this.findAnnotation(annotation.id)?.revision;
      if (!revision) throw new Error('共享标注缺少服务端版本，请刷新后重试');
      record = await updateSharedAnnotation(
        studyUid,
        seriesUid,
        annotation.id,
        revision,
        payload.geometry as Record<string, unknown>,
        false,
      );
    }
    this.acceptSyncedRecord(record, annotation);
  }

  private async deleteSharedAnnotation(id: string): Promise<void> {
    const studyUid = this.state?.metadata.study_uid;
    const seriesUid = this.state?.metadata.series_uid;
    const record = this.sharedAnnotationRecords.get(id);
    if (!studyUid || !seriesUid || !record) return;
    const updated = await updateSharedAnnotation(
      studyUid,
      seriesUid,
      id,
      record.revision,
      record.geometry,
      true,
    );
    this.sharedAnnotationRecords.set(id, updated);
  }

  private sharedAnnotationPayload(
    key: string,
    annotation: Annotation,
  ): Record<string, unknown> {
    if (!this.state) throw new Error('没有已打开的序列');
    if (key.startsWith('mpr:')) {
      if (!this.mpr) throw new Error('MPR 尚未准备完成');
      const [, rawPlane, rawSlice] = key.split(':');
      const plane = rawPlane as MprPlane;
      const sliceIndex = Number(rawSlice);
      const metadata = this.mprPlaneMetadata(plane);
      return {
        id: annotation.id,
        schema_version: 1,
        kind: annotation.kind,
        coordinate_space: 'patient',
        sop_instance_uid: null,
        frame_number: null,
        mpr_plane: plane,
        geometry: annotationGeometry(annotation, (point) =>
          patientPointForMprImage(point, sliceIndex, metadata)),
      };
    }
    const frame = this.state.metadata.frames.find((candidate) => candidate.frame_key === key);
    if (!frame?.sop_instance_uid) throw new Error('当前图像缺少 SOPInstanceUID，不能共享标注');
    return {
      id: annotation.id,
      schema_version: 1,
      kind: annotation.kind,
      coordinate_space: 'image',
      sop_instance_uid: frame.sop_instance_uid,
      frame_number: frame.source_frame + 1,
      mpr_plane: null,
      geometry: annotationGeometry(annotation),
    };
  }

  private acceptSyncedRecord(record: SharedAnnotationRecord, saved?: Annotation): void {
    this.sharedAnnotationRecords.set(record.id, record);
    const annotation = this.findAnnotation(record.id);
    if (annotation) {
      if (saved && annotation.kind === saved.kind) Object.assign(annotation, structuredClone(saved));
      annotation.revision = record.revision;
      annotation.syncState = 'synced';
      if ((annotation.kind === 'point_probe'
        || annotation.kind === 'ellipse_roi'
        || annotation.kind === 'rectangle_roi') && !annotation.statistics) {
        void this.refreshAnnotationStatistics(annotation, record.mpr_plane ?? undefined);
      }
    }
    this.render();
  }

  private findAnnotation(id: string): Annotation | null {
    for (const map of [this.measurements, this.mprMeasurements]) {
      for (const list of map.values()) {
        const annotation = list.find((candidate) => candidate.id === id);
        if (annotation) return annotation;
      }
    }
    return null;
  }

  private removeAnnotationById(id: string): void {
    for (const map of [this.measurements, this.mprMeasurements]) {
      for (const [key, list] of map) {
        const remaining = list.filter((annotation) => annotation.id !== id);
        if (remaining.length !== list.length) map.set(key, remaining);
      }
    }
  }

  private isCurrentAnnotationKey(key: string): boolean {
    if (!this.state) return false;
    if (key.startsWith('mpr:')) {
      return this.mpr != null && key === this.mprMeasurementKey(this.mpr.activePlane);
    }
    return key === this.currentFrame().frame_key;
  }

  private async refreshAnnotationStatistics(annotation: Annotation, plane?: MprPlane): Promise<void> {
    if (annotation.kind !== 'point_probe' && annotation.kind !== 'ellipse_roi' && annotation.kind !== 'rectangle_roi') return;
    // Filled by the native pixel-statistics command. Keep geometry usable if sampling is unavailable.
    try {
      annotation.statistics = await this.measureAnnotation(annotation, plane);
      annotation.measurementError = undefined;
      this.render();
    } catch (error) {
      annotation.measurementError = '测量不可用';
      this.render();
    }
  }

  private ensureCurrentStatistics(plane?: MprPlane): void {
    const annotations = plane ? this.currentMprMeasurements(plane) : this.currentMeasurements();
    for (const annotation of annotations) {
      if (
        (annotation.kind === 'point_probe'
          || annotation.kind === 'ellipse_roi'
          || annotation.kind === 'rectangle_roi')
        && !annotation.statistics
        && !annotation.measurementError
      ) {
        void this.refreshAnnotationStatistics(annotation, plane);
      }
    }
  }

  private async measureAnnotation(
    annotation: Annotation,
    plane?: MprPlane,
  ): Promise<import('./types').RoiStatistics> {
    if (!this.state) throw new Error('没有已打开的序列');
    const shape = annotation.kind === 'point_probe'
      ? 'point'
      : annotation.kind === 'ellipse_roi' ? 'ellipse' : 'rectangle';
    const startPoint = annotation.kind === 'point_probe' ? annotation.point : annotation.start;
    const endPoint = annotation.kind === 'point_probe' ? annotation.point : annotation.end;
    const start: [number, number] = [startPoint.x, startPoint.y];
    const end: [number, number] = [endPoint.x, endPoint.y];
    if (plane && this.mpr) {
      if (plane === 'oblique' || this.mprObliqueMode) {
        throw new Error('Oblique MPR 的 ROI 统计暂不支持后端采样');
      }
      return measureMprRoi(
        this.state.metadata.handle,
        plane,
        this.mpr.viewports[plane].sliceIndex,
        shape,
        start,
        end,
      );
    }
    return measureFrameRoi(
      this.state.metadata.handle,
      this.state.metadata.active_stack,
      this.state.currentFrame,
      shape,
      start,
      end,
    );
  }

  private setupResizeObserver(): void {
    const observer = new ResizeObserver(() => this.resizeViewport());
    observer.observe(this.viewport);
    this.resizeViewport();
  }

  private resizeViewport(): void {
    const rect = this.viewport.getBoundingClientRect();
    if (this.viewerMode === 'vr') {
      this.volumeRenderer?.resize(rect.width, rect.height);
    } else if (this.viewerMode === 'mpr') {
      for (const plane of MPR_PLANES) {
        const pane = requiredPlaneElement(plane);
        const paneRect = pane.getBoundingClientRect();
        this.mprRenderers[plane].resize(paneRect.width, paneRect.height);
      }
    } else {
      for (const pane of this.panes) {
        const paneRect = pane.element.getBoundingClientRect();
        pane.renderer.resize(paneRect.width, paneRect.height);
      }
    }
    this.render();
  }

  private render(): void {
    if (this.viewerMode === 'vr') return;
    if (this.viewerMode === 'mpr' && this.mpr) {
      for (const plane of MPR_PLANES) this.renderMprPlane(plane);
      return;
    }
    for (const pane of this.panes) {
      const state = pane.state;
      if (!state) {
        pane.renderer.clear();
        continue;
      }
      const frame = state.metadata.frames[state.currentFrame];
      const annotations = pane.measurements.get(frame?.frame_key ?? '') ?? [];
      pane.renderer.render(
        state,
        annotations,
        pane.draft,
        pane.selectedMeasurementId,
        pane.annotationsVisible,
        pane === this.activePane ? this.currentMaskLayers() : [],
        this.crossReferenceLinesFor(pane),
      );
    }
  }

  /** 本窗格当前帧上应绘制的其他序列扫描定位线（屏幕坐标）。 */
  private crossReferenceLinesFor(pane: SeriesPane): CrossReferenceLine[] {
    const state = pane.state;
    if (!state || !this.syncScrollEnabled) return [];
    if (this.panes.filter((candidate) => candidate.state != null).length < 2) return [];
    const targetFrame = state.metadata.frames[state.currentFrame];
    if (!targetFrame || !frameHasGeometry(targetFrame)) return [];
    const lines: CrossReferenceLine[] = [];
    for (const other of this.panes) {
      if (other === pane || other.state == null || other.syncExcludedReason) continue;
      const referenceFrame = other.state.metadata.frames[other.state.currentFrame];
      if (!referenceFrame) continue;
      const segment = crossReferenceSegment(
        seriesGeometryFrame(referenceFrame),
        seriesGeometryFrame(targetFrame),
      );
      if (!segment) continue;
      lines.push({
        start: pane.renderer.toScreenFor(segment.start, targetFrame, state),
        end: pane.renderer.toScreenFor(segment.end, targetFrame, state),
        color: SYNC_LINE_COLORS[this.paneIndex(other) % SYNC_LINE_COLORS.length],
      });
    }
    return lines;
  }

  /** 刷新各窗格的同步参与状态与角标（几何缺失 / 手动退出）。 */
  private refreshSyncBadges(): void {
    for (const pane of this.panes) {
      const state = pane.state;
      let reason: string | null = null;
      if (pane.syncManualExcluded) reason = '手动';
      else if (state) reason = syncEligibility(state.metadata.frames).reason;
      pane.syncExcludedReason = reason;
      const show = reason != null && this.syncScrollEnabled;
      pane.syncBadge.hidden = !show;
      if (show) {
        pane.syncBadge.textContent = reason === '手动'
          ? '已退出联动'
          : reason === '无帧' ? '无帧' : '缺几何 · 未同步';
      }
    }
  }

  private renderMprPlane(plane: MprPlane): void {
    if (!this.mpr || this.viewerMode !== 'mpr') return;
    if (plane === 'oblique') return;
    if (this.mprObliqueMode && this.gpuMprRenderer) {
      const viewport = this.mpr.viewports[plane];
      const pane = requiredPlaneElement(plane);
      const rect = pane.getBoundingClientRect();
      this.gpuMprRenderer.resize(rect.width, rect.height);
      this.gpuMprRenderer.setPlane(this.mprSlicePlaneMetadata(plane));
      this.gpuMprRenderer.setView(viewport, rect.width, rect.height);
      this.gpuMprRenderer.setWindow(this.state?.windowCenter ?? 0, this.state?.windowWidth ?? 1);
      this.gpuMprRenderer.setInverted(viewport.inverted);
      this.gpuMprRenderer.setProjection(this.mprProjection, this.mprSlabThicknessMm);
      this.gpuMprRenderer.setVoiFunction(this.state?.voiFunction ?? 'LINEAR');
      this.gpuMprRenderer.render();
      this.mprRenderers[plane].drawExternalImage(this.gpuMprRenderer.getCanvas());
      this.mprRenderers[plane].renderMprOverlay(
        this.mpr.viewports[plane],
        this.mprFrame(plane),
        this.currentMprMeasurements(plane),
        this.mprDraft?.plane === plane ? this.mprDraft.measurement : null,
        this.selectedMeasurementId,
        this.mprObliqueMode ? null : this.mprCrosshairImagePoint(plane),
        this.annotationsVisible,
        this.currentMprMaskLayers(plane),
      );
      this.drawRotatedCrosshair(plane);
      this.drawOrientationHint(plane);
      return;
    }
    this.mprRenderers[plane].renderMpr(
      this.mpr.viewports[plane],
      this.mprFrame(plane),
      this.currentMprMeasurements(plane),
      this.mprDraft?.plane === plane ? this.mprDraft.measurement : null,
      this.selectedMeasurementId,
      this.mprCrosshairImagePoint(plane),
      this.annotationsVisible,
      this.currentMprMaskLayers(plane),
    );
  }

  private renderMprOverlay(plane: MprPlane): void {
    if (!this.mpr || this.viewerMode !== 'mpr') return;
    this.mprRenderers[plane].renderMprOverlay(
      this.mpr.viewports[plane],
      this.mprFrame(plane),
      this.currentMprMeasurements(plane),
      this.mprDraft?.plane === plane ? this.mprDraft.measurement : null,
      this.selectedMeasurementId,
      this.mprObliqueMode ? null : this.mprCrosshairImagePoint(plane),
      this.annotationsVisible,
      this.currentMprMaskLayers(plane),
    );
    this.drawRotatedCrosshair(plane);
    this.drawOrientationHint(plane);
  }

  private rotatedCrosshairScreen(plane: MprPlane): { center: Point; xDir: Point; yDir: Point } | null {
    if (!this.mpr || this.viewerMode !== 'mpr' || !this.mprObliqueMode || plane === 'oblique') return null;
    const state = this.mpr.obliquePlanes[plane];
    const frame = this.mprFrame(plane);
    const viewport = this.mpr.viewports[plane];
    const renderer = this.mprRenderers[plane];
    const imagePoint = this.mprCrosshairImagePoint(plane);
    const center = renderer.toScreenFor(imagePoint, frame, viewport);
    const others = STANDARD_MPR_PLANES.filter((candidate) => candidate !== plane);
    const screenDirections = others.map((otherPlane) => {
      let direction = crossArray(state.normal, this.mpr!.obliquePlanes[otherPlane].normal);
      if (Math.hypot(direction[0], direction[1], direction[2]) < 1e-9) {
        direction = otherPlane === 'axial' ? state.yAxis : state.xAxis;
      }
      direction = normalizedArray(direction);
      const imageDirection = {
        x: dotArray(direction, state.xAxis),
        y: dotArray(direction, state.yAxis),
      };
      const imageLength = Math.hypot(imageDirection.x, imageDirection.y) || 1;
      const unit = { x: imageDirection.x / imageLength, y: imageDirection.y / imageLength };
      const end = renderer.toScreenFor(
        { x: imagePoint.x + unit.x, y: imagePoint.y + unit.y },
        frame,
        viewport,
      );
      return { x: end.x - center.x, y: end.y - center.y };
    });
    return { center, xDir: screenDirections[0], yDir: screenDirections[1] };
  }

  private crosshairLineHit(plane: MprPlane, point: Point): boolean {
    const cross = this.rotatedCrosshairScreen(plane);
    if (!cross) return false;
    for (const direction of [cross.xDir, cross.yDir]) {
      const length = Math.hypot(direction.x, direction.y);
      if (length < 1e-9) continue;
      const unitX = direction.x / length;
      const unitY = direction.y / length;
      const dx = point.x - cross.center.x;
      const dy = point.y - cross.center.y;
      const along = dx * unitX + dy * unitY;
      const distance = Math.abs(dx * unitY - dy * unitX);
      if (distance <= 8 && Math.abs(along) >= 18) return true;
    }
    return false;
  }

  private drawRotatedCrosshair(plane: MprPlane): void {
    const cross = this.rotatedCrosshairScreen(plane);
    if (!cross) return;
    const overlay = requiredElement<HTMLCanvasElement>(`${plane}-overlay-canvas`);
    const context = overlay.getContext('2d');
    if (!context) return;
    const renderer = this.mprRenderers[plane];
    const radius = Math.hypot(renderer.getViewport().width, renderer.getViewport().height) + 40;
    context.save();
    context.strokeStyle = '#45d4e3';
    context.lineWidth = 1;
    context.setLineDash([5, 4]);
    for (const direction of [cross.xDir, cross.yDir]) {
      const length = Math.hypot(direction.x, direction.y);
      if (length < 1e-9) continue;
      const unitX = direction.x / length;
      const unitY = direction.y / length;
      context.beginPath();
      context.moveTo(cross.center.x - unitX * radius, cross.center.y - unitY * radius);
      context.lineTo(cross.center.x + unitX * radius, cross.center.y + unitY * radius);
      context.stroke();
    }
    context.setLineDash([]);
    context.beginPath();
    context.arc(cross.center.x, cross.center.y, 4, 0, Math.PI * 2);
    context.stroke();
    context.restore();
  }

  private drawOrientationHint(plane: MprPlane): void {
    if (!this.mpr || this.viewerMode !== 'mpr' || !this.mprObliqueMode || plane === 'oblique') return;
    const state = this.mpr.obliquePlanes[plane];
    const right = patientAxisLabel(state.xAxis);
    const left = patientAxisLabel(scaleArray(state.xAxis, -1));
    const bottom = patientAxisLabel(state.yAxis);
    const top = patientAxisLabel(scaleArray(state.yAxis, -1));
    const pane = requiredPlaneElement(plane);
    setOrientationLabel(pane, 'top', top);
    setOrientationLabel(pane, 'right', right);
    setOrientationLabel(pane, 'bottom', bottom);
    setOrientationLabel(pane, 'left', left);
    this.drawOrientationCube(plane, state);
  }

  private drawOrientationCube(plane: MprPlane, state: ObliquePlaneState): void {
    const overlay = requiredElement<HTMLCanvasElement>(`${plane}-overlay-canvas`);
    const context = overlay.getContext('2d');
    if (!context) return;
    const renderer = this.mprRenderers[plane];
    const viewport = renderer.getViewport();
    const size = Math.max(16, Math.min(22, viewport.width * 0.045));
    const centerX = viewport.width - size * 2.4;
    const centerY = size * 1.9;

    // 以当前视图的切片平面作为图标基准平面；患者正方体相对于该平面旋转。
    const projectPatient = (patient: [number, number, number]): Point => {
      const u = dotArray(patient, state.xAxis);
      const v = dotArray(patient, state.yAxis);
      const n = dotArray(patient, state.normal);
      // 标准正交角度：正前方平面保持横平竖直，深度沿 45° 斜向缩短。
      const screenX = u - n * 0.38;
      const screenY = v - n * 0.38;
      return { x: centerX + screenX * size, y: centerY + screenY * size };
    };

    const vertices: Array<[number, number, number]> = [];
    for (const x of [-1, 1]) {
      for (const y of [-1, 1]) {
        for (const z of [-1, 1]) {
          vertices.push([x, y, z]);
        }
      }
    }
    const edges = [
      [0, 1], [2, 3], [4, 5], [6, 7],
      [0, 2], [1, 3], [4, 6], [5, 7],
      [0, 4], [1, 5], [2, 6], [3, 7],
    ];

    context.save();
    context.strokeStyle = 'rgba(69, 212, 227, 0.9)';
    context.lineWidth = 1;
    context.beginPath();
    for (const [start, end] of edges) {
      const from = projectPatient(vertices[start]);
      const to = projectPatient(vertices[end]);
      context.moveTo(from.x, from.y);
      context.lineTo(to.x, to.y);
    }
    context.stroke();

    // 计算当前视图平面与正方体的交面多边形。
    const normal = state.normal;
    const intersections: Array<[number, number, number]> = [];
    for (const [start, end] of edges) {
      const from = vertices[start];
      const to = vertices[end];
      const distanceFrom = dotArray(from, normal);
      const distanceTo = dotArray(to, normal);
      const denominator = distanceFrom - distanceTo;
      if (Math.abs(denominator) < 1e-12) continue;
      const t = distanceFrom / denominator;
      if (t < -1e-9 || t > 1 + 1e-9) continue;
      const point = [
        from[0] + (to[0] - from[0]) * t,
        from[1] + (to[1] - from[1]) * t,
        from[2] + (to[2] - from[2]) * t,
      ] as [number, number, number];
      if (!intersections.some((existing) => Math.hypot(
        existing[0] - point[0],
        existing[1] - point[1],
        existing[2] - point[2],
      ) < 1e-6)) {
        intersections.push(point);
      }
    }
    const ordered = intersections
      .map((point) => ({
        point,
        angle: Math.atan2(
          dotArray(point, state.yAxis),
          dotArray(point, state.xAxis),
        ),
      }))
      .sort((left, right) => left.angle - right.angle)
      .map((entry) => projectPatient(entry.point));

    if (ordered.length >= 3) {
      context.beginPath();
      context.moveTo(ordered[0].x, ordered[0].y);
      for (const point of ordered.slice(1)) context.lineTo(point.x, point.y);
      context.closePath();
      context.fillStyle = 'rgba(255, 86, 86, 0.38)';
      context.fill();
      context.strokeStyle = '#ff5a5a';
      context.lineWidth = 2;
      context.stroke();
    }
    context.restore();
  }

  private updateMprPositionUi(): void {
    if (!this.mpr) return;
    this.updateMprLayout();
    for (const plane of MPR_PLANES) {
      const viewport = this.mpr.viewports[plane];
      const metadata = this.mprPlaneMetadata(plane);
      setText(`${plane}-slice-counter`, `${viewport.sliceIndex + 1} / ${metadata.slice_count}`);
    }
    setText(
      'annotation-count',
      `${this.annotationCountText(this.currentMprMeasurements(this.mpr.activePlane))}${this.maskStatusText()}`,
    );
    setText(
      'zoom-readout',
      `${Math.round(this.mpr.viewports[this.mpr.activePlane].zoom * 100)}%`,
    );
  }

  private updateMprLayout(): void {
    for (const plane of MPR_PLANES) {
      const pane = requiredPlaneElement(plane);
      pane.classList.toggle('mpr-main', this.mpr?.mainPlane === plane);
      pane.classList.toggle('active', this.mpr?.activePlane === plane);
    }
  }

  private updateToolbarMenuStates(): void {
    for (const trigger of document.querySelectorAll<HTMLButtonElement>('[data-toolbar-menu-button]')) {
      const panelId = trigger.getAttribute('aria-controls');
      const panel = panelId ? document.getElementById(panelId) : null;
      if (!panel) continue;
      const items = [...panel.querySelectorAll<HTMLButtonElement>('button')]
        .filter((item) => !item.hidden);
      if (trigger.hasAttribute('data-requires-series')) {
        trigger.disabled = items.length === 0 || items.every((item) => item.disabled);
      }
      const activeSelector = trigger.dataset.menuActiveSelector;
      trigger.classList.toggle('active', Boolean(activeSelector && panel.querySelector(activeSelector)));
      if (trigger.disabled) {
        panel.hidden = true;
        trigger.setAttribute('aria-expanded', 'false');
      }
    }
  }

  private clearDetailsPanel(): void {
    for (const id of [
      'patient-name-detail',
      'patient-id-detail',
      'study-date',
      'accession-number',
      'modality',
      'study-description',
      'series-description',
      'dimensions',
      'instance-number',
      'pixel-format',
      'projection-orientation',
      'quantitative-status',
    ]) {
      setText(id, '--');
    }
    setText('spacing-description', '未加载影像');
    setText('annotation-count', '0 项标注');
    requiredElement<HTMLElement>('spacing-badge').dataset.confidence = 'none';
    requiredElement<HTMLElement>('spacing-badge').textContent = '仅像素';
  }

  private updateUi(): void {
    this.refreshSyncBadges();
    const hasSeries = this.state != null;
    requiredElement<HTMLButtonElement>('admin-console-btn').hidden = this.remoteUser?.role !== 'admin';
    const canManageExamRequests = this.canManageExamRequests();
    requiredElement<HTMLButtonElement>('exam-request-btn').hidden = !canManageExamRequests;
    if (!canManageExamRequests && this.examRequestPage.isOpen()) this.examRequestPage.close();
    if (this.remoteUser?.role !== 'admin') this.adminConsole.close();
    const workspaceHasSeries = this.panes.some((pane) => pane.state != null);
    const frameCount = this.state?.metadata.frames.length ?? 0;
    const appShell = requiredElement<HTMLElement>('app-shell');
    appShell.classList.toggle(
      'has-multiple-frames',
      frameCount > 1 && this.viewerMode === '2d',
    );
    appShell.classList.toggle('mpr-mode', this.viewerMode === 'mpr');
    appShell.classList.toggle('vr-mode', this.viewerMode === 'vr');
    appShell.classList.toggle('multi-pane', this.multiPane);
    this.viewport.classList.toggle('mpr-active', this.viewerMode === 'mpr');
    this.viewport.classList.toggle('vr-active', this.viewerMode === 'vr');
    requiredElement<HTMLElement>('mpr-grid').hidden = this.viewerMode !== 'mpr';
    requiredElement<HTMLElement>('vr-view').hidden = this.viewerMode !== 'vr';
    requiredElement<HTMLElement>('empty-state').hidden = workspaceHasSeries;
    requiredElement<HTMLElement>('details-panel').setAttribute('aria-disabled', String(!hasSeries));
    for (const overlay of document.querySelectorAll<HTMLElement>('[data-series-overlay]')) {
      overlay.hidden = !hasSeries;
    }
    requiredElement<HTMLButtonElement>('open-btn').disabled = this.busy;
    requiredElement<HTMLButtonElement>('empty-open-btn').disabled = this.busy;
    document.body.dataset.tool = this.state?.tool ?? 'window';
    for (const control of document.querySelectorAll<HTMLButtonElement | HTMLInputElement | HTMLSelectElement>(
      '[data-requires-series]',
    )) {
      control.disabled = !hasSeries;
    }
    const reportButton = requiredElement<HTMLButtonElement>('report-panel-btn');
    reportButton.disabled = !hasSeries || !this.remoteSeriesOpen;
    reportButton.title = this.remoteSeriesOpen
      ? '诊断报告：结构化撰写、签发与修订'
      : '本地序列仅供阅片，不提供报告服务';
    for (const button of document.querySelectorAll<HTMLButtonElement>('[data-tool]')) {
      button.classList.toggle('active', button.dataset.tool === this.state?.tool);
      button.setAttribute('aria-pressed', String(button.dataset.tool === this.state?.tool));
      button.disabled = !hasSeries || this.viewerMode === 'vr';
    }
    const maskMode = isMaskTool(this.state?.tool);
    requiredElement<HTMLButtonElement>('undo-annotation').disabled = !hasSeries
      || (maskMode ? this.maskUndoEntries.length === 0 : !this.annotationHistory.canUndo);
    requiredElement<HTMLButtonElement>('redo-annotation').disabled = !hasSeries
      || (maskMode ? this.maskRedoEntries.length === 0 : !this.annotationHistory.canRedo);
    requiredElement<HTMLButtonElement>('retry-annotation-sync').disabled = !hasSeries
      || this.annotationSyncRetries.size === 0;
    const visibility = requiredElement<HTMLButtonElement>('toggle-annotations');
    visibility.classList.toggle('active', this.annotationsVisible);
    visibility.setAttribute('aria-pressed', String(this.annotationsVisible));
    setText('toggle-annotations-label', this.annotationsVisible ? '隐藏标注' : '显示标注');
    const view = this.activeViewTransform();
    const invert = requiredElement<HTMLButtonElement>('invert-btn');
    invert.classList.toggle('active', view?.inverted === true);
    invert.setAttribute('aria-pressed', String(view?.inverted === true));
    const horizontal = requiredElement<HTMLButtonElement>('flip-horizontal-btn');
    horizontal.classList.toggle('active', view?.flipHorizontal === true);
    horizontal.setAttribute('aria-pressed', String(view?.flipHorizontal === true));
    const vertical = requiredElement<HTMLButtonElement>('flip-vertical-btn');
    vertical.classList.toggle('active', view?.flipVertical === true);
    vertical.setAttribute('aria-pressed', String(view?.flipVertical === true));
    for (const id of ['invert-btn', 'flip-horizontal-btn', 'flip-vertical-btn', 'rotate-left-btn', 'rotate-right-btn']) {
      requiredElement<HTMLButtonElement>(id).disabled = !hasSeries || this.viewerMode === 'vr';
    }
    const currentCount = this.mpr && this.viewerMode === 'mpr'
      ? this.currentMprMeasurements(this.mpr.activePlane).length
      : this.currentMeasurements().length;
    requiredElement<HTMLButtonElement>('clear-current-annotations').disabled = !hasSeries || currentCount === 0;
    const annotationMap = this.viewerMode === 'mpr' ? this.mprMeasurements : this.measurements;
    requiredElement<HTMLButtonElement>('clear-all-annotations').disabled = !hasSeries
      || ![...annotationMap.values()].some((annotations) => annotations.length > 0);
    for (const button of document.querySelectorAll<HTMLButtonElement>('[data-view-mode]')) {
      const active = button.dataset.viewMode === this.viewerMode;
      button.classList.toggle('active', active);
      button.setAttribute('aria-pressed', String(active));
      if (button.dataset.viewMode === 'mpr') {
        button.disabled = !hasSeries || this.busy || this.multiPane || !this.canAttemptMpr();
      } else if (button.dataset.viewMode === 'vr') {
        const metadata = this.mpr?.metadata.volume_rendering ?? {
          dimensions: [1, 1, 1] as [number, number, number],
          spacing_mm: [1, 1, 1] as [number, number, number],
          value_range: [0, 1] as [number, number],
          byte_length: 2,
          available: true,
          unavailable_reason: null,
        };
        const reason = this.canAttemptMpr()
          ? volumeCapabilityReason(this.volumeCanvas, metadata)
          : '当前图像组不是规则薄层灰度体数据';
        button.disabled = !hasSeries || this.busy || this.multiPane || reason != null;
        button.title = this.multiPane
          ? '多窗格对比模式下暂不可用'
          : reason ? `GPU 体渲染不可用：${reason}` : 'GPU 体渲染';
      }
    }
    for (const control of document.querySelectorAll<HTMLElement>('[data-mpr-tool]')) {
      control.hidden = this.viewerMode !== 'mpr';
    }
    const projectionControl = requiredElement<HTMLElement>('mpr-projection-control');
    projectionControl.hidden = this.viewerMode !== 'mpr';
    for (const button of document.querySelectorAll<HTMLButtonElement>('[data-mpr-projection]')) {
      const active = button.dataset.mprProjection === this.mprProjection;
      button.classList.toggle('active', active);
      button.setAttribute('aria-pressed', String(active));
      button.disabled = this.viewerMode !== 'mpr' || !this.mpr;
    }
    this.mprSlabThickness.value = String(this.mprSlabThicknessMm);
    this.mprSlabThickness.disabled = this.viewerMode !== 'mpr'
      || !this.mpr
      || this.mprProjection === 'slice';
    setText(
      'mpr-slab-value',
      `${this.mprSlabThicknessMm.toFixed(this.mprSlabThicknessMm < 10 ? 1 : 0)} mm`,
    );
    const volumeControls = requiredElement<HTMLElement>('vr-controls');
    volumeControls.hidden = this.viewerMode !== 'vr';
    this.updateMaskSegmentOptions();
    this.updateAiControls();
    const maskButton = requiredElement<HTMLButtonElement>('mask-menu-button');
    const maskPanel = requiredElement<HTMLElement>('mask-menu-panel');
    if (!hasSeries) maskPanel.hidden = true;
    maskButton.classList.toggle('active', maskMode);
    maskButton.disabled = !hasSeries || this.viewerMode === 'vr' || this.multiPane;
    if (this.multiPane) maskPanel.hidden = true;
    maskButton.title = this.multiPane ? '多窗格对比模式下暂不可用' : 'Mask 标注';
    maskButton.setAttribute('aria-expanded', String(!maskPanel.hidden));
    requiredElement<HTMLButtonElement>('revision-history-btn').hidden = !(
      hasSeries && this.remoteSeriesOpen && this.canViewDicomRevisions()
    );
    this.applyPaneLayout();
    this.updatePaneLabels();
    this.updateToolbarMenuStates();
    this.updateMprLayout();
    if (!this.state) {
      requiredElement<HTMLElement>('mpr-source-control').hidden = true;
      setText('window-readout', 'WL -- WW --');
      setText('zoom-readout', '100%');
      setText('frame-counter', '0 / 0');
      this.clearDetailsPanel();
      return;
    }

    const frame = this.currentFrame();
    const total = frameCount;
    const isColor = frame.pixel_format === 'rgb8' && this.viewerMode === '2d';
    const statisticsUnavailable = isColor
      || (this.viewerMode === 'mpr' && this.mprProjection !== 'slice');
    const windowTool = document.querySelector<HTMLButtonElement>('[data-tool="window"]');
    if (windowTool) windowTool.disabled = !hasSeries || isColor || this.viewerMode === 'vr';
    for (const tool of ['point_probe', 'ellipse_roi', 'rectangle_roi']) {
      const control = document.querySelector<HTMLButtonElement>(`[data-tool="${tool}"]`);
      if (control) control.disabled = !hasSeries || statisticsUnavailable || this.viewerMode === 'vr';
    }
    this.presetSelect.disabled = isColor || this.viewerMode === 'vr';
    this.updateToolbarMenuStates();
    setText(
      'mask-brush-size-value',
      `${this.maskBrushRadius} mm`,
    );
    setText('frame-counter', `${this.state.currentFrame + 1} / ${total}`);
    setText(
      'window-readout',
      isColor
        ? frame.photometric_interpretation
        : this.viewerMode === 'vr'
          ? `VR WL ${this.volumeWindowCenter.toFixed(0)}  WW ${this.volumeWindowWidth.toFixed(0)}`
        : `WL ${this.state.windowCenter.toFixed(0)}  WW ${this.state.windowWidth.toFixed(0)}`,
    );
    const activeZoom = this.mpr && this.viewerMode === 'mpr'
      ? this.mpr.viewports[this.mpr.activePlane].zoom
      : this.state.zoom;
    setText('zoom-readout', `${Math.round(activeZoom * 100)}%`);
    this.frameSlider.max = String(Math.max(0, total - 1));
    this.frameSlider.value = String(this.state.currentFrame);
    this.frameSlider.disabled = total <= 1;
    requiredElement<HTMLButtonElement>('previous-frame').disabled = this.state.currentFrame === 0;
    requiredElement<HTMLButtonElement>('next-frame').disabled = this.state.currentFrame === total - 1;
    const cineToggle = requiredElement<HTMLButtonElement>('cine-toggle');
    cineToggle.disabled = total <= 1 || this.viewerMode !== '2d';
    cineToggle.classList.toggle('active', this.cinePlaying);
    cineToggle.setAttribute('aria-pressed', String(this.cinePlaying));
    cineToggle.title = this.cinePlaying ? '暂停 Cine' : '播放 Cine';
    requiredElement<HTMLElement>('cine-play-icon').hidden = this.cinePlaying;
    requiredElement<HTMLElement>('cine-pause-icon').hidden = !this.cinePlaying;
    this.cineSpeedSelect.value = String(this.cineSpeed);
    this.cineSpeedSelect.disabled = total <= 1 || this.viewerMode !== '2d';
    const sourceFps = frame.cine_rate_fps ?? 15;
    setText('cine-fps', `${(sourceFps * this.cineSpeed).toFixed(1)} fps`);

    if (this.mpr) {
      requiredElement<HTMLSelectElement>('vr-preset').value = this.volumePreset;
      requiredElement<HTMLSelectElement>('vr-quality').value = this.volumeQuality;
    }

    this.updateImageStackOptions();
    this.updateMprSourceOptions();
    this.updatePresetOptions(frame);
    const patient = this.state.metadata.patient;
    setText('patient-name', formatPersonName(patient.patient_name) || '未提供');
    setText('patient-name-detail', formatPersonName(patient.patient_name) || '未提供');
    setText('patient-id', patient.patient_id || '未提供');
    setText('patient-id-detail', patient.patient_id || '未提供');
    setText('study-date', formatDicomDate(patient.study_date) || '未提供');
    setText('accession-number', patient.accession_number || '未提供');
    setText('modality', patient.modality || '未提供');
    setText('modality-overlay', patient.modality || '');
    setText('study-description', patient.study_description || '未提供');
    setText('series-description', patient.series_description || '未提供');
    setText(
      'dimensions',
      this.mpr && this.viewerMode === 'mpr'
        ? `${this.mpr.metadata.dimensions.join(' x ')} / MPR`
        : this.mpr && this.viewerMode === 'vr'
          ? `${this.mpr.metadata.dimensions.join(' x ')} / GPU VR`
        : `${frame.cols} x ${frame.rows} / ${frame.pixel_format === 'rgb8' ? 'RGB8' : `${frame.bits_allocated} bit`}`,
    );
    setText('instance-number', frame.instance_number == null ? '未提供' : String(frame.instance_number));
    setText('pixel-format', `${frame.photometric_interpretation} · ${frame.pixel_format.toUpperCase()}`);
    setText(
      'projection-orientation',
      [frame.laterality, frame.view_position, ...frame.patient_orientation]
        .filter((value): value is string => Boolean(value))
        .join(' · ') || '未提供',
    );
    setText(
      'quantitative-status',
      frame.quantitative.suvbw_status
        ? `${frame.quantitative.suvbw_status}${frame.quantitative.unit ? ` · ${frame.quantitative.unit}` : ''}`
        : (frame.quantitative.unit ? `单位 ${frame.quantitative.unit}` : '不适用'),
    );
    setText(
      'spacing-description',
      this.mpr && this.viewerMode === 'mpr'
        ? `三维重建体素 ${this.mpr.metadata.source_spacing_mm.map((value) => value.toFixed(3)).join(' x ')} mm`
        : this.mpr && this.viewerMode === 'vr'
          ? `GPU 体素 ${this.mpr.metadata.source_spacing_mm.map((value) => value.toFixed(3)).join(' x ')} mm`
        : frame.spacing.description,
    );
    const spacingBadge = requiredElement<HTMLElement>('spacing-badge');
    spacingBadge.dataset.confidence = frame.spacing.confidence;
    spacingBadge.textContent =
      frame.spacing.confidence === 'calibrated'
        ? '已标定'
        : frame.spacing.confidence === 'detector'
          ? '探测器平面'
          : '仅像素';
    if (this.mpr && this.viewerMode === 'mpr') {
      for (const plane of MPR_PLANES) {
        const viewport = this.mpr.viewports[plane];
        const metadata = this.mprPlaneMetadata(plane);
        setText(`${plane}-slice-counter`, `${viewport.sliceIndex + 1} / ${metadata.slice_count}`);
      }
      setText('annotation-count', `${this.annotationCountText(this.currentMprMeasurements(this.mpr.activePlane))}${this.maskStatusText()}`);
    } else {
      setText('annotation-count', `${this.annotationCountText(this.currentMeasurements())}${this.maskStatusText()}`);
    }
  }

  private maskStatusText(): string {
    const segment = this.segmentationSegment;
    const volume = segment ? this.maskVolumes.get(segment.id) : null;
    if (!segment || !volume) {
      setText('mask-statistics', '0 voxel · 最大径 --');
      return '';
    }
    const spacing = this.mpr?.metadata.source_spacing_mm ?? null;
    const statistics = calculateMaskStatistics(volume, spacing);
    const pending = [...volume.syncStates.values()].some((state) => state === 'pending')
      || this.maskSyncingSegments.has(segment.id);
    const error = this.maskSyncErrors.has(segment.id)
      || [...volume.syncStates.values()].some((state) => state === 'error');
    const diameter = statistics.maximumDiameterMm == null ? '--' : `${statistics.maximumDiameterMm.toFixed(1)} mm`;
    const volumeText = statistics.volumeMm3 == null ? '' : ` · ${(statistics.volumeMm3 / 1000).toFixed(2)} mL`;
    setText(
      'mask-statistics',
      `${statistics.voxelCount} voxel${volumeText} · 最大径 ${diameter}${pending ? ' · 保存中' : error ? ' · 同步失败' : ''}`,
    );
    return statistics.voxelCount
      ? ` · ${segment.label} ${statistics.voxelCount} voxel · 最大径 ${diameter}`
      : '';
  }

  private annotationCountText(annotations: Annotation[]): string {
    const pending = annotations.filter((annotation) => annotation.syncState === 'pending').length;
    const errors = annotations.filter((annotation) => annotation.syncState === 'error').length;
    const suffix = !this.remoteSeriesOpen
      ? ' · 本地会话，未同步'
      : errors ? ` · ${errors} 项同步失败`
        : pending ? ` · ${pending} 项保存中`
          : ' · 已同步';
    return `${annotations.length} 项标注${suffix}`;
  }

  private updateImageStackOptions(): void {
    if (!this.state) return;
    const stacks = this.state.metadata.image_stacks;
    const control = requiredElement<HTMLElement>('image-stack-control');
    control.hidden = stacks.length <= 1 || this.viewerMode !== '2d';
    const signature = stacks
      .map((stack) => `${stack.index}:${stack.label}:${stack.cols}x${stack.rows}`)
      .join('|');
    if (this.imageStackSelect.dataset.signature !== signature) {
      this.imageStackSelect.replaceChildren();
      for (const stack of stacks) {
        const option = document.createElement('option');
        option.value = String(stack.index);
        option.textContent = `${stack.label} · ${stack.cols} x ${stack.rows}`;
        this.imageStackSelect.append(option);
      }
      this.imageStackSelect.dataset.signature = signature;
    }
    this.imageStackSelect.value = String(this.state.metadata.active_stack);
    this.imageStackSelect.disabled = this.busy;
  }

  private updateMprSourceOptions(): void {
    const control = requiredElement<HTMLElement>('mpr-source-control');
    if (this.multiPane) {
      control.hidden = true;
      return;
    }
    const studyUid = this.state?.metadata.study_uid;
    const entries = studyUid ? this.series.get(studyUid) : undefined;
    if (!this.state || !entries || entries.length <= 1) {
      control.hidden = true;
      return;
    }
    const recommended = recommendMprSeries(entries)?.series_uid;
    const signature = entries
      .map((entry) => `${entry.series_uid}:${entry.instance_count}:${entry.description ?? ''}`)
      .join('|');
    if (this.mprSourceSelect.dataset.signature !== signature) {
      this.mprSourceSelect.replaceChildren();
      for (const entry of entries) {
        const option = document.createElement('option');
        option.value = entry.series_uid;
        const title = entry.description?.trim() || `序列 ${entry.series_number ?? '--'}`;
        const suffix = entry.series_uid === recommended ? ' · 推荐' : '';
        option.textContent = `${title} · ${entry.instance_count} 张${suffix}`;
        this.mprSourceSelect.append(option);
      }
      this.mprSourceSelect.dataset.signature = signature;
    }
    this.mprSourceSelect.value = this.state.metadata.series_uid ?? '';
    this.mprSourceSelect.disabled = this.busy;
    control.hidden = false;
  }

  private updatePresetOptions(frame: FrameMetadata): void {
    const modality = this.currentModality();
    const userPresets = userPresetsForModality(this.userWindowPresets, modality);
    const previousValue = this.presetSelect.value;
    const previousPreset = this.resolveWindowPreset(previousValue);
    const previousCount = this.presetSelect.options.length;
    const signature = `${modality ?? ''}:${frame.window_presets.map(presetSignature).join('|')}:user:${userPresets.map(userPresetSignature).join('|')}`;
    if (this.presetSelect.dataset.signature !== signature || previousCount === 0) {
      this.presetSelect.replaceChildren();
      const dicomGroup = document.createElement('optgroup');
      dicomGroup.label = 'DICOM 自带';
      frame.window_presets.forEach((preset, index) => {
        const option = document.createElement('option');
        option.value = `dicom:${index}`;
        option.textContent = `DICOM · ${preset.explanation?.trim() || `窗 ${index + 1}`}`;
        dicomGroup.append(option);
      });
      this.presetSelect.append(dicomGroup);
      if (userPresets.length > 0) {
        const userGroup = document.createElement('optgroup');
        userGroup.label = '我的预设';
        for (const preset of userPresets) {
          const option = document.createElement('option');
          option.value = `user:${preset.id}`;
          option.textContent = `我的 · ${preset.name}`;
          userGroup.append(option);
        }
        this.presetSelect.append(userGroup);
      }
      this.presetSelect.dataset.signature = signature;
    }
    let selectedValue = '';
    if (previousPreset && this.windowPresetMatchesState(previousPreset)) {
      selectedValue = previousValue;
    } else {
      for (const option of Array.from(this.presetSelect.options)) {
        const preset = this.resolveWindowPreset(option.value);
        if (preset && this.windowPresetMatchesState(preset)) {
          selectedValue = option.value;
          break;
        }
      }
    }
    if (selectedValue && Array.from(this.presetSelect.options).some((option) => option.value === selectedValue)) {
      this.presetSelect.value = selectedValue;
    } else {
      this.presetSelect.selectedIndex = -1;
    }
    this.updateWindowPresetControls();
  }

  private windowPresetMatchesState(preset: WindowPreset): boolean {
    return !!this.state && windowPresetMatchesState(preset, this.state);
  }

  private updateWindowPresetControls(): void {
    const frame = this.state ? this.currentFrame() : null;
    const isColor = frame?.pixel_format === 'rgb8' && this.viewerMode === '2d';
    const canSave = !!this.state
      && !!this.currentModality()
      && !isColor
      && this.viewerMode !== 'vr'
      && !this.windowPresetBusy;
    const selected = this.selectedUserWindowPreset();
    requiredElement<HTMLButtonElement>('window-preset-save').disabled = !canSave;
    requiredElement<HTMLButtonElement>('window-preset-rename').disabled = !canSave || !selected;
    requiredElement<HTMLButtonElement>('window-preset-delete').disabled = !canSave || !selected;
  }

  private currentFrame(): FrameMetadata {
    if (!this.state) throw new Error('没有已打开的序列');
    return this.state.metadata.frames[this.state.currentFrame];
  }

  private currentMeasurements(): Annotation[] {
    if (!this.state) return [];
    return this.measurements.get(this.currentFrame().frame_key) ?? [];
  }

  private mprMeasurementKey(plane: MprPlane): string {
    if (!this.mpr) return `mpr:${plane}:0`;
    return `mpr:${plane}:${this.mpr.viewports[plane].sliceIndex}`;
  }

  private currentMprMeasurements(plane: MprPlane): Annotation[] {
    return this.mprMeasurements.get(this.mprMeasurementKey(plane)) ?? [];
  }

  private eventPoint(event: MouseEvent | PointerEvent | WheelEvent): Point {
    const canvas = event.currentTarget instanceof HTMLCanvasElement
      ? event.currentTarget
      : this.overlayCanvas;
    const rect = canvas.getBoundingClientRect();
    return { x: event.clientX - rect.left, y: event.clientY - rect.top };
  }

  private setBusy(busy: boolean, message = '', cancellable = false): void {
    this.busy = busy;
    const loading = requiredElement<HTMLElement>('loading');
    loading.hidden = !busy;
    setText('loading-text', message);
    const cancel = requiredElement<HTMLButtonElement>('cancel-download');
    cancel.hidden = !busy || !cancellable;
    cancel.disabled = false;
  }

  private showError(message: string): void {
    if (this.statusBannerTimer != null) window.clearTimeout(this.statusBannerTimer);
    setText('error-message', message);
    this.errorBanner.hidden = false;
    this.statusBannerTimer = window.setTimeout(() => {
      this.statusBannerTimer = null;
      this.errorBanner.hidden = true;
    }, STATUS_BANNER_TIMEOUT_MS);
  }

  private hideStatusBanner(): void {
    if (this.statusBannerTimer != null) {
      window.clearTimeout(this.statusBannerTimer);
      this.statusBannerTimer = null;
    }
    this.errorBanner.hidden = true;
  }

  private showSeriesWarning(): void {
    if (!this.state?.metadata.warnings.length) return;
    this.showError(this.state.metadata.warnings.join('；'));
  }
}

function annotationGeometry(
  annotation: Annotation,
  map: (point: Point) => Point | PatientPoint3D = (point) => ({ ...point }),
): Record<string, unknown> {
  if (annotation.kind === 'point_probe') return { point: map(annotation.point) };
  if (annotation.kind === 'angle') {
    return {
      start: map(annotation.start),
      vertex: map(annotation.vertex),
      end: map(annotation.end),
    };
  }
  return { start: map(annotation.start), end: map(annotation.end) };
}

function annotationFromGeometry(
  id: string,
  kind: AnnotationKind,
  geometry: Record<string, unknown>,
  map: (value: unknown) => Point | null,
): Annotation | null {
  if (kind === 'point_probe') {
    const point = map(geometry.point);
    return point ? { id, kind, point } : null;
  }
  if (kind === 'angle') {
    const start = map(geometry.start);
    const vertex = map(geometry.vertex);
    const end = map(geometry.end);
    return start && vertex && end ? { id, kind, start, vertex, end } : null;
  }
  const start = map(geometry.start);
  const end = map(geometry.end);
  if (!start || !end) return null;
  if (kind === 'length' || kind === 'arrow') return { id, kind, start, end };
  if (kind === 'ellipse_roi' || kind === 'rectangle_roi') return { id, kind, start, end };
  return null;
}

function pointFromUnknown(value: unknown): Point | null {
  if (!value || typeof value !== 'object') return null;
  const candidate = value as Record<string, unknown>;
  return typeof candidate.x === 'number' && Number.isFinite(candidate.x)
    && typeof candidate.y === 'number' && Number.isFinite(candidate.y)
    ? { x: candidate.x, y: candidate.y }
    : null;
}

function patientPointFromUnknown(value: unknown): PatientPoint3D | null {
  const point = pointFromUnknown(value);
  if (!point || !value || typeof value !== 'object') return null;
  const z = (value as Record<string, unknown>).z;
  return typeof z === 'number' && Number.isFinite(z) ? { ...point, z } : null;
}

function firstGeometryPoint(geometry: Record<string, unknown>): unknown {
  return geometry.point ?? geometry.start ?? geometry.vertex ?? geometry.end;
}

function requiredElement<T extends HTMLElement>(id: string): T {
  const element = document.getElementById(id);
  if (!element) throw new Error(`缺少界面元素 #${id}`);
  return element as T;
}

function setText(id: string, value: string): void {
  requiredElement<HTMLElement>(id).textContent = value;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function scopeLabel(scope: TransformScope): string {
  if (scope === 'patient') return '患者';
  if (scope === 'study') return '检查';
  return '序列';
}

function dicomInputValue(
  keyword: string,
  value: string | number | null | undefined,
): string {
  if (value === null || value === undefined) return '';
  const text = String(value).trim();
  if (keyword === 'PatientBirthDate') return text.replace(/-/g, '');
  return text;
}

function editorInputValue(keyword: string, value: string): string {
  return keyword === 'PatientBirthDate' ? formatDicomDate(value) : value;
}

function displayTagValue(value: string | null): string {
  return value?.trim() ? value : '（空）';
}

function transformModeLabel(mode: TransformJob['mode']): string {
  if (mode === 'clinical_correction') return '临床修订';
  return '版本回滚';
}

function transformStatusLabel(status: TransformJob['status']): string {
  const labels: Record<TransformJob['status'], string> = {
    previewed: '等待确认',
    queued: '等待处理',
    running: '处理中',
    succeeded: '已完成',
    failed: '失败',
    blocked: '已阻止',
    expired: '已过期',
  };
  return labels[status];
}

function revisionKindLabel(kind: DicomRevision['derivation_kind']): string {
  if (kind === 'original') return '原始归档';
  if (kind === 'clinical_correction') return '临床修订';
  return '版本回滚';
}

function makeId(): string {
  if (globalThis.crypto?.randomUUID) return globalThis.crypto.randomUUID();
  const bytes = new Uint8Array(16);
  if (globalThis.crypto?.getRandomValues) globalThis.crypto.getRandomValues(bytes);
  else for (let index = 0; index < bytes.length; index += 1) bytes[index] = Math.floor(Math.random() * 256);
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = [...bytes].map((value) => value.toString(16).padStart(2, '0')).join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

function presetSignature(preset: WindowPreset): string {
  return `${preset.center}:${preset.width}:${preset.function}:${preset.explanation ?? ''}`;
}

function userPresetSignature(preset: UserWindowPreset): string {
  return `${preset.id}:${preset.modality}:${preset.name}:${presetSignature(preset)}`;
}

function clampWorklistWidth(width: number): number {
  return Math.min(WORKLIST_MAX_WIDTH, Math.max(WORKLIST_MIN_WIDTH, Math.round(width)));
}

function clampDetailsWidth(width: number): number {
  return Math.min(DETAILS_MAX_WIDTH, Math.max(DETAILS_MIN_WIDTH, Math.round(width)));
}

function formatPersonName(value: string | null): string {
  return value?.split('=')[0].split('^').filter(Boolean).join(' ') ?? '';
}

function formatDicomDate(value: string | null): string {
  if (!value || !/^\d{8}$/.test(value)) return value ?? '';
  return `${value.slice(0, 4)}-${value.slice(4, 6)}-${value.slice(6, 8)}`;
}

function formatApiDate(value: string | null): string {
  if (!value) return '';
  return /^\d{4}-\d{2}-\d{2}$/.test(value) ? value : formatDicomDate(value);
}

function reportStatusBadge(text: string, key: string): HTMLSpanElement {
  const badge = document.createElement('span');
  badge.className = 'worklist-report-badge';
  badge.dataset.status = key;
  badge.textContent = text;
  return badge;
}

function patientReportStatus(patient: PatientSummary): { text: string; key: string } {
  if (patient.locked_studies > 0) return { text: '已锁定', key: 'locked' };
  if (patient.writing_studies > 0) return { text: '书写中', key: 'writing' };
  if (patient.pending_studies > 0) return { text: `${patient.pending_studies} 个检查待书写`, key: 'pending' };
  return { text: '已签发', key: 'signed' };
}

function studyReportStatusText(status: string): string {
  if (status === 'signed') return '已签发';
  if (status === 'submitted') return '待审核';
  if (status === 'under_review') return '审核中';
  if (status === 'writing') return '书写中';
  return '待书写';
}

function worklistRow(
  title: string,
  subtitle: string,
  meta: string,
  expanded: boolean,
): HTMLButtonElement {
  const button = document.createElement('button');
  button.type = 'button';
  button.className = 'worklist-row';
  button.setAttribute('aria-expanded', String(expanded));
  const disclosure = document.createElement('span');
  disclosure.className = 'disclosure';
  disclosure.setAttribute('aria-hidden', 'true');
  const content = document.createElement('span');
  content.className = 'worklist-row-content';
  const heading = document.createElement('strong');
  heading.textContent = title;
  const sub = document.createElement('span');
  sub.textContent = subtitle;
  const metadata = document.createElement('small');
  metadata.textContent = meta;
  content.append(heading, sub, metadata);
  button.append(disclosure, content);
  return button;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GiB`;
}

function emptyWorklistMessage(message: string): HTMLElement {
  const element = document.createElement('p');
  element.className = 'empty-worklist-message';
  element.textContent = message;
  return element;
}

function requiredPlaneElement(plane: MprPlane): HTMLElement {
  const element = document.querySelector<HTMLElement>(`.mpr-viewport[data-plane="${plane}"]`);
  if (!element) throw new Error(`缺少 ${plane} MPR 视图`);
  return element;
}

function eventPointFor(
  canvas: HTMLCanvasElement,
  event: MouseEvent | PointerEvent | WheelEvent,
): Point {
  const rect = canvas.getBoundingClientRect();
  return { x: event.clientX - rect.left, y: event.clientY - rect.top };
}

function requirePlane(metadata: MprMetadata, plane: MprPlane): MprPlaneMetadata {
  const value = metadata.planes.find((candidate) => candidate.plane === plane);
  if (!value) throw new Error(`MPR 元数据缺少 ${plane} 切面`);
  return value;
}

export function sliceForPatientPoint(point: PatientPoint3D, plane: MprPlaneMetadata): number {
  const relative = subtractPointArray(point, plane.origin);
  const requested = Math.round(dotArray(relative, plane.normal) / plane.slice_spacing_mm);
  return Math.max(0, Math.min(requested, plane.slice_count - 1));
}

export function patientPointForMprImage(
  imagePoint: Point,
  sliceIndex: number,
  plane: MprPlaneMetadata,
): PatientPoint3D {
  const slicePlane: MprPlaneMetadata = {
    ...plane,
    origin: addArray(
      plane.origin,
      scaleArray(plane.normal, sliceIndex * plane.slice_spacing_mm),
    ),
  };
  const value = applyMat4(planeToPatientMat4(slicePlane), [imagePoint.x, imagePoint.y, 0]);
  return point3(value);
}

export function mprImageForPatient(
  point: PatientPoint3D,
  sliceIndex: number,
  plane: MprPlaneMetadata,
): Point {
  const slicePlane: MprPlaneMetadata = {
    ...plane,
    origin: addArray(
      plane.origin,
      scaleArray(plane.normal, sliceIndex * plane.slice_spacing_mm),
    ),
  };
  const value = applyMat4(patientToPlaneMat4(slicePlane), [point.x, point.y, point.z]);
  return { x: value[0], y: value[1] };
}

export function consumeMprWheel(
  accumulator: MprWheelAccumulator,
  deltaX: number,
  deltaY: number,
  shiftKey: boolean,
): Point {
  const horizontal = shiftKey ? (Math.abs(deltaX) > 0 ? deltaX : deltaY) : deltaX;
  const vertical = shiftKey ? 0 : deltaY;
  accumulator.x += Number.isFinite(horizontal) ? horizontal : 0;
  accumulator.y += Number.isFinite(vertical) ? vertical : 0;
  const x = wheelSteps(accumulator.x);
  const y = wheelSteps(accumulator.y);
  accumulator.x -= x * MPR_WHEEL_THRESHOLD;
  accumulator.y -= y * MPR_WHEEL_THRESHOLD;
  return { x, y };
}

function normalizeWheelDelta(value: number, mode: number, pageSize: number): number {
  if (mode === WheelEvent.DOM_DELTA_LINE) return value * 16;
  if (mode === WheelEvent.DOM_DELTA_PAGE) return value * pageSize;
  return value;
}

function wheelSteps(value: number): number {
  const steps = Math.trunc(value / MPR_WHEEL_THRESHOLD);
  return Math.max(-6, Math.min(steps, 6));
}

function frameSourceKey(frame: FrameMetadata): string {
  return frame.sop_instance_uid ?? frame.frame_key.replace(/#\d+$/, '');
}

function point3(value: [number, number, number]): PatientPoint3D {
  return { x: value[0], y: value[1], z: value[2] };
}

function addPatientVector(
  point: PatientPoint3D,
  direction: [number, number, number],
  amount: number,
): PatientPoint3D {
  return {
    x: point.x + direction[0] * amount,
    y: point.y + direction[1] * amount,
    z: point.z + direction[2] * amount,
  };
}

function addArray(
  left: [number, number, number],
  right: [number, number, number],
): [number, number, number] {
  return [left[0] + right[0], left[1] + right[1], left[2] + right[2]];
}

function scaleArray(
  value: [number, number, number],
  amount: number,
): [number, number, number] {
  return [value[0] * amount, value[1] * amount, value[2] * amount];
}

function subtractPointArray(
  point: PatientPoint3D,
  value: [number, number, number],
): [number, number, number] {
  return [point.x - value[0], point.y - value[1], point.z - value[2]];
}

function dotArray(
  left: [number, number, number],
  right: [number, number, number],
): number {
  return left[0] * right[0] + left[1] * right[1] + left[2] * right[2];
}

function crossArray(
  left: [number, number, number],
  right: [number, number, number],
): [number, number, number] {
  return [
    left[1] * right[2] - left[2] * right[1],
    left[2] * right[0] - left[0] * right[2],
    left[0] * right[1] - left[1] * right[0],
  ];
}

function normalizedArray(value: [number, number, number]): [number, number, number] {
  const length = Math.hypot(value[0], value[1], value[2]);
  if (!Number.isFinite(length) || length < 1e-12) return [1, 0, 0];
  return [value[0] / length, value[1] / length, value[2] / length];
}

function rotateVectorAroundAxis(
  value: [number, number, number],
  axis: [number, number, number],
  angleRadians: number,
): [number, number, number] {
  const normalized = normalizedArray(axis);
  const cosine = Math.cos(angleRadians);
  const sine = Math.sin(angleRadians);
  const dot = dotArray(value, normalized);
  const cross = crossArray(normalized, value);
  return [
    value[0] * cosine + cross[0] * sine + normalized[0] * dot * (1 - cosine),
    value[1] * cosine + cross[1] * sine + normalized[1] * dot * (1 - cosine),
    value[2] * cosine + cross[2] * sine + normalized[2] * dot * (1 - cosine),
  ];
}

function orthogonalizeArray(
  value: [number, number, number],
  normal: [number, number, number],
): [number, number, number] {
  const unitNormal = normalizedArray(normal);
  const dot = dotArray(value, unitNormal);
  return normalizedArray([
    value[0] - unitNormal[0] * dot,
    value[1] - unitNormal[1] * dot,
    value[2] - unitNormal[2] * dot,
  ]);
}

function patientAxisLabel(direction: [number, number, number]): string {
  const absX = Math.abs(direction[0]);
  const absY = Math.abs(direction[1]);
  const absZ = Math.abs(direction[2]);
  if (absX >= absY && absX >= absZ) return direction[0] >= 0 ? 'L' : 'R';
  if (absY >= absZ) return direction[1] >= 0 ? 'P' : 'A';
  return direction[2] >= 0 ? 'S' : 'I';
}

function setOrientationLabel(pane: HTMLElement, side: 'top' | 'right' | 'bottom' | 'left', text: string): void {
  const element = pane.querySelector<HTMLElement>(`.orientation.${side}`);
  if (element) element.textContent = text;
}

export function recommendMprSeries(series: RemoteSeriesSummary[]): RemoteSeriesSummary | null {
  const eligible = series.filter((entry) => {
    if (!['CT', 'MR'].includes(entry.modality?.toUpperCase() ?? '')) return false;
    const description = entry.description?.toLowerCase() ?? '';
    return !/(locali[sz]er|scout|定位|冠状|矢状|coronal|sagittal|\bmpr\b)/i.test(description);
  });
  return eligible.reduce<RemoteSeriesSummary | null>((best, entry) => {
    if (!best) return entry;
    return mprSeriesScore(entry) > mprSeriesScore(best) ? entry : best;
  }, null);
}

function mprSeriesScore(entry: RemoteSeriesSummary): number {
  const description = entry.description?.toLowerCase() ?? '';
  const preferred = /(original|primary|axial|thin|薄层|轴位)/i.test(description) ? 10_000 : 0;
  return preferred + entry.instance_count;
}

function isMaskTool(tool: ToolMode | null | undefined): tool is MaskTool {
  return tool === 'mask_brush' || tool === 'mask_eraser';
}

function localSegmentationSegment(): SegmentationSegment {
  const timestamp = new Date().toISOString();
  return {
    id: 'local-segment-1',
    project_id: 'local-project',
    segment_number: 1,
    label: 'Segment 1',
    description: '本地手工标注',
    color_r: 55,
    color_g: 213,
    color_b: 216,
    algorithm_type: 'manual',
    tags: [],
    created_at: timestamp,
    updated_at: timestamp,
  };
}

function maskSegmentColor(segment: SegmentationSegment): [number, number, number] {
  return [segment.color_r, segment.color_g, segment.color_b];
}

function cloneMaskSnapshot(snapshot: MaskSliceSnapshot): MaskSliceSnapshot {
  const clone: MaskSliceSnapshot = new Map();
  for (const [slice, data] of snapshot) clone.set(slice, data?.slice() ?? null);
  return clone;
}

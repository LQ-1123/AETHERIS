import {
  buildLut,
  cancelMprBuild,
  cancelRemoteDownload,
  cancelTransfer,
  chooseCaCertificate,
  chooseDicomFiles,
  chooseImportFiles,
  chooseImportFolder,
  closeSeries,
  closeMpr,
  confirmTransform,
  createSegmentationProject,
  exportFromPacs,
  createSharedAnnotation,
  getTransformSchema,
  listInstanceRevisionsBySop,
  listPatientStudies,
  listPatients,
  listRouteDestinations,
  listSegmentationProjects,
  listSegmentationSegments,
  listSegmentationVolume,
  listStudySeries,
  listSharedAnnotations,
  listTransformJobs,
  loadFrame,
  measureFrameRoi,
  measureMprRoi,
  openRemoteSeries,
  openSeries,
  importToPacs,
  prepareMpr,
  previewClinicalTransform,
  previewRollback,
  remoteLogin,
  remoteLogout,
  renderMprSlice,
  selectImageStack,
  sendRouteScope,
  updateSharedAnnotation,
  updateSegmentationSegmentTags,
  upsertSegmentationMasks,
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
import { imageGeometry, Renderer } from './renderer';
import { RequestVersion } from './request-version';
import { RouterPanel } from './router-panel';
import { importConflictMessage, importSummary } from './transfer-report';
import type {
  Annotation,
  AnnotationKind,
  DicomRevision,
  FrameMetadata,
  DownloadProgress,
  MprBuildProgress,
  MprMetadata,
  MprPlane,
  MprPlaneMetadata,
  MprViewportState,
  MaskTool,
  PatientSummary,
  PatientPoint3D,
  Point,
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

interface MprSession {
  metadata: MprMetadata;
  crosshair: PatientPoint3D;
  mainPlane: MprPlane;
  activePlane: MprPlane;
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
const PATIENT_PAGE_SIZE = 30;
const MPR_PLANES: readonly MprPlane[] = ['axial', 'coronal', 'sagittal'];
const MPR_WHEEL_THRESHOLD = 30;
const CT_WINDOW_PRESETS: readonly WindowPreset[] = [
  { center: 40, width: 80, explanation: '脑窗', function: 'LINEAR' },
  { center: 50, width: 130, explanation: '硬膜下', function: 'LINEAR' },
  { center: -600, width: 1500, explanation: '肺窗', function: 'LINEAR' },
  { center: 40, width: 350, explanation: '纵隔', function: 'LINEAR' },
  { center: 50, width: 400, explanation: '腹部', function: 'LINEAR' },
  { center: 400, width: 2000, explanation: '骨窗', function: 'LINEAR' },
];
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

export class App {
  private state: ViewState | null = null;
  private renderer: Renderer;
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
  };
  private mprRequestPending: Record<MprPlane, boolean> = {
    axial: false,
    coronal: false,
    sagittal: false,
  };
  private mprReloadQueued: Record<MprPlane, boolean> = {
    axial: false,
    coronal: false,
    sagittal: false,
  };
  private mprWheelDelta: Record<MprPlane, MprWheelAccumulator> = {
    axial: { x: 0, y: 0 },
    coronal: { x: 0, y: 0 },
    sagittal: { x: 0, y: 0 },
  };
  private mprWindowFrameRequest: number | null = null;
  private mprBuildActive = false;
  private frameCache = new ByteLruCache(FRONTEND_CACHE_BYTES);
  private pendingFrames = new Map<string, Promise<ArrayBuffer>>();
  private measurements = new Map<string, Annotation[]>();
  private draft: Annotation | null = null;
  private selectedMeasurementId: string | null = null;
  private annotationHistory = new AnnotationHistory();
  private annotationsVisible = true;
  private angleAwaitingEnd = false;
  private sharedAnnotationRecords = new Map<string, SharedAnnotationRecord>();
  private annotationSyncCursor: string | null = null;
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
  private maskBrushRadius = 5;
  private maskOpacity = 0.38;
  private drag: DragState | null = null;
  private frameRequests = new RequestVersion();
  private lutRequests = new RequestVersion();
  private windowFrameRequest: number | null = null;
  private wheelFrameDelta = 0;
  private busy = false;
  private remoteDownloadActive = false;
  private remoteSeriesOpen = false;
  private remoteUser: RemoteUser | null = null;
  private patients: PatientSummary[] = [];
  private patientPage = 0;
  private hasNextPatientPage = false;
  private expandedPatientId: number | null = null;
  private expandedStudyUid: string | null = null;
  private studies = new Map<number, StudySummary[]>();
  private series = new Map<string, RemoteSeriesSummary[]>();
  private worklistBusy = false;
  private transferActive = false;
  private transferKind: 'imports' | 'exports' | null = null;
  private transformSchema: TransformSchema | null = null;
  private tagEditorContext: TagEditorContext | null = null;
  private transformPreview: TransformPreviewResponse | null = null;
  private transformTaskTimer: number | null = null;
  private observedCompletedTransformJobs = new Set<string>();
  private revisions: DicomRevision[] = [];
  private selectedRollbackRevision: DicomRevision | null = null;
  private rollbackPreview: TransformPreviewResponse | null = null;
  private routerPanel: RouterPanel;
  private lifecyclePanel: LifecyclePanel;
  private shareStudyUid: string | null = null;

  private viewport = requiredElement<HTMLElement>('viewport');
  private overlayCanvas = requiredElement<HTMLCanvasElement>('overlay-canvas');
  private frameSlider = requiredElement<HTMLInputElement>('frame-slider');
  private presetSelect = requiredElement<HTMLSelectElement>('preset-select');
  private imageStackSelect = requiredElement<HTMLSelectElement>('image-stack-select');
  private mprSourceSelect = requiredElement<HTMLSelectElement>('mpr-source-select');
  private errorBanner = requiredElement<HTMLElement>('error-banner');
  private tagEditorDialog = requiredElement<HTMLDialogElement>('tag-editor-dialog');
  private transformTasksDialog = requiredElement<HTMLDialogElement>('transform-tasks-dialog');
  private revisionHistoryDialog = requiredElement<HTMLDialogElement>('revision-history-dialog');

  constructor() {
    this.routerPanel = new RouterPanel((message) => this.showError(message));
    this.lifecyclePanel = new LifecyclePanel((message) => this.showError(message));
    this.renderer = new Renderer(
      requiredElement<HTMLCanvasElement>('image-canvas'),
      this.overlayCanvas,
    );
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
    };
    this.setupEventListeners();
    this.setupRemoteProgress();
    this.setupImportDrop();
    this.setupResizeObserver();
    this.restoreConnectionFields();
    this.updateUi();
  }

  async openFiles(): Promise<void> {
    try {
      const paths = await chooseDicomFiles();
      if (!paths?.length) return;
      await this.activateSeries(() => openSeries(paths), '正在解析序列...');
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
      this.angleAwaitingEnd = false;
      this.draft = null;
      this.mprDraft = null;
      this.selectedMeasurementId = null;
      this.viewerMode = '2d';
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
        tool: 'window',
      };
      this.remoteSeriesOpen = remoteDownload;
      await this.loadCurrentFrame();
      await this.loadSegmentationWorkspace();
      if (remoteDownload) {
        await this.refreshSharedAnnotations();
        this.startAnnotationSync();
      }
      if (previous) void closeSeries(previous.metadata.handle).catch(console.error);
      openedHandle = null;
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
        this.frameCache.clear();
        this.pendingFrames.clear();
        if (previous) {
          await this.loadCurrentFrame().catch(console.error);
        } else {
          this.renderer.clear();
        }
      }
      throw error;
    } finally {
      this.remoteDownloadActive = false;
      this.setBusy(false);
      this.updateUi();
    }
  }

  async setFrame(requested: number): Promise<void> {
    if (!this.state) return;
    const next = Math.max(0, Math.min(requested, this.state.metadata.frames.length - 1));
    if (next === this.state.currentFrame && this.state.lut) return;
    this.state.currentFrame = next;
    this.selectedMeasurementId = null;
    this.draft = null;
    this.updateUi();
    try {
      await this.loadCurrentFrame();
    } catch (error) {
      this.showError(errorMessage(error));
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
    try {
      this.setBusy(true, '正在切换图像组...');
      const metadata = await selectImageStack(previous.metadata.handle, requested);
      if (!metadata.frames.length) throw new Error('所选图像组没有可显示的帧');
      const preset = metadata.frames[0].window_presets[0];
      if (!preset) throw new Error('所选图像组没有可用的显示窗口');

      this.frameRequests.invalidate();
      this.lutRequests.invalidate();
      this.frameCache.clear();
      this.pendingFrames.clear();
      this.draft = null;
      this.mprDraft = null;
      this.selectedMeasurementId = null;
      this.viewerMode = '2d';
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
        tool: previous.tool === 'crosshair' ? 'window' : previous.tool,
      };
      changed = true;
      this.updateUi();
      await this.loadCurrentFrame();
      await this.loadSegmentationVolumes();
      await closeMpr(previous.metadata.handle).catch(() => undefined);
      this.showSeriesWarning();
    } catch (error) {
      if (changed) {
        this.frameRequests.invalidate();
        this.lutRequests.invalidate();
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
    const state = this.state;
    const generation = this.frameRequests.next();
    const frameIndex = state.currentFrame;
    const frame = state.metadata.frames[frameIndex];
    this.setBusy(true, `正在加载第 ${frameIndex + 1} 帧...`);
    try {
      const [buffer, lut] = await Promise.all([
        this.getFrame(frameIndex),
        buildLut(
          state.metadata.handle,
          state.metadata.active_stack,
          frameIndex,
          state.windowCenter,
          state.windowWidth,
          state.voiFunction,
        ),
      ]);
      if (!this.frameRequests.isCurrent(generation) || state !== this.state) return;
      state.lut = lut;
      this.renderer.setFrame(buffer, frame);
      this.renderer.applyLut(lut);
      this.render();
      this.ensureCurrentStatistics();
      this.prefetch(frameIndex);
    } finally {
      if (this.frameRequests.isCurrent(generation)) this.setBusy(false);
      this.updateUi();
    }
  }

  private async refreshLut(): Promise<void> {
    if (!this.state) return;
    const state = this.state;
    const frameIndex = state.currentFrame;
    const generation = this.lutRequests.next();
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
        !this.lutRequests.isCurrent(generation) ||
        state !== this.state ||
        frameIndex !== state.currentFrame
      ) {
        return;
      }
      state.lut = lut;
      this.renderer.applyLut(lut);
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
    if (mode === '2d') {
      this.viewerMode = '2d';
      if (this.state.tool === 'crosshair') this.state.tool = 'window';
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
      this.viewerMode = 'mpr';
      this.state.tool = 'crosshair';
      this.updateUi();
      this.resizeViewport();
      await this.refreshMprSlices();
    } catch (error) {
      this.viewerMode = '2d';
      this.showError(errorMessage(error));
    } finally {
      this.mprBuildActive = false;
      this.setBusy(false);
      this.updateUi();
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
    const viewport = (plane: MprPlane): MprViewportState => ({
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
    return {
      metadata,
      crosshair,
      mainPlane: 'axial',
      activePlane: 'axial',
      viewports: {
        axial: viewport('axial'),
        coronal: viewport('coronal'),
        sagittal: viewport('sagittal'),
      },
    };
  }

  private canAttemptMpr(): boolean {
    if (!this.state || this.state.metadata.frames.length < 3) return false;
    const frame = this.state.metadata.frames[0];
    if (frame.spacing.row_mm == null || frame.spacing.col_mm == null) return false;
    const description = this.state.metadata.patient.series_description?.toLowerCase() ?? '';
    return !/(locali[sz]er|scout|定位|冠状|矢状|coronal|sagittal|\bmpr\b)/i.test(description);
  }

  private async refreshMprSlices(planes: readonly MprPlane[] = MPR_PLANES): Promise<void> {
    await Promise.all(planes.map((plane) => this.loadMprPlane(plane)));
  }

  private async loadMprPlane(plane: MprPlane): Promise<void> {
    if (!this.state || !this.mpr) return;
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
    const generation = this.mprRequests[plane].next();
    try {
      const buffer = await renderMprSlice(
        state.metadata.handle,
        plane,
        sliceIndex,
        windowCenter,
        windowWidth,
        voiFunction,
      );
      if (
        this.viewerMode !== 'mpr' ||
        this.mpr !== session ||
        this.state !== state ||
        !this.mprRequests[plane].isCurrent(generation) ||
        viewport.sliceIndex !== sliceIndex ||
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
    if (this.mprWindowFrameRequest != null) return;
    this.mprWindowFrameRequest = requestAnimationFrame(() => {
      this.mprWindowFrameRequest = null;
      void this.refreshMprSlices();
    });
  }

  private changeMprSlice(plane: MprPlane, requested: number): void {
    if (!this.mpr) return;
    const viewport = this.mpr.viewports[plane];
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
      const index = sliceForPatientPoint(next, requirePlane(this.mpr.metadata, plane));
      if (index !== this.mpr.viewports[plane].sliceIndex) changed.push(plane);
      this.mpr.viewports[plane].sliceIndex = index;
    }
    this.mpr.crosshair = next;
    this.selectedMeasurementId = null;
    this.mprDraft = null;
    this.updateMprPositionUi();
    for (const plane of MPR_PLANES) this.renderMprOverlay(plane);
    if (changed.length) void this.refreshMprSlices(changed);
  }

  private mprFrame(plane: MprPlane): FrameMetadata {
    if (!this.state || !this.mpr) throw new Error('MPR 尚未初始化');
    const metadata = requirePlane(this.mpr.metadata, plane);
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
      window_presets: this.currentFrame().window_presets,
      spacing: {
        confidence: 'calibrated',
        source: 'mpr-patient-space',
        description: `MPR 患者空间重采样 ${metadata.pixel_spacing_mm.toFixed(3)} mm`,
        row_mm: metadata.pixel_spacing_mm,
        col_mm: metadata.pixel_spacing_mm,
        column_over_row: 1,
      },
    };
  }

  private mprCrosshairImagePoint(plane: MprPlane): Point {
    if (!this.mpr) return { x: 0, y: 0 };
    const metadata = requirePlane(this.mpr.metadata, plane);
    const viewport = this.mpr.viewports[plane];
    return mprImageForPatient(this.mpr.crosshair, viewport.sliceIndex, metadata);
  }

  private mprImagePointToPatient(plane: MprPlane, imagePoint: Point): PatientPoint3D {
    if (!this.mpr) throw new Error('MPR 尚未初始化');
    const metadata = requirePlane(this.mpr.metadata, plane);
    const viewport = this.mpr.viewports[plane];
    return patientPointForMprImage(imagePoint, viewport.sliceIndex, metadata);
  }

  private invalidateMprRequests(): void {
    for (const plane of MPR_PLANES) {
      this.mprRequests[plane].invalidate();
      this.mprReloadQueued[plane] = false;
    }
  }

  private async getFrame(index: number): Promise<ArrayBuffer> {
    const cached = this.frameCache.get(index);
    if (cached) return cached;
    if (!this.state) throw new Error('没有已打开的序列');
    const handle = this.state.metadata.handle;
    const stack = this.state.metadata.active_stack;
    const requestKey = `${handle}:${stack}:${index}`;
    const pending = this.pendingFrames.get(requestKey);
    if (pending) return pending;
    const request = loadFrame(handle, stack, index)
      .then((buffer) => {
        if (
          this.state?.metadata.handle === handle &&
          this.state.metadata.active_stack === stack
        ) {
          this.frameCache.set(index, buffer);
        }
        return buffer;
      })
      .finally(() => this.pendingFrames.delete(requestKey));
    this.pendingFrames.set(requestKey, request);
    return request;
  }

  private prefetch(current: number): void {
    if (!this.state) return;
    const last = this.state.metadata.frames.length - 1;
    for (let distance = 1; distance <= 2; distance += 1) {
      for (const index of [current - distance, current + distance]) {
        if (index >= 0 && index <= last) void this.getFrame(index).catch(() => undefined);
      }
    }
  }

  private async setTool(tool: ToolMode): Promise<void> {
    if (!this.state) return;
    if (tool === 'crosshair' && this.viewerMode !== 'mpr') return;
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
    const studyUid = this.state?.metadata.study_uid;
    const seriesUid = this.state?.metadata.series_uid;
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
    const segmentGroups = await Promise.all(
      projects.map((project) => listSegmentationSegments(studyUid, seriesUid, project.id)),
    );
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
    const [source, rawIndex] = value.split(':');
    const index = Number(rawIndex);
    const preset = source === 'ct'
      ? CT_WINDOW_PRESETS[index]
      : this.currentFrame()?.window_presets[index];
    if (!preset) return;
    this.state.windowCenter = preset.center;
    this.state.windowWidth = preset.width;
    this.state.voiFunction = preset.function;
    this.updateUi();
    if (this.viewerMode === 'mpr') void this.refreshMprSlices();
    else void this.refreshLut();
  }

  private setupEventListeners(): void {
    requiredElement<HTMLFormElement>('login-form').addEventListener('submit', (event) => {
      event.preventDefault();
      void this.login();
    });
    requiredElement<HTMLButtonElement>('choose-ca-btn').addEventListener('click', () => {
      void this.chooseCertificate();
    });
    requiredElement<HTMLButtonElement>('logout-btn').addEventListener('click', () => {
      void this.logout();
    });
    requiredElement<HTMLFormElement>('study-share-form').addEventListener('submit', (event) => {
      event.preventDefault();
      void this.submitStudyShare();
    });
    for (const id of ['study-share-close', 'study-share-cancel']) {
      requiredElement<HTMLButtonElement>(id).addEventListener('click', () => this.closeStudyShare());
    }
    requiredElement<HTMLButtonElement>('worklist-toggle').addEventListener('click', () => {
      document.getElementById('worklist-panel')?.classList.toggle('collapsed');
      document.getElementById('workspace')?.classList.toggle('worklist-hidden');
      setTimeout(() => this.resizeViewport(), 0);
    });
    requiredElement<HTMLButtonElement>('refresh-worklist').addEventListener('click', () => {
      void this.loadPatients();
    });
    const importMenu = requiredElement<HTMLElement>('import-menu');
    const importMenuButton = requiredElement<HTMLButtonElement>('import-menu-button');
    const importMenuPanel = requiredElement<HTMLElement>('import-menu-panel');
    const closeImportMenu = (): void => {
      importMenuPanel.hidden = true;
      importMenuButton.setAttribute('aria-expanded', 'false');
    };
    importMenuButton.addEventListener('click', (event) => {
      event.stopPropagation();
      importMenuPanel.hidden = !importMenuPanel.hidden;
      importMenuButton.setAttribute('aria-expanded', String(!importMenuPanel.hidden));
    });
    requiredElement<HTMLButtonElement>('import-files').addEventListener('click', () => {
      closeImportMenu();
      void this.chooseAndImport(false);
    });
    requiredElement<HTMLButtonElement>('import-folder').addEventListener('click', () => {
      closeImportMenu();
      void this.chooseAndImport(true);
    });
    document.addEventListener('click', (event) => {
      if (!importMenu.contains(event.target as Node)) closeImportMenu();
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
    maskMenuButton.addEventListener('click', (event) => {
      event.stopPropagation();
      maskMenuPanel.hidden = !maskMenuPanel.hidden;
      maskMenuButton.setAttribute('aria-expanded', String(!maskMenuPanel.hidden));
      if (!maskMenuPanel.hidden) {
        positionMaskMenu();
        this.updateMaskSegmentOptions();
      }
    });
    maskMenuPanel.addEventListener('click', (event) => event.stopPropagation());
    document.addEventListener('click', (event) => {
      if (!maskMenuPanel.contains(event.target as Node) && event.target !== maskMenuButton) closeMaskMenu();
    });
    requiredElement<HTMLSelectElement>('mask-segment-select').addEventListener('change', (event) => {
      void this.selectMaskSegment((event.currentTarget as HTMLSelectElement).value);
    });
    requiredElement<HTMLFormElement>('mask-tag-form').addEventListener('submit', (event) => {
      event.preventDefault();
      void this.saveMaskTags();
    });
    requiredElement<HTMLSelectElement>('mask-tag-filter').addEventListener('change', (event) => {
      void this.applyMaskTagFilter((event.currentTarget as HTMLSelectElement).value);
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
      if (this.mprBuildActive) void cancelMprBuild();
      else void cancelRemoteDownload();
      setText('loading-text', '正在取消下载...');
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
    requiredElement<HTMLButtonElement>('panel-toggle').addEventListener('click', () => {
      document.getElementById('details-panel')?.classList.toggle('collapsed');
      document.getElementById('workspace')?.classList.toggle('details-hidden');
      setTimeout(() => this.resizeViewport(), 0);
    });
    requiredElement<HTMLButtonElement>('error-close').addEventListener('click', () => {
      this.errorBanner.hidden = true;
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

    this.frameSlider.addEventListener('input', () => void this.setFrame(Number(this.frameSlider.value)));
    this.presetSelect.addEventListener('change', () => this.applyPreset(this.presetSelect.value));
    this.imageStackSelect.addEventListener('change', () => {
      void this.switchImageStack(Number(this.imageStackSelect.value));
    });
    this.mprSourceSelect.addEventListener('change', () => {
      const studyUid = this.state?.metadata.study_uid;
      const seriesUid = this.mprSourceSelect.value;
      if (!studyUid || !seriesUid || seriesUid === this.state?.metadata.series_uid) return;
      void this.openRemote(studyUid, seriesUid);
    });
    this.overlayCanvas.addEventListener('contextmenu', (event) => event.preventDefault());
    this.overlayCanvas.addEventListener('pointerdown', (event) => this.pointerDown(event));
    this.overlayCanvas.addEventListener('pointermove', (event) => this.pointerMove(event));
    this.overlayCanvas.addEventListener('pointerup', (event) => this.pointerUp(event));
    this.overlayCanvas.addEventListener('pointercancel', (event) => this.pointerUp(event));
    this.overlayCanvas.addEventListener('wheel', (event) => this.wheel(event), { passive: false });

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
      pane.addEventListener('dblclick', () => this.promoteMprPlane(plane));
    }

    window.addEventListener('keydown', (event) => this.keyDown(event));
  }

  private restoreConnectionFields(): void {
    const savedUrl = localStorage.getItem('remote-pacs.server-url');
    const savedCa = localStorage.getItem('remote-pacs.ca-cert-path');
    if (savedUrl) requiredElement<HTMLInputElement>('server-url').value = savedUrl;
    if (savedCa) requiredElement<HTMLInputElement>('ca-cert-path').value = savedCa;
  }

  private async chooseCertificate(): Promise<void> {
    const selected = await chooseCaCertificate();
    if (selected) requiredElement<HTMLInputElement>('ca-cert-path').value = selected;
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
    try {
      const user = await remoteLogin(serverUrl, caCertPath, username, passwordInput.value);
      this.remoteUser = user;
      passwordInput.value = '';
      localStorage.setItem('remote-pacs.server-url', serverUrl);
      localStorage.setItem('remote-pacs.ca-cert-path', caCertPath);
      setText('current-user', user.display_name?.trim() || user.username);
      this.routerPanel.setAvailable(user.role === 'admin');
      this.lifecyclePanel.setAvailable(user.role === 'admin');
      requiredElement<HTMLElement>('login-screen').hidden = true;
      requiredElement<HTMLElement>('app-shell').removeAttribute('aria-hidden');
      await this.initializeTransformTools();
      this.resizeViewport();
      await this.loadPatients();
    } catch (error) {
      loginError.textContent = errorMessage(error);
      loginError.hidden = false;
    } finally {
      loginButton.disabled = false;
    }
  }

  private async logout(): Promise<void> {
    try {
      await remoteLogout();
    } catch (error) {
      this.showError(errorMessage(error));
    } finally {
      this.remoteUser = null;
      this.remoteSeriesOpen = false;
      this.stopAnnotationSync();
      this.patients = [];
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
  }

  private setupImportDrop(): void {
    void import('@tauri-apps/api/webview').then(({ getCurrentWebview }) =>
      getCurrentWebview().onDragDropEvent(({ payload }) => {
        const overlay = requiredElement<HTMLElement>('import-drop-overlay');
        if (payload.type === 'enter') {
          if (!this.canEditDicomTags() || this.transferActive) return;
          setText(
            'import-drop-detail',
            `${payload.paths.length} 个项目 · DICOM、ZIP/RAR 或文件夹`,
          );
          overlay.hidden = false;
          return;
        }
        if (payload.type === 'leave') {
          overlay.hidden = true;
          return;
        }
        if (payload.type !== 'drop') return;
        overlay.hidden = true;
        if (!this.canEditDicomTags()) {
          if (this.remoteUser) this.showError('当前账号没有导入 DICOM 的权限');
          return;
        }
        if (this.transferActive) {
          this.showError('已有导入或导出任务正在进行');
          return;
        }
        if (payload.paths.length) void this.importPaths(payload.paths);
      }),
    ).catch((error) => console.warn('无法启用拖拽导入', error));
  }

  private async chooseAndImport(folder: boolean): Promise<void> {
    if (!this.canEditDicomTags() || this.transferActive) return;
    const paths = folder ? await chooseImportFolder() : await chooseImportFiles();
    if (!paths?.length) return;
    await this.importPaths(paths);
  }

  private async importPaths(paths: string[]): Promise<void> {
    this.transferActive = true; this.transferKind = 'imports'; this.setWorklistBusy(true, '准备上传...');
    try {
      const response = await importToPacs(paths);
      const summary = importSummary(response);
      setText('worklist-status', summary);
      const conflict = importConflictMessage(response);
      if (conflict) this.showError(conflict);
      this.setWorklistBusy(false);
      await this.loadPatients();
      setText('worklist-status', summary);
    } catch (error) { this.showError(errorMessage(error)); setText('worklist-status', errorMessage(error)); }
    finally { this.transferActive = false; this.transferKind = null; this.setWorklistBusy(false); }
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
      this.renderPatients();
      return;
    }
    this.expandedStudyUid = studyUid;
    this.renderPatients();
    if (this.series.has(studyUid)) return;
    this.setWorklistBusy(true, '正在加载序列...');
    try {
      this.series.set(studyUid, await listStudySeries(studyUid));
      setText('worklist-status', '');
      this.renderPatients();
    } catch (error) {
      this.showError(errorMessage(error));
    } finally {
      this.setWorklistBusy(false);
    }
  }

  private async openRemote(studyUid: string, seriesUid: string): Promise<void> {
    try {
      await this.activateSeries(
        () => openRemoteSeries(studyUid, seriesUid),
        '正在准备远程序列...',
        true,
      );
    } catch (error) {
      this.showError(errorMessage(error));
    }
  }

  private canEditDicomTags(): boolean {
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
        await this.loadPatients();
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
          for (const study of studies) {
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
            studyRow.addEventListener('click', () => void this.toggleStudy(study.study_uid));
            studyItem.append(studyRow);
            this.appendShareButton(studyItem, study.study_uid, studyTitle);
            this.appendExportButton(studyItem, study.study_uid);
            this.appendTagEditButton(studyItem, {
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
            if (this.expandedStudyUid === study.study_uid) {
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
                  seriesButton.addEventListener('click', () => {
                    void this.openRemote(study.study_uid, entry.series_uid);
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
            }
            studyList.append(studyItem);
          }
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
    if (event.button === 1 || this.state.tool === 'pan') {
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
    if (
      !this.state ||
      !this.mpr ||
      !this.mprDrag ||
      this.mprDrag.plane !== plane ||
      this.mprDrag.pointerId !== event.pointerId
    ) {
      return;
    }
    const point = eventPointFor(canvas, event);
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
    this.render();
    this.updateUi();
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

  private promoteMprPlane(plane: MprPlane): void {
    if (!this.mpr || this.mpr.mainPlane === plane) return;
    this.mpr.mainPlane = plane;
    this.mpr.activePlane = plane;
    this.updateMprLayout();
    setTimeout(() => this.resizeViewport(), 0);
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
    this.overlayCanvas.setPointerCapture(event.pointerId);
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
    if (this.overlayCanvas.hasPointerCapture(event.pointerId)) {
      this.overlayCanvas.releasePointerCapture(event.pointerId);
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
    if (this.remoteSeriesOpen) this.syncAnnotationDelta(key, before, after);
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
      const metadata = requirePlane(this.mpr.metadata, plane);
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
      const metadata = requirePlane(this.mpr.metadata, plane);
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
    this.renderer.resize(rect.width, rect.height);
    if (this.viewerMode === 'mpr') {
      for (const plane of MPR_PLANES) {
        const pane = requiredPlaneElement(plane);
        const paneRect = pane.getBoundingClientRect();
        this.mprRenderers[plane].resize(paneRect.width, paneRect.height);
      }
    }
    this.render();
  }

  private render(): void {
    if (!this.state) return;
    if (this.viewerMode === 'mpr' && this.mpr) {
      for (const plane of MPR_PLANES) this.renderMprPlane(plane);
      return;
    }
    this.renderer.render(
      this.state,
      this.currentMeasurements(),
      this.draft,
      this.selectedMeasurementId,
      this.annotationsVisible,
      this.currentMaskLayers(),
    );
  }

  private renderMprPlane(plane: MprPlane): void {
    if (!this.mpr || this.viewerMode !== 'mpr') return;
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
      this.mprCrosshairImagePoint(plane),
      this.annotationsVisible,
      this.currentMprMaskLayers(plane),
    );
  }

  private updateMprPositionUi(): void {
    if (!this.mpr) return;
    this.updateMprLayout();
    for (const plane of MPR_PLANES) {
      const viewport = this.mpr.viewports[plane];
      const metadata = requirePlane(this.mpr.metadata, plane);
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

  private updateUi(): void {
    const hasSeries = this.state != null;
    const frameCount = this.state?.metadata.frames.length ?? 0;
    const appShell = requiredElement<HTMLElement>('app-shell');
    appShell.classList.toggle(
      'has-multiple-frames',
      frameCount > 1 && this.viewerMode === '2d',
    );
    appShell.classList.toggle('mpr-mode', this.viewerMode === 'mpr');
    this.viewport.classList.toggle('mpr-active', this.viewerMode === 'mpr');
    requiredElement<HTMLElement>('mpr-grid').hidden = this.viewerMode !== 'mpr';
    requiredElement<HTMLElement>('empty-state').hidden = hasSeries;
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
    for (const button of document.querySelectorAll<HTMLButtonElement>('[data-tool]')) {
      button.classList.toggle('active', button.dataset.tool === this.state?.tool);
      button.setAttribute('aria-pressed', String(button.dataset.tool === this.state?.tool));
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
        button.disabled = !hasSeries || this.busy || !this.canAttemptMpr();
      }
    }
    for (const control of document.querySelectorAll<HTMLElement>('[data-mpr-tool]')) {
      control.hidden = this.viewerMode !== 'mpr';
    }
    this.updateMaskSegmentOptions();
    const maskButton = requiredElement<HTMLButtonElement>('mask-menu-button');
    const maskPanel = requiredElement<HTMLElement>('mask-menu-panel');
    if (!hasSeries) maskPanel.hidden = true;
    maskButton.classList.toggle('active', maskMode);
    maskButton.setAttribute('aria-expanded', String(!maskPanel.hidden));
    requiredElement<HTMLButtonElement>('revision-history-btn').hidden = !(
      hasSeries && this.remoteSeriesOpen && this.canViewDicomRevisions()
    );
    this.updateMprLayout();
    if (!this.state) return;

    const frame = this.currentFrame();
    const total = frameCount;
    setText(
      'mask-brush-size-value',
      `${this.maskBrushRadius} mm`,
    );
    setText('frame-counter', `${this.state.currentFrame + 1} / ${total}`);
    setText('window-readout', `WL ${this.state.windowCenter.toFixed(0)}  WW ${this.state.windowWidth.toFixed(0)}`);
    const activeZoom = this.mpr && this.viewerMode === 'mpr'
      ? this.mpr.viewports[this.mpr.activePlane].zoom
      : this.state.zoom;
    setText('zoom-readout', `${Math.round(activeZoom * 100)}%`);
    this.frameSlider.max = String(Math.max(0, total - 1));
    this.frameSlider.value = String(this.state.currentFrame);
    this.frameSlider.disabled = total <= 1;
    requiredElement<HTMLButtonElement>('previous-frame').disabled = this.state.currentFrame === 0;
    requiredElement<HTMLButtonElement>('next-frame').disabled = this.state.currentFrame === total - 1;

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
        : `${frame.cols} x ${frame.rows} / ${frame.bits_allocated} bit`,
    );
    setText('instance-number', frame.instance_number == null ? '未提供' : String(frame.instance_number));
    setText(
      'spacing-description',
      this.mpr && this.viewerMode === 'mpr'
        ? `三维重建体素 ${this.mpr.metadata.source_spacing_mm.map((value) => value.toFixed(3)).join(' x ')} mm`
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
        const metadata = requirePlane(this.mpr.metadata, plane);
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
    control.hidden = stacks.length <= 1 || this.viewerMode === 'mpr';
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
    const isCt = this.state?.metadata.patient.modality?.trim().toUpperCase() === 'CT';
    const presets = isCt ? [...frame.window_presets, ...CT_WINDOW_PRESETS] : frame.window_presets;
    const previousCount = this.presetSelect.options.length;
    const signature = `${isCt}:${presets.map(presetSignature).join('|')}`;
    if (this.presetSelect.dataset.signature !== signature || previousCount === 0) {
      this.presetSelect.replaceChildren();
      frame.window_presets.forEach((preset, index) => {
        const option = document.createElement('option');
        option.value = `dicom:${index}`;
        option.textContent = `DICOM · ${preset.explanation?.trim() || `窗 ${index + 1}`}`;
        this.presetSelect.append(option);
      });
      if (isCt) {
        CT_WINDOW_PRESETS.forEach((preset, index) => {
          const option = document.createElement('option');
          option.value = `ct:${index}`;
          option.textContent = `CT · ${preset.explanation}`;
          this.presetSelect.append(option);
        });
      }
      this.presetSelect.dataset.signature = signature;
    }
    const match = presets.findIndex(
      (preset) =>
        Math.abs(preset.center - this.state!.windowCenter) < 0.001 &&
        Math.abs(preset.width - this.state!.windowWidth) < 0.001 &&
        preset.function === this.state!.voiFunction,
    );
    this.presetSelect.selectedIndex = match;
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
    const rect = this.overlayCanvas.getBoundingClientRect();
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
    setText('error-message', message);
    this.errorBanner.hidden = false;
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
  const value = addArray(
    addArray(
      addArray(
        plane.origin,
        scaleArray(plane.normal, sliceIndex * plane.slice_spacing_mm),
      ),
      scaleArray(plane.x_axis, imagePoint.x * plane.pixel_spacing_mm),
    ),
    scaleArray(plane.y_axis, imagePoint.y * plane.pixel_spacing_mm),
  );
  return point3(value);
}

export function mprImageForPatient(
  point: PatientPoint3D,
  sliceIndex: number,
  plane: MprPlaneMetadata,
): Point {
  const sliceOrigin = addArray(
    plane.origin,
    scaleArray(plane.normal, sliceIndex * plane.slice_spacing_mm),
  );
  const relative = subtractPointArray(point, sliceOrigin);
  return {
    x: dotArray(relative, plane.x_axis) / plane.pixel_spacing_mm,
    y: dotArray(relative, plane.y_axis) / plane.pixel_spacing_mm,
  };
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

function recommendMprSeries(series: RemoteSeriesSummary[]): RemoteSeriesSummary | null {
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

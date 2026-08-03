import {
  buildLut,
  cancelMprBuild,
  cancelRemoteDownload,
  chooseCaCertificate,
  chooseDicomFiles,
  closeSeries,
  closeMpr,
  listPatientStudies,
  listPatients,
  listStudySeries,
  loadFrame,
  openRemoteSeries,
  openSeries,
  prepareMpr,
  remoteLogin,
  remoteLogout,
  renderMprSlice,
  selectImageStack,
} from './api';
import { clampToImage, pointToSegmentDistance, zoomAt } from './geometry';
import { ByteLruCache } from './lru';
import { imageGeometry, Renderer } from './renderer';
import { RequestVersion } from './request-version';
import type {
  FrameMetadata,
  DownloadProgress,
  LengthMeasurement,
  MprBuildProgress,
  MprMetadata,
  MprPlane,
  MprPlaneMetadata,
  MprViewportState,
  PatientSummary,
  PatientPoint3D,
  Point,
  RemoteSeriesSummary,
  RemoteUser,
  SeriesMetadata,
  StudySummary,
  ToolMode,
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
  | { kind: 'length'; pointerId: number };

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
  | { kind: 'crosshair' | 'length'; plane: MprPlane; pointerId: number };

interface MprSession {
  metadata: MprMetadata;
  crosshair: PatientPoint3D;
  mainPlane: MprPlane;
  activePlane: MprPlane;
  viewports: Record<MprPlane, MprViewportState>;
}

const FRONTEND_CACHE_BYTES = 128 * 1024 * 1024;
const PATIENT_PAGE_SIZE = 30;
const MPR_PLANES: readonly MprPlane[] = ['axial', 'coronal', 'sagittal'];
const MPR_WHEEL_THRESHOLD = 30;

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
  private mprMeasurements = new Map<string, LengthMeasurement[]>();
  private mprDraft: { plane: MprPlane; measurement: LengthMeasurement } | null = null;
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
  private measurements = new Map<string, LengthMeasurement[]>();
  private draft: LengthMeasurement | null = null;
  private selectedMeasurementId: string | null = null;
  private drag: DragState | null = null;
  private frameRequests = new RequestVersion();
  private lutRequests = new RequestVersion();
  private windowFrameRequest: number | null = null;
  private wheelFrameDelta = 0;
  private busy = false;
  private remoteDownloadActive = false;
  private remoteUser: RemoteUser | null = null;
  private patients: PatientSummary[] = [];
  private patientPage = 0;
  private hasNextPatientPage = false;
  private expandedPatientId: number | null = null;
  private expandedStudyUid: string | null = null;
  private studies = new Map<number, StudySummary[]>();
  private series = new Map<string, RemoteSeriesSummary[]>();
  private worklistBusy = false;

  private viewport = requiredElement<HTMLElement>('viewport');
  private overlayCanvas = requiredElement<HTMLCanvasElement>('overlay-canvas');
  private frameSlider = requiredElement<HTMLInputElement>('frame-slider');
  private presetSelect = requiredElement<HTMLSelectElement>('preset-select');
  private imageStackSelect = requiredElement<HTMLSelectElement>('image-stack-select');
  private mprSourceSelect = requiredElement<HTMLSelectElement>('mpr-source-select');
  private errorBanner = requiredElement<HTMLElement>('error-banner');

  constructor() {
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
        lut: null,
        tool: 'window',
      };
      await this.loadCurrentFrame();
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
        lut: null,
        tool: previous.tool === 'crosshair' ? 'window' : previous.tool,
      };
      changed = true;
      this.updateUi();
      await this.loadCurrentFrame();
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
        this.mprBuildActive = true;
        this.setBusy(true, '正在构建三维体数据...', true);
        this.updateUi();
        const metadata = await prepareMpr(
          this.state.metadata.handle,
          this.state.metadata.active_stack,
        );
        this.mprMeasurements = new Map();
        this.mpr = this.createMprSession(metadata);
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

  private createMprSession(metadata: MprMetadata): MprSession {
    const crosshair = point3(metadata.initial_crosshair);
    const viewport = (plane: MprPlane): MprViewportState => ({
      plane,
      sliceIndex: sliceForPatientPoint(crosshair, requirePlane(metadata, plane)),
      zoom: 1,
      panX: 0,
      panY: 0,
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

  private setTool(tool: ToolMode): void {
    if (!this.state) return;
    if (tool === 'crosshair' && this.viewerMode !== 'mpr') return;
    this.state.tool = tool;
    this.draft = null;
    this.mprDraft = null;
    this.selectedMeasurementId = null;
    this.updateUi();
    this.render();
  }

  private resetView(): void {
    if (!this.state) return;
    if (this.viewerMode === 'mpr' && this.mpr) {
      for (const plane of MPR_PLANES) {
        this.mpr.viewports[plane].zoom = 1;
        this.mpr.viewports[plane].panX = 0;
        this.mpr.viewports[plane].panY = 0;
      }
      this.setMprCrosshair(point3(this.mpr.metadata.initial_crosshair));
      return;
    }
    this.state.zoom = 1;
    this.state.panX = 0;
    this.state.panY = 0;
    this.render();
    this.updateUi();
  }

  private applyPreset(index: number): void {
    if (!this.state) return;
    const preset = this.currentFrame()?.window_presets[index];
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
    requiredElement<HTMLButtonElement>('worklist-toggle').addEventListener('click', () => {
      document.getElementById('worklist-panel')?.classList.toggle('collapsed');
      document.getElementById('workspace')?.classList.toggle('worklist-hidden');
      setTimeout(() => this.resizeViewport(), 0);
    });
    requiredElement<HTMLButtonElement>('refresh-worklist').addEventListener('click', () => {
      void this.loadPatients();
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
    requiredElement<HTMLButtonElement>('cancel-download').addEventListener('click', () => {
      if (this.mprBuildActive) void cancelMprBuild();
      else void cancelRemoteDownload();
      setText('loading-text', '正在取消下载...');
      requiredElement<HTMLButtonElement>('cancel-download').disabled = true;
    });
    requiredElement<HTMLButtonElement>('open-btn').addEventListener('click', () => void this.openFiles());
    requiredElement<HTMLButtonElement>('empty-open-btn').addEventListener('click', () => void this.openFiles());
    requiredElement<HTMLButtonElement>('reset-btn').addEventListener('click', () => this.resetView());
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
      button.addEventListener('click', () => this.setTool(button.dataset.tool as ToolMode));
    }
    for (const button of document.querySelectorAll<HTMLButtonElement>('[data-view-mode]')) {
      button.addEventListener('click', () => {
        void this.setViewerMode(button.dataset.viewMode as ViewerMode);
      });
    }

    this.frameSlider.addEventListener('input', () => void this.setFrame(Number(this.frameSlider.value)));
    this.presetSelect.addEventListener('change', () => this.applyPreset(Number(this.presetSelect.value)));
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
      requiredElement<HTMLElement>('login-screen').hidden = true;
      requiredElement<HTMLElement>('app-shell').removeAttribute('aria-hidden');
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
      this.patients = [];
      this.studies.clear();
      this.series.clear();
      this.expandedPatientId = null;
      this.expandedStudyUid = null;
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
            const studyRow = worklistRow(
              study.description?.trim() || '未命名检查',
              `${formatApiDate(study.study_date) || '无日期'} · ${modality}`,
              `${study.series_count} 序列 · ${study.instance_count} 实例`,
              this.expandedStudyUid === study.study_uid,
            );
            studyRow.classList.add('study-row');
            studyRow.addEventListener('click', () => void this.toggleStudy(study.study_uid));
            studyItem.append(studyRow);

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
                  seriesList.append(seriesButton);
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
  }

  private setWorklistBusy(busy: boolean, message = ''): void {
    this.worklistBusy = busy;
    if (busy) setText('worklist-status', message);
    requiredElement<HTMLButtonElement>('refresh-worklist').disabled = busy;
    requiredElement<HTMLInputElement>('patient-query').disabled = busy;
    requiredElement<HTMLButtonElement>('refresh-worklist').classList.toggle('spinning', busy);
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
    } else if (this.state.tool === 'crosshair') {
      this.mprDrag = { kind: 'crosshair', plane, pointerId: event.pointerId };
      this.moveMprCrosshairFromScreen(plane, point);
    } else {
      const hit = this.mprHitTest(plane, point);
      if (hit) {
        this.selectedMeasurementId = hit.id;
        this.mprDrag = null;
        canvas.releasePointerCapture(event.pointerId);
      } else {
        const frame = this.mprFrame(plane);
        const imagePoint = clampToImage(
          this.mprRenderers[plane].toImageFor(point, frame, viewport),
          imageGeometry(frame),
        );
        this.mprDraft = {
          plane,
          measurement: { id: makeId(), start: imagePoint, end: imagePoint },
        };
        this.selectedMeasurementId = null;
        this.mprDrag = { kind: 'length', plane, pointerId: event.pointerId };
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
    } else if (this.mprDraft?.plane === plane) {
      const frame = this.mprFrame(plane);
      this.mprDraft.measurement.end = clampToImage(
        this.mprRenderers[plane].toImageFor(point, frame, viewport),
        imageGeometry(frame),
      );
      this.renderMprPlane(plane);
    }
  }

  private mprPointerUp(
    plane: MprPlane,
    canvas: HTMLCanvasElement,
    event: PointerEvent,
  ): void {
    if (!this.mpr || !this.mprDrag || this.mprDrag.pointerId !== event.pointerId) return;
    if (this.mprDrag.kind === 'length' && this.mprDraft?.plane === plane) {
      const frame = this.mprFrame(plane);
      const viewport = this.mpr.viewports[plane];
      const start = this.mprRenderers[plane].toScreenFor(
        this.mprDraft.measurement.start,
        frame,
        viewport,
      );
      const end = this.mprRenderers[plane].toScreenFor(
        this.mprDraft.measurement.end,
        frame,
        viewport,
      );
      if (Math.hypot(end.x - start.x, end.y - start.y) >= 4) {
        const list = this.currentMprMeasurements(plane);
        list.push(this.mprDraft.measurement);
        this.mprMeasurements.set(this.mprMeasurementKey(plane), list);
        this.selectedMeasurementId = this.mprDraft.measurement.id;
      }
      this.mprDraft = null;
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

  private mprHitTest(plane: MprPlane, screenPoint: Point): LengthMeasurement | null {
    if (!this.mpr) return null;
    const renderer = this.mprRenderers[plane];
    const frame = this.mprFrame(plane);
    const viewport = this.mpr.viewports[plane];
    for (const measurement of [...this.currentMprMeasurements(plane)].reverse()) {
      const start = renderer.toScreenFor(measurement.start, frame, viewport);
      const end = renderer.toScreenFor(measurement.end, frame, viewport);
      if (pointToSegmentDistance(screenPoint, start, end) <= 8) return measurement;
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

    const hit = this.hitTest(point);
    if (hit) {
      this.selectedMeasurementId = hit.id;
      this.overlayCanvas.releasePointerCapture(event.pointerId);
      this.render();
      return;
    }
    const imagePoint = clampToImage(this.renderer.toImage(point, this.state), imageGeometry(this.currentFrame()));
    this.draft = {
      id: makeId(),
      start: imagePoint,
      end: imagePoint,
    };
    this.selectedMeasurementId = null;
    this.drag = { kind: 'length', pointerId: event.pointerId };
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
    if (this.draft) {
      this.draft.end = clampToImage(
        this.renderer.toImage(point, this.state),
        imageGeometry(this.currentFrame()),
      );
      this.render();
    }
  }

  private pointerUp(event: PointerEvent): void {
    if (!this.state || !this.drag || this.drag.pointerId !== event.pointerId) return;
    if (this.drag.kind === 'length' && this.draft) {
      const start = this.renderer.toScreen(this.draft.start, this.state);
      const end = this.renderer.toScreen(this.draft.end, this.state);
      if (Math.hypot(end.x - start.x, end.y - start.y) >= 4) {
        const list = this.currentMeasurements();
        list.push(this.draft);
        this.measurements.set(this.currentFrame().frame_key, list);
        this.selectedMeasurementId = this.draft.id;
      }
      this.draft = null;
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
    } else if ((event.key === 'Delete' || event.key === 'Backspace') && this.selectedMeasurementId) {
      event.preventDefault();
      if (this.mpr && this.viewerMode === 'mpr') {
        const plane = this.mpr.activePlane;
        const remaining = this.currentMprMeasurements(plane).filter(
          (measurement) => measurement.id !== this.selectedMeasurementId,
        );
        this.mprMeasurements.set(this.mprMeasurementKey(plane), remaining);
      } else {
        const remaining = this.currentMeasurements().filter(
          (measurement) => measurement.id !== this.selectedMeasurementId,
        );
        this.measurements.set(this.currentFrame().frame_key, remaining);
      }
      this.selectedMeasurementId = null;
      this.render();
      this.updateUi();
    } else if (event.key === 'Escape') {
      this.draft = null;
      this.mprDraft = null;
      this.selectedMeasurementId = null;
      this.render();
    }
  }

  private hitTest(screenPoint: Point): LengthMeasurement | null {
    if (!this.state) return null;
    for (const measurement of [...this.currentMeasurements()].reverse()) {
      const start = this.renderer.toScreen(measurement.start, this.state);
      const end = this.renderer.toScreen(measurement.end, this.state);
      if (pointToSegmentDistance(screenPoint, start, end) <= 8) return measurement;
    }
    return null;
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
      `${this.currentMprMeasurements(this.mpr.activePlane).length} 项标注`,
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
    this.updateMprLayout();
    if (!this.state) return;

    const frame = this.currentFrame();
    const total = frameCount;
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
      setText('annotation-count', `${this.currentMprMeasurements(this.mpr.activePlane).length} 项标注`);
    } else {
      setText('annotation-count', `${this.currentMeasurements().length} 项标注`);
    }
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
    const previousCount = this.presetSelect.options.length;
    const signature = frame.window_presets.map(presetSignature).join('|');
    if (this.presetSelect.dataset.signature !== signature || previousCount === 0) {
      this.presetSelect.replaceChildren();
      frame.window_presets.forEach((preset, index) => {
        const option = document.createElement('option');
        option.value = String(index);
        option.textContent = preset.explanation?.trim() || `窗 ${index + 1}`;
        this.presetSelect.append(option);
      });
      this.presetSelect.dataset.signature = signature;
    }
    const match = frame.window_presets.findIndex(
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

  private currentMeasurements(): LengthMeasurement[] {
    if (!this.state) return [];
    return this.measurements.get(this.currentFrame().frame_key) ?? [];
  }

  private mprMeasurementKey(plane: MprPlane): string {
    if (!this.mpr) return `mpr:${plane}:0`;
    return `mpr:${plane}:${this.mpr.viewports[plane].sliceIndex}`;
  }

  private currentMprMeasurements(plane: MprPlane): LengthMeasurement[] {
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

function makeId(): string {
  return globalThis.crypto?.randomUUID?.() ?? `measurement-${Date.now()}-${Math.random()}`;
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

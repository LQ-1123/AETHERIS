import {
  ClampToEdgeWrapping,
  Color,
  Data3DTexture,
  DoubleSide,
  GLSL3,
  LinearFilter,
  Mesh,
  OrthographicCamera,
  PlaneGeometry,
  RedFormat,
  RGFormat,
  Scene,
  ShaderMaterial,
  UnsignedByteType,
  UnsignedShortType,
  Vector2,
  Vector3,
  WebGLRenderer,
} from 'three';
import { physicalSpacingAlong, type VolumeGeometry } from './patient-space';
import type { MprMetadata, MprPlaneMetadata, MprProjectionMode, ViewTransform, VoiFunction } from './types';

export interface GpuMprSettings {
  windowCenter: number;
  windowWidth: number;
  inverted: boolean;
  projection: MprProjectionMode;
  slabThicknessMm: number;
  voiFunction: VoiFunction;
}

const VOI_INDEX: Record<VoiFunction, number> = {
  LINEAR: 0,
  LINEAR_EXACT: 1,
  SIGMOID: 2,
};

const PROJECTION_INDEX: Record<MprProjectionMode, number> = {
  slice: 0,
  mip: 1,
  minip: 2,
};

export class GpuMprRenderer {
  private readonly renderer: WebGLRenderer;
  private readonly scene = new Scene();
  private readonly camera = new OrthographicCamera(-1, 1, 1, -1, 0, 1);
  private readonly geometry = new PlaneGeometry(2, 2);
  private readonly material: ShaderMaterial;
  private readonly mesh: Mesh;
  private readonly texture: Data3DTexture;
  private readonly volumeGeometry: VolumeGeometry;
  private disposed = false;

  constructor(
    data: ArrayBuffer,
    mpr: MprMetadata,
    settings: GpuMprSettings,
  ) {
    const canvas = document.createElement('canvas');
    const context = canvas.getContext('webgl2', { antialias: true, alpha: false, preserveDrawingBuffer: true });
    if (!context) throw new Error('当前显卡或 WebView 不支持 WebGL2');
    this.renderer = new WebGLRenderer({ canvas, context, antialias: true, alpha: false, preserveDrawingBuffer: true });
    this.renderer.setClearColor(new Color('#050607'), 1);
    this.renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));

    const volume = mpr.volume_rendering;
    if (data.byteLength !== volume.byte_length) {
      throw new Error(`体纹理长度异常: 收到 ${data.byteLength} 字节，预期 ${volume.byte_length} 字节`);
    }
    this.volumeGeometry = {
      origin: mpr.source_origin,
      xAxis: mpr.source_x_axis,
      yAxis: mpr.source_y_axis,
      normal: mpr.source_normal,
      spacingMm: mpr.source_spacing_mm,
      dimensions: mpr.dimensions,
    };
    const [width, height, depth] = volume.dimensions;
    const supportsNormalized16 = context.getExtension('EXT_texture_norm16') != null;
    const source = new Uint16Array(data);
    let textureData: Uint16Array | Uint8Array;
    let textureFormat: typeof RedFormat | typeof RGFormat;
    let packed8 = false;
    if (supportsNormalized16) {
      textureData = source;
      textureFormat = RedFormat;
    } else {
      // 保留 16-bit 精度：高/低字节分两个 8-bit 通道上传。
      packed8 = true;
      textureData = new Uint8Array(width * height * depth * 2);
      for (let index = 0; index < source.length; index += 1) {
        const value = source[index];
        textureData[index * 2] = value >> 8;
        textureData[index * 2 + 1] = value & 0xff;
      }
      textureFormat = RGFormat;
    }
    this.texture = new Data3DTexture(textureData, width, height, depth);
    this.texture.format = textureFormat;
    this.texture.type = supportsNormalized16 ? UnsignedShortType : UnsignedByteType;
    this.texture.normalized = true;
    this.texture.minFilter = LinearFilter;
    this.texture.magFilter = LinearFilter;
    this.texture.wrapS = ClampToEdgeWrapping;
    this.texture.wrapT = ClampToEdgeWrapping;
    this.texture.wrapR = ClampToEdgeWrapping;
    this.texture.unpackAlignment = 1;
    this.texture.needsUpdate = true;

    this.material = new ShaderMaterial({
      glslVersion: GLSL3,
      side: DoubleSide,
      depthWrite: false,
      uniforms: {
        uVolume: { value: this.texture },
        uVolumeDimensions: { value: new Vector3(width, height, depth) },
        uSourceOrigin: { value: new Vector3(...mpr.source_origin) },
        uSourceXAxis: { value: new Vector3(...mpr.source_x_axis) },
        uSourceYAxis: { value: new Vector3(...mpr.source_y_axis) },
        uSourceNormal: { value: new Vector3(...mpr.source_normal) },
        uSourceSpacing: { value: new Vector3(...mpr.source_spacing_mm) },
        uPlaneOrigin: { value: new Vector3(0, 0, 0) },
        uPlaneXAxis: { value: new Vector3(1, 0, 0) },
        uPlaneYAxis: { value: new Vector3(0, 1, 0) },
        uPlaneNormal: { value: new Vector3(0, 0, 1) },
        uImageSize: { value: new Vector2(1, 1) },
        uSpacingX: { value: 1 },
        uSpacingY: { value: 1 },
        uPixelAspect: { value: 1 },
        uNormalSpacingMm: { value: 1 },
        uPacked8: { value: packed8 ? 1 : 0 },
        uViewportSize: { value: new Vector2(1, 1) },
        uCenter: { value: new Vector2(0, 0) },
        uScale: { value: 1 },
        uRotationRadians: { value: 0 },
        uFlip: { value: new Vector2(1, 1) },
        uValueMin: { value: volume.value_range[0] },
        uValueMax: { value: volume.value_range[1] },
        uWindowCenter: { value: settings.windowCenter },
        uWindowWidth: { value: Math.max(1, settings.windowWidth) },
        uInverted: { value: settings.inverted ? 1 : 0 },
        uProjection: { value: PROJECTION_INDEX[settings.projection] },
        uSlabThicknessMm: { value: Math.max(0.5, settings.slabThicknessMm) },
        uVoiFunction: { value: VOI_INDEX[settings.voiFunction] },
      },
      vertexShader: VERTEX_SHADER,
      fragmentShader: FRAGMENT_SHADER,
    });
    this.mesh = new Mesh(this.geometry, this.material);
    this.mesh.frustumCulled = false;
    this.scene.add(this.mesh);
  }

  resize(width: number, height: number): void {
    if (this.disposed) return;
    const safeWidth = Math.max(1, Math.round(width));
    const safeHeight = Math.max(1, Math.round(height));
    this.renderer.setSize(safeWidth, safeHeight, false);
    this.renderer.domElement.style.width = `${safeWidth}px`;
    this.renderer.domElement.style.height = `${safeHeight}px`;
    (this.material.uniforms.uViewportSize.value as Vector2).set(safeWidth, safeHeight);
  }

  setPlane(plane: MprPlaneMetadata): void {
    (this.material.uniforms.uPlaneOrigin.value as Vector3).set(...plane.origin);
    (this.material.uniforms.uPlaneXAxis.value as Vector3).set(...plane.x_axis);
    (this.material.uniforms.uPlaneYAxis.value as Vector3).set(...plane.y_axis);
    (this.material.uniforms.uPlaneNormal.value as Vector3).set(...plane.normal);
    (this.material.uniforms.uImageSize.value as Vector2).set(plane.cols, plane.rows);
    const spacingX = plane.spacing_x_mm ?? plane.pixel_spacing_mm;
    const spacingY = plane.spacing_y_mm ?? plane.pixel_spacing_mm;
    this.material.uniforms.uSpacingX.value = spacingX;
    this.material.uniforms.uSpacingY.value = spacingY;
    this.material.uniforms.uPixelAspect.value = Math.max(1e-6, spacingX / Math.max(1e-6, spacingY));
    this.material.uniforms.uNormalSpacingMm.value = Math.max(
      1e-6,
      physicalSpacingAlong(plane.normal, this.volumeGeometry),
    );
  }

  setView(view: ViewTransform, viewportWidth: number, viewportHeight: number): void {
    const safeWidth = Math.max(1, viewportWidth);
    const safeHeight = Math.max(1, viewportHeight);
    const imageSize = this.material.uniforms.uImageSize.value as Vector2;
    const cols = Math.max(1, imageSize.x);
    const rows = Math.max(1, imageSize.y);
    const pixelAspect = this.material.uniforms.uPixelAspect.value as number;
    const quarterTurn = view.rotation === 90 || view.rotation === 270;
    const sourceWidth = cols * pixelAspect;
    const displayWidth = quarterTurn ? rows : sourceWidth;
    const displayHeight = quarterTurn ? sourceWidth : rows;
    const fitScale = Math.min(safeWidth / displayWidth, safeHeight / displayHeight);
    const scale = Math.max(0.0001, fitScale * view.zoom);
    const centerX = safeWidth / 2 + view.panX;
    const centerY = safeHeight / 2 + view.panY;
    (this.material.uniforms.uViewportSize.value as Vector2).set(safeWidth, safeHeight);
    (this.material.uniforms.uCenter.value as Vector2).set(centerX, centerY);
    this.material.uniforms.uScale.value = scale;
    this.material.uniforms.uRotationRadians.value = view.rotation * Math.PI / 180;
    (this.material.uniforms.uFlip.value as Vector2).set(
      view.flipHorizontal ? -1 : 1,
      view.flipVertical ? -1 : 1,
    );
  }

  setWindow(center: number, width: number): void {
    this.material.uniforms.uWindowCenter.value = center;
    this.material.uniforms.uWindowWidth.value = Math.max(1, width);
  }

  setInverted(inverted: boolean): void {
    this.material.uniforms.uInverted.value = inverted ? 1 : 0;
  }

  setVoiFunction(voiFunction: VoiFunction): void {
    this.material.uniforms.uVoiFunction.value = VOI_INDEX[voiFunction];
  }

  setProjection(projection: MprProjectionMode, slabThicknessMm: number): void {
    this.material.uniforms.uProjection.value = PROJECTION_INDEX[projection];
    this.material.uniforms.uSlabThicknessMm.value = Math.max(0.5, slabThicknessMm);
  }

  render(): void {
    if (this.disposed) return;
    this.renderer.render(this.scene, this.camera);
  }

  getCanvas(): HTMLCanvasElement {
    return this.renderer.domElement;
  }

  drawTo(context: CanvasRenderingContext2D, width: number, height: number): void {
    if (this.disposed) return;
    context.drawImage(this.renderer.domElement, 0, 0, width, height);
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.scene.remove(this.mesh);
    this.geometry.dispose();
    this.material.dispose();
    this.texture.dispose();
    this.renderer.dispose();
  }
}

const VERTEX_SHADER = `
  out vec2 vUv;

  void main() {
    vUv = uv;
    gl_Position = vec4(position.xy, 0.0, 1.0);
  }
`;

const FRAGMENT_SHADER = `
  precision highp float;
  precision highp sampler3D;

  in vec2 vUv;
  uniform sampler3D uVolume;
  uniform vec3 uVolumeDimensions;
  uniform vec3 uSourceOrigin;
  uniform vec3 uSourceXAxis;
  uniform vec3 uSourceYAxis;
  uniform vec3 uSourceNormal;
  uniform vec3 uSourceSpacing;
  uniform vec3 uPlaneOrigin;
  uniform vec3 uPlaneXAxis;
  uniform vec3 uPlaneYAxis;
  uniform vec3 uPlaneNormal;
  uniform vec2 uImageSize;
  uniform float uSpacingX;
  uniform float uSpacingY;
  uniform float uPixelAspect;
  uniform float uNormalSpacingMm;
  uniform float uPacked8;
  uniform vec2 uViewportSize;
  uniform vec2 uCenter;
  uniform float uScale;
  uniform float uRotationRadians;
  uniform vec2 uFlip;
  uniform float uValueMin;
  uniform float uValueMax;
  uniform float uWindowCenter;
  uniform float uWindowWidth;
  uniform float uInverted;
  uniform float uProjection;
  uniform float uSlabThicknessMm;
  uniform float uVoiFunction;
  out vec4 outColor;

  vec2 rotate(vec2 value, float angle) {
    float c = cos(angle);
    float s = sin(angle);
    return vec2(value.x * c - value.y * s, value.x * s + value.y * c);
  }

  vec3 patientToVoxel(vec3 patient) {
    vec3 offset = patient - uSourceOrigin;
    return vec3(
      dot(offset, uSourceXAxis) / uSourceSpacing.x,
      dot(offset, uSourceYAxis) / uSourceSpacing.y,
      dot(offset, uSourceNormal) / uSourceSpacing.z
    );
  }

  vec3 voxelToTexCoord(vec3 voxel) {
    return (voxel + 0.5) / uVolumeDimensions;
  }

  float sampleVolume(vec3 texCoord) {
    if (texCoord.x < 0.0 || texCoord.y < 0.0 || texCoord.z < 0.0 ||
        texCoord.x > 1.0 || texCoord.y > 1.0 || texCoord.z > 1.0) {
      return -1.0;
    }
    vec4 texSample = texture(uVolume, texCoord);
    if (uPacked8 > 0.5) {
      // 高字节在 R，低字节在 G，恢复 16-bit 归一化值。
      return dot(texSample.rg, vec2(65280.0, 255.0)) / 65535.0;
    }
    return texSample.r;
  }

  float samplePlane(vec2 imagePoint) {
    vec3 patient = uPlaneOrigin
      + uPlaneXAxis * (imagePoint.x * uSpacingX)
      + uPlaneYAxis * (imagePoint.y * uSpacingY);
    vec3 voxel = patientToVoxel(patient);
    vec3 texCoord = voxelToTexCoord(voxel);
    if (uProjection < 0.5) {
      return sampleVolume(texCoord);
    }
    vec3 normalStep = vec3(
      dot(uPlaneNormal, uSourceXAxis) / (uSourceSpacing.x * uVolumeDimensions.x),
      dot(uPlaneNormal, uSourceYAxis) / (uSourceSpacing.y * uVolumeDimensions.y),
      dot(uPlaneNormal, uSourceNormal) / (uSourceSpacing.z * uVolumeDimensions.z)
    );
    float stepCount = max(1.0, min(512.0, ceil(uSlabThicknessMm / max(1e-6, uNormalSpacingMm))));
    float best = uProjection < 1.5 ? -1.0 : 1.0e30;
    for (int index = 0; index < 512; index += 1) {
      if (float(index) >= stepCount) break;
      float t = stepCount <= 1.0 ? 0.0 : (float(index) / (stepCount - 1.0) - 0.5) * uSlabThicknessMm;
      vec3 sampleTexCoord = texCoord + normalStep * t;
      float value = sampleVolume(sampleTexCoord);
      if (value < 0.0) continue;
      if (uProjection < 1.5) {
        best = max(best, value);
      } else {
        best = min(best, value);
      }
    }
    return best < 0.0 || best > 1.0e29 ? 0.0 : best;
  }

  void main() {
    vec2 screen = vec2(vUv.x * uViewportSize.x, (1.0 - vUv.y) * uViewportSize.y);
    vec2 local = (screen - uCenter) / uScale;
    local = rotate(local, -uRotationRadians);
    local *= uFlip;
    vec2 imagePoint = vec2(local.x / uPixelAspect, local.y) + uImageSize * 0.5;
    float normalized = samplePlane(imagePoint);
    float physical = mix(uValueMin, uValueMax, normalized);
    float gray;
    if (uVoiFunction > 1.5) {
      float x = -4.0 * (physical - uWindowCenter) / uWindowWidth;
      gray = 1.0 / (1.0 + exp(x));
    } else {
      gray = clamp(
        (physical - (uWindowCenter - uWindowWidth * 0.5)) / uWindowWidth,
        0.0,
        1.0
      );
    }
    if (uInverted > 0.5) gray = 1.0 - gray;
    outColor = vec4(vec3(gray), 1.0);
  }
`;

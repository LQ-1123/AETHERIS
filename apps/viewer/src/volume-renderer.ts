import {
  BackSide,
  BoxGeometry,
  Color,
  Data3DTexture,
  GLSL3,
  LinearFilter,
  Mesh,
  MOUSE,
  PerspectiveCamera,
  RedFormat,
  Scene,
  ShaderMaterial,
  UnsignedByteType,
  UnsignedShortType,
  Vector3,
  WebGLRenderer,
} from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import type { VolumeRenderingMetadata } from './types';

export type VolumePreset =
  | 'grayscale'
  | 'soft_tissue'
  | 'bone'
  | 'bone_color'
  | 'lung'
  | 'pet';
export type VolumeQuality = 'low' | 'medium' | 'high';

export interface VolumeRenderSettings {
  windowCenter: number;
  windowWidth: number;
  preset: VolumePreset;
  quality: VolumeQuality;
}

const QUALITY_STEPS: Record<VolumeQuality, number> = {
  low: 128,
  medium: 256,
  high: 512,
};

const PRESET_INDEX: Record<VolumePreset, number> = {
  grayscale: 0,
  soft_tissue: 1,
  bone: 2,
  bone_color: 3,
  lung: 4,
  pet: 5,
};

export class VolumeRenderer {
  private readonly renderer: WebGLRenderer;
  private readonly scene = new Scene();
  private readonly camera = new PerspectiveCamera(36, 1, 0.01, 100);
  private readonly controls: OrbitControls;
  private readonly texture: Data3DTexture;
  private readonly geometry = new BoxGeometry(1, 1, 1);
  private readonly material: ShaderMaterial;
  private readonly mesh: Mesh;
  private animationFrame: number | null = null;
  private disposed = false;

  constructor(
    canvas: HTMLCanvasElement,
    data: ArrayBuffer,
    metadata: VolumeRenderingMetadata,
    settings: VolumeRenderSettings,
  ) {
    if (data.byteLength !== metadata.byte_length) {
      throw new Error(`体纹理长度异常: 收到 ${data.byteLength} 字节，预期 ${metadata.byte_length} 字节`);
    }
    const context = canvas.getContext('webgl2', { antialias: true, alpha: false });
    if (!context) throw new Error('当前显卡或 WebView 不支持 WebGL2');
    this.renderer = new WebGLRenderer({ canvas, context, antialias: true, alpha: false });
    this.renderer.setClearColor(new Color('#080b0d'), 1);
    this.renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));

    const [width, height, depth] = metadata.dimensions;
    const supportsNormalized16 = context.getExtension('EXT_texture_norm16') != null;
    const source = new Uint16Array(data);
    const textureData = supportsNormalized16
      ? source
      : Uint8Array.from(source, (value) => Math.round(value / 257));
    this.texture = new Data3DTexture(textureData, width, height, depth);
    this.texture.format = RedFormat;
    this.texture.type = supportsNormalized16 ? UnsignedShortType : UnsignedByteType;
    this.texture.normalized = supportsNormalized16;
    this.texture.minFilter = LinearFilter;
    this.texture.magFilter = LinearFilter;
    this.texture.unpackAlignment = 1;
    this.texture.needsUpdate = true;

    this.material = new ShaderMaterial({
      glslVersion: GLSL3,
      side: BackSide,
      transparent: true,
      depthWrite: false,
      uniforms: {
        uVolume: { value: this.texture },
        uValueMin: { value: metadata.value_range[0] },
        uValueMax: { value: metadata.value_range[1] },
        uWindowCenter: { value: settings.windowCenter },
        uWindowWidth: { value: Math.max(1, settings.windowWidth) },
        uPreset: { value: PRESET_INDEX[settings.preset] },
        uSteps: { value: QUALITY_STEPS[settings.quality] },
      },
      vertexShader: VERTEX_SHADER,
      fragmentShader: FRAGMENT_SHADER,
    });
    this.mesh = new Mesh(this.geometry, this.material);
    const physical = new Vector3(
      width * metadata.spacing_mm[0],
      height * metadata.spacing_mm[1],
      depth * metadata.spacing_mm[2],
    );
    const longest = Math.max(physical.x, physical.y, physical.z, 1);
    this.mesh.scale.copy(physical.multiplyScalar(1 / longest));
    this.scene.add(this.mesh);

    this.camera.position.set(1.45, 1.1, 1.65);
    this.camera.lookAt(0, 0, 0);
    this.controls = new OrbitControls(this.camera, canvas);
    // 左键交给业务层做窗宽窗位；右键旋转；中键平移；滚轮保持缩放。
    this.controls.mouseButtons = {
      LEFT: null,
      MIDDLE: MOUSE.PAN,
      RIGHT: MOUSE.ROTATE,
    };
    this.controls.enableDamping = true;
    this.controls.dampingFactor = 0.08;
    this.controls.minDistance = 0.9;
    this.controls.maxDistance = 5;
    this.controls.target.set(0, 0, 0);
    this.controls.update();
    this.animate();
  }

  resize(width: number, height: number): void {
    if (this.disposed) return;
    const safeWidth = Math.max(1, Math.round(width));
    const safeHeight = Math.max(1, Math.round(height));
    this.renderer.setSize(safeWidth, safeHeight, false);
    this.camera.aspect = safeWidth / safeHeight;
    this.camera.updateProjectionMatrix();
  }

  setWindow(center: number, width: number): void {
    this.material.uniforms.uWindowCenter.value = center;
    this.material.uniforms.uWindowWidth.value = Math.max(1, width);
  }

  setPreset(preset: VolumePreset): void {
    this.material.uniforms.uPreset.value = PRESET_INDEX[preset];
  }

  setQuality(quality: VolumeQuality): void {
    this.material.uniforms.uSteps.value = QUALITY_STEPS[quality];
  }

  resetView(): void {
    this.camera.position.set(1.45, 1.1, 1.65);
    this.controls.target.set(0, 0, 0);
    this.controls.update();
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    if (this.animationFrame != null) cancelAnimationFrame(this.animationFrame);
    this.animationFrame = null;
    this.controls.dispose();
    this.scene.remove(this.mesh);
    this.geometry.dispose();
    this.material.dispose();
    this.texture.dispose();
    this.renderer.dispose();
  }

  private animate = (): void => {
    if (this.disposed) return;
    this.controls.update();
    this.renderer.render(this.scene, this.camera);
    this.animationFrame = requestAnimationFrame(this.animate);
  };
}

const VERTEX_SHADER = `
  out vec3 vOrigin;
  out vec3 vDirection;

  void main() {
    vec4 localCamera = inverse(modelMatrix) * vec4(cameraPosition, 1.0);
    vOrigin = localCamera.xyz;
    vDirection = position - vOrigin;
    gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
  }
`;

const FRAGMENT_SHADER = `
  precision highp float;
  precision highp sampler3D;

  in vec3 vOrigin;
  in vec3 vDirection;
  uniform sampler3D uVolume;
  uniform float uValueMin;
  uniform float uValueMax;
  uniform float uWindowCenter;
  uniform float uWindowWidth;
  uniform float uPreset;
  uniform float uSteps;
  out vec4 outColor;

  vec2 hitBox(vec3 origin, vec3 direction) {
    vec3 inverseDirection = 1.0 / direction;
    vec3 minimum = (-0.5 - origin) * inverseDirection;
    vec3 maximum = (0.5 - origin) * inverseDirection;
    vec3 nearer = min(minimum, maximum);
    vec3 farther = max(minimum, maximum);
    float entry = max(max(nearer.x, nearer.y), nearer.z);
    float exit = min(min(farther.x, farther.y), farther.z);
    return vec2(entry, exit);
  }

  vec3 petColor(float value) {
    vec3 dark = vec3(0.02, 0.0, 0.08);
    vec3 violet = vec3(0.35, 0.02, 0.45);
    vec3 red = vec3(0.92, 0.12, 0.08);
    vec3 yellow = vec3(1.0, 0.86, 0.12);
    if (value < 0.35) return mix(dark, violet, value / 0.35);
    if (value < 0.72) return mix(violet, red, (value - 0.35) / 0.37);
    return mix(red, yellow, (value - 0.72) / 0.28);
  }

  vec3 boneColor(float physical) {
    if (physical < 120.0) {
      return mix(vec3(0.10, 0.30, 0.33), vec3(0.28, 0.62, 0.64), smoothstep(40.0, 120.0, physical));
    }
    if (physical < 260.0) {
      return mix(vec3(0.28, 0.62, 0.64), vec3(0.52, 0.28, 0.15), smoothstep(120.0, 260.0, physical));
    }
    if (physical < 500.0) {
      return mix(vec3(0.52, 0.28, 0.15), vec3(0.72, 0.46, 0.25), smoothstep(260.0, 500.0, physical));
    }
    if (physical < 900.0) {
      return mix(vec3(0.72, 0.46, 0.25), vec3(0.91, 0.76, 0.53), smoothstep(500.0, 900.0, physical));
    }
    return mix(vec3(0.91, 0.76, 0.53), vec3(1.0, 0.94, 0.80), smoothstep(900.0, 1800.0, physical));
  }

  float boneAlpha(float physical) {
    if (physical < 80.0) return 0.0;
    if (physical < 160.0) return smoothstep(80.0, 160.0, physical) * 0.035;
    if (physical < 280.0) return smoothstep(160.0, 280.0, physical) * 0.075;
    if (physical < 520.0) return smoothstep(280.0, 520.0, physical) * 0.15;
    if (physical < 900.0) return smoothstep(520.0, 900.0, physical) * 0.24;
    return mix(0.24, 0.34, smoothstep(900.0, 1800.0, physical));
  }

  vec4 transfer(float normalized) {
    float physical = mix(uValueMin, uValueMax, normalized);
    float value = clamp((physical - (uWindowCenter - uWindowWidth * 0.5)) / uWindowWidth, 0.0, 1.0);
    vec3 color;
    float alpha;
    if (uPreset < 0.5) {
      color = vec3(value);
      alpha = smoothstep(0.08, 0.92, value) * 0.055;
    } else if (uPreset < 1.5) {
      color = mix(vec3(0.18, 0.08, 0.07), vec3(1.0, 0.68, 0.52), value);
      alpha = smoothstep(0.18, 0.78, value) * 0.07;
    } else if (uPreset < 2.5) {
      color = mix(vec3(0.45, 0.24, 0.12), vec3(1.0, 0.96, 0.84), value);
      alpha = smoothstep(0.48, 0.82, value) * 0.12;
    } else if (uPreset < 3.5) {
      color = boneColor(physical);
      alpha = boneAlpha(physical);
      // Keep the HU window useful as a visibility control for the material map.
      alpha *= smoothstep(0.02, 0.22, value);
      alpha *= 1.0 - smoothstep(0.94, 1.0, value);
    } else if (uPreset < 4.5) {
      color = mix(vec3(0.12, 0.28, 0.34), vec3(0.92, 0.96, 0.9), value);
      alpha = smoothstep(0.06, 0.5, value) * (1.0 - smoothstep(0.72, 1.0, value)) * 0.08;
    } else {
      color = petColor(value);
      alpha = smoothstep(0.12, 0.88, value) * 0.1;
    }
    alpha = 1.0 - pow(1.0 - alpha, 256.0 / uSteps);
    return vec4(color, alpha);
  }

  vec3 volumeNormal(vec3 position) {
    vec3 texel = 1.0 / vec3(textureSize(uVolume, 0));
    float x = texture(uVolume, position + vec3(texel.x, 0.0, 0.0)).r
      - texture(uVolume, position - vec3(texel.x, 0.0, 0.0)).r;
    float y = texture(uVolume, position + vec3(0.0, texel.y, 0.0)).r
      - texture(uVolume, position - vec3(0.0, texel.y, 0.0)).r;
    float z = texture(uVolume, position + vec3(0.0, 0.0, texel.z)).r
      - texture(uVolume, position - vec3(0.0, 0.0, texel.z)).r;
    vec3 normal = vec3(x, y, z);
    return length(normal) > 0.0001 ? normalize(normal) : vec3(0.0, 0.0, 1.0);
  }

  void main() {
    vec3 direction = normalize(vDirection);
    vec2 bounds = hitBox(vOrigin, direction);
    if (bounds.x > bounds.y) discard;
    bounds.x = max(bounds.x, 0.0);
    float distance = bounds.y - bounds.x;
    float stepLength = distance / uSteps;
    vec3 position = vOrigin + (bounds.x + stepLength * 0.5) * direction + 0.5;
    vec3 stepVector = direction * stepLength;
    vec4 accumulated = vec4(0.0);
    for (int index = 0; index < 512; index += 1) {
      if (float(index) >= uSteps || accumulated.a > 0.985) break;
      float sampleValue = texture(uVolume, position).r;
      vec4 sampleColor = transfer(sampleValue);
      if (uPreset > 2.5 && uPreset < 3.5 && sampleColor.a > 0.001) {
        vec3 normal = volumeNormal(position);
        if (dot(normal, direction) > 0.0) normal = -normal;
        vec3 lightDirection = normalize(vec3(-0.45, 0.65, 0.75));
        float diffuse = 0.38 + 0.62 * max(dot(normal, lightDirection), 0.0);
        float specular = pow(max(dot(reflect(-lightDirection, normal), -direction), 0.0), 24.0) * 0.16;
        sampleColor.rgb *= diffuse + specular;
      }
      accumulated.rgb += (1.0 - accumulated.a) * sampleColor.a * sampleColor.rgb;
      accumulated.a += (1.0 - accumulated.a) * sampleColor.a;
      position += stepVector;
    }
    outColor = accumulated;
  }
`;

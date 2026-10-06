import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import type { ModelMetadata, ModelPacketData, ResolvedModelTexture } from "./contracts";

export type ModelView = "auto" | "isometric" | "front" | "back" | "left" | "right" | "top";

export interface ModelMaterialResolution {
  shaderIndex: number;
  texture: ResolvedModelTexture | null;
}

export interface BuiltModelObject {
  group: THREE.Group;
  materials: Map<number, THREE.MeshBasicMaterial>;
  textures: THREE.DataTexture[];
}

export function buildModelObject(
  packet: ModelPacketData,
  resolutions: ModelMaterialResolution[],
  wireframe = false,
): BuiltModelObject {
  const group = new THREE.Group();
  const materials = new Map<number, THREE.MeshBasicMaterial>();
  const textures: THREE.DataTexture[] = [];

  for (const resolution of resolutions) {
    const texture = resolution.texture ? makeResolvedTexture(resolution.texture) : null;
    if (texture) textures.push(texture);
    materials.set(
      resolution.shaderIndex,
      new THREE.MeshBasicMaterial({
        color: 0xffffff,
        map: texture,
        side: THREE.DoubleSide,
        wireframe,
      }),
    );
  }

  for (const primitive of packet.metadata.primitives) {
    if (primitive.topology !== "triangleList") continue;

    const geometry = new THREE.BufferGeometry();
    const positionStart = primitive.positionFloatOffset;
    const positionEnd = positionStart + primitive.vertexCount * 3;
    geometry.setAttribute(
      "position",
      new THREE.BufferAttribute(packet.positions.subarray(positionStart, positionEnd), 3),
    );

    if (primitive.normalFloatOffset !== null) {
      const normalStart = primitive.normalFloatOffset;
      const normalEnd = normalStart + primitive.vertexCount * 3;
      geometry.setAttribute(
        "normal",
        new THREE.BufferAttribute(packet.normals.subarray(normalStart, normalEnd), 3),
      );
    }

    if (primitive.uvFloatOffset !== null) {
      const uvStart = primitive.uvFloatOffset;
      const uvEnd = uvStart + primitive.vertexCount * 2;
      geometry.setAttribute(
        "uv",
        new THREE.BufferAttribute(packet.uv0.subarray(uvStart, uvEnd), 2),
      );
    }

    const indexStart = primitive.indexOffset;
    const indexEnd = indexStart + primitive.indexCount;
    geometry.setIndex(
      new THREE.BufferAttribute(packet.indices.subarray(indexStart, indexEnd), 1),
    );
    geometry.computeBoundingSphere();

    let material =
      primitive.shaderIndex !== null ? materials.get(primitive.shaderIndex) : undefined;
    if (!material) {
      material = new THREE.MeshBasicMaterial({
        color: 0xb7c1d1,
        side: THREE.DoubleSide,
        wireframe,
      });
      materials.set(-(primitive.index + 1), material);
    }

    const mesh = new THREE.Mesh(geometry, material);
    mesh.name = `primitive-${primitive.index}`;
    mesh.userData.primitiveIndex = primitive.index;
    mesh.userData.shaderIndex = primitive.shaderIndex;
    group.add(mesh);
  }

  return { group, materials, textures };
}

export function disposeModelObject(object: BuiltModelObject): void {
  object.group.traverse((child) => {
    if (child instanceof THREE.Mesh) child.geometry.dispose();
  });
  for (const material of object.materials.values()) material.dispose();
  for (const texture of object.textures) texture.dispose();
}

function makeResolvedTexture(resolved: ResolvedModelTexture): THREE.DataTexture {
  const { width, height } = resolved.metadata;
  const texture = new THREE.DataTexture(
    Uint8Array.from(resolved.rgba),
    width,
    height,
    THREE.RGBAFormat,
    THREE.UnsignedByteType,
  );
  texture.colorSpace = THREE.SRGBColorSpace;
  texture.flipY = false;
  texture.needsUpdate = true;
  return texture;
}

export class ModelViewer {
  readonly canvas: HTMLCanvasElement;

  private readonly renderer: THREE.WebGLRenderer;
  private readonly scene = new THREE.Scene();
  private readonly camera = new THREE.PerspectiveCamera(45, 1, 0.01, 100_000);
  private readonly controls: OrbitControls;
  private readonly root = new THREE.Group();
  private readonly resizeObserver: ResizeObserver;
  private grid: THREE.GridHelper | null = null;
  private boundsHelper: THREE.Box3Helper | null = null;
  private metadata: ModelMetadata | null = null;
  private builtObject: BuiltModelObject | null = null;
  private gridVisible = true;
  private wireframe = false;
  private boundsVisible = false;
  private turntable = false;
  private lastFrame = performance.now();
  private animationFrame = 0;

  constructor(private readonly host: HTMLElement) {
    this.canvas = document.createElement("canvas");
    this.canvas.id = "model-canvas";
    this.canvas.dataset.renderer = "three";
    this.host.replaceChildren(this.canvas);

    this.renderer = new THREE.WebGLRenderer({
      canvas: this.canvas,
      antialias: true,
      preserveDrawingBuffer: true,
      alpha: false,
    });
    this.renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    this.renderer.outputColorSpace = THREE.SRGBColorSpace;
    this.scene.background = new THREE.Color(0x111720);
    this.scene.add(this.root);

    this.camera.up.set(0, 0, 1);
    this.controls = new OrbitControls(this.camera, this.canvas);
    this.controls.enableDamping = true;
    this.controls.dampingFactor = 0.08;

    this.resizeObserver = new ResizeObserver(() => this.resize());
    this.resizeObserver.observe(this.host);
    this.resize();
    this.animate();
  }

  setModel(packet: ModelPacketData, resolutions: ModelMaterialResolution[]): void {
    this.clearModel();
    this.metadata = packet.metadata;
    this.builtObject = buildModelObject(packet, resolutions, this.wireframe);
    this.root.add(this.builtObject.group);
    this.rebuildHelpers();
    this.setView("auto");
  }

  setView(view: ModelView): void {
    if (!this.metadata) return;
    const center = new THREE.Vector3(...this.metadata.bounds.center);
    const radius = Math.max(this.metadata.bounds.radius, 0.01);
    const fov = THREE.MathUtils.degToRad(this.camera.fov);
    const distance = Math.max(radius * 2, radius / Math.tan(fov * 0.5)) * 1.25;

    const directions: Record<ModelView, THREE.Vector3> = {
      auto: new THREE.Vector3(1, -1, 0.78),
      isometric: new THREE.Vector3(1, -1, 0.78),
      front: new THREE.Vector3(0, -1, 0),
      back: new THREE.Vector3(0, 1, 0),
      left: new THREE.Vector3(-1, 0, 0),
      right: new THREE.Vector3(1, 0, 0),
      top: new THREE.Vector3(0, 0, 1),
    };
    const direction = directions[view].clone().normalize();

    this.camera.up.set(0, 0, 1);
    if (view === "top") this.camera.up.set(0, 1, 0);
    this.camera.position.copy(center).addScaledVector(direction, distance);
    this.camera.near = Math.max(radius * 0.005, 0.001);
    this.camera.far = Math.max(distance + radius * 8, 100);
    this.camera.updateProjectionMatrix();
    this.controls.target.copy(center);
    this.controls.update();
  }

  setGrid(visible: boolean): void {
    this.gridVisible = visible;
    if (this.grid) this.grid.visible = visible;
  }

  setWireframe(enabled: boolean): void {
    this.wireframe = enabled;
    for (const material of this.builtObject?.materials.values() ?? []) {
      material.wireframe = enabled;
      material.needsUpdate = true;
    }
  }

  setBounds(visible: boolean): void {
    this.boundsVisible = visible;
    if (this.boundsHelper) this.boundsHelper.visible = visible;
  }

  setTurntable(enabled: boolean): void {
    this.turntable = enabled;
  }

  async screenshotPng(): Promise<Blob> {
    this.render();
    return new Promise<Blob>((resolve, reject) => {
      this.canvas.toBlob((blob) => {
        if (blob) resolve(blob);
        else reject(new Error("Browser could not encode the model canvas as PNG"));
      }, "image/png");
    });
  }

  dispose(): void {
    cancelAnimationFrame(this.animationFrame);
    this.resizeObserver.disconnect();
    this.controls.dispose();
    this.clearModel();
    this.renderer.dispose();
  }

  private rebuildHelpers(): void {
    if (!this.metadata) return;

    const radius = Math.max(this.metadata.bounds.radius, 0.1);
    const gridSize = Math.max(2, Math.ceil(radius * 4));
    const divisions = Math.min(100, Math.max(10, Math.ceil(gridSize)));
    this.grid = new THREE.GridHelper(gridSize, divisions);
    this.grid.rotation.x = Math.PI / 2;
    this.grid.position.z = this.metadata.bounds.min[2];
    this.grid.visible = this.gridVisible;
    this.scene.add(this.grid);

    const min = new THREE.Vector3(...this.metadata.bounds.min);
    const max = new THREE.Vector3(...this.metadata.bounds.max);
    this.boundsHelper = new THREE.Box3Helper(new THREE.Box3(min, max), 0xf2b84b);
    this.boundsHelper.visible = this.boundsVisible;
    this.scene.add(this.boundsHelper);
  }

  private clearModel(): void {
    if (this.builtObject) {
      this.root.remove(this.builtObject.group);
      disposeModelObject(this.builtObject);
      this.builtObject = null;
    }

    if (this.grid) {
      this.scene.remove(this.grid);
      this.grid.geometry.dispose();
      if (Array.isArray(this.grid.material)) {
        for (const material of this.grid.material) material.dispose();
      } else {
        this.grid.material.dispose();
      }
      this.grid = null;
    }
    if (this.boundsHelper) {
      this.scene.remove(this.boundsHelper);
      this.boundsHelper.geometry.dispose();
      if (Array.isArray(this.boundsHelper.material)) {
        for (const material of this.boundsHelper.material) material.dispose();
      } else {
        this.boundsHelper.material.dispose();
      }
      this.boundsHelper = null;
    }
    this.metadata = null;
    this.root.rotation.set(0, 0, 0);
  }

  private resize(): void {
    const width = Math.max(1, this.host.clientWidth);
    const height = Math.max(1, this.host.clientHeight);
    this.renderer.setSize(width, height, false);
    this.camera.aspect = width / height;
    this.camera.updateProjectionMatrix();
  }

  private animate = (): void => {
    this.animationFrame = requestAnimationFrame(this.animate);
    const now = performance.now();
    const deltaSeconds = Math.min(0.1, (now - this.lastFrame) / 1000);
    this.lastFrame = now;
    if (this.turntable) this.root.rotation.z += deltaSeconds * 0.6;
    this.controls.update();
    this.render();
  };

  private render(): void {
    this.renderer.render(this.scene, this.camera);
  }
}

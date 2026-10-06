import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import {
  buildModelObject,
  disposeModelObject,
  type BuiltModelObject,
  type ModelMaterialResolution,
  type ModelView,
} from "./model-viewer";
import type { ModelPacketData, SuppliedSceneEntity } from "./contracts";

export interface YmapRenderableEntity {
  entity: SuppliedSceneEntity;
  packet: ModelPacketData;
  materials: ModelMaterialResolution[];
}

interface SceneObject {
  entityIndex: number;
  built: BuiltModelObject;
  root: THREE.Group;
}

export interface SceneBoundsSnapshot {
  empty: boolean;
  min: [number, number, number] | null;
  max: [number, number, number] | null;
  center: [number, number, number] | null;
  radius: number;
}

function disposeMaterial(material: THREE.Material | THREE.Material[]): void {
  if (Array.isArray(material)) {
    for (const item of material) item.dispose();
  } else {
    material.dispose();
  }
}

export class YmapSceneViewer {
  readonly canvas: HTMLCanvasElement;

  private readonly renderer: THREE.WebGLRenderer;
  private readonly scene = new THREE.Scene();
  private readonly camera = new THREE.PerspectiveCamera(45, 1, 0.01, 1_000_000);
  private readonly controls: OrbitControls;
  private readonly world = new THREE.Group();
  private readonly resizeObserver: ResizeObserver;
  private readonly raycaster = new THREE.Raycaster();
  private readonly pointer = new THREE.Vector2();
  private objects: SceneObject[] = [];
  private grid: THREE.GridHelper | null = null;
  private boundsHelper: THREE.Box3Helper | null = null;
  private selectionHelper: THREE.Box3Helper | null = null;
  private selectedIndex: number | null = null;
  private isolatedIndex: number | null = null;
  private gridVisible = true;
  private wireframe = false;
  private boundsVisible = false;
  private animationFrame = 0;
  private onSelect: ((index: number) => void) | null = null;

  constructor(private readonly host: HTMLElement) {
    this.canvas = document.createElement("canvas");
    this.canvas.id = "ymap-canvas";
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
    this.scene.add(this.world);

    this.camera.up.set(0, 0, 1);
    this.controls = new OrbitControls(this.camera, this.canvas);
    this.controls.enableDamping = true;
    this.controls.dampingFactor = 0.08;

    this.canvas.addEventListener("click", this.pick);
    this.resizeObserver = new ResizeObserver(() => this.resize());
    this.resizeObserver.observe(this.host);
    this.resize();
    this.animate();
  }

  setSelectionCallback(callback: (index: number) => void): void {
    this.onSelect = callback;
  }

  setScene(renderables: YmapRenderableEntity[]): SceneBoundsSnapshot {
    this.clearSceneObjects();

    for (const renderable of renderables) {
      const built = buildModelObject(renderable.packet, renderable.materials, this.wireframe);
      const root = built.group;
      const entity = renderable.entity;
      root.position.set(...entity.position);
      root.quaternion.set(...entity.rotation);
      const scaleXY = entity.scaleXY ?? 1;
      const scaleZ = entity.scaleZ ?? scaleXY;
      root.scale.set(scaleXY, scaleXY, scaleZ);
      root.userData.entityIndex = entity.index;
      root.traverse((child) => {
        child.userData.entityIndex = entity.index;
      });
      this.world.add(root);
      this.objects.push({ entityIndex: entity.index, built, root });
    }

    this.rebuildHelpers();
    this.setView("auto");
    return this.boundsSnapshot();
  }

  selectEntity(index: number | null): void {
    this.selectedIndex = index;
    if (this.selectionHelper) {
      this.scene.remove(this.selectionHelper);
      this.selectionHelper.geometry.dispose();
      disposeMaterial(this.selectionHelper.material);
      this.selectionHelper = null;
    }
    if (index === null) return;
    const object = this.objects.find((candidate) => candidate.entityIndex === index);
    if (!object) return;
    const box = new THREE.Box3().setFromObject(object.root);
    if (box.isEmpty()) return;
    this.selectionHelper = new THREE.Box3Helper(box, 0x55aaff);
    this.scene.add(this.selectionHelper);
  }

  focusEntity(index: number): void {
    const object = this.objects.find((candidate) => candidate.entityIndex === index);
    if (!object) return;
    const box = new THREE.Box3().setFromObject(object.root);
    if (box.isEmpty()) return;
    this.fitBox(box, "auto");
  }

  isolateEntity(index: number | null): void {
    this.isolatedIndex = index;
    for (const object of this.objects) {
      object.root.visible = index === null || object.entityIndex === index;
    }
    this.rebuildHelpers();
    if (index !== null) this.focusEntity(index);
    else this.setView("auto");
  }

  setView(view: ModelView): void {
    const box = this.visibleBounds();
    if (box.isEmpty()) return;
    this.fitBox(box, view);
  }

  setGrid(visible: boolean): void {
    this.gridVisible = visible;
    if (this.grid) this.grid.visible = visible;
  }

  setWireframe(enabled: boolean): void {
    this.wireframe = enabled;
    for (const object of this.objects) {
      for (const material of object.built.materials.values()) {
        material.wireframe = enabled;
        material.needsUpdate = true;
      }
    }
  }

  setBounds(visible: boolean): void {
    this.boundsVisible = visible;
    if (this.boundsHelper) this.boundsHelper.visible = visible;
  }

  boundsSnapshot(): SceneBoundsSnapshot {
    const box = this.visibleBounds();
    if (box.isEmpty()) {
      return { empty: true, min: null, max: null, center: null, radius: 0 };
    }
    const sphere = box.getBoundingSphere(new THREE.Sphere());
    return {
      empty: false,
      min: box.min.toArray() as [number, number, number],
      max: box.max.toArray() as [number, number, number],
      center: sphere.center.toArray() as [number, number, number],
      radius: sphere.radius,
    };
  }

  async screenshotPng(): Promise<Blob> {
    this.render();
    return new Promise<Blob>((resolve, reject) => {
      this.canvas.toBlob((blob) => {
        if (blob) resolve(blob);
        else reject(new Error("Browser could not encode the YMAP canvas as PNG"));
      }, "image/png");
    });
  }

  dispose(): void {
    cancelAnimationFrame(this.animationFrame);
    this.canvas.removeEventListener("click", this.pick);
    this.resizeObserver.disconnect();
    this.controls.dispose();
    this.clearSceneObjects();
    this.disposeHelpers();
    this.renderer.dispose();
  }

  private fitBox(box: THREE.Box3, view: ModelView): void {
    const sphere = box.getBoundingSphere(new THREE.Sphere());
    const radius = Math.max(sphere.radius, 0.1);
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
    this.camera.position.copy(sphere.center).addScaledVector(direction, distance);
    this.camera.near = Math.max(radius * 0.005, 0.001);
    this.camera.far = Math.max(distance + radius * 8, 100);
    this.camera.updateProjectionMatrix();
    this.controls.target.copy(sphere.center);
    this.controls.update();
  }

  private visibleBounds(): THREE.Box3 {
    const box = new THREE.Box3();
    for (const object of this.objects) {
      if (object.root.visible) box.expandByObject(object.root);
    }
    return box;
  }

  private rebuildHelpers(): void {
    this.disposeHelpers();
    const box = this.visibleBounds();
    if (box.isEmpty()) return;
    const sphere = box.getBoundingSphere(new THREE.Sphere());
    const gridSize = Math.max(4, Math.ceil(sphere.radius * 4));
    const divisions = Math.min(200, Math.max(10, Math.ceil(gridSize / 2)));
    this.grid = new THREE.GridHelper(gridSize, divisions);
    this.grid.rotation.x = Math.PI / 2;
    this.grid.position.set(sphere.center.x, sphere.center.y, box.min.z);
    this.grid.visible = this.gridVisible;
    this.scene.add(this.grid);

    this.boundsHelper = new THREE.Box3Helper(box.clone(), 0xf2b84b);
    this.boundsHelper.visible = this.boundsVisible;
    this.scene.add(this.boundsHelper);

    if (this.selectedIndex !== null) this.selectEntity(this.selectedIndex);
  }

  private disposeHelpers(): void {
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
      disposeMaterial(this.boundsHelper.material);
      this.boundsHelper = null;
    }
    if (this.selectionHelper) {
      this.scene.remove(this.selectionHelper);
      this.selectionHelper.geometry.dispose();
      disposeMaterial(this.selectionHelper.material);
      this.selectionHelper = null;
    }
  }

  private clearSceneObjects(): void {
    this.selectedIndex = null;
    this.isolatedIndex = null;
    for (const object of this.objects) {
      this.world.remove(object.root);
      disposeModelObject(object.built);
    }
    this.objects = [];
  }

  private resize(): void {
    const width = Math.max(1, this.host.clientWidth);
    const height = Math.max(1, this.host.clientHeight);
    this.renderer.setSize(width, height, false);
    this.camera.aspect = width / height;
    this.camera.updateProjectionMatrix();
  }

  private pick = (event: MouseEvent): void => {
    if (this.objects.length === 0) return;
    const rect = this.canvas.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return;
    this.pointer.x = ((event.clientX - rect.left) / rect.width) * 2 - 1;
    this.pointer.y = -((event.clientY - rect.top) / rect.height) * 2 + 1;
    this.raycaster.setFromCamera(this.pointer, this.camera);
    const hit = this.raycaster.intersectObjects(this.world.children, true)[0];
    const index = hit?.object.userData.entityIndex;
    if (typeof index === "number") {
      this.selectEntity(index);
      this.onSelect?.(index);
    }
  };

  private animate = (): void => {
    this.animationFrame = requestAnimationFrame(this.animate);
    this.controls.update();
    this.render();
  };

  private render(): void {
    this.renderer.render(this.scene, this.camera);
  }
}

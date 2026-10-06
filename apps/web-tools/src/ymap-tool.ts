import type {
  ModelDependency,
  SuppliedSceneEntity,
  SuppliedYmapSceneReport,
} from "./contracts";
import {
  modelPacket,
  resolveDiffuseTexture,
  resolveSuppliedScene,
  setYmapEntityFlags,
  setYmapEntityTransform,
} from "./core";
import type { ModelMaterialResolution, ModelView } from "./model-viewer";
import {
  YmapSceneViewer,
  type SceneBoundsSnapshot,
  type YmapRenderableEntity,
} from "./ymap-viewer";

const MAX_RENDER_ENTITIES = 512;

interface YmapDocumentState {
  name: string;
  originalBytes: Uint8Array;
  bytes: Uint8Array;
  scene: SuppliedYmapSceneReport;
  dirty: boolean;
  semanticValidated: boolean;
}

export interface YmapToolCallbacks {
  onError(error: unknown): void;
  onSuccess(message: string): void;
}

export interface YmapToolDebugSnapshot {
  name: string | null;
  dependencyCount: number;
  entityCount: number;
  renderedEntityCount: number;
  selectedIndex: number | null;
  diagnosticCodes: string[];
  dirty: boolean;
  semanticValidated: boolean;
  bounds: SceneBoundsSnapshot;
}

function q<T extends Element>(selector: string): T {
  const element = document.querySelector<T>(selector);
  if (!element) throw new Error(`Missing YMAP tool element ${selector}`);
  return element;
}

function extension(name: string): string {
  return name.toLowerCase().split(".").pop() ?? "";
}

function downloadBytes(bytes: Uint8Array, name: string): void {
  const blob = new Blob([Uint8Array.from(bytes)], { type: "application/octet-stream" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = name;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

export function createYmapTool(callbacks: YmapToolCallbacks): {
  openBridgeBundle(name: string, bytes: Uint8Array, suppliedDependencies: ModelDependency[]): void;
  debugSnapshot(): YmapToolDebugSnapshot;
  dispose(): void;
} {
  const dropZone = q<HTMLElement>("#ymap-drop-zone");
  const ymapInput = q<HTMLInputElement>("#ymap-input");
  const dependencyInput = q<HTMLInputElement>("#ymap-dependency-input");
  const fixtureButton = q<HTMLButtonElement>("#ymap-fixture-button");
  const assetName = q<HTMLElement>("#ymap-asset-name");
  const summary = q<HTMLElement>("#ymap-summary");
  const dependencyList = q<HTMLElement>("#ymap-dependency-list");
  const entityList = q<HTMLElement>("#ymap-entity-list");
  const diagnosticsPanel = q<HTMLElement>("#ymap-diagnostics");
  const boundsText = q<HTMLElement>("#ymap-bounds");
  const viewerHost = q<HTMLElement>("#ymap-canvas-shell");
  const viewSelect = q<HTMLSelectElement>("#ymap-view-select");
  const gridToggle = q<HTMLInputElement>("#ymap-grid-toggle");
  const wireToggle = q<HTMLInputElement>("#ymap-wire-toggle");
  const boundsToggle = q<HTMLInputElement>("#ymap-bounds-toggle");
  const focusButton = q<HTMLButtonElement>("#ymap-focus-button");
  const isolateToggle = q<HTMLInputElement>("#ymap-isolate-toggle");
  const inspectorEmpty = q<HTMLElement>("#ymap-inspector-empty");
  const inspector = q<HTMLElement>("#ymap-inspector");
  const inspectorIndex = q<HTMLElement>("#ymap-inspector-index");
  const inspectorArchetype = q<HTMLElement>("#ymap-inspector-archetype");
  const inspectorResolution = q<HTMLElement>("#ymap-inspector-resolution");
  const inspectorParent = q<HTMLElement>("#ymap-inspector-parent");
  const positionX = q<HTMLInputElement>("#ymap-position-x");
  const positionY = q<HTMLInputElement>("#ymap-position-y");
  const positionZ = q<HTMLInputElement>("#ymap-position-z");
  const flagsInput = q<HTMLInputElement>("#ymap-flags");
  const applyButton = q<HTMLButtonElement>("#ymap-apply-button");
  const semanticStatus = q<HTMLElement>("#ymap-semantic-status");
  const downloadButton = q<HTMLButtonElement>("#ymap-download-button");

  const viewer = new YmapSceneViewer(viewerHost);
  let state: YmapDocumentState | null = null;
  let dependencies: ModelDependency[] = [];
  let selectedIndex: number | null = null;
  let renderedEntityCount = 0;
  let bounds: SceneBoundsSnapshot = {
    empty: true,
    min: null,
    max: null,
    center: null,
    radius: 0,
  };
  let extraDiagnosticCodes: string[] = [];

  function selectedEntity(): SuppliedSceneEntity | null {
    if (!state || selectedIndex === null) return null;
    return state.scene.entities.find((entity) => entity.index === selectedIndex) ?? null;
  }

  function renderDependencyList(): void {
    dependencyList.replaceChildren();
    if (dependencies.length === 0) {
      dependencyList.textContent = "No supplied dependencies.";
      return;
    }
    for (const dependency of dependencies) {
      const chip = document.createElement("span");
      chip.className = "dependency-chip";
      chip.textContent = dependency.name;
      dependencyList.append(chip);
    }
  }

  function entityDepth(entity: SuppliedSceneEntity): number {
    if (!state) return 0;
    let depth = 0;
    let parent = entity.parentIndex;
    const seen = new Set<number>();
    while (parent !== null && parent >= 0 && depth < 16 && !seen.has(parent)) {
      seen.add(parent);
      depth += 1;
      parent = state.scene.entities.find((candidate) => candidate.index === parent)?.parentIndex ?? null;
    }
    return depth;
  }

  function renderEntityList(): void {
    entityList.replaceChildren();
    if (!state) return;
    for (const entity of state.scene.entities) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "ymap-entity-row";
      row.dataset.entityIndex = String(entity.index);
      row.classList.toggle("selected", entity.index === selectedIndex);
      row.style.setProperty("--entity-depth", String(entityDepth(entity)));
      const status = entity.resolution
        ? `${entity.resolution.modelFormat.toUpperCase()} · ${entity.resolution.assetHash}`
        : "Unresolved";
      row.innerHTML = "<span></span><span></span>";
      const cells = row.querySelectorAll("span");
      cells[0].textContent = `#${entity.index} · ${entity.archetypeHash}`;
      cells[1].textContent = status;
      row.addEventListener("click", () => selectEntity(entity.index, false));
      entityList.append(row);
    }
  }

  function renderInspector(): void {
    const entity = selectedEntity();
    inspectorEmpty.hidden = entity !== null;
    inspector.hidden = entity === null;
    focusButton.disabled = entity === null || entity?.resolution === null;
    isolateToggle.disabled = entity === null || entity?.resolution === null;
    applyButton.disabled = entity === null;
    if (!entity) return;

    inspectorIndex.textContent = `#${entity.index}`;
    inspectorArchetype.textContent = entity.archetypeHash;
    inspectorResolution.textContent = entity.resolution
      ? `${entity.resolution.modelFormat.toUpperCase()} ${entity.resolution.assetHash}`
      : "Unresolved";
    inspectorParent.textContent =
      entity.parentIndex === null || entity.parentIndex < 0 ? "None" : `#${entity.parentIndex}`;
    positionX.value = String(entity.position[0]);
    positionY.value = String(entity.position[1]);
    positionZ.value = String(entity.position[2]);
    flagsInput.value = String(entity.flags);
  }

  function renderDiagnostics(textureDiagnostics: string[]): void {
    diagnosticsPanel.replaceChildren();
    if (!state) {
      diagnosticsPanel.textContent = "Open a YMAP to resolve supplied dependencies.";
      diagnosticsPanel.classList.add("ok");
      return;
    }

    const messages = [
      ...state.scene.diagnostics.map((diagnostic) =>
        `${diagnostic.code}: ${diagnostic.message}`
      ),
      ...textureDiagnostics,
    ];
    extraDiagnosticCodes = [
      ...state.scene.diagnostics.map((diagnostic) => diagnostic.code),
      ...textureDiagnostics.map(() => "textureDiagnostic"),
    ];
    if (messages.length === 0) {
      diagnosticsPanel.textContent = "No dependency or material diagnostics.";
      diagnosticsPanel.classList.add("ok");
      return;
    }
    diagnosticsPanel.classList.remove("ok");
    const list = document.createElement("ul");
    for (const message of messages) {
      const item = document.createElement("li");
      item.textContent = message;
      list.append(item);
    }
    diagnosticsPanel.append(list);
  }

  function updateBoundsText(): void {
    if (!state) {
      boundsText.textContent = "No scene bounds.";
      return;
    }
    const declaredMin = state.scene.ymap.entitiesExtentsMin;
    const declaredMax = state.scene.ymap.entitiesExtentsMax;
    const rendered = bounds.empty
      ? "rendered: empty"
      : `rendered: [${bounds.min?.map((value) => value.toFixed(2)).join(", ")}] → [${bounds.max?.map((value) => value.toFixed(2)).join(", ")}]`;
    const declared = declaredMin && declaredMax
      ? `declared: [${declaredMin.join(", ")}] → [${declaredMax.join(", ")}]`
      : "declared: unavailable";
    boundsText.textContent = `${declared} · ${rendered}`;
  }

  function buildRenderables(scene: SuppliedYmapSceneReport): {
    renderables: YmapRenderableEntity[];
    textureDiagnostics: string[];
  } {
    const renderables: YmapRenderableEntity[] = [];
    const textureDiagnostics: string[] = [];
    const ytdDependencies = dependencies.filter((dependency) => extension(dependency.name) === "ytd");

    for (const entity of scene.entities) {
      if (!entity.resolution) continue;
      if (renderables.length >= MAX_RENDER_ENTITIES) {
        textureDiagnostics.push(
          `renderLimit: only the first ${MAX_RENDER_ENTITIES} resolved entities are rendered`,
        );
        break;
      }
      const resolution = entity.resolution;
      const model = dependencies[resolution.modelDependencyIndex];
      if (!model) {
        textureDiagnostics.push(
          `entity ${entity.index}: model dependency index ${resolution.modelDependencyIndex} is unavailable`,
        );
        continue;
      }

      try {
        const drawableIndex = resolution.drawableIndex ?? 0;
        const packet = modelPacket(resolution.modelFormat, model.bytes, drawableIndex);
        const materials: ModelMaterialResolution[] = [];
        for (const shader of packet.metadata.shaders) {
          if (!shader.diffuseTextureName) {
            materials.push({ shaderIndex: shader.index, texture: null });
            continue;
          }
          const diffuse = resolveDiffuseTexture(
            resolution.modelFormat,
            model.bytes,
            drawableIndex,
            shader.diffuseTextureName,
            ytdDependencies,
          );
          materials.push({ shaderIndex: shader.index, texture: diffuse.texture });
          textureDiagnostics.push(
            ...diffuse.diagnostics.map(
              (diagnostic) => `entity ${entity.index} shader ${shader.index}: ${diagnostic}`,
            ),
          );
        }
        renderables.push({ entity, packet, materials });
      } catch (error) {
        textureDiagnostics.push(
          `entity ${entity.index}: model preview failed: ${error instanceof Error ? error.message : String(error)}`,
        );
      }
    }
    return { renderables, textureDiagnostics };
  }

  function renderCurrentScene(): void {
    if (!state) return;
    state.scene = resolveSuppliedScene(state.bytes, dependencies);
    const { renderables, textureDiagnostics } = buildRenderables(state.scene);
    renderedEntityCount = renderables.length;
    bounds = viewer.setScene(renderables);
    summary.textContent =
      `${state.scene.entities.length} entities · ${renderedEntityCount} rendered · `
      + `${dependencies.length} supplied dependencies · ${state.scene.diagnostics.length} resolver diagnostics`;
    assetName.textContent = state.name;
    renderEntityList();
    renderDiagnostics(textureDiagnostics);
    updateBoundsText();

    if (
      selectedIndex !== null
      && !state.scene.entities.some((entity) => entity.index === selectedIndex)
    ) {
      selectedIndex = null;
    }
    if (selectedIndex !== null) viewer.selectEntity(selectedIndex);
    renderInspector();
  }

  function selectEntity(index: number, fromViewport: boolean): void {
    selectedIndex = index;
    viewer.selectEntity(index);
    isolateToggle.checked = false;
    if (!fromViewport) viewer.isolateEntity(null);
    renderEntityList();
    renderInspector();
  }

  async function openYmapFile(file: File): Promise<void> {
    const bytes = new Uint8Array(await file.arrayBuffer());
    if (extension(file.name) !== "ymap") {
      throw new Error("YMAP scene preview requires a .ymap primary file");
    }
    state = {
      name: file.name,
      originalBytes: bytes,
      bytes,
      scene: resolveSuppliedScene(bytes, dependencies),
      dirty: false,
      semanticValidated: false,
    };
    selectedIndex = null;
    semanticStatus.textContent = "Original bytes";
    downloadButton.disabled = true;
    renderCurrentScene();
    callbacks.onSuccess(`${file.name} parsed by Rust/WASM from caller-supplied bytes.`);
  }

  async function setDependencies(files: File[]): Promise<void> {
    const allowed = new Set(["ytyp", "ydr", "ydd", "yft", "ytd"]);
    const next: ModelDependency[] = [];
    for (const file of files) {
      if (!allowed.has(extension(file.name))) {
        throw new Error(`Unsupported YMAP dependency: ${file.name}`);
      }
      next.push({ name: file.name, bytes: new Uint8Array(await file.arrayBuffer()) });
    }
    dependencies = next;
    renderDependencyList();
    if (state) renderCurrentScene();
    callbacks.onSuccess(
      dependencies.length === 0
        ? "Supplied dependency catalog cleared."
        : `${dependencies.length} supplied files cataloged by Rust/WASM.`,
    );
  }

  async function loadFixture(): Promise<void> {
    const paths = [
      ["/fixtures/ymap-simple.ytyp", "simple.ytyp"],
      ["/fixtures/test_drawable.ydr", "test_drawable.ydr"],
    ] as const;
    const next: ModelDependency[] = [];
    for (const [path, name] of paths) {
      const response = await fetch(path);
      if (!response.ok) throw new Error(`fixture request failed: ${response.status}`);
      next.push({ name, bytes: new Uint8Array(await response.arrayBuffer()) });
    }
    dependencies = next;
    renderDependencyList();

    const response = await fetch("/fixtures/ymap-simple.ymap");
    if (!response.ok) throw new Error(`fixture request failed: ${response.status}`);
    const bytes = new Uint8Array(await response.arrayBuffer());
    state = {
      name: "ymap-simple.ymap",
      originalBytes: bytes,
      bytes,
      scene: resolveSuppliedScene(bytes, dependencies),
      dirty: false,
      semanticValidated: false,
    };
    selectedIndex = null;
    semanticStatus.textContent = "Original bytes";
    downloadButton.disabled = true;
    renderCurrentScene();
    callbacks.onSuccess("Synthetic YMAP scene loaded through Rust/WASM supplied-only resolution.");
  }

  viewer.setSelectionCallback((index) => selectEntity(index, true));

  ymapInput.addEventListener("change", async () => {
    const file = ymapInput.files?.[0];
    if (!file) return;
    try {
      await openYmapFile(file);
    } catch (error) {
      callbacks.onError(error);
    } finally {
      ymapInput.value = "";
    }
  });

  dependencyInput.addEventListener("change", async () => {
    const files = [...(dependencyInput.files ?? [])];
    try {
      await setDependencies(files);
    } catch (error) {
      callbacks.onError(error);
    } finally {
      dependencyInput.value = "";
    }
  });

  for (const type of ["dragenter", "dragover"]) {
    dropZone.addEventListener(type, (event) => {
      event.preventDefault();
      dropZone.classList.add("dragging");
    });
  }
  for (const type of ["dragleave", "drop"]) {
    dropZone.addEventListener(type, (event) => {
      event.preventDefault();
      dropZone.classList.remove("dragging");
    });
  }
  dropZone.addEventListener("drop", async (event) => {
    const files = [...(event.dataTransfer?.files ?? [])];
    const ymap = files.find((file) => extension(file.name) === "ymap");
    const deps = files.filter((file) => extension(file.name) !== "ymap");
    try {
      if (deps.length > 0) await setDependencies(deps);
      if (ymap) await openYmapFile(ymap);
      else if (deps.length === 0) throw new Error("Drop a .ymap and optional dependencies");
    } catch (error) {
      callbacks.onError(error);
    }
  });

  fixtureButton.addEventListener("click", () => {
    void loadFixture().catch(callbacks.onError);
  });

  viewSelect.addEventListener("change", () => viewer.setView(viewSelect.value as ModelView));
  gridToggle.addEventListener("change", () => viewer.setGrid(gridToggle.checked));
  wireToggle.addEventListener("change", () => viewer.setWireframe(wireToggle.checked));
  boundsToggle.addEventListener("change", () => viewer.setBounds(boundsToggle.checked));

  focusButton.addEventListener("click", () => {
    if (selectedIndex !== null) viewer.focusEntity(selectedIndex);
  });
  isolateToggle.addEventListener("change", () => {
    viewer.isolateEntity(isolateToggle.checked ? selectedIndex : null);
    bounds = viewer.boundsSnapshot();
    updateBoundsText();
  });

  applyButton.addEventListener("click", () => {
    if (!state) return;
    const entity = selectedEntity();
    if (!entity) return;
    try {
      const position = [
        Number(positionX.value),
        Number(positionY.value),
        Number(positionZ.value),
      ] as [number, number, number];
      const flags = Number(flagsInput.value);
      if (!position.every(Number.isFinite) || !Number.isInteger(flags) || flags < 0 || flags > 0xffff_ffff) {
        throw new Error("Position must be finite and flags must be an unsigned 32-bit integer");
      }

      const transformed = setYmapEntityTransform(
        state.bytes,
        entity.index,
        position,
        entity.rotation,
        entity.scaleXY,
        entity.scaleZ,
      );
      const flagged = setYmapEntityFlags(transformed.bytes, entity.index, flags);
      state.bytes = flagged.bytes;
      state.dirty = true;
      state.semanticValidated = true;
      semanticStatus.textContent = "PASS · Rust/WASM semantic reopen";
      downloadButton.disabled = false;
      renderCurrentScene();
      selectEntity(entity.index, false);
      callbacks.onSuccess(`Entity #${entity.index} updated and semantically reopened by Rust/WASM.`);
    } catch (error) {
      callbacks.onError(error);
    }
  });

  downloadButton.addEventListener("click", () => {
    if (!state || !state.dirty || !state.semanticValidated) return;
    const stem = state.name.replace(/\.ymap$/i, "");
    downloadBytes(state.bytes, `${stem}-edited.ymap`);
    callbacks.onSuccess("Explicit edited YMAP download started.");
  });

  renderDependencyList();
  renderDiagnostics([]);

  return {
    openBridgeBundle(name, bytes, suppliedDependencies) {
      if (extension(name) !== "ymap") {
        throw new Error("Bridge YMAP bundle requires a .ymap primary file");
      }
      const allowed = new Set(["ytyp", "ydr", "ydd", "yft", "ytd"]);
      const next: ModelDependency[] = [];
      for (const dependency of suppliedDependencies) {
        if (!allowed.has(extension(dependency.name))) {
          throw new Error("Unsupported bridge YMAP dependency: " + dependency.name);
        }
        next.push({
          name: dependency.name,
          bytes: Uint8Array.from(dependency.bytes),
        });
      }
      dependencies = next;
      renderDependencyList();

      const primary = Uint8Array.from(bytes);
      state = {
        name,
        originalBytes: primary,
        bytes: primary,
        scene: resolveSuppliedScene(primary, dependencies),
        dirty: false,
        semanticValidated: false,
      };
      selectedIndex = null;
      semanticStatus.textContent = "Original bytes";
      downloadButton.disabled = true;
      renderCurrentScene();
      callbacks.onSuccess(
        name + " loaded from the read-only local bridge through Rust/WASM supplied-only resolution.",
      );
    },
    debugSnapshot() {
      return {
        name: state?.name ?? null,
        dependencyCount: dependencies.length,
        entityCount: state?.scene.entities.length ?? 0,
        renderedEntityCount,
        selectedIndex,
        diagnosticCodes: [...extraDiagnosticCodes],
        dirty: state?.dirty ?? false,
        semanticValidated: state?.semanticValidated ?? false,
        bounds,
      };
    },
    dispose() {
      viewer.dispose();
    },
  };
}

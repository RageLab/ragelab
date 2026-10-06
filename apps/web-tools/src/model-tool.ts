import type {
  ModelDependency,
  ModelFormat,
  ModelMetadata,
  YddInspectReport,
} from "./contracts";
import {
  inspectYddBytes,
  inspectYftBytes,
  inspectYtdBytes,
  modelPacket,
  resolveDiffuseTexture,
  wasmError,
} from "./core";
import { ModelViewer, type ModelMaterialResolution, type ModelView } from "./model-viewer";

interface ModelDocumentState {
  name: string;
  format: ModelFormat;
  bytes: Uint8Array;
  drawableIndex: number;
  ydd: YddInspectReport | null;
  metadata: ModelMetadata;
}

export interface ModelToolCallbacks {
  onError(error: unknown): void;
  onSuccess(message: string): void;
}

export interface ModelToolDebugSnapshot {
  name: string | null;
  format: ModelFormat | null;
  drawableIndex: number;
  primitiveCount: number;
  vertexCount: number;
  indexCount: number;
  dependencyCount: number;
  diagnostics: string[];
}

function q<T extends Element>(selector: string): T {
  const element = document.querySelector<T>(selector);
  if (!element) throw new Error(`Missing model tool element ${selector}`);
  return element;
}

function inferModelFormat(name: string): ModelFormat {
  const match = name.toLowerCase().match(/\.([a-z0-9]+)$/);
  if (match?.[1] === "ydr" || match?.[1] === "ydd" || match?.[1] === "yft") {
    return match[1];
  }
  throw new Error("Model preview accepts only .ydr, .ydd or .yft caller-supplied files");
}

function downloadBlob(blob: Blob, name: string): void {
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = name;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

export function createModelTool(callbacks: ModelToolCallbacks): {
  debugSnapshot(): ModelToolDebugSnapshot;
  dispose(): void;
} {
  const dropZone = q<HTMLElement>("#model-drop-zone");
  const modelInput = q<HTMLInputElement>("#model-input");
  const dependencyInput = q<HTMLInputElement>("#model-dependency-input");
  const embeddedFixtureButton = q<HTMLButtonElement>("#model-fixture-embedded");
  const yddFixtureButton = q<HTMLButtonElement>("#model-fixture-ydd");
  const assetName = q<HTMLElement>("#model-asset-name");
  const assetSummary = q<HTMLElement>("#model-summary");
  const drawableWrap = q<HTMLElement>("#model-drawable-wrap");
  const drawableSelect = q<HTMLSelectElement>("#model-drawable-select");
  const dependencyList = q<HTMLElement>("#model-dependency-list");
  const materialsBody = q<HTMLTableSectionElement>("#model-material-table tbody");
  const diagnosticsPanel = q<HTMLElement>("#model-diagnostics");
  const viewerHost = q<HTMLElement>("#model-canvas-shell");
  const viewSelect = q<HTMLSelectElement>("#model-view-select");
  const gridToggle = q<HTMLInputElement>("#model-grid-toggle");
  const wireToggle = q<HTMLInputElement>("#model-wire-toggle");
  const boundsToggle = q<HTMLInputElement>("#model-bounds-toggle");
  const screenshotButton = q<HTMLButtonElement>("#model-screenshot-button");
  const turntableButton = q<HTMLButtonElement>("#model-turntable-button");

  const viewer = new ModelViewer(viewerHost);
  let state: ModelDocumentState | null = null;
  let dependencies: ModelDependency[] = [];
  let diagnostics: string[] = [];
  let turntable = false;

  function renderDependencyList(): void {
    dependencyList.replaceChildren();
    if (dependencies.length === 0) {
      dependencyList.textContent = "No supplied YTD dependencies.";
      return;
    }
    for (const dependency of dependencies) {
      const item = document.createElement("span");
      item.className = "dependency-chip";
      item.textContent = dependency.name;
      dependencyList.append(item);
    }
  }

  function renderDiagnostics(next: string[]): void {
    diagnostics = next;
    diagnosticsPanel.replaceChildren();
    if (next.length === 0) {
      diagnosticsPanel.textContent = "No unresolved proven diffuse bindings.";
      diagnosticsPanel.classList.add("ok");
      return;
    }
    diagnosticsPanel.classList.remove("ok");
    const list = document.createElement("ul");
    for (const diagnostic of next) {
      const item = document.createElement("li");
      item.textContent = diagnostic;
      list.append(item);
    }
    diagnosticsPanel.append(list);
  }

  function populateDrawableSelect(report: YddInspectReport | null, selected: number): void {
    drawableSelect.replaceChildren();
    drawableWrap.hidden = !report;
    if (!report) return;
    for (const entry of report.entries) {
      const option = document.createElement("option");
      option.value = String(entry.index);
      option.selected = entry.index === selected;
      option.textContent = entry.name
        ? `#${entry.index} · ${entry.name} · ${entry.nameHash}`
        : `#${entry.index} · ${entry.nameHash}`;
      drawableSelect.append(option);
    }
  }

  function buildMaterialRows(
    metadata: ModelMetadata,
    resolutions: ModelMaterialResolution[],
    allDiagnostics: string[],
  ): void {
    materialsBody.replaceChildren();
    for (const shader of metadata.shaders) {
      const resolution = resolutions.find((candidate) => candidate.shaderIndex === shader.index);
      const row = document.createElement("tr");
      const diffuse = shader.diffuseTextureName;
      const source = resolution?.texture;
      row.innerHTML = "<td></td><td></td><td></td><td></td>";
      const cells = row.querySelectorAll("td");
      cells[0].textContent = `#${shader.index} · ${shader.nameHash}`;
      cells[1].textContent = diffuse ?? "No proven diffuse binding";
      cells[2].textContent = source
        ? source.source === "embedded"
          ? "Embedded"
          : source.sourceName
        : diffuse
          ? "Unresolved"
          : "Neutral";
      cells[3].textContent = source
        ? `${source.metadata.format} · ${source.metadata.width}×${source.metadata.height}`
        : diffuse
          ? "See diagnostics"
          : "Inspect-only shader semantics";
      materialsBody.append(row);

      if (diffuse && !source) {
        allDiagnostics.push(
          `shader ${shader.index} diffuse ${JSON.stringify(diffuse)} is unresolved`,
        );
      }
    }
  }

  function renderPacket(
    name: string,
    format: ModelFormat,
    bytes: Uint8Array,
    drawableIndex: number,
    ydd: YddInspectReport | null,
  ): ModelDocumentState {
    const packet = modelPacket(format, bytes, drawableIndex);
    const resolutions: ModelMaterialResolution[] = [];
    const nextDiagnostics: string[] = [];

    for (const shader of packet.metadata.shaders) {
      if (!shader.diffuseTextureName) {
        resolutions.push({ shaderIndex: shader.index, texture: null });
        continue;
      }
      const result = resolveDiffuseTexture(
        format,
        bytes,
        drawableIndex,
        shader.diffuseTextureName,
        dependencies,
      );
      resolutions.push({ shaderIndex: shader.index, texture: result.texture });
      nextDiagnostics.push(
        ...result.diagnostics.map((diagnostic) => `shader ${shader.index}: ${diagnostic}`),
      );
    }

    viewer.setModel(packet, resolutions);
    assetName.textContent = name;
    assetSummary.textContent =
      `${format.toUpperCase()} · ${packet.metadata.primitiveCount} primitive${packet.metadata.primitiveCount === 1 ? "" : "s"} · `
      + `${packet.metadata.vertexCount} vertices · ${packet.metadata.indexCount} indices · `
      + `${packet.metadata.lod} · ${packet.metadata.coordinateConvention}`;
    populateDrawableSelect(ydd, drawableIndex);
    buildMaterialRows(packet.metadata, resolutions, nextDiagnostics);
    renderDiagnostics(nextDiagnostics);
    screenshotButton.disabled = false;
    turntableButton.disabled = false;

    return {
      name,
      format,
      bytes,
      drawableIndex,
      ydd,
      metadata: packet.metadata,
    };
  }

  function openModelBytes(name: string, bytes: Uint8Array): void {
    const format = inferModelFormat(name);
    let ydd: YddInspectReport | null = null;
    let drawableIndex = 0;

    if (format === "ydd") {
      ydd = inspectYddBytes(bytes);
      if (ydd.entries.length === 0) {
        throw new Error("YDD contains no selectable drawable entries");
      }
      drawableIndex = ydd.entries[0].index;
    } else if (format === "yft") {
      const report = inspectYftBytes(bytes);
      if (!report.hasMainDrawable) {
        throw {
          schema: "ragelab.wasm.error",
          schemaVersion: 1,
          format: "yft",
          code: "unsupportedAsset",
          message: "YFT has no pristine main drawable available for preview",
        };
      }
    }

    const next = renderPacket(name, format, bytes, drawableIndex, ydd);
    state = next;
  }

  async function openModelFile(file: File): Promise<void> {
    const bytes = new Uint8Array(await file.arrayBuffer());
    openModelBytes(file.name, bytes);
    callbacks.onSuccess(`${file.name} parsed by Rust/WASM and sent to Three.js typed buffers.`);
  }

  async function loadFixture(path: string, name: string): Promise<void> {
    const response = await fetch(path);
    if (!response.ok) throw new Error(`fixture request failed: ${response.status}`);
    openModelBytes(name, new Uint8Array(await response.arrayBuffer()));
    callbacks.onSuccess(`${name} synthetic fixture loaded through Rust/WASM.`);
  }

  async function replaceDependencies(files: File[]): Promise<void> {
    const next: ModelDependency[] = [];
    for (const file of files) {
      const bytes = new Uint8Array(await file.arrayBuffer());
      inspectYtdBytes(bytes);
      next.push({ name: file.name, bytes });
    }
    dependencies = next;
    renderDependencyList();
    if (state) {
      state = renderPacket(
        state.name,
        state.format,
        state.bytes,
        state.drawableIndex,
        state.ydd,
      );
    }
    callbacks.onSuccess(
      dependencies.length === 0
        ? "Model YTD dependency set cleared."
        : `${dependencies.length} caller-supplied YTD dependenc${dependencies.length === 1 ? "y" : "ies"} validated by Rust/WASM.`,
    );
  }

  modelInput.addEventListener("change", async () => {
    const file = modelInput.files?.[0];
    if (!file) return;
    try {
      await openModelFile(file);
    } catch (error) {
      callbacks.onError(error);
    } finally {
      modelInput.value = "";
    }
  });

  dependencyInput.addEventListener("change", async () => {
    const files = [...(dependencyInput.files ?? [])];
    try {
      await replaceDependencies(files);
    } catch (error) {
      callbacks.onError(error);
    } finally {
      dependencyInput.value = "";
    }
  });

  for (const event of ["dragenter", "dragover"]) {
    dropZone.addEventListener(event, (value) => {
      value.preventDefault();
      dropZone.classList.add("dragging");
    });
  }
  for (const event of ["dragleave", "drop"]) {
    dropZone.addEventListener(event, (value) => {
      value.preventDefault();
      dropZone.classList.remove("dragging");
    });
  }
  dropZone.addEventListener("drop", async (event) => {
    const file = event.dataTransfer?.files[0];
    if (!file) return;
    try {
      await openModelFile(file);
    } catch (error) {
      callbacks.onError(error);
    }
  });

  embeddedFixtureButton.addEventListener("click", () => {
    void loadFixture("/fixtures/model-embedded.ydr", "model-embedded.ydr").catch(callbacks.onError);
  });
  yddFixtureButton.addEventListener("click", () => {
    void loadFixture("/fixtures/model-editable.ydd", "model-editable.ydd").catch(callbacks.onError);
  });

  drawableSelect.addEventListener("change", () => {
    if (!state || state.format !== "ydd" || !state.ydd) return;
    const drawableIndex = Number(drawableSelect.value);
    try {
      const next = renderPacket(state.name, state.format, state.bytes, drawableIndex, state.ydd);
      state = next;
    } catch (error) {
      drawableSelect.value = String(state.drawableIndex);
      callbacks.onError(error);
    }
  });

  viewSelect.addEventListener("change", () => viewer.setView(viewSelect.value as ModelView));
  gridToggle.addEventListener("change", () => viewer.setGrid(gridToggle.checked));
  wireToggle.addEventListener("change", () => viewer.setWireframe(wireToggle.checked));
  boundsToggle.addEventListener("change", () => viewer.setBounds(boundsToggle.checked));

  screenshotButton.addEventListener("click", async () => {
    if (!state) return;
    try {
      const blob = await viewer.screenshotPng();
      const stem = state.name.replace(/\.(ydr|ydd|yft)$/i, "");
      downloadBlob(blob, `${stem}-preview.png`);
      callbacks.onSuccess("PNG screenshot captured from the Three.js model viewport.");
    } catch (error) {
      callbacks.onError(error);
    }
  });

  turntableButton.addEventListener("click", () => {
    turntable = !turntable;
    viewer.setTurntable(turntable);
    turntableButton.setAttribute("aria-pressed", String(turntable));
    turntableButton.textContent = turntable ? "Stop turntable" : "Turntable";
  });

  renderDependencyList();
  renderDiagnostics([]);

  return {
    debugSnapshot() {
      return {
        name: state?.name ?? null,
        format: state?.format ?? null,
        drawableIndex: state?.drawableIndex ?? 0,
        primitiveCount: state?.metadata.primitiveCount ?? 0,
        vertexCount: state?.metadata.vertexCount ?? 0,
        indexCount: state?.metadata.indexCount ?? 0,
        dependencyCount: dependencies.length,
        diagnostics: [...diagnostics],
      };
    },
    dispose() {
      viewer.dispose();
    },
  };
}

export function modelToolError(error: unknown): { code: string; message: string } {
  const report = wasmError(error);
  return { code: report.code, message: report.message };
}

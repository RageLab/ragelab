import "./styles.css";
import { initCore, probeMipError, wasmError } from "./core";
import type { ChannelMode } from "./components/texture-preview";
import { renderTexture } from "./components/texture-preview";
import { YtdDocument } from "./ytd-document";
import { createModelTool, type ModelToolDebugSnapshot } from "./model-tool";

const q = <T extends Element>(selector: string): T => {
  const element = document.querySelector<T>(selector);
  if (!element) throw new Error(`Missing element ${selector}`);
  return element;
};

const coreStatus = q<HTMLElement>("#core-status");
const dropZone = q<HTMLElement>("#drop-zone");
const assetInput = q<HTMLInputElement>("#asset-input");
const replaceInput = q<HTMLInputElement>("#replace-input");
const fixtureButton = q<HTMLButtonElement>("#fixture-button");
const errorPanel = q<HTMLElement>("#error-panel");
const successPanel = q<HTMLElement>("#success-panel");
const assetName = q<HTMLElement>("#asset-name");
const textureCount = q<HTMLElement>("#texture-count");
const tableBody = q<HTMLTableSectionElement>("#texture-table tbody");
const selectedName = q<HTMLElement>("#selected-name");
const previewDimensions = q<HTMLElement>("#preview-dimensions");
const mipSelect = q<HTMLSelectElement>("#mip-select");
const canvas = q<HTMLCanvasElement>("#texture-canvas");
const details = q<HTMLElement>("#texture-details");
const exportButton = q<HTMLButtonElement>("#export-button");
const resetButton = q<HTMLButtonElement>("#reset-button");
const downloadButton = q<HTMLButtonElement>("#download-button");
const writePolicy = q<HTMLElement>("#write-policy");
const semanticStatus = q<HTMLElement>("#semantic-status");
const channelControls = q<HTMLElement>("#channel-controls");
const ytdToolButton = q<HTMLButtonElement>("#tool-ytd");
const modelToolButton = q<HTMLButtonElement>("#tool-model");
const ytdToolPanel = q<HTMLElement>("#ytd-tool");
const modelToolPanel = q<HTMLElement>("#model-tool");

let documentState: YtdDocument | null = null;
let modelTool: ReturnType<typeof createModelTool> | null = null;
let selectedTexture = 0;
let selectedMip = 0;
let channel: ChannelMode = "rgba";

function setError(error: unknown): void {
  const report = wasmError(error);
  errorPanel.textContent = `${report.code}: ${report.message}`;
  errorPanel.hidden = false;
  successPanel.hidden = true;
}

function setSuccess(message: string): void {
  successPanel.textContent = message;
  successPanel.hidden = false;
  errorPanel.hidden = true;
}

function clearMessages(): void {
  errorPanel.hidden = true;
  successPanel.hidden = true;
}

function downloadBytes(bytes: Uint8Array, name: string, type: string): void {
  const copy = Uint8Array.from(bytes);
  const blob = new Blob([copy.buffer], { type });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = name;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

async function openBytes(name: string, bytes: Uint8Array): Promise<void> {
  const next = YtdDocument.open(name, bytes);
  documentState = next;
  selectedTexture = 0;
  selectedMip = 0;
  channel = "rgba";
  clearMessages();
  renderAll();
}

async function openFile(file: File): Promise<void> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  await openBytes(file.name, bytes);
}

function renderTable(): void {
  tableBody.replaceChildren();
  if (!documentState) return;
  for (const texture of documentState.report.textures) {
    const row = document.createElement("tr");
    row.dataset.index = String(texture.index);
    row.classList.toggle("selected", texture.index === selectedTexture);
    row.innerHTML = `
      <td><strong></strong><span class="hash"></span></td>
      <td></td>
      <td></td>
      <td></td>
      <td class="reason"></td>
    `;
    const cells = row.querySelectorAll("td");
    cells[0].querySelector("strong")!.textContent = texture.name || "(unnamed)";
    cells[0].querySelector(".hash")!.textContent = texture.nameHash;
    cells[1].textContent = texture.format;
    cells[2].textContent = `${texture.width}×${texture.height}`;
    cells[3].textContent = String(texture.mipLevels);
    cells[4].textContent = texture.replacement.png
      ? "PNG replace"
      : texture.replacement.reason ?? "Inspect only";
    row.addEventListener("click", () => {
      selectedTexture = texture.index;
      selectedMip = 0;
      renderAll();
    });
    tableBody.append(row);
  }
}

function renderMipOptions(): void {
  mipSelect.replaceChildren();
  if (!documentState) {
    mipSelect.disabled = true;
    return;
  }
  const texture = documentState.texture(selectedTexture);
  for (let index = 0; index < texture.mipLevels; index += 1) {
    const option = document.createElement("option");
    option.value = String(index);
    option.textContent = `Mip ${index}`;
    option.selected = index === selectedMip;
    mipSelect.append(option);
  }
  mipSelect.disabled = texture.mipLevels === 0;
}

function renderSelected(): void {
  if (!documentState || documentState.report.textureCount === 0) {
    selectedName.textContent = "Select a texture";
    previewDimensions.textContent = "—";
    return;
  }
  const texture = documentState.texture(selectedTexture);
  const packet = documentState.mip(selectedTexture, selectedMip);
  selectedName.textContent = texture.name || texture.nameHash;
  previewDimensions.textContent =
    `${packet.metadata.width} × ${packet.metadata.height} · mip ${selectedMip}/${Math.max(0, texture.mipLevels - 1)}`;
  renderTexture(canvas, packet.metadata, packet.rgba, channel);

  details.innerHTML = `
    <dt>Name hash</dt><dd></dd>
    <dt>Dictionary hash</dt><dd></dd>
    <dt>Format</dt><dd></dd>
    <dt>Raw</dt><dd></dd>
    <dt>Encoded</dt><dd></dd>
    <dt>Depth</dt><dd></dd>
  `;
  const values = details.querySelectorAll("dd");
  values[0].textContent = texture.nameHash;
  values[1].textContent = texture.dictionaryHash;
  values[2].textContent = texture.format;
  values[3].textContent = texture.formatRaw;
  values[4].textContent = `${texture.encodedBytes} bytes`;
  values[5].textContent = String(texture.depth);

  exportButton.disabled = !texture.export.pngTopMip;
  replaceInput.disabled = !texture.replacement.png;
  q<HTMLElement>("#replace-label").classList.toggle("disabled", replaceInput.disabled);
  writePolicy.textContent = texture.replacement.png
    ? `RGBA8/BC1/BC3 safe repack · preserves ${texture.format} · regenerates mips`
    : texture.replacement.reason ?? "Inspect only";
}

function renderAll(): void {
  if (!documentState) return;
  assetName.textContent = documentState.name;
  textureCount.textContent = `${documentState.report.textureCount} texture${documentState.report.textureCount === 1 ? "" : "s"}`;
  semanticStatus.textContent = documentState.semanticReopen
    ? "PASS — reopened by Rust Core"
    : "Not run";
  downloadButton.disabled = !(documentState.dirty && documentState.semanticReopen);
  resetButton.disabled = !documentState.dirty;
  renderTable();
  renderMipOptions();
  renderSelected();

  for (const button of channelControls.querySelectorAll<HTMLButtonElement>("button")) {
    button.classList.toggle("active", button.dataset.channel === channel);
  }
}

assetInput.addEventListener("change", async () => {
  const file = assetInput.files?.[0];
  if (!file) return;
  try {
    await openFile(file);
  } catch (error) {
    setError(error);
  } finally {
    assetInput.value = "";
  }
});

fixtureButton.addEventListener("click", async () => {
  try {
    const response = await fetch("/fixtures/simple.ytd");
    if (!response.ok) throw new Error(`fixture request failed: ${response.status}`);
    await openBytes("simple.ytd", new Uint8Array(await response.arrayBuffer()));
  } catch (error) {
    setError(error);
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
    await openFile(file);
  } catch (error) {
    setError(error);
  }
});

mipSelect.addEventListener("change", () => {
  selectedMip = Number(mipSelect.value);
  try {
    renderSelected();
  } catch (error) {
    setError(error);
  }
});

channelControls.addEventListener("click", (event) => {
  const button = (event.target as HTMLElement).closest<HTMLButtonElement>("button[data-channel]");
  if (!button) return;
  channel = button.dataset.channel as ChannelMode;
  try {
    renderAll();
  } catch (error) {
    setError(error);
  }
});

exportButton.addEventListener("click", () => {
  if (!documentState) return;
  try {
    const texture = documentState.texture(selectedTexture);
    const png = documentState.exportPng(selectedTexture);
    downloadBytes(png, `${texture.name || texture.nameHash}.png`, "image/png");
    setSuccess("PNG exported through the Rust Core top-mip contract.");
  } catch (error) {
    setError(error);
  }
});

replaceInput.addEventListener("change", async () => {
  const file = replaceInput.files?.[0];
  if (!file || !documentState) return;
  try {
    const png = new Uint8Array(await file.arrayBuffer());
    documentState.replacePng(selectedTexture, png);
    selectedMip = 0;
    renderAll();
    setSuccess("Replacement committed in memory only after semantic reopen PASS.");
  } catch (error) {
    setError(error);
  } finally {
    replaceInput.value = "";
  }
});

resetButton.addEventListener("click", () => {
  if (!documentState) return;
  documentState.reset();
  selectedMip = 0;
  renderAll();
  setSuccess("Restored original caller-supplied bytes.");
});

downloadButton.addEventListener("click", () => {
  if (!documentState || !documentState.dirty || !documentState.semanticReopen) return;
  const stem = documentState.name.replace(/\.ytd$/i, "");
  downloadBytes(documentState.bytes, `${stem}-rebuilt.ytd`, "application/octet-stream");
});

function ensureModelTool(): void {
  modelTool ??= createModelTool({
    onError: setError,
    onSuccess: setSuccess,
  });
}

function selectTool(tool: "ytd" | "model"): void {
  const isModel = tool === "model";
  ytdToolPanel.hidden = isModel;
  modelToolPanel.hidden = !isModel;
  ytdToolButton.classList.toggle("active", !isModel);
  modelToolButton.classList.toggle("active", isModel);
  ytdToolButton.setAttribute("aria-pressed", String(!isModel));
  modelToolButton.setAttribute("aria-pressed", String(isModel));
  clearMessages();
  if (isModel) {
    try {
      ensureModelTool();
    } catch (error) {
      setError(error);
    }
  }
}

ytdToolButton.addEventListener("click", () => selectTool("ytd"));
modelToolButton.addEventListener("click", () => selectTool("model"));

declare global {
  interface Window {
    __ragelabWebTools?: {
      probeMip(textureIndex: number, mipIndex: number): ReturnType<typeof probeMipError>;
      modelSnapshot(): ModelToolDebugSnapshot | null;
    };
  }
}

async function boot(): Promise<void> {
  try {
    await initCore();
    coreStatus.textContent = "Rust/WASM Core ready";
    if (import.meta.env.DEV) {
      window.__ragelabWebTools = {
        probeMip(textureIndex, mipIndex) {
          if (!documentState) {
            return { ok: false, code: "noDocument", message: "No YTD loaded" };
          }
          return probeMipError(documentState.bytes, textureIndex, mipIndex);
        },
        modelSnapshot() {
          return modelTool?.debugSnapshot() ?? null;
        },
      };
    }
  } catch (error) {
    coreStatus.textContent = "Core initialization failed";
    setError(error);
  }
}

void boot();

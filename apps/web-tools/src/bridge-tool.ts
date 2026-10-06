export interface BridgeHealth {
  schema: "ragelab.bridge.health";
  schemaVersion: number;
  connected: boolean;
  readOnly: boolean;
  writesEnabled: boolean;
  capabilities: string[];
  limits: {
    maxAssetBytes: number;
    maxBundleAssets: number;
    maxBundleBytes: number;
    maxSearchResults: number;
  };
}

export interface BridgeSearchResult {
  kind: "ymap" | "archetype" | "asset";
  hash: string;
  label: string;
  assetId: string | null;
  format: string | null;
  mapHash: string | null;
  entityIndex: number | null;
  entityCount: number | null;
}

interface BridgeSearchResponse {
  schema: "ragelab.bridge.search";
  schemaVersion: number;
  query: string;
  results: BridgeSearchResult[];
  truncated: boolean;
}

interface BridgeBundleFile {
  role: string;
  assetId: string;
  name: string;
  format: string;
  hash: string;
  byteLength: number;
  bytesBase64: string;
}

interface BridgeBundleResponse {
  schema: "ragelab.bridge.ymap-bundle";
  schemaVersion: number;
  mapHash: string;
  files: BridgeBundleFile[];
  limits: {
    assetCount: number;
    declaredBytes: number;
    actualBytes: number;
    maxAssets: number;
    maxBytes: number;
  };
}

export interface BridgeSuppliedFile {
  role: string;
  name: string;
  format: string;
  bytes: Uint8Array;
}

export interface BridgeToolCallbacks {
  onError(error: unknown): void;
  onSuccess(message: string): void;
  onOpenAsset(name: string, format: string, bytes: Uint8Array): Promise<void>;
  onOpenYmapBundle(files: BridgeSuppliedFile[]): Promise<void>;
}

export interface BridgeToolDebugSnapshot {
  connected: boolean;
  endpoint: string | null;
  readOnly: boolean;
  writesEnabled: boolean;
  resultCount: number;
}

function q<T extends Element>(selector: string): T {
  const element = document.querySelector<T>(selector);
  if (!element) throw new Error("Missing bridge element " + selector);
  return element;
}

function normalizeEndpoint(raw: string): string {
  const url = new URL(raw.trim());
  if (url.protocol !== "http:") {
    throw new Error("Local bridge URL must use http:// loopback");
  }
  if (url.username || url.password || url.pathname !== "/" || url.search || url.hash) {
    throw new Error("Local bridge URL must contain only scheme, loopback host and port");
  }
  const host = url.hostname.toLowerCase();
  if (host !== "127.0.0.1" && host !== "localhost") {
    throw new Error("Local bridge URL must target 127.0.0.1 or localhost");
  }
  if (!url.port) {
    throw new Error("Local bridge URL must include the launch port");
  }
  return url.origin;
}

function validateToken(raw: string): string {
  const token = raw.trim();
  if (!/^[0-9a-f]{64}$/i.test(token)) {
    throw new Error("Bridge launch token must be the 64-character hex capability printed by the CLI");
  }
  return token;
}

function decodeBase64(value: string): Uint8Array {
  const raw = atob(value);
  const bytes = new Uint8Array(raw.length);
  for (let index = 0; index < raw.length; index += 1) {
    bytes[index] = raw.charCodeAt(index);
  }
  return bytes;
}

class BridgeClient {
  readonly endpoint: string;
  readonly token: string;
  private healthReport: BridgeHealth | null = null;

  constructor(endpoint: string, token: string) {
    this.endpoint = normalizeEndpoint(endpoint);
    this.token = validateToken(token);
  }

  private async get(path: string): Promise<Response> {
    const response = await fetch(this.endpoint + path, {
      method: "GET",
      headers: {
        Authorization: "Bearer " + this.token,
      },
      cache: "no-store",
      credentials: "omit",
      referrerPolicy: "no-referrer",
    });
    if (!response.ok) {
      let message = "bridge request failed: HTTP " + response.status;
      try {
        const body = await response.json() as { code?: string; message?: string };
        message = (body.code ?? "bridgeError") + ": " + (body.message ?? message);
      } catch {
        // Keep the bounded HTTP status message.
      }
      throw new Error(message);
    }
    return response;
  }

  async health(): Promise<BridgeHealth> {
    const body = await (await this.get("/v1/health")).json() as BridgeHealth;
    if (
      body.schema !== "ragelab.bridge.health"
      || body.connected !== true
      || body.readOnly !== true
      || body.writesEnabled !== false
    ) {
      throw new Error("Bridge health response violated the read-only contract");
    }
    this.healthReport = body;
    return body;
  }

  async search(query: string): Promise<BridgeSearchResponse> {
    const value = query.trim();
    if (value.length === 0 || value.length > 128) {
      throw new Error("Bridge search must contain 1..128 characters");
    }
    const body = await (
      await this.get("/v1/search?q=" + encodeURIComponent(value) + "&limit=30")
    ).json() as BridgeSearchResponse;
    if (body.schema !== "ragelab.bridge.search" || !Array.isArray(body.results)) {
      throw new Error("Bridge search response has an invalid schema");
    }
    return body;
  }

  async asset(assetId: string): Promise<Uint8Array> {
    if (!/^asset-[0-9a-f]{32}$/i.test(assetId)) {
      throw new Error("Bridge returned an invalid opaque asset ID");
    }
    const response = await this.get("/v1/assets/" + encodeURIComponent(assetId));
    const bytes = new Uint8Array(await response.arrayBuffer());
    const limit = this.healthReport?.limits.maxAssetBytes ?? 128 * 1024 * 1024;
    if (bytes.byteLength > limit) {
      throw new Error("Bridge asset exceeded the negotiated byte limit");
    }
    return bytes;
  }

  async ymapBundle(assetId: string): Promise<BridgeSuppliedFile[]> {
    if (!/^asset-[0-9a-f]{32}$/i.test(assetId)) {
      throw new Error("Bridge returned an invalid opaque YMAP asset ID");
    }
    const body = await (
      await this.get("/v1/ymap-bundle/" + encodeURIComponent(assetId))
    ).json() as BridgeBundleResponse;
    if (body.schema !== "ragelab.bridge.ymap-bundle" || !Array.isArray(body.files)) {
      throw new Error("Bridge YMAP bundle response has an invalid schema");
    }
    const negotiated = this.healthReport?.limits;
    if (
      body.files.length !== body.limits.assetCount
      || body.limits.actualBytes > body.limits.maxBytes
      || body.files.length > body.limits.maxAssets
      || (negotiated !== null && negotiated !== undefined
        && (body.files.length > negotiated.maxBundleAssets
          || body.limits.actualBytes > negotiated.maxBundleBytes))
    ) {
      throw new Error("Bridge YMAP bundle exceeded its declared response limits");
    }
    return body.files.map((file) => {
      const bytes = decodeBase64(file.bytesBase64);
      if (bytes.byteLength !== file.byteLength) {
        throw new Error("Bridge byte length mismatch for " + file.name);
      }
      return {
        role: file.role,
        name: file.name,
        format: file.format,
        bytes,
      };
    });
  }
}

export function createBridgeTool(callbacks: BridgeToolCallbacks): {
  debugSnapshot(): BridgeToolDebugSnapshot;
  disconnect(): void;
} {
  const urlInput = q<HTMLInputElement>("#bridge-url");
  const tokenInput = q<HTMLInputElement>("#bridge-token");
  const connectButton = q<HTMLButtonElement>("#bridge-connect");
  const disconnectButton = q<HTMLButtonElement>("#bridge-disconnect");
  const status = q<HTMLElement>("#bridge-status");
  const browser = q<HTMLElement>("#bridge-browser");
  const searchForm = q<HTMLFormElement>("#bridge-search-form");
  const searchInput = q<HTMLInputElement>("#bridge-search-input");
  const searchButton = q<HTMLButtonElement>("#bridge-search-button");
  const results = q<HTMLElement>("#bridge-search-results");

  let client: BridgeClient | null = null;
  let health: BridgeHealth | null = null;
  let resultCount = 0;

  function setDisconnected(): void {
    client = null;
    health = null;
    resultCount = 0;
    tokenInput.value = "";
    status.textContent = "Disconnected · optional";
    status.classList.add("muted");
    browser.hidden = true;
    disconnectButton.hidden = true;
    connectButton.hidden = false;
    searchButton.disabled = true;
    results.replaceChildren();
  }

  function renderResults(items: BridgeSearchResult[], truncated: boolean): void {
    results.replaceChildren();
    resultCount = items.length;
    if (items.length === 0) {
      results.textContent = "No indexed assets matched.";
      return;
    }

    for (const item of items) {
      const row = document.createElement("div");
      row.className = "bridge-result";

      const info = document.createElement("div");
      info.className = "bridge-result-main";
      const titleLine = document.createElement("div");
      titleLine.className = "bridge-result-title";
      const title = document.createElement("strong");
      title.textContent = item.label;
      titleLine.append(title);
      const meta = document.createElement("div");
      meta.className = "bridge-result-meta";
      meta.textContent = item.kind + " · " + (item.format ?? "metadata") + " · " + item.hash;
      info.append(titleLine, meta);
      row.append(info);

      if (item.assetId) {
        const format = item.format?.toLowerCase() ?? "";
        const canOpenScene = item.kind === "ymap";
        const canOpenAsset = ["ytd", "ydr", "ydd", "yft"].includes(format);
        if (canOpenScene || canOpenAsset) {
          const open = document.createElement("button");
          open.type = "button";
          open.className = "button";
          open.textContent = canOpenScene ? "Open scene" : "Open preview";
          open.addEventListener("click", () => {
            const active = client;
            if (!active) return;
            open.disabled = true;
            const operation = canOpenScene
              ? active.ymapBundle(item.assetId as string).then(async (files) => {
                  await callbacks.onOpenYmapBundle(files);
                  callbacks.onSuccess(
                    "Loaded " + files.length
                      + " bounded bridge files through the Rust/WASM supplied-only resolver.",
                  );
                })
              : active.asset(item.assetId as string).then(async (bytes) => {
                  await callbacks.onOpenAsset(item.label, format, bytes);
                  callbacks.onSuccess(
                    item.label + " loaded from the read-only bridge and parsed by Rust/WASM.",
                  );
                });
            void operation
              .catch(callbacks.onError)
              .finally(() => {
                open.disabled = false;
              });
          });
          row.append(open);
        }
      }

      results.append(row);
    }

    if (truncated) {
      const note = document.createElement("p");
      note.className = "bridge-note";
      note.textContent = "Results truncated by the Core bridge limit.";
      results.append(note);
    }
  }

  connectButton.addEventListener("click", () => {
    connectButton.disabled = true;
    let next: BridgeClient;
    try {
      next = new BridgeClient(urlInput.value, tokenInput.value);
    } catch (error) {
      connectButton.disabled = false;
      callbacks.onError(error);
      return;
    }

    void next
      .health()
      .then((nextHealth) => {
        client = next;
        health = nextHealth;
        tokenInput.value = "";
        status.textContent = "Connected · read-only";
        status.classList.remove("muted");
        browser.hidden = false;
        disconnectButton.hidden = false;
        connectButton.hidden = true;
        searchButton.disabled = false;
        callbacks.onSuccess(
          "Connected to the optional local bridge. GTA access is read-only; the launch token is kept only in page memory.",
        );
      })
      .catch((error) => {
        setDisconnected();
        callbacks.onError(error);
      })
      .finally(() => {
        connectButton.disabled = false;
      });
  });

  disconnectButton.addEventListener("click", () => {
    setDisconnected();
    callbacks.onSuccess("Local bridge disconnected; launch capability cleared from page memory.");
  });

  searchForm.addEventListener("submit", (event) => {
    event.preventDefault();
    const active = client;
    if (!active) {
      callbacks.onError(new Error("Connect explicitly before searching the local GTA index"));
      return;
    }
    searchButton.disabled = true;
    void active
      .search(searchInput.value)
      .then((response) => renderResults(response.results, response.truncated))
      .catch(callbacks.onError)
      .finally(() => {
        searchButton.disabled = false;
      });
  });

  setDisconnected();

  return {
    debugSnapshot() {
      return {
        connected: client !== null,
        endpoint: client?.endpoint ?? null,
        readOnly: health?.readOnly ?? false,
        writesEnabled: health?.writesEnabled ?? false,
        resultCount,
      };
    },
    disconnect: setDisconnected,
  };
}

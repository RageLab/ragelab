# Web local bridge security model

The optional RageLab Web bridge exists only to let a browser session request installed GTA assets through the authoritative Rust Core. It is not required for ordinary Web Tools: caller-supplied files remain the default path.

## Trust boundary

Loopback is **not** treated as an authentication boundary. A malicious local page can attempt requests to localhost, so a bridge launch requires both:

1. an explicit allow-listed browser `Origin`; and
2. a random 256-bit bearer capability generated for that launch.

The server binds only to `127.0.0.1`. The Web client accepts only `http://127.0.0.1:<port>` or `http://localhost:<port>`, never a remote host. CORS is an additional browser boundary, not a replacement for the bearer capability.

The capability is printed once in the bridge-ready JSON. Treat it as a secret. Web Tools never persist it in local storage, session storage, IndexedDB, cookies or the URL; the password field is cleared after a successful connection. Disconnecting drops the client/capability from page state.

## Disabled by default

There is no auto-discovery, background daemon or implicit connection. The user must deliberately run:

```text
ragelab bridge serve --game-root ... --index ... --keys ... --origin ...
```

and then explicitly connect from the Web Tools panel. Closing the process removes the capability and all opaque asset grants.

## Read-only API

The bridge accepts only `GET` and CORS `OPTIONS`. A bodyless non-GET/OPTIONS request reaches the read-only policy and fails with `405 readOnly`; requests carrying a body or unsupported transfer encoding are rejected earlier as malformed/unsupported requests.

Available routes:

- `GET /v1/health`
- `GET /v1/search?q=...&limit=...`
- `GET /v1/assets/:opaque-id`
- `GET /v1/ymap-bundle/:opaque-ymap-id`

There is no bridge write route. GTA Legacy archives and key caches are never modified.

Browser authoring remains independent of bridge access: writes are in-memory until an explicit user download, and supported YTD/YMAP writes remain gated by semantic reopen through the Rust/WASM Core.

## Path and key non-disclosure

Core search/index records contain archive-relative paths, nested RPF paths and materialized key paths. The bridge never serializes those records directly.

Search results are projected to sanitized metadata plus random per-session opaque IDs. The opaque grant table remains process-local. Browser responses may contain asset basenames, hashes, transforms/bounds and file bytes that the user explicitly requests, but not:

- GTA key material;
- key-cache paths;
- absolute GTA filesystem paths;
- archive-relative provider paths;
- nested RPF provider paths;
- unrestricted filesystem handles.

Errors returned to HTTP clients are intentionally generic where Core errors could include provenance paths.

## Bounded reads

The server enforces both configurable defaults and hard ceilings:

| Limit | Default | Hard ceiling |
| --- | ---: | ---: |
| Single asset bytes | 32 MiB | 128 MiB |
| YMAP bundle assets | 96 | 256 |
| YMAP bundle bytes | 64 MiB | 256 MiB |
| Search results | 30 requested by UI | 50 bridge maximum |
| Search text | 128 characters | 128 characters |
| Request headers | 16 KiB | fixed |
| Request target | 4 KiB | fixed |
| Socket read/write timeout | 10 s | fixed |

Before reading a granted RPF payload, `RpfEntryLocator::info()` resolves the same archive/nested chain and checks the declared stored/memory size. YMAP bundles preflight every selected provider before any payload is read. Actual byte lengths are checked again after reading.

YMAP dependency selection reuses `GtaRpfAssetIndex::plan_for_archetypes`; there is no second RPF resolver in the bridge. The returned bundle is then consumed by the existing Web supplied-only Rust/WASM YMAP resolver. YBN collision entries are not part of the current Web scene bundle and their omitted count is diagnostic only.

## Request handling

- security-sensitive duplicate `Origin` or `Authorization` headers are rejected;
- request bodies and non-identity transfer encoding are rejected;
- the allow-listed `Origin` is matched exactly;
- bearer comparison does not early-return on content differences;
- responses use `Cache-Control: no-store` and `X-Content-Type-Options: nosniff`;
- opaque IDs are strict fixed-size random hex tokens.

## Audit logging

Every successfully parsed request produces a JSON-line audit event to stderr. `--audit-log <absolute-file>` optionally appends the same events to a user-selected log. Requests rejected while parsing malformed or oversized request syntax fail before route-level audit metadata exists.

Audit entries include normalized route, method, origin, status, output byte count and duration. They do **not** contain the bearer token, search query, opaque asset ID, GTA path or key path.

## Operational guidance

Allow only a Web origin you control. Do not launch with a wildcard origin. Stop the bridge when installed-GTA access is no longer needed. If a launch capability may have been exposed, stop and restart the process to rotate it.

The bridge is a convenience transport around existing Core contracts; it does not weaken the project rule that Rust owns RAGE parsing, provider resolution and semantic validation.

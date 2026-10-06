use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use ragelab_engine::{
    GtaRpfAssetIndex, GtaRpfAssetKind, GtaRpfBrowserResultKind, GtaRpfBrowserSearchResult,
    GtaRpfIndexLocator, GtaRpfIndexPlan,
};
use ragelab_hash::joaat;
use rand::{rngs::OsRng, RngCore};
use serde_json::{json, Value};

const DEFAULT_PORT: u16 = 32_191;
const DEFAULT_MAX_ASSET_BYTES: u64 = 32 * 1024 * 1024;
const DEFAULT_MAX_BUNDLE_ASSETS: usize = 96;
const DEFAULT_MAX_BUNDLE_BYTES: u64 = 64 * 1024 * 1024;
const HARD_MAX_ASSET_BYTES: u64 = 128 * 1024 * 1024;
const HARD_MAX_BUNDLE_ASSETS: usize = 256;
const HARD_MAX_BUNDLE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_TARGET_BYTES: usize = 4 * 1024;
const MAX_SEARCH_CHARS: usize = 128;
const MAX_SEARCH_RESULTS: usize = 50;
const IO_TIMEOUT: Duration = Duration::from_secs(10);

const USAGE: &str = "usage: ragelab bridge serve --game-root <directory> --index <legacy-v5.bin> --keys <directory> --origin <http(s)://host[:port]> [--origin <...>]... [--port <1..65535>] [--max-asset-bytes <n>] [--max-bundle-assets <n>] [--max-bundle-bytes <n>] [--audit-log <file>]";

#[derive(Debug, Clone)]
struct BridgeConfig {
    game_root: PathBuf,
    index: PathBuf,
    keys: PathBuf,
    origins: BTreeSet<String>,
    port: u16,
    max_asset_bytes: u64,
    max_bundle_assets: usize,
    max_bundle_bytes: u64,
    audit_log: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct GrantKey {
    kind: GtaRpfAssetKind,
    hash: u32,
    map_hash: Option<u32>,
    locator: GtaRpfIndexLocator,
}

#[derive(Debug, Clone)]
struct AssetGrant {
    kind: GtaRpfAssetKind,
    hash: u32,
    map_hash: Option<u32>,
    locator: GtaRpfIndexLocator,
}

#[derive(Default)]
struct GrantStore {
    by_id: BTreeMap<String, AssetGrant>,
    by_key: BTreeMap<GrantKey, String>,
}

impl GrantStore {
    fn grant(
        &mut self,
        kind: GtaRpfAssetKind,
        hash: u32,
        map_hash: Option<u32>,
        locator: GtaRpfIndexLocator,
    ) -> String {
        let key = GrantKey {
            kind,
            hash,
            map_hash,
            locator: locator.clone(),
        };
        if let Some(id) = self.by_key.get(&key) {
            return id.clone();
        }
        let id = loop {
            let mut bytes = [0_u8; 16];
            OsRng.fill_bytes(&mut bytes);
            let candidate = format!("asset-{}", hex_bytes(&bytes));
            if !self.by_id.contains_key(&candidate) {
                break candidate;
            }
        };
        self.by_id.insert(
            id.clone(),
            AssetGrant {
                kind,
                hash,
                map_hash,
                locator,
            },
        );
        self.by_key.insert(key, id.clone());
        id
    }

    fn get(&self, id: &str) -> Option<&AssetGrant> {
        self.by_id.get(id)
    }
}

struct AuditLog {
    file: Option<File>,
}

impl AuditLog {
    fn new(path: Option<&Path>) -> Result<Self, io::Error> {
        let file = path
            .map(|path| OpenOptions::new().create(true).append(true).open(path))
            .transpose()?;
        Ok(Self { file })
    }

    fn event(
        &mut self,
        route: &str,
        method: &str,
        origin: Option<&str>,
        status: u16,
        bytes_out: usize,
        elapsed: Duration,
    ) {
        let event = json!({
            "schema": "ragelab.bridge.audit",
            "schemaVersion": 1,
            "atUnixMs": unix_millis(),
            "route": route,
            "method": method,
            "origin": origin,
            "status": status,
            "bytesOut": bytes_out,
            "durationMs": elapsed.as_secs_f64() * 1000.0
        });
        let line = event.to_string();
        eprintln!("{line}");
        if let Some(file) = self.file.as_mut() {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
    }
}

struct BridgeState {
    config: BridgeConfig,
    index: GtaRpfAssetIndex,
    token: String,
    grants: GrantStore,
    audit: AuditLog,
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    target: String,
    headers: BTreeMap<String, String>,
}

impl HttpRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    fn origin(&self) -> Option<&str> {
        self.header("origin")
    }
}

struct HttpResponse {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

impl HttpResponse {
    fn json(status: u16, value: Value) -> Self {
        Self {
            status,
            content_type: "application/json; charset=utf-8",
            body: value.to_string().into_bytes(),
        }
    }

    fn bytes(bytes: Vec<u8>) -> Self {
        Self {
            status: 200,
            content_type: "application/octet-stream",
            body: bytes,
        }
    }

    fn empty(status: u16) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            body: Vec::new(),
        }
    }
}

#[derive(Debug)]
struct BridgeHttpError {
    status: u16,
    code: &'static str,
    message: String,
}

impl BridgeHttpError {
    fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn response(self) -> HttpResponse {
        HttpResponse::json(
            self.status,
            json!({
                "schema": "ragelab.bridge.error",
                "schemaVersion": 1,
                "code": self.code,
                "message": self.message
            }),
        )
    }
}

pub fn serve(args: impl Iterator<Item = String>) -> Result<(), Box<dyn Error>> {
    let config = parse_config(args)?;
    let index = GtaRpfAssetIndex::load(&config.index)?;
    if !index.matches_installation(&config.game_root)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "selected GTA index does not match the selected GTA installation",
        )
        .into());
    }

    let token = random_token();
    let audit = AuditLog::new(config.audit_log.as_deref())?;
    let listener = TcpListener::bind(("127.0.0.1", config.port))?;
    let address = listener.local_addr()?;
    let ready = json!({
        "schema": "ragelab.bridge.ready",
        "schemaVersion": 1,
        "url": format!("http://127.0.0.1:{}", address.port()),
        "token": token,
        "allowedOrigins": config.origins,
        "readOnly": true,
        "writesEnabled": false,
        "limits": {
            "maxAssetBytes": config.max_asset_bytes,
            "maxBundleAssets": config.max_bundle_assets,
            "maxBundleBytes": config.max_bundle_bytes,
            "maxSearchResults": MAX_SEARCH_RESULTS
        }
    });
    println!("{ready}");
    io::stdout().flush()?;

    let mut state = BridgeState {
        config,
        index,
        token,
        grants: GrantStore::default(),
        audit,
    };

    for incoming in listener.incoming() {
        match incoming {
            Ok(mut stream) => {
                if let Err(error) = handle_connection(&mut state, &mut stream) {
                    eprintln!(
                        "{}",
                        json!({
                            "schema": "ragelab.bridge.server-error",
                            "schemaVersion": 1,
                            "code": "connectionFailed",
                            "message": error.to_string()
                        })
                    );
                }
            }
            Err(error) => {
                eprintln!(
                    "{}",
                    json!({
                        "schema": "ragelab.bridge.server-error",
                        "schemaVersion": 1,
                        "code": "acceptFailed",
                        "message": error.to_string()
                    })
                );
            }
        }
    }
    Ok(())
}

fn parse_config(args: impl Iterator<Item = String>) -> Result<BridgeConfig, Box<dyn Error>> {
    let mut game_root = None;
    let mut index = None;
    let mut keys = None;
    let mut origins = BTreeSet::new();
    let mut port = DEFAULT_PORT;
    let mut max_asset_bytes = DEFAULT_MAX_ASSET_BYTES;
    let mut max_bundle_assets = DEFAULT_MAX_BUNDLE_ASSETS;
    let mut max_bundle_bytes = DEFAULT_MAX_BUNDLE_BYTES;
    let mut audit_log = None;

    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--game-root" if game_root.is_none() => {
                game_root = Some(PathBuf::from(next_option_value(&mut args, "--game-root")?));
            }
            "--index" if index.is_none() => {
                index = Some(PathBuf::from(next_option_value(&mut args, "--index")?));
            }
            "--keys" if keys.is_none() => {
                keys = Some(PathBuf::from(next_option_value(&mut args, "--keys")?));
            }
            "--origin" => {
                let origin = next_option_value(&mut args, "--origin")?;
                validate_origin(&origin)?;
                origins.insert(origin);
            }
            "--port" => {
                port = next_option_value(&mut args, "--port")?
                    .parse::<u16>()
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidInput, "--port must fit in u16")
                    })?;
                if port == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "--port must be within 1..=65535",
                    )
                    .into());
                }
            }
            "--max-asset-bytes" => {
                max_asset_bytes = parse_positive_u64(
                    &next_option_value(&mut args, "--max-asset-bytes")?,
                    "--max-asset-bytes",
                    HARD_MAX_ASSET_BYTES,
                )?;
            }
            "--max-bundle-assets" => {
                max_bundle_assets = parse_positive_usize(
                    &next_option_value(&mut args, "--max-bundle-assets")?,
                    "--max-bundle-assets",
                    HARD_MAX_BUNDLE_ASSETS,
                )?;
            }
            "--max-bundle-bytes" => {
                max_bundle_bytes = parse_positive_u64(
                    &next_option_value(&mut args, "--max-bundle-bytes")?,
                    "--max-bundle-bytes",
                    HARD_MAX_BUNDLE_BYTES,
                )?;
            }
            "--audit-log" if audit_log.is_none() => {
                audit_log = Some(PathBuf::from(next_option_value(&mut args, "--audit-log")?));
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unsupported bridge option: {arg}\n{USAGE}"),
                )
                .into());
            }
        }
    }

    let game_root = game_root.ok_or_else(|| invalid_config("--game-root is required"))?;
    let index = index.ok_or_else(|| invalid_config("--index is required"))?;
    let keys = keys.ok_or_else(|| invalid_config("--keys is required"))?;
    if origins.is_empty() {
        return Err(invalid_config("at least one explicit --origin is required").into());
    }
    if !game_root.is_absolute() || !game_root.is_dir() {
        return Err(invalid_config("--game-root must be an existing absolute directory").into());
    }
    if !index.is_absolute() || !index.is_file() {
        return Err(invalid_config("--index must be an existing absolute file").into());
    }
    if !keys.is_absolute() || !keys.is_dir() {
        return Err(invalid_config("--keys must be an existing absolute directory").into());
    }
    if let Some(path) = &audit_log {
        if !path.is_absolute() {
            return Err(invalid_config("--audit-log must be an absolute path").into());
        }
    }
    if max_bundle_bytes < max_asset_bytes {
        return Err(invalid_config(
            "--max-bundle-bytes must be greater than or equal to --max-asset-bytes",
        )
        .into());
    }

    Ok(BridgeConfig {
        game_root,
        index,
        keys,
        origins,
        port,
        max_asset_bytes,
        max_bundle_assets,
        max_bundle_bytes,
        audit_log,
    })
}

fn next_option_value<I: Iterator<Item = String>>(
    args: &mut std::iter::Peekable<I>,
    flag: &str,
) -> Result<String, io::Error> {
    args.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{flag} requires a value\n{USAGE}"),
        )
    })
}

fn parse_positive_u64(value: &str, flag: &str, hard_max: u64) -> Result<u64, io::Error> {
    let value = value.parse::<u64>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{flag} must be a positive integer"),
        )
    })?;
    if value == 0 || value > hard_max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{flag} must be within 1..={hard_max}"),
        ));
    }
    Ok(value)
}

fn parse_positive_usize(value: &str, flag: &str, hard_max: usize) -> Result<usize, io::Error> {
    let value = value.parse::<usize>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{flag} must be a positive integer"),
        )
    })?;
    if value == 0 || value > hard_max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{flag} must be within 1..={hard_max}"),
        ));
    }
    Ok(value)
}

fn invalid_config(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("{message}\n{USAGE}"))
}

fn validate_origin(origin: &str) -> Result<(), io::Error> {
    if origin == "*" || origin.eq_ignore_ascii_case("null") || origin.len() > 256 {
        return Err(invalid_config(
            "--origin must be an explicit http(s) origin",
        ));
    }
    let rest = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .ok_or_else(|| invalid_config("--origin must start with http:// or https://"))?;
    if rest.is_empty()
        || rest.contains('/')
        || rest.contains('?')
        || rest.contains('#')
        || rest.chars().any(char::is_whitespace)
    {
        return Err(invalid_config(
            "--origin must contain scheme + host[:port] only, without path/query/fragment",
        ));
    }
    Ok(())
}

fn handle_connection(state: &mut BridgeState, stream: &mut TcpStream) -> Result<(), io::Error> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let request = match read_request(stream) {
        Ok(request) => request,
        Err(error) => {
            let response =
                BridgeHttpError::new(400, "badRequest", "Malformed or oversized request")
                    .response();
            write_response(stream, &response, None)?;
            return Err(error);
        }
    };

    let started = Instant::now();
    let route = audit_route(&request.target);
    let origin = request.origin().map(str::to_string);
    let response = match route_request(state, &request) {
        Ok(response) => response,
        Err(error) => error.response(),
    };
    let cors_origin = origin
        .as_deref()
        .filter(|origin| state.config.origins.contains(*origin));
    write_response(stream, &response, cors_origin)?;
    state.audit.event(
        route,
        &request.method,
        origin.as_deref(),
        response.status,
        response.body.len(),
        started.elapsed(),
    );
    Ok(())
}

fn read_request(stream: &mut TcpStream) -> Result<HttpRequest, io::Error> {
    let clone = stream.try_clone()?;
    let mut reader = BufReader::new(clone);
    let mut total = 0_usize;

    let mut request_line = String::new();
    let read = reader.read_line(&mut request_line)?;
    total += read;
    if read == 0 || total > MAX_HEADER_BYTES || request_line.len() > MAX_TARGET_BYTES + 64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid request line",
        ));
    }
    let parts = request_line.split_whitespace().collect::<Vec<_>>();
    if parts.len() != 3 || !parts[2].starts_with("HTTP/1.") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid HTTP request line",
        ));
    }
    if parts[1].len() > MAX_TARGET_BYTES || !parts[1].starts_with('/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid HTTP target",
        ));
    }

    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        total = total.saturating_add(read);
        if read == 0 || total > MAX_HEADER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "request headers exceed limit",
            ));
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "malformed request header",
            ));
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        if matches!(name.as_str(), "authorization" | "origin") {
            if headers.insert(name, value).is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "duplicate security-sensitive header",
                ));
            }
        } else {
            headers.entry(name).or_insert(value);
        }
    }

    if headers
        .get("transfer-encoding")
        .is_some_and(|value| !value.eq_ignore_ascii_case("identity"))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "request bodies are not supported",
        ));
    }
    if headers
        .get("content-length")
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length != 0)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "request bodies are not supported",
        ));
    }

    Ok(HttpRequest {
        method: parts[0].to_string(),
        target: parts[1].to_string(),
        headers,
    })
}

fn route_request(
    state: &mut BridgeState,
    request: &HttpRequest,
) -> Result<HttpResponse, BridgeHttpError> {
    let origin = request.origin().ok_or_else(|| {
        BridgeHttpError::new(
            403,
            "originRequired",
            "An explicit browser Origin is required",
        )
    })?;
    if !state.config.origins.contains(origin) {
        return Err(BridgeHttpError::new(
            403,
            "originDenied",
            "Browser Origin is not allowed for this bridge launch",
        ));
    }

    if request.method == "OPTIONS" {
        return Ok(HttpResponse::empty(204));
    }
    if request.method != "GET" {
        return Err(BridgeHttpError::new(
            405,
            "readOnly",
            "The local bridge is read-only; only GET and OPTIONS are allowed",
        ));
    }

    let expected = format!("Bearer {}", state.token);
    let provided = request.header("authorization").unwrap_or_default();
    if !constant_time_eq(expected.as_bytes(), provided.as_bytes()) {
        return Err(BridgeHttpError::new(
            401,
            "unauthorized",
            "Valid per-launch bearer capability required",
        ));
    }

    let (path, query) = split_target(&request.target);
    match path {
        "/v1/health" => Ok(health_response(state)),
        "/v1/search" => search_response(state, query),
        _ if path.starts_with("/v1/assets/") => {
            let id = &path["/v1/assets/".len()..];
            asset_response(state, id)
        }
        _ if path.starts_with("/v1/ymap-bundle/") => {
            let id = &path["/v1/ymap-bundle/".len()..];
            ymap_bundle_response(state, id)
        }
        _ => Err(BridgeHttpError::new(
            404,
            "notFound",
            "Bridge endpoint not found",
        )),
    }
}

fn health_response(state: &BridgeState) -> HttpResponse {
    HttpResponse::json(
        200,
        json!({
            "schema": "ragelab.bridge.health",
            "schemaVersion": 1,
            "connected": true,
            "readOnly": true,
            "writesEnabled": false,
            "capabilities": [
                "search",
                "asset.read",
                "ymap.bundle"
            ],
            "limits": {
                "maxAssetBytes": state.config.max_asset_bytes,
                "maxBundleAssets": state.config.max_bundle_assets,
                "maxBundleBytes": state.config.max_bundle_bytes,
                "maxSearchResults": MAX_SEARCH_RESULTS
            }
        }),
    )
}

fn search_response(
    state: &mut BridgeState,
    query: Option<&str>,
) -> Result<HttpResponse, BridgeHttpError> {
    let params = parse_query(query.unwrap_or_default())?;
    let q = params
        .get("q")
        .map(String::as_str)
        .unwrap_or_default()
        .trim();
    if q.is_empty() || q.chars().count() > MAX_SEARCH_CHARS {
        return Err(BridgeHttpError::new(
            400,
            "invalidSearch",
            format!(
                "Search query must contain 1..={MAX_SEARCH_CHARS} characters (Core requires at least two for text search)"
            ),
        ));
    }
    let limit = params
        .get("limit")
        .map(|value| value.parse::<usize>())
        .transpose()
        .map_err(|_| BridgeHttpError::new(400, "invalidSearch", "Search limit must be an integer"))?
        .unwrap_or(30);
    if !(1..=MAX_SEARCH_RESULTS).contains(&limit) {
        return Err(BridgeHttpError::new(
            400,
            "invalidSearch",
            format!("Search limit must be within 1..={MAX_SEARCH_RESULTS}"),
        ));
    }

    let report = state.index.search_browser(q, limit).map_err(|_| {
        BridgeHttpError::new(
            400,
            "invalidSearch",
            "Core rejected the search query or result limit",
        )
    })?;
    let results = report
        .results
        .into_iter()
        .map(|result| sanitize_search_result(&mut state.grants, result))
        .collect::<Vec<_>>();

    Ok(HttpResponse::json(
        200,
        json!({
            "schema": "ragelab.bridge.search",
            "schemaVersion": 1,
            "query": q,
            "results": results,
            "truncated": report.truncated
        }),
    ))
}

fn sanitize_search_result(grants: &mut GrantStore, result: GtaRpfBrowserSearchResult) -> Value {
    let (asset_id, format) = match result.kind {
        GtaRpfBrowserResultKind::Ymap => {
            let map_hash = result.map_hash.or(Some(result.hash));
            let id = grants.grant(
                GtaRpfAssetKind::Ymap,
                result.hash,
                map_hash,
                result.provider.clone(),
            );
            (Some(id), Some("ymap"))
        }
        GtaRpfBrowserResultKind::Asset => {
            match (
                result.asset_kind,
                result.asset_provider.clone(),
                result.asset_hash,
            ) {
                (Some(kind), Some(provider), Some(hash)) => {
                    let id = grants.grant(kind, hash, None, provider);
                    (Some(id), Some(kind.extension()))
                }
                _ => (None, None),
            }
        }
        GtaRpfBrowserResultKind::Archetype => {
            match (
                result.asset_kind,
                result.asset_provider.clone(),
                result.asset_hash,
            ) {
                (Some(kind), Some(provider), Some(hash)) => {
                    let id = grants.grant(kind, hash, result.map_hash, provider);
                    (Some(id), Some(kind.extension()))
                }
                _ => (None, None),
            }
        }
    };

    json!({
        "kind": browser_kind_name(result.kind),
        "hash": format!("0x{:08X}", result.hash),
        "label": result.label,
        "assetId": asset_id,
        "format": format,
        "mapHash": result.map_hash.map(|hash| format!("0x{hash:08X}")),
        "entityIndex": result.entity_index,
        "position": result.position,
        "bounds": result.bounds,
        "entityCount": result.entity_count
    })
}

fn asset_response(state: &BridgeState, id: &str) -> Result<HttpResponse, BridgeHttpError> {
    validate_opaque_id(id)?;
    let grant = state.grants.get(id).ok_or_else(|| {
        BridgeHttpError::new(404, "unknownAsset", "Unknown or expired opaque asset ID")
    })?;
    let bytes = read_grant_bounded(state, grant)?;
    Ok(HttpResponse::bytes(bytes))
}

fn ymap_bundle_response(
    state: &mut BridgeState,
    id: &str,
) -> Result<HttpResponse, BridgeHttpError> {
    validate_opaque_id(id)?;
    let grant = state.grants.get(id).cloned().ok_or_else(|| {
        BridgeHttpError::new(404, "unknownAsset", "Unknown or expired opaque asset ID")
    })?;
    if grant.kind != GtaRpfAssetKind::Ymap {
        return Err(BridgeHttpError::new(
            400,
            "notYmap",
            "YMAP bundle endpoint requires an opaque YMAP asset ID",
        ));
    }
    let map_hash = grant.map_hash.ok_or_else(|| {
        BridgeHttpError::new(
            409,
            "missingMapIdentity",
            "Selected YMAP grant has no indexed map identity",
        )
    })?;
    let record = state
        .index
        .world_map_candidates(map_hash)
        .iter()
        .find(|record| record.provider == grant.locator)
        .cloned()
        .ok_or_else(|| {
            BridgeHttpError::new(
                409,
                "mapProviderMismatch",
                "Selected YMAP provider is no longer present in the loaded index",
            )
        })?;
    let plan = state
        .index
        .plan_for_archetypes(record.archetype_hashes.iter().copied());

    let mut specs = vec![BundleSpec {
        role: "ymap",
        kind: GtaRpfAssetKind::Ymap,
        hash: grant.hash,
        locator: grant.locator.clone(),
    }];
    for locator in &plan.provider_entries {
        specs.push(BundleSpec {
            role: "archetypeProvider",
            kind: GtaRpfAssetKind::Ytyp,
            hash: hash_for_locator(locator),
            locator: locator.clone(),
        });
    }
    for locator in &plan.asset_entries {
        let kind = kind_from_locator(locator).ok_or_else(|| {
            BridgeHttpError::new(
                500,
                "indexContract",
                "Core index plan contained an unsupported model asset extension",
            )
        })?;
        specs.push(BundleSpec {
            role: "model",
            kind,
            hash: hash_for_locator(locator),
            locator: locator.clone(),
        });
    }
    for locator in &plan.texture_entries {
        specs.push(BundleSpec {
            role: "texture",
            kind: GtaRpfAssetKind::Ytd,
            hash: hash_for_locator(locator),
            locator: locator.clone(),
        });
    }

    let mut seen = BTreeSet::new();
    specs.retain(|spec| seen.insert(spec.locator.clone()));
    if specs.len() > state.config.max_bundle_assets {
        return Err(BridgeHttpError::new(
            413,
            "bundleAssetLimit",
            format!(
                "YMAP bundle requires {} files; configured limit is {}",
                specs.len(),
                state.config.max_bundle_assets
            ),
        ));
    }

    let declared = preflight_bundle_sizes(state, &specs)?;
    let mut actual_total = 0_u64;
    let mut files = Vec::with_capacity(specs.len());
    for spec in specs {
        let materialized = spec
            .locator
            .materialize(&state.config.game_root, &state.config.keys);
        let bytes = materialized.read().map_err(|_| {
            BridgeHttpError::new(
                502,
                "assetReadFailed",
                "Core failed to read a selected RPF asset",
            )
        })?;
        let byte_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if byte_len > state.config.max_asset_bytes {
            return Err(BridgeHttpError::new(
                413,
                "assetLimit",
                "Asset exceeded configured byte limit while reading",
            ));
        }
        actual_total = actual_total.checked_add(byte_len).ok_or_else(|| {
            BridgeHttpError::new(413, "bundleByteLimit", "YMAP bundle byte count overflow")
        })?;
        if actual_total > state.config.max_bundle_bytes {
            return Err(BridgeHttpError::new(
                413,
                "bundleByteLimit",
                "YMAP bundle exceeded configured byte limit while reading",
            ));
        }
        let opaque_id = state.grants.grant(
            spec.kind,
            spec.hash,
            if spec.kind == GtaRpfAssetKind::Ymap {
                Some(map_hash)
            } else {
                None
            },
            spec.locator.clone(),
        );
        files.push(json!({
            "role": spec.role,
            "assetId": opaque_id,
            "name": locator_label(&spec.locator),
            "format": spec.kind.extension(),
            "hash": format!("0x{:08X}", spec.hash),
            "byteLength": bytes.len(),
            "bytesBase64": BASE64.encode(&bytes)
        }));
    }

    Ok(HttpResponse::json(
        200,
        json!({
            "schema": "ragelab.bridge.ymap-bundle",
            "schemaVersion": 1,
            "mapHash": format!("0x{map_hash:08X}"),
            "files": files,
            "limits": {
                "assetCount": files.len(),
                "declaredBytes": declared,
                "actualBytes": actual_total,
                "maxAssets": state.config.max_bundle_assets,
                "maxBytes": state.config.max_bundle_bytes
            },
            "diagnostics": plan_diagnostics(&plan)
        }),
    ))
}

struct BundleSpec {
    role: &'static str,
    kind: GtaRpfAssetKind,
    hash: u32,
    locator: GtaRpfIndexLocator,
}

fn preflight_bundle_sizes(
    state: &BridgeState,
    specs: &[BundleSpec],
) -> Result<u64, BridgeHttpError> {
    let mut total = 0_u64;
    for spec in specs {
        let materialized = spec
            .locator
            .materialize(&state.config.game_root, &state.config.keys);
        let info = materialized.info().map_err(|_| {
            BridgeHttpError::new(
                502,
                "assetMetadataFailed",
                "Core failed to inspect a selected RPF asset before reading",
            )
        })?;
        let declared = u64::from(info.size.max(info.memory_size));
        if declared > state.config.max_asset_bytes {
            return Err(BridgeHttpError::new(
                413,
                "assetLimit",
                format!(
                    "Bundle contains an asset declaring {declared} bytes; configured per-asset limit is {}",
                    state.config.max_asset_bytes
                ),
            ));
        }
        total = total.checked_add(declared).ok_or_else(|| {
            BridgeHttpError::new(413, "bundleByteLimit", "YMAP bundle byte count overflow")
        })?;
        if total > state.config.max_bundle_bytes {
            return Err(BridgeHttpError::new(
                413,
                "bundleByteLimit",
                format!(
                    "YMAP bundle declares more than configured {} bytes",
                    state.config.max_bundle_bytes
                ),
            ));
        }
    }
    Ok(total)
}

fn read_grant_bounded(state: &BridgeState, grant: &AssetGrant) -> Result<Vec<u8>, BridgeHttpError> {
    let materialized = grant
        .locator
        .materialize(&state.config.game_root, &state.config.keys);
    read_with_metadata_preflight(
        state.config.max_asset_bytes,
        || {
            let info = materialized.info().map_err(|_| {
                BridgeHttpError::new(
                    502,
                    "assetMetadataFailed",
                    "Core failed to inspect the selected RPF asset",
                )
            })?;
            Ok(u64::from(info.size.max(info.memory_size)))
        },
        || {
            materialized.read().map_err(|_| {
                BridgeHttpError::new(
                    502,
                    "assetReadFailed",
                    "Core failed to read the selected RPF asset",
                )
            })
        },
    )
}

fn read_with_metadata_preflight<M, R>(
    max_bytes: u64,
    metadata: M,
    read: R,
) -> Result<Vec<u8>, BridgeHttpError>
where
    M: FnOnce() -> Result<u64, BridgeHttpError>,
    R: FnOnce() -> Result<Vec<u8>, BridgeHttpError>,
{
    let declared = metadata()?;
    if declared > max_bytes {
        return Err(BridgeHttpError::new(
            413,
            "assetLimit",
            format!("Asset declares {declared} bytes; configured limit is {max_bytes}"),
        ));
    }

    let bytes = read()?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_bytes {
        return Err(BridgeHttpError::new(
            413,
            "assetLimit",
            "Asset exceeded configured limit while reading",
        ));
    }
    Ok(bytes)
}

fn plan_diagnostics(plan: &GtaRpfIndexPlan) -> Value {
    json!({
        "requestedArchetypes": plan.requested_archetypes,
        "unresolvedArchetypes": plan.unresolved_archetypes.iter().map(|hash| format!("0x{hash:08X}")).collect::<Vec<_>>(),
        "ambiguousArchetypes": plan.ambiguous_archetypes.iter().map(|hash| format!("0x{hash:08X}")).collect::<Vec<_>>(),
        "unresolvedAssets": plan.unresolved_assets.iter().map(|(kind, hash)| format!("{}:0x{hash:08X}", kind.extension())).collect::<Vec<_>>(),
        "ambiguousAssets": plan.ambiguous_assets.iter().map(|(kind, hash)| format!("{}:0x{hash:08X}", kind.extension())).collect::<Vec<_>>(),
        "ambiguousTextureParents": plan.ambiguous_texture_parents.iter().map(|hash| format!("0x{hash:08X}")).collect::<Vec<_>>(),
        "textureParentCycles": plan.texture_parent_cycles.iter().map(|hash| format!("0x{hash:08X}")).collect::<Vec<_>>(),
        "collisionEntriesOmitted": plan.collision_entries.len()
    })
}

fn validate_opaque_id(id: &str) -> Result<(), BridgeHttpError> {
    let Some(hex) = id.strip_prefix("asset-") else {
        return Err(BridgeHttpError::new(
            400,
            "invalidAssetId",
            "Invalid opaque asset ID",
        ));
    };
    if hex.len() != 32 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(BridgeHttpError::new(
            400,
            "invalidAssetId",
            "Invalid opaque asset ID",
        ));
    }
    Ok(())
}

fn browser_kind_name(kind: GtaRpfBrowserResultKind) -> &'static str {
    match kind {
        GtaRpfBrowserResultKind::Ymap => "ymap",
        GtaRpfBrowserResultKind::Archetype => "archetype",
        GtaRpfBrowserResultKind::Asset => "asset",
    }
}

fn kind_from_locator(locator: &GtaRpfIndexLocator) -> Option<GtaRpfAssetKind> {
    match Path::new(&locator.entry)
        .extension()
        .and_then(|value| value.to_str())?
        .to_ascii_lowercase()
        .as_str()
    {
        "ytyp" => Some(GtaRpfAssetKind::Ytyp),
        "ymap" => Some(GtaRpfAssetKind::Ymap),
        "ydr" => Some(GtaRpfAssetKind::Ydr),
        "ydd" => Some(GtaRpfAssetKind::Ydd),
        "ytd" => Some(GtaRpfAssetKind::Ytd),
        "ybn" => Some(GtaRpfAssetKind::Ybn),
        "yft" => Some(GtaRpfAssetKind::Yft),
        _ => None,
    }
}

fn hash_for_locator(locator: &GtaRpfIndexLocator) -> u32 {
    Path::new(&locator.entry)
        .file_stem()
        .and_then(|value| value.to_str())
        .map(joaat)
        .unwrap_or(0)
}

fn locator_label(locator: &GtaRpfIndexLocator) -> String {
    Path::new(&locator.entry)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("asset.bin")
        .to_string()
}

fn split_target(target: &str) -> (&str, Option<&str>) {
    match target.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (target, None),
    }
}

fn parse_query(query: &str) -> Result<BTreeMap<String, String>, BridgeHttpError> {
    let mut values = BTreeMap::new();
    if query.is_empty() {
        return Ok(values);
    }
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = percent_decode(key)?;
        let value = percent_decode(value)?;
        if values.insert(key, value).is_some() {
            return Err(BridgeHttpError::new(
                400,
                "invalidQuery",
                "Duplicate query parameter",
            ));
        }
    }
    Ok(values)
}

fn percent_decode(value: &str) -> Result<String, BridgeHttpError> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' => {
                if index + 2 >= bytes.len() {
                    return Err(BridgeHttpError::new(
                        400,
                        "invalidQuery",
                        "Invalid percent encoding",
                    ));
                }
                let high = hex_nibble(bytes[index + 1]).ok_or_else(|| {
                    BridgeHttpError::new(400, "invalidQuery", "Invalid percent encoding")
                })?;
                let low = hex_nibble(bytes[index + 2]).ok_or_else(|| {
                    BridgeHttpError::new(400, "invalidQuery", "Invalid percent encoding")
                })?;
                out.push((high << 4) | low);
                index += 3;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(out)
        .map_err(|_| BridgeHttpError::new(400, "invalidQuery", "Query must be valid UTF-8"))
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn write_response(
    stream: &mut TcpStream,
    response: &HttpResponse,
    cors_origin: Option<&str>,
) -> Result<(), io::Error> {
    let mut headers = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n",
        response.status,
        reason_phrase(response.status),
        response.content_type,
        response.body.len()
    );
    if let Some(origin) = cors_origin {
        headers.push_str(&format!(
            "Access-Control-Allow-Origin: {origin}\r\nVary: Origin\r\nAccess-Control-Allow-Methods: GET, OPTIONS\r\nAccess-Control-Allow-Headers: Authorization, Content-Type\r\nAccess-Control-Max-Age: 300\r\n"
        ));
    }
    headers.push_str("\r\n");
    stream.write_all(headers.as_bytes())?;
    stream.write_all(&response.body)?;
    stream.flush()
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        _ => "Error",
    }
}

fn audit_route(target: &str) -> &'static str {
    let (path, _) = split_target(target);
    if path == "/v1/health" {
        "/v1/health"
    } else if path == "/v1/search" {
        "/v1/search"
    } else if path.starts_with("/v1/assets/") {
        "/v1/assets/:id"
    } else if path.starts_with("/v1/ymap-bundle/") {
        "/v1/ymap-bundle/:id"
    } else {
        "unknown"
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut diff = left.len() ^ right.len();
    let max_len = left.len().max(right.len());
    for index in 0..max_len {
        let a = left.get(index).copied().unwrap_or(0);
        let b = right.get(index).copied().unwrap_or(0);
        diff |= usize::from(a ^ b);
    }
    diff == 0
}

fn random_token() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    hex_bytes(&bytes)
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[(byte >> 4) as usize]));
        output.push(char::from(HEX[(byte & 0x0f) as usize]));
    }
    output
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ragelab_engine::{GtaRpfWorldBounds, GtaRpfWorldPoint};

    fn locator() -> GtaRpfIndexLocator {
        GtaRpfIndexLocator {
            archive_relative: "secret/update/update.rpf".into(),
            nested: vec!["hidden/common.rpf".into()],
            entry: "levels/gta5/test_drawable.ydr".into(),
            load_rank: 42,
        }
    }

    #[test]
    fn origin_policy_rejects_wildcards_paths_and_null() {
        assert!(validate_origin("*").is_err());
        assert!(validate_origin("null").is_err());
        assert!(validate_origin("http://127.0.0.1:1420/path").is_err());
        assert!(validate_origin("http://127.0.0.1:1420").is_ok());
        assert!(validate_origin("https://tools.example.test").is_ok());
    }

    #[test]
    fn query_decoder_is_bounded_and_utf8_aware() {
        let values = parse_query("q=picnic%20table&limit=10").expect("query");
        assert_eq!(values.get("q").map(String::as_str), Some("picnic table"));
        assert_eq!(values.get("limit").map(String::as_str), Some("10"));
        assert!(parse_query("q=a&q=b").is_err());
        assert!(parse_query("q=%ZZ").is_err());
    }

    #[test]
    fn bearer_comparison_handles_length_and_content() {
        assert!(constant_time_eq(b"Bearer abc", b"Bearer abc"));
        assert!(!constant_time_eq(b"Bearer abc", b"Bearer abd"));
        assert!(!constant_time_eq(b"Bearer abc", b"Bearer abc0"));
    }

    #[test]
    fn sanitized_search_result_never_serializes_locator_paths() {
        let mut grants = GrantStore::default();
        let value = sanitize_search_result(
            &mut grants,
            GtaRpfBrowserSearchResult {
                kind: GtaRpfBrowserResultKind::Asset,
                hash: 0x1234,
                label: "test_drawable.ydr".into(),
                provider: locator(),
                asset_kind: Some(GtaRpfAssetKind::Ydr),
                asset_hash: Some(0x1234),
                asset_provider: Some(locator()),
                map_hash: None,
                entity_index: None,
                position: Some(GtaRpfWorldPoint {
                    x: 1.0,
                    y: 2.0,
                    z: 3.0,
                }),
                bounds: Some(GtaRpfWorldBounds {
                    min: GtaRpfWorldPoint {
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                    },
                    max: GtaRpfWorldPoint {
                        x: 1.0,
                        y: 1.0,
                        z: 1.0,
                    },
                }),
                entity_count: None,
            },
        );
        let serialized = value.to_string();
        assert!(serialized.contains("test_drawable.ydr"));
        assert!(serialized.contains("asset-"));
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("hidden"));
        assert!(!serialized.contains("update.rpf"));
    }

    #[test]
    fn opaque_asset_ids_are_strict() {
        assert!(validate_opaque_id("asset-0123456789abcdef0123456789abcdef").is_ok());
        assert!(validate_opaque_id("asset-1234").is_err());
        assert!(validate_opaque_id("../secret").is_err());
    }

    #[test]
    fn audit_routes_drop_query_and_opaque_values() {
        assert_eq!(audit_route("/v1/search?q=secret"), "/v1/search");
        assert_eq!(
            audit_route("/v1/assets/asset-0123456789abcdef0123456789abcdef"),
            "/v1/assets/:id"
        );
    }

    #[test]
    fn parse_config_accepts_flags_only_and_enforces_hard_limits() {
        let root = std::env::temp_dir().join(format!(
            "ragelab-bridge-config-{}-{}",
            std::process::id(),
            unix_millis()
        ));
        let game_root = root.join("game");
        let keys = root.join("keys");
        let index = root.join("legacy-v5.bin");
        std::fs::create_dir_all(&game_root).expect("game root");
        std::fs::create_dir_all(&keys).expect("keys root");
        std::fs::write(&index, b"test-index-placeholder").expect("index placeholder");

        let base = vec![
            "--game-root".to_string(),
            game_root.display().to_string(),
            "--index".to_string(),
            index.display().to_string(),
            "--keys".to_string(),
            keys.display().to_string(),
            "--origin".to_string(),
            "http://127.0.0.1:5173".to_string(),
        ];

        let config = parse_config(base.clone().into_iter()).expect("flags-only bridge config");
        assert_eq!(config.port, DEFAULT_PORT);
        assert_eq!(config.max_asset_bytes, DEFAULT_MAX_ASSET_BYTES);
        assert!(config.origins.contains("http://127.0.0.1:5173"));

        let mut too_large = base.clone();
        too_large.extend([
            "--max-asset-bytes".to_string(),
            (HARD_MAX_ASSET_BYTES + 1).to_string(),
        ]);
        assert!(parse_config(too_large.into_iter()).is_err());

        let mut inverted = base;
        inverted.extend([
            "--max-asset-bytes".to_string(),
            "4096".to_string(),
            "--max-bundle-bytes".to_string(),
            "2048".to_string(),
        ]);
        assert!(parse_config(inverted.into_iter()).is_err());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn metadata_limit_rejects_before_payload_reader_runs() {
        let read_called = std::cell::Cell::new(false);
        let error = read_with_metadata_preflight(
            1024,
            || Ok(2048),
            || {
                read_called.set(true);
                Ok(vec![0_u8; 2048])
            },
        )
        .expect_err("oversized metadata must fail closed");

        assert_eq!(error.status, 413);
        assert_eq!(error.code, "assetLimit");
        assert!(
            !read_called.get(),
            "payload reader must not run before preflight passes"
        );
    }

    #[test]
    fn metadata_preflight_rechecks_actual_payload_size() {
        let error = read_with_metadata_preflight(1024, || Ok(512), || Ok(vec![0_u8; 1536]))
            .expect_err("actual payload must remain bounded after metadata preflight");
        assert_eq!(error.status, 413);
        assert_eq!(error.code, "assetLimit");
    }
}

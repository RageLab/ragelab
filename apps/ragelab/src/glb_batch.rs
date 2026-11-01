use std::{
    collections::{BTreeSet, HashSet},
    error::Error,
    fs::{self, File},
    io::{self, Read},
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
};

use ragelab_engine::{
    export_glb_to_ydr, export_glb_to_ydr_with_nodes, GlbShaderTemplateFormat,
    GlbShaderTemplateInput, GlbYdrExportRequest, GlbYdrExportResult,
};
use serde::Deserialize;
use serde_json::{json, Value};

const MANIFEST_SCHEMA: &str = "ragelab.glb.batch-manifest";
const REPORT_SCHEMA: &str = "ragelab.glb.batch-report";
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_PROFILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TEMPLATE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ITEMS: usize = 128;
const MAX_CONCURRENCY: usize = 8;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    schema: String,
    schema_version: u32,
    failure_policy: FailurePolicy,
    concurrency: usize,
    report: PathBuf,
    items: Vec<ManifestItem>,
}

#[derive(Debug, Clone, Copy, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FailurePolicy {
    FailFast,
    Continue,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManifestItem {
    id: String,
    source: PathBuf,
    output: PathBuf,
    profile: PathBuf,
    #[serde(deserialize_with = "required_nullable_path")]
    shader_template: Option<PathBuf>,
    shader_index: usize,
    nodes: Vec<usize>,
    report: PathBuf,
}

fn required_nullable_path<'de, D>(d: D) -> Result<Option<PathBuf>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<PathBuf>::deserialize(d)
}

#[derive(Debug)]
struct PlannedItem {
    item: ManifestItem,
    source: PathBuf,
    output: PathBuf,
    profile: PathBuf,
    template: Option<PathBuf>,
    report: PathBuf,
    source_hash: String,
    profile_hash: String,
    template_hash: Option<String>,
}

struct ExportedItem {
    result: GlbYdrExportResult,
    source_bytes: Vec<u8>,
    profile_bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
struct ItemResult {
    id: String,
    status: &'static str,
    source: String,
    output: String,
    report: String,
    source_sha256: Option<String>,
    output_sha256: Option<String>,
    error: Option<String>,
}

impl ItemResult {
    fn cancelled(item: &PlannedItem) -> Self {
        Self {
            id: item.item.id.clone(),
            status: "cancelled",
            source: display(&item.item.source),
            output: display(&item.item.output),
            report: display(&item.item.report),
            source_sha256: Some(item.source_hash.clone()),
            output_sha256: None,
            error: Some(
                "not started because fail_fast stopped scheduling after an earlier failure".into(),
            ),
        }
    }
    fn value(&self) -> Value {
        json!({
            "id": self.id, "status": self.status, "source": self.source,
            "output": self.output, "report": self.report,
            "sourceSha256": self.source_sha256, "outputSha256": self.output_sha256,
            "error": self.error
        })
    }
}

#[derive(Debug)]
struct BatchRun {
    report_path: PathBuf,
    report: Value,
    succeeded: usize,
    failed: usize,
    cancelled: usize,
}

#[derive(Debug)]
pub(crate) struct BatchCommandError {
    pub exit_code: i32,
    pub error_code: &'static str,
    pub message: String,
    pub report: Value,
}
impl std::fmt::Display for BatchCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl Error for BatchCommandError {}

#[derive(Debug)]
struct Scheduler {
    next: usize,
    stopped: bool,
}

pub(crate) fn run(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    const USAGE: &str = "usage: ragelab glb batch <manifest.json> [--json]";
    let mut manifest = None;
    let mut json_output = false;
    for arg in args {
        match arg.as_str() {
            "--json" if !json_output => json_output = true,
            value if value.starts_with('-') => {
                return Err(invalid_input(format!("unknown option {value}\n{USAGE}")).into())
            }
            value => {
                if manifest.replace(PathBuf::from(value)).is_some() {
                    return Err(invalid_input(USAGE).into());
                }
            }
        }
    }
    let batch = run_manifest(manifest.as_deref().ok_or_else(|| invalid_input(USAGE))?)?;
    let mut data = batch.report;
    data.as_object_mut()
        .ok_or_else(|| invalid_data("batch report must be an object"))?
        .insert("aggregateReport".into(), json!(batch.report_path));
    if batch.failed > 0 || batch.cancelled > 0 {
        let message = format!(
            "GLB batch completed: {} succeeded, {} failed, {} cancelled; report: {}",
            batch.succeeded,
            batch.failed,
            batch.cancelled,
            batch.report_path.display()
        );
        if !json_output {
            print_summary(&data);
        }
        return Err(Box::new(BatchCommandError {
            exit_code: 1,
            error_code: "batch_failed",
            message,
            report: data,
        }));
    }
    if json_output {
        super::agent::print_success("glb.batch", data)?;
    } else {
        print_summary(&data);
    }
    Ok(())
}

fn run_manifest(path: &Path) -> Result<BatchRun, Box<dyn Error>> {
    run_manifest_with_executor(path, execute_item)
}

fn run_manifest_with_executor<F>(path: &Path, executor: F) -> Result<BatchRun, Box<dyn Error>>
where
    F: Fn(&PlannedItem) -> Result<ItemResult, String> + Sync,
{
    let manifest_path = path.canonicalize()?;
    let root = manifest_path
        .parent()
        .ok_or_else(|| invalid_input("manifest needs a parent directory"))?
        .canonicalize()?;
    let manifest_bytes = read_bounded(&manifest_path, MAX_MANIFEST_BYTES, "manifest")?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| invalid_data(format!("cannot parse batch manifest JSON: {e}")))?;
    validate_header(&manifest)?;

    let aggregate_path = resolve_destination(&root, &manifest.report, "aggregate report")?;
    if extension(&aggregate_path) != "json" {
        return Err(invalid_input("aggregate report must use the .json extension").into());
    }
    let mut ids = HashSet::new();
    let mut input_keys = HashSet::new();
    input_keys.insert(path_key(&manifest_path));
    let mut planned = Vec::with_capacity(manifest.items.len());

    for item in &manifest.items {
        validate_item(item, &mut ids)?;
        let source = resolve_input(&root, &item.source, MAX_SOURCE_BYTES, "source")?;
        let profile = resolve_input(&root, &item.profile, MAX_PROFILE_BYTES, "profile")?;
        let template = item
            .shader_template
            .as_ref()
            .map(|p| resolve_input(&root, p, MAX_TEMPLATE_BYTES, "shader template"))
            .transpose()?;
        input_keys.insert(path_key(&source));
        input_keys.insert(path_key(&profile));
        if let Some(p) = &template {
            input_keys.insert(path_key(p));
        }
        if template.is_none() && item.shader_index != 0 {
            return Err(invalid_input(format!(
                "item {} uses shaderIndex without shaderTemplate; set shaderIndex=0",
                item.id
            ))
            .into());
        }

        let output = resolve_destination(&root, &item.output, "output")?;
        let report = resolve_destination(&root, &item.report, "item report")?;
        if extension(&output) != "ydr" {
            return Err(invalid_input(format!("item {} output must end in .ydr", item.id)).into());
        }
        if extension(&report) != "json" {
            return Err(invalid_input(format!("item {} report must end in .json", item.id)).into());
        }
        if path_key(&output) == path_key(&report) {
            return Err(invalid_input(format!("item {} output/report collision", item.id)).into());
        }
        planned.push(PlannedItem {
            item: item.clone(),
            source,
            output,
            profile,
            template,
            report,
            source_hash: String::new(),
            profile_hash: String::new(),
            template_hash: None,
        });
    }

    // Validate the complete destination set before exporting or publishing anything.
    let mut destinations = HashSet::new();
    let mut all_destinations = vec![aggregate_path.clone()];
    for item in &planned {
        all_destinations.push(item.output.clone());
        all_destinations.push(item.report.clone());
    }
    for destination in &all_destinations {
        let key = path_key(destination);
        if !destinations.insert(key.clone()) {
            return Err(invalid_input(format!(
                "batch destination collision: {}",
                destination.display()
            ))
            .into());
        }
        if input_keys.contains(&key) {
            return Err(invalid_input(format!(
                "batch destination aliases an input/manifest: {}",
                destination.display()
            ))
            .into());
        }
    }

    // Full export + item-report generation in memory for every entry. No output
    // path is created until every item clears this pass.
    for item in &mut planned {
        let exported = export_item(item)?;
        let value = build_item_report(
            item,
            &exported.source_bytes,
            &exported.profile_bytes,
            &exported.result,
        )?;
        let _ = serde_json::to_vec(&value)?;
        item.source_hash = exported.result.report.source_sha256.clone();
        item.profile_hash = exported.result.report.profile_sha256.clone();
        item.template_hash = exported
            .result
            .report
            .shader_template
            .as_ref()
            .map(|v| v.sha256.clone());
    }

    let planned = Arc::new(planned);
    let results = Arc::new(Mutex::new(vec![None::<ItemResult>; planned.len()]));
    let scheduler = Arc::new(Mutex::new(Scheduler {
        next: 0,
        stopped: false,
    }));
    let completed = Arc::new(AtomicUsize::new(0));
    thread::scope(|scope| {
        for _ in 0..manifest.concurrency.min(planned.len()) {
            let planned = Arc::clone(&planned);
            let results = Arc::clone(&results);
            let scheduler = Arc::clone(&scheduler);
            let completed = Arc::clone(&completed);
            let policy = manifest.failure_policy;
            let executor = &executor;
            scope.spawn(move || loop {
                let index = {
                    let mut state = scheduler.lock().unwrap_or_else(|p| p.into_inner());
                    if state.stopped || state.next == planned.len() {
                        None
                    } else {
                        let i = state.next;
                        state.next += 1;
                        Some(i)
                    }
                };
                let Some(index) = index else { break };
                let execution = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    executor(&planned[index])
                }))
                .unwrap_or_else(|_| Err("worker panicked while processing item".to_string()));
                let item_result = match execution {
                    Ok(result) => result,
                    Err(error) => ItemResult {
                        id: planned[index].item.id.clone(),
                        status: "failed",
                        source: display(&planned[index].item.source),
                        output: display(&planned[index].item.output),
                        report: display(&planned[index].item.report),
                        source_sha256: Some(planned[index].source_hash.clone()),
                        output_sha256: None,
                        error: Some(error),
                    },
                };
                if item_result.status == "failed" && policy == FailurePolicy::FailFast {
                    scheduler.lock().unwrap_or_else(|p| p.into_inner()).stopped = true;
                }
                results.lock().unwrap_or_else(|p| p.into_inner())[index] = Some(item_result);
                let count = completed.fetch_add(1, Ordering::Relaxed) + 1;
                eprintln!("[glb.batch] {count}/{} item(s) finished", planned.len());
            });
        }
    });

    let mut ordered = Vec::with_capacity(planned.len());
    let mut guard = results.lock().unwrap_or_else(|p| p.into_inner());
    for (i, item) in planned.iter().enumerate() {
        ordered.push(
            guard[i]
                .take()
                .unwrap_or_else(|| ItemResult::cancelled(item)),
        );
    }
    drop(guard);

    let succeeded = ordered.iter().filter(|v| v.status == "succeeded").count();
    let failed = ordered.iter().filter(|v| v.status == "failed").count();
    let cancelled = ordered.iter().filter(|v| v.status == "cancelled").count();
    let report = json!({
        "schema": REPORT_SCHEMA, "schemaVersion": 1,
        "manifest": manifest_path,
        "failurePolicy": manifest.failure_policy,
        "concurrency": manifest.concurrency,
        "summary": {"total": ordered.len(), "succeeded": succeeded, "failed": failed, "cancelled": cancelled},
        "items": ordered.iter().map(ItemResult::value).collect::<Vec<_>>()
    });
    let mut bytes = serde_json::to_vec_pretty(&report)?;
    bytes.push(b'\n');
    super::glb_export_report::write_bytes_atomic(&aggregate_path, &bytes)?;
    Ok(BatchRun {
        report_path: aggregate_path,
        report,
        succeeded,
        failed,
        cancelled,
    })
}

fn validate_header(m: &Manifest) -> Result<(), io::Error> {
    if m.schema != MANIFEST_SCHEMA {
        return Err(invalid_input(format!("schema must be {MANIFEST_SCHEMA}")));
    }
    if m.schema_version != 1 {
        return Err(invalid_input("schemaVersion must be 1"));
    }
    if !(1..=MAX_CONCURRENCY).contains(&m.concurrency) {
        return Err(invalid_input(format!(
            "concurrency must be 1..={MAX_CONCURRENCY}"
        )));
    }
    if m.items.is_empty() || m.items.len() > MAX_ITEMS {
        return Err(invalid_input(format!(
            "items must contain 1..={MAX_ITEMS} entries"
        )));
    }
    Ok(())
}

fn validate_item(item: &ManifestItem, ids: &mut HashSet<String>) -> Result<(), io::Error> {
    if item.id.is_empty()
        || item.id.len() > 64
        || !item
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(invalid_input(format!(
            "invalid batch item id: {:?}",
            item.id
        )));
    }
    if !ids.insert(item.id.clone()) {
        return Err(invalid_input(format!("duplicate item id: {}", item.id)));
    }
    if item.nodes.iter().copied().collect::<BTreeSet<_>>().len() != item.nodes.len() {
        return Err(invalid_input(format!(
            "item {} has duplicate node indices",
            item.id
        )));
    }
    if let Some(path) = &item.shader_template {
        if !matches!(extension(path).as_str(), "yft" | "ydr") {
            return Err(invalid_input(format!(
                "item {} shaderTemplate must end in .yft or .ydr",
                item.id
            )));
        }
    }
    Ok(())
}

fn resolve_input(
    root: &Path,
    relative: &Path,
    limit: u64,
    label: &str,
) -> Result<PathBuf, io::Error> {
    validate_relative(relative, label)?;
    let path = root.join(relative).canonicalize()?;
    ensure_inside(root, &path, label)?;
    let metadata = fs::metadata(&path)?;
    if !metadata.is_file() {
        return Err(invalid_input(format!(
            "{label} is not a regular file: {}",
            relative.display()
        )));
    }
    if metadata.len() > limit {
        return Err(invalid_input(format!(
            "{label} exceeds {limit} bytes: {}",
            relative.display()
        )));
    }
    Ok(path)
}

fn resolve_destination(root: &Path, relative: &Path, label: &str) -> Result<PathBuf, io::Error> {
    validate_relative(relative, label)?;
    let name = relative
        .file_name()
        .ok_or_else(|| invalid_input(format!("{label} must name a file")))?;
    let parent = root
        .join(relative.parent().unwrap_or_else(|| Path::new("")))
        .canonicalize()?;
    ensure_inside(root, &parent, label)?;
    if !parent.is_dir() {
        return Err(invalid_input(format!("{label} parent is not a directory")));
    }
    let destination = parent.join(name);
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(invalid_input(format!(
            "{label} already exists; batch never overwrites: {}",
            destination.display()
        )));
    }
    Ok(destination)
}

fn validate_relative(path: &Path, label: &str) -> Result<(), io::Error> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(invalid_input(format!(
            "{label} path must be relative to the manifest directory"
        )));
    }
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(invalid_input(format!(
            "{label} path cannot contain '.', '..', roots or prefixes: {}",
            path.display()
        )));
    }
    Ok(())
}

fn ensure_inside(root: &Path, candidate: &Path, label: &str) -> Result<(), io::Error> {
    let r = path_key(root);
    let c = path_key(candidate);
    let prefix = format!(
        "{}{}",
        r.trim_end_matches(std::path::MAIN_SEPARATOR),
        std::path::MAIN_SEPARATOR
    );
    if c != r && !c.starts_with(&prefix) {
        return Err(invalid_input(format!(
            "{label} resolves outside manifest root: {}",
            candidate.display()
        )));
    }
    Ok(())
}

fn path_key(path: &Path) -> String {
    let text = path.to_string_lossy().into_owned();
    #[cfg(windows)]
    {
        text.replace('/', "\\").to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        text
    }
}
fn display(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|v| v.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn read_bounded(path: &Path, limit: u64, label: &str) -> Result<Vec<u8>, io::Error> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > limit {
        return Err(invalid_input(format!("{label} exceeds {limit} bytes")));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)?
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid_input(format!(
            "{label} grew past its byte limit while reading"
        )));
    }
    Ok(bytes)
}

fn export_item(item: &PlannedItem) -> Result<ExportedItem, io::Error> {
    let source_bytes = read_bounded(&item.source, MAX_SOURCE_BYTES, "source")?;
    let profile_bytes = read_bounded(&item.profile, MAX_PROFILE_BYTES, "profile")?;
    let template_bytes = item
        .template
        .as_ref()
        .map(|p| read_bounded(p, MAX_TEMPLATE_BYTES, "shader template"))
        .transpose()?;
    let template = match (&item.template, &template_bytes) {
        (Some(path), Some(bytes)) => {
            let format = match extension(path).as_str() {
                "yft" => GlbShaderTemplateFormat::LegacyYft,
                "ydr" => GlbShaderTemplateFormat::LegacyYdr,
                _ => {
                    return Err(invalid_input(
                        "shaderTemplate must reference Legacy .yft or .ydr",
                    ))
                }
            };
            Some(GlbShaderTemplateInput {
                format,
                source_name: path.display().to_string(),
                bytes: bytes.clone(),
            })
        }
        (None, None) => None,
        _ => return Err(invalid_data("shader template path/bytes mismatch")),
    };
    let source_name = item
        .source
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("model.glb")
        .to_owned();
    let request = GlbYdrExportRequest {
        source_name,
        source_bytes: source_bytes.clone(),
        profile_bytes: Some(profile_bytes.clone()),
        shader_template: template,
        shader_index: item.item.shader_index,
    };
    let result = if item.item.nodes.is_empty() {
        export_glb_to_ydr(request)?
    } else {
        export_glb_to_ydr_with_nodes(request, &item.item.nodes)?
    };
    Ok(ExportedItem {
        result,
        source_bytes,
        profile_bytes,
    })
}

fn build_item_report(
    item: &PlannedItem,
    source: &[u8],
    profile: &[u8],
    result: &GlbYdrExportResult,
) -> Result<Value, Box<dyn Error>> {
    super::glb_export_report::build_report(super::glb_export_report::GlbExportReportRequest {
        source_path: &item.source,
        source_bytes: source,
        output_path: &item.output,
        profile_path: Some(&item.profile),
        profile_bytes: Some(profile),
        shader_template_path: item.template.as_deref(),
        selected_node_indices: &item.item.nodes,
        output_overwrite: false,
        result,
    })
}

fn execute_item(item: &PlannedItem) -> Result<ItemResult, String> {
    let exported = export_item(item).map_err(|e| e.to_string())?;
    let result = exported.result;
    if result.report.source_sha256 != item.source_hash
        || result.report.profile_sha256 != item.profile_hash
        || result
            .report
            .shader_template
            .as_ref()
            .map(|v| v.sha256.clone())
            != item.template_hash
    {
        return Err("source/profile/template changed after global preflight".into());
    }
    let report = build_item_report(
        item,
        &exported.source_bytes,
        &exported.profile_bytes,
        &result,
    )
    .map_err(|e| e.to_string())?;
    let mut report_bytes = serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?;
    report_bytes.push(b'\n');
    super::glb_export_report::write_bytes_atomic(&item.output, &result.output_bytes)
        .map_err(|e| format!("cannot publish YDR output: {e}"))?;
    if let Err(error) = super::glb_export_report::write_bytes_atomic(&item.report, &report_bytes) {
        return match fs::remove_file(&item.output) {
            Ok(()) => Err(format!(
                "cannot publish item report; YDR rolled back: {error}"
            )),
            Err(rollback) => Err(format!(
                "cannot publish item report ({error}); rollback failed ({rollback})"
            )),
        };
    }
    Ok(ItemResult {
        id: item.item.id.clone(),
        status: "succeeded",
        source: display(&item.item.source),
        output: display(&item.item.output),
        report: display(&item.item.report),
        source_sha256: Some(result.report.source_sha256),
        output_sha256: Some(result.report.output_sha256),
        error: None,
    })
}

fn print_summary(data: &Value) {
    println!("GLB batch");
    if let Some(summary) = data.get("summary") {
        println!(
            "items: {} total, {} succeeded, {} failed, {} cancelled",
            summary["total"].as_u64().unwrap_or(0),
            summary["succeeded"].as_u64().unwrap_or(0),
            summary["failed"].as_u64().unwrap_or(0),
            summary["cancelled"].as_u64().unwrap_or(0)
        );
    }
    if let Some(items) = data.get("items").and_then(Value::as_array) {
        for item in items {
            println!(
                "  {}: {}",
                item["id"].as_str().unwrap_or("<unknown>"),
                item["status"].as_str().unwrap_or("unknown")
            );
            if let Some(error) = item["error"].as_str() {
                println!("    {error}");
            }
        }
    }
    if let Some(path) = data["aggregateReport"].as_str() {
        println!("aggregate report: {path}");
    }
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
    };
    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn root() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "ragelab-batch-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        for dir in ["output", "reports", "profiles"] {
            fs::create_dir_all(path.join(dir)).unwrap();
        }
        path
    }
    fn glb() -> Vec<u8> {
        let mut bin = Vec::new();
        for v in [[0.0_f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            for c in v {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        let off = bin.len();
        for i in [0_u16, 1, 2] {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        let doc = json!({
            "asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}],
            "meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":1}]}],
            "accessors":[
                {"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]},
                {"bufferView":1,"componentType":5123,"count":3,"type":"SCALAR"}
            ],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":36},{"buffer":0,"byteOffset":off,"byteLength":6}],
            "buffers":[{"byteLength":bin.len()}]
        });
        let mut json_bytes = serde_json::to_vec(&doc).unwrap();
        while json_bytes.len() % 4 != 0 {
            json_bytes.push(b' ');
        }
        let len = 12 + 8 + json_bytes.len() + 8 + bin.len();
        let mut out = Vec::new();
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2_u32.to_le_bytes());
        out.extend_from_slice(&(len as u32).to_le_bytes());
        out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x4E4F534A_u32.to_le_bytes());
        out.extend_from_slice(&json_bytes);
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x004E4942_u32.to_le_bytes());
        out.extend_from_slice(&bin);
        out
    }
    fn item(id: &str, profile: &str, out: &str, report: &str) -> Value {
        json!({"id":id,"source":"source.glb","output":out,"profile":profile,
            "shaderTemplate":null,"shaderIndex":0,"nodes":[],"report":report})
    }
    fn manifest(root: &Path, items: Value) -> PathBuf {
        manifest_with_policy(root, items, "continue", 2)
    }

    fn manifest_with_policy(
        root: &Path,
        items: Value,
        failure_policy: &str,
        concurrency: usize,
    ) -> PathBuf {
        let value = json!({
            "schema": MANIFEST_SCHEMA,
            "schemaVersion": 1,
            "failurePolicy": failure_policy,
            "concurrency": concurrency,
            "report": "reports/batch.json",
            "items": items
        });
        let path = root.join("batch.json");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        path
    }

    fn prepare_two_item_root() -> PathBuf {
        let root = root();
        let bytes = glb();
        fs::write(root.join("source.glb"), &bytes).unwrap();
        let profile = ragelab_engine::default_glb_authoring_profile(&bytes, "source.glb").unwrap();
        for name in ["first.json", "second.json"] {
            fs::write(
                root.join("profiles").join(name),
                serde_json::to_vec(&profile).unwrap(),
            )
            .unwrap();
        }
        root
    }

    #[test]
    fn manifest_is_strict_and_requires_explicit_nullable_template() {
        let bad = json!({"schema":MANIFEST_SCHEMA,"schemaVersion":1,"failurePolicy":"continue",
            "concurrency":1,"report":"reports/batch.json","extra":true,"items":[]});
        assert!(serde_json::from_value::<Manifest>(bad).is_err());
        let mut item = item("x", "p.json", "o.ydr", "r.json");
        item.as_object_mut().unwrap().remove("shaderTemplate");
        assert!(serde_json::from_value::<ManifestItem>(item).is_err());
    }

    #[test]
    fn relative_paths_reject_escape_and_absolute_forms() {
        for path in [
            "../outside.glb",
            "nested/../../outside.glb",
            "C:\\outside.glb",
            "",
        ] {
            assert!(
                validate_relative(Path::new(path), "source").is_err(),
                "{path}"
            );
        }
    }

    #[test]
    fn global_preflight_failure_creates_no_outputs() {
        let root = root();
        let bytes = glb();
        fs::write(root.join("source.glb"), &bytes).unwrap();
        let p = ragelab_engine::default_glb_authoring_profile(&bytes, "source.glb").unwrap();
        fs::write(
            root.join("profiles/ok.json"),
            serde_json::to_vec(&p).unwrap(),
        )
        .unwrap();
        let path = manifest(
            &root,
            json!([
                item(
                    "valid",
                    "profiles/ok.json",
                    "output/a.ydr",
                    "reports/a.json"
                ),
                item(
                    "invalid",
                    "profiles/missing.json",
                    "output/b.ydr",
                    "reports/b.json"
                )
            ]),
        );
        assert!(run_manifest(&path).is_err());
        for name in [
            "output/a.ydr",
            "output/b.ydr",
            "reports/a.json",
            "reports/b.json",
            "reports/batch.json",
        ] {
            assert!(!root.join(name).exists(), "unexpected output: {name}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn successful_batch_writes_item_and_aggregate_reports_in_manifest_order() {
        let root = root();
        let bytes = glb();
        fs::write(root.join("source.glb"), &bytes).unwrap();
        let profile = ragelab_engine::default_glb_authoring_profile(&bytes, "source.glb").unwrap();
        for name in ["a.json", "b.json"] {
            fs::write(
                root.join("profiles").join(name),
                serde_json::to_vec(&profile).unwrap(),
            )
            .unwrap();
        }
        let path = manifest(
            &root,
            json!([
                item("first", "profiles/a.json", "output/a.ydr", "reports/a.json"),
                item(
                    "second",
                    "profiles/b.json",
                    "output/b.ydr",
                    "reports/b.json"
                )
            ]),
        );
        let result = run_manifest(&path).unwrap();
        assert_eq!(
            (result.succeeded, result.failed, result.cancelled),
            (2, 0, 0)
        );
        for name in [
            "output/a.ydr",
            "output/b.ydr",
            "reports/a.json",
            "reports/b.json",
            "reports/batch.json",
        ] {
            assert!(root.join(name).is_file(), "missing {name}");
        }
        let data: Value =
            serde_json::from_slice(&fs::read(root.join("reports/batch.json")).unwrap()).unwrap();
        assert_eq!(data["schema"], REPORT_SCHEMA);
        assert_eq!(data["summary"]["succeeded"], 2);
        assert_eq!(
            fs::read(root.join("output/a.ydr")).unwrap(),
            fs::read(root.join("output/b.ydr")).unwrap(),
            "identical source/profile/options must produce byte-identical YDR outputs"
        );
        assert_eq!(data["items"][0]["id"], "first");
        assert_eq!(data["items"][1]["id"], "second");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn destination_collisions_fail_before_any_output_is_created() {
        let root = prepare_two_item_root();
        let path = manifest(
            &root,
            json!([
                item(
                    "first",
                    "profiles/first.json",
                    "output/shared.ydr",
                    "reports/first.json"
                ),
                item(
                    "second",
                    "profiles/second.json",
                    "output/shared.ydr",
                    "reports/second.json"
                )
            ]),
        );

        assert!(run_manifest(&path).is_err());
        assert!(!root.join("output/shared.ydr").exists());
        assert!(!root.join("reports/first.json").exists());
        assert!(!root.join("reports/second.json").exists());
        assert!(!root.join("reports/batch.json").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn continue_policy_isolates_failure_and_runs_later_items() {
        let root = prepare_two_item_root();
        let path = manifest_with_policy(
            &root,
            json!([
                item(
                    "first",
                    "profiles/first.json",
                    "output/first.ydr",
                    "reports/first.json"
                ),
                item(
                    "second",
                    "profiles/second.json",
                    "output/second.ydr",
                    "reports/second.json"
                )
            ]),
            "continue",
            1,
        );
        let result = run_manifest_with_executor(&path, |planned| {
            if planned.item.id == "first" {
                Err("simulated post-preflight publication failure".into())
            } else {
                execute_item(planned)
            }
        })
        .unwrap();

        assert_eq!(
            (result.succeeded, result.failed, result.cancelled),
            (1, 1, 0)
        );
        let report: Value =
            serde_json::from_slice(&fs::read(root.join("reports/batch.json")).unwrap()).unwrap();
        assert_eq!(report["items"][0]["status"], "failed");
        assert_eq!(report["items"][1]["status"], "succeeded");
        assert!(!root.join("output/first.ydr").exists());
        assert!(root.join("output/second.ydr").is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fail_fast_stops_scheduling_and_marks_remaining_items_cancelled() {
        let root = prepare_two_item_root();
        let path = manifest_with_policy(
            &root,
            json!([
                item(
                    "first",
                    "profiles/first.json",
                    "output/first.ydr",
                    "reports/first.json"
                ),
                item(
                    "second",
                    "profiles/second.json",
                    "output/second.ydr",
                    "reports/second.json"
                )
            ]),
            "fail_fast",
            1,
        );
        let result = run_manifest_with_executor(&path, |planned| {
            if planned.item.id == "first" {
                Err("simulated post-preflight worker failure".into())
            } else {
                execute_item(planned)
            }
        })
        .unwrap();

        assert_eq!(
            (result.succeeded, result.failed, result.cancelled),
            (0, 1, 1)
        );
        let report: Value =
            serde_json::from_slice(&fs::read(root.join("reports/batch.json")).unwrap()).unwrap();
        assert_eq!(report["items"][0]["status"], "failed");
        assert_eq!(report["items"][1]["status"], "cancelled");
        assert!(!root.join("output/first.ydr").exists());
        assert!(!root.join("output/second.ydr").exists());
        fs::remove_dir_all(root).unwrap();
    }
}

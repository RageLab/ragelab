use std::{
    error::Error,
    fs, io,
    path::{Path, PathBuf},
};

use ragelab_assets::AssetKind;
use ragelab_engine::{
    apply_operation_document, asset_capabilities, build_vanilla_catalog, discover_fivem_legacy,
    discover_gta_v_legacy, inspect_asset, isolated_asset_spatial_context, parse_operation_document,
    plan_operation_document, preview_asset, render_vanilla_catalog_paths, validate_asset,
    workspace_scene_report_with_sources, ymap_spatial_context, EngineError, OperationError,
    PreviewOptions, SceneAssemblyOptions, SceneRpfMount, SpatialContext, SpatialProvenance,
};
use ragelab_ybn::YbnCollision;
use ragelab_ydd::YddDictionary;
use ragelab_ydr::YdrDocument;
use ragelab_ymap::Ymap;
use ragelab_ytd::Ytd;
use ragelab_ytyp::Ytyp;
use serde_json::{json, Value};

pub const RESPONSE_SCHEMA: &str = "ragelab.cli.response";
pub const RESPONSE_SCHEMA_VERSION: u64 = 1;

pub fn is_structured_command(command: &str) -> bool {
    matches!(
        command,
        "version"
            | "capabilities"
            | "inspect"
            | "validate"
            | "plan"
            | "apply"
            | "spatial"
            | "preview"
            | "scene"
            | "preflight"
            | "export"
            | "gta.discover"
            | "gta.catalog"
            | "rpf.keys"
            | "rpf.info"
            | "rpf.list"
            | "rpf.extract"
            | "fivem.discover"
    )
}

pub fn classify_exit(error: &(dyn Error + 'static)) -> (i32, &'static str) {
    if let Some(error) = error.downcast_ref::<io::Error>() {
        return match error.kind() {
            io::ErrorKind::InvalidInput => (2, "invalid_input"),
            io::ErrorKind::Unsupported => (3, "unsupported"),
            io::ErrorKind::InvalidData => (4, "validation_failed"),
            _ => (1, "operation_failed"),
        };
    }
    if let Some(error) = error.downcast_ref::<EngineError>() {
        return match error {
            EngineError::ExportGate(_) => (3, "unsupported"),
            _ => (1, "operation_failed"),
        };
    }

    (1, "operation_failed")
}

pub fn print_error(command: &str, error: &(dyn Error + 'static)) {
    let (_, code) = classify_exit(error);
    let payload = json!({
        "schema": RESPONSE_SCHEMA,
        "schemaVersion": RESPONSE_SCHEMA_VERSION,
        "ok": false,
        "command": command,
        "error": {
            "code": code,
            "message": error.to_string(),
        }
    });

    match serde_json::to_string_pretty(&payload) {
        Ok(body) => println!("{body}"),
        Err(_) => eprintln!("error: {error}"),
    }
}

pub fn inspect(path: &Path, json_output: bool) -> Result<(), Box<dyn Error>> {
    let report = inspect_asset(path)?;

    if json_output {
        print_success("inspect", serde_json::to_value(&report)?)?;
    } else {
        println!("file: {}", report.path);
        println!("type: {}", report.asset_type);
        println!("bytes: {}", report.bytes);
        if let Some(container) = &report.container {
            println!("container: {}", container.kind);
        }
        println!(
            "status: {}",
            if report.details.is_null() {
                "container inspected"
            } else {
                "format parsed"
            }
        );
    }

    Ok(())
}

pub fn validate(path: &Path, json_output: bool) -> Result<(), Box<dyn Error>> {
    let report = validate_asset(path)?;

    if json_output {
        print_success("validate", serde_json::to_value(&report)?)?;
    } else {
        println!("file: {}", report.path);
        println!("type: {}", report.asset_type);
        println!("valid: yes");
        for check in report.checks {
            println!("check: {check}");
        }
    }

    Ok(())
}

pub fn plan_operation_file(path: &Path, json_output: bool) -> Result<(), Box<dyn Error>> {
    let operation_path = path.canonicalize()?;
    let body = fs::read_to_string(&operation_path)?;
    let document = parse_operation_document(&body).map_err(operation_error)?;
    let base_dir = operation_path.parent().unwrap_or_else(|| Path::new("."));
    let plan = plan_operation_document(&document, base_dir).map_err(operation_error)?;

    if json_output {
        print_success("plan", serde_json::to_value(&plan)?)?;
    } else {
        println!("operation: {}", path.display());
        println!("source: {}", plan.source.display());
        println!("output: {}", plan.output.display());
        println!("asset-type: {}", plan.asset_type);
        println!("allowed: {}", if plan.allowed { "yes" } else { "no" });
        println!("non-destructive: yes");
        for operation in &plan.operations {
            let state = if operation.allowed {
                "allowed"
            } else {
                "blocked"
            };
            println!(
                "operation[{}]: {} [{state}]",
                operation.index, operation.operation_type
            );
            if let Some(reason) = &operation.reason {
                println!("  reason: {reason}");
            }
        }
        for reason in &plan.reasons {
            println!("reason: {reason}");
        }
    }

    Ok(())
}

pub fn apply_operation_file(path: &Path, json_output: bool) -> Result<(), Box<dyn Error>> {
    let operation_path = path.canonicalize()?;
    let body = fs::read_to_string(&operation_path)?;
    let document = parse_operation_document(&body).map_err(operation_error)?;
    let base_dir = operation_path.parent().unwrap_or_else(|| Path::new("."));
    let result = apply_operation_document(&document, base_dir).map_err(operation_error)?;

    if json_output {
        print_success("apply", serde_json::to_value(&result)?)?;
    } else {
        println!("operation: {}", path.display());
        println!("source: {}", result.source.display());
        println!("output: {}", result.output.display());
        println!("asset-type: {}", result.asset_type);
        println!("operations-applied: {}", result.operations_applied);
        println!("bytes-written: {}", result.bytes_written);
        println!("non-destructive: yes");
        println!(
            "semantic-reopen: {}",
            if result.validation.semantic_reopen {
                "ok"
            } else {
                "failed"
            }
        );
        println!(
            "source-unchanged: {}",
            if result.validation.source_unchanged {
                "yes"
            } else {
                "no"
            }
        );
    }

    Ok(())
}

pub fn capabilities(path: &Path, json_output: bool) -> Result<(), Box<dyn Error>> {
    let report = asset_capabilities(path)?;

    if json_output {
        print_success("capabilities", serde_json::to_value(&report)?)?;
    } else {
        println!("file: {}", report.path);
        println!("type: {}", report.asset_type);
        println!("operations:");
        for operation in report.operations {
            let mode = if operation.writes_asset {
                "write"
            } else {
                "read"
            };
            println!("  {} [{mode}]", operation.id);
        }
    }

    Ok(())
}

pub fn parse_preview_args(
    args: impl Iterator<Item = String>,
    usage: &str,
) -> Result<(PathBuf, PreviewOptions, bool), io::Error> {
    let mut args = args;
    let path = args
        .next()
        .filter(|value| !value.starts_with('-'))
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
    let mut options = PreviewOptions::default();
    let mut json_output = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" if !json_output => json_output = true,
            "--drawable-index" if options.drawable_index.is_none() => {
                options.drawable_index = Some(parse_preview_index(args.next(), usage)?);
            }
            "--max-primitives" => {
                options.max_primitives =
                    parse_preview_limit(args.next(), "--max-primitives", usage)?;
            }
            "--max-vertices" => {
                options.max_vertices = parse_preview_limit(args.next(), "--max-vertices", usage)?;
            }
            "--max-indices" => {
                options.max_indices = parse_preview_limit(args.next(), "--max-indices", usage)?;
            }
            "--max-shaders" => {
                options.max_shaders = parse_preview_limit(args.next(), "--max-shaders", usage)?;
            }
            "--max-texture-references" => {
                options.max_texture_references =
                    parse_preview_limit(args.next(), "--max-texture-references", usage)?;
            }
            "--max-children" => {
                options.max_children = parse_preview_limit(args.next(), "--max-children", usage)?;
            }
            "--max-materials" => {
                options.max_materials = parse_preview_limit(args.next(), "--max-materials", usage)?;
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{usage}; unknown option: {arg}"),
                ))
            }
        }
    }

    Ok((path, options.validate()?, json_output))
}

fn parse_preview_index(value: Option<String>, usage: &str) -> Result<usize, io::Error> {
    let value = value.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
    value.parse::<usize>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "--drawable-index must be a non-negative integer",
        )
    })
}

fn parse_preview_limit(
    value: Option<String>,
    label: &str,
    usage: &str,
) -> Result<usize, io::Error> {
    let value = value.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
    let parsed = value.parse::<usize>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{label} must be a positive integer"),
        )
    })?;
    if parsed == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{label} must be greater than zero"),
        ));
    }
    Ok(parsed)
}

pub fn preview(
    path: &Path,
    options: PreviewOptions,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let report = preview_asset(path, options)?;

    if json_output {
        print_success("preview", serde_json::to_value(&report)?)?;
    } else {
        println!("file: {}", report.path);
        println!("type: {}", report.asset_type);

        if report.asset_type == "YBN" {
            let counts = &report.preview["counts"];
            println!("children: {}", counts["children"].as_u64().unwrap_or(0));
            println!("materials: {}", counts["materials"].as_u64().unwrap_or(0));
            println!(
                "mesh-primitives: {}",
                counts["meshPrimitives"].as_u64().unwrap_or(0)
            );
            println!(
                "shape-primitives: {}",
                counts["shapePrimitives"].as_u64().unwrap_or(0)
            );
            println!("vertices: {}", counts["vertices"].as_u64().unwrap_or(0));
            println!("indices: {}", counts["indices"].as_u64().unwrap_or(0));
            println!("triangles: {}", counts["triangles"].as_u64().unwrap_or(0));
        } else {
            if let Some(index) = options.drawable_index {
                println!("drawable-index: {index}");
            }
            println!(
                "lod: {}",
                report.preview["lod"].as_str().unwrap_or("unknown")
            );
            let counts = &report.preview["counts"];
            println!("primitives: {}", counts["primitives"].as_u64().unwrap_or(0));
            println!("vertices: {}", counts["vertices"].as_u64().unwrap_or(0));
            println!("indices: {}", counts["indices"].as_u64().unwrap_or(0));
            println!("triangles: {}", counts["triangles"].as_u64().unwrap_or(0));
        }

        println!("spatial: {}", report.spatial.classification);
    }

    Ok(())
}

pub fn spatial(path: &Path, json_output: bool) -> Result<(), Box<dyn Error>> {
    let bytes = fs::read(path)?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let kind = AssetKind::from_extension(extension).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            format!("spatial context is unavailable for {}", path.display()),
        )
    })?;

    let context = match kind {
        AssetKind::Ymap => {
            let ymap = Ymap::from_bytes(&bytes).map_err(validation_error)?;
            ymap_spatial_context(&ymap, Some(path.to_path_buf()))
        }
        AssetKind::Ydr => {
            YdrDocument::from_bytes(&bytes).map_err(validation_error)?;
            isolated_asset_spatial_context(kind)
        }
        AssetKind::Ydd => {
            YddDictionary::from_bytes(&bytes).map_err(validation_error)?;
            isolated_asset_spatial_context(kind)
        }
        AssetKind::Ytd => {
            Ytd::from_bytes(&bytes).map_err(validation_error)?;
            isolated_asset_spatial_context(kind)
        }
        AssetKind::Ybn => {
            YbnCollision::from_bytes(&bytes).map_err(validation_error)?;
            isolated_asset_spatial_context(kind)
        }
        AssetKind::Ytyp => {
            Ytyp::from_bytes(&bytes).map_err(validation_error)?;
            isolated_asset_spatial_context(kind)
        }
        AssetKind::Yft | AssetKind::Ycd => isolated_asset_spatial_context(kind),
    };

    if json_output {
        print_success(
            "spatial",
            json!({
                "path": path.display().to_string(),
                "type": kind.to_string(),
                "context": spatial_context_json(&context),
            }),
        )?;
    } else {
        println!("file: {}", path.display());
        println!("type: {kind}");
        println!("classification: {}", context.classification.as_str());
        println!("provenance: {}", context.provenance.as_str());
        if let Some(center) = context.world_center {
            println!("world-center: {:?}", center);
        }
        if let Some(reason) = &context.reason {
            println!("reason: {}: {}", reason.code.as_str(), reason.message);
        }
    }

    Ok(())
}

pub fn scene(
    workspace: &Path,
    ymap_arg: &Path,
    fallback_roots: &[PathBuf],
    rpf_mounts: &[SceneRpfMount],
    options: SceneAssemblyOptions,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let report = workspace_scene_report_with_sources(
        workspace,
        ymap_arg,
        fallback_roots,
        rpf_mounts,
        options,
    )?;

    if json_output {
        print_success("scene", serde_json::to_value(&report)?)?;
    } else {
        println!("workspace: {}", workspace.display());
        println!("ymap: {}", report.root.path);
        println!("entities: {}", report.summary.total_entities);
        println!("nodes: {}", report.summary.emitted_nodes);
        println!("resolved: {}", report.summary.resolved_nodes);
        println!("unresolved: {}", report.summary.unresolved_nodes);
        println!("assets: {}", report.summary.asset_references);
        println!(
            "truncated: {}",
            if report.limits.truncated { "yes" } else { "no" }
        );
        println!("warnings: {}", report.warnings.len());
    }

    Ok(())
}

pub struct SceneCommandArgs {
    pub workspace: PathBuf,
    pub ymap: PathBuf,
    pub fallback_roots: Vec<PathBuf>,
    pub rpf_mounts: Vec<SceneRpfMount>,
    pub options: SceneAssemblyOptions,
    pub json_output: bool,
}

pub fn parse_scene_args(
    args: impl Iterator<Item = String>,
    usage: &str,
) -> Result<SceneCommandArgs, io::Error> {
    let mut args = args;
    let workspace = args
        .next()
        .filter(|value| !value.starts_with('-'))
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
    let ymap = args
        .next()
        .filter(|value| !value.starts_with('-'))
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;

    let mut json_output = false;
    let mut fallback_roots = Vec::new();
    let mut rpf_mounts: Vec<(PathBuf, Vec<String>)> = Vec::new();
    let mut rpf_keys = None;
    let mut max_nodes = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" if !json_output => json_output = true,
            "--fallback-root" => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                fallback_roots.push(PathBuf::from(value));
            }
            "--rpf-mount" => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                rpf_mounts.push((PathBuf::from(value), Vec::new()));
            }
            "--rpf-nested" => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                let Some((_, nested)) = rpf_mounts.last_mut() else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "--rpf-nested requires a preceding --rpf-mount",
                    ));
                };
                nested.push(value);
            }
            "--rpf-keys" if rpf_keys.is_none() => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                rpf_keys = Some(PathBuf::from(value));
            }
            "--max-nodes" if max_nodes.is_none() => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                let parsed = value.parse::<usize>().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "--max-nodes must be a positive integer",
                    )
                })?;
                if parsed == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "--max-nodes must be greater than zero",
                    ));
                }
                max_nodes = Some(parsed);
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{usage}; unknown option: {arg}"),
                ))
            }
        }
    }

    let mounts = if rpf_mounts.is_empty() {
        Vec::new()
    } else {
        let keys = rpf_keys.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "--rpf-keys is required when --rpf-mount is used",
            )
        })?;
        rpf_mounts
            .into_iter()
            .map(|(archive, nested)| SceneRpfMount::new(archive, nested, keys.clone()))
            .collect()
    };

    Ok(SceneCommandArgs {
        workspace,
        ymap,
        fallback_roots,
        rpf_mounts: mounts,
        options: max_nodes.map(SceneAssemblyOptions::new).unwrap_or_default(),
        json_output,
    })
}

fn spatial_context_json(context: &SpatialContext) -> Value {
    let transform = context.world_transform.map(|transform| {
        json!({
            "translation": transform.translation,
            "rotation": transform.rotation,
            "scale": transform.scale,
        })
    });
    let bounds = context.world_bounds.map(|bounds| {
        json!({
            "min": bounds.min,
            "max": bounds.max,
        })
    });
    let reason = context.reason.as_ref().map(|reason| {
        json!({
            "code": reason.code.as_str(),
            "message": reason.message,
        })
    });

    json!({
        "classification": context.classification.as_str(),
        "worldTransform": transform,
        "worldCenter": context.world_center,
        "worldBounds": bounds,
        "provenance": spatial_provenance_json(&context.provenance),
        "contextAsset": context
            .context_asset
            .as_ref()
            .map(|path| path.display().to_string()),
        "reason": reason,
    })
}

fn spatial_provenance_json(provenance: &SpatialProvenance) -> Value {
    match provenance {
        SpatialProvenance::AssetSemantics { kind } => json!({
            "kind": provenance.as_str(),
            "assetKind": kind.to_string(),
        }),
        SpatialProvenance::YmapEntityTransform {
            entity_index,
            archetype_hash,
        } => json!({
            "kind": provenance.as_str(),
            "entityIndex": entity_index,
            "archetypeHash": format!("0x{archetype_hash:08X}"),
        }),
        _ => json!({ "kind": provenance.as_str() }),
    }
}

pub fn gta_discover(json_output: bool) -> Result<(), Box<dyn Error>> {
    let report = discover_gta_v_legacy();

    if json_output {
        print_success("gta.discover", serde_json::to_value(&report)?)?;
    } else {
        println!("target: GTA V Legacy");
        println!("platform: {}", report.platform);
        println!("valid-installations: {}", report.valid_legacy_installations);
        println!("candidates: {}", report.candidates.len());

        for candidate in report.candidates {
            println!(
                "{} [{}] valid={}",
                candidate.root.display(),
                candidate.edition.as_str(),
                if candidate.valid { "yes" } else { "no" }
            );
            for provenance in candidate.provenance {
                println!(
                    "  source: {} ({})",
                    provenance.source.as_str(),
                    provenance.reference
                );
            }
            for check in candidate.checks {
                if check.required || !check.passed {
                    println!(
                        "  check {}: {}{}",
                        check.id,
                        if check.passed { "ok" } else { "missing" },
                        if check.required { " [required]" } else { "" }
                    );
                }
            }
        }
    }

    Ok(())
}

pub fn fivem_discover(json_output: bool) -> Result<(), Box<dyn Error>> {
    let report = discover_fivem_legacy();

    if json_output {
        print_success("fivem.discover", serde_json::to_value(&report)?)?;
    } else {
        println!("target: FiveM + GTA V Legacy");
        println!("platform: {}", report.platform);
        println!("valid-installations: {}", report.valid_installations);
        println!(
            "legacy-linked-installations: {}",
            report.legacy_linked_installations
        );
        println!("candidates: {}", report.candidates.len());

        for candidate in report.candidates {
            println!(
                "{} valid={} gta={}",
                candidate.root.display(),
                if candidate.valid { "yes" } else { "no" },
                candidate.gta.status.as_str()
            );
            println!("  app-root: {}", candidate.app_root.display());
            println!("  citizenfx: {}", candidate.citizen_fx_ini.display());
            if let Some(build) = &candidate.saved_build_number {
                println!("  saved-build: {build}");
            }
            if let Some(channel) = &candidate.update_channel {
                println!("  update-channel: {channel}");
            }
            if let Some(path) = &candidate.gta.configured_path {
                println!("  gta-path: {}", path.display());
            }
            for provenance in candidate.provenance {
                println!(
                    "  source: {} ({})",
                    provenance.source.as_str(),
                    provenance.reference
                );
            }
            for storage in candidate.storage_paths {
                if storage.exists {
                    println!("  storage {}: {}", storage.id, storage.path.display());
                }
            }
        }
    }

    Ok(())
}

pub fn gta_catalog(
    root: &Path,
    output: &Path,
    overwrite: bool,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let build = build_vanilla_catalog(root)?;
    let body = render_vanilla_catalog_paths(&build.paths);

    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }

    if overwrite {
        fs::write(output, body.as_bytes())?;
    } else {
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)?;
        file.write_all(body.as_bytes())?;
        file.flush()?;
    }

    if json_output {
        let mut data = serde_json::to_value(&build.report)?;
        if let Some(object) = data.as_object_mut() {
            object.insert("output".into(), Value::String(output.display().to_string()));
            object.insert(
                "outputBytes".into(),
                Value::from(u64::try_from(body.len()).unwrap_or(u64::MAX)),
            );
        }
        print_success("gta.catalog", data)?;
    } else {
        println!("root: {}", build.report.root.display());
        println!("source-kind: {}", build.report.source_kind.as_str());
        println!("coverage: {}", build.report.coverage.as_str());
        println!(
            "complete: {}",
            if build.report.complete { "yes" } else { "no" }
        );
        println!("scanned-files: {}", build.report.scanned_files);
        println!(
            "supported-loose-files: {}",
            build.report.supported_loose_files
        );
        println!("path-entries: {}", build.report.path_entries);
        println!(
            "unique-catalog-entries: {}",
            build.report.unique_catalog_entries
        );
        println!(
            "rpf-archives: {}",
            build.report.archive_boundary.rpf_archives_found
        );
        println!("output: {}", output.display());
    }

    Ok(())
}

pub fn parse_gta_catalog_args(
    args: impl Iterator<Item = String>,
    usage: &str,
) -> Result<(PathBuf, PathBuf, bool, bool), io::Error> {
    let mut args = args;
    let root = args
        .next()
        .filter(|value| !value.starts_with('-'))
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;

    let mut output = None;
    let mut overwrite = false;
    let mut json_output = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output" if output.is_none() => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                if value.starts_with('-') {
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, usage));
                }
                output = Some(PathBuf::from(value));
            }
            "--overwrite" if !overwrite => overwrite = true,
            "--json" if !json_output => json_output = true,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{usage}; unknown option: {arg}"),
                ))
            }
        }
    }

    let output = output.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;

    Ok((root, output, overwrite, json_output))
}

pub fn parse_path_json_args(
    args: impl Iterator<Item = String>,
    usage: &str,
) -> Result<(PathBuf, bool), io::Error> {
    let mut path = None;
    let mut json_output = false;

    for arg in args {
        match arg.as_str() {
            "--json" if !json_output => json_output = true,
            _ if arg.starts_with('-') => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{usage}; unknown option: {arg}"),
                ))
            }
            _ if path.is_none() => path = Some(PathBuf::from(arg)),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    usage.to_string(),
                ))
            }
        }
    }

    let path = path.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
    Ok((path, json_output))
}

pub fn parse_capabilities_args(
    args: impl Iterator<Item = String>,
    usage: &str,
) -> Result<(Option<PathBuf>, bool), io::Error> {
    let mut path = None;
    let mut json_output = false;

    for arg in args {
        match arg.as_str() {
            "--json" if !json_output => json_output = true,
            _ if arg.starts_with('-') => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{usage}; unknown option: {arg}"),
                ))
            }
            _ if path.is_none() => path = Some(PathBuf::from(arg)),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    usage.to_string(),
                ))
            }
        }
    }

    Ok((path, json_output))
}

pub fn normalize_command_args(args: Vec<String>) -> Vec<String> {
    if args.len() < 2 {
        return args;
    }

    let mapped = match (args[0].as_str(), args[1].as_str()) {
        ("ydr", "info") => Some("ydr-info"),
        ("ydr", "translate") => Some("ydr-translate"),
        ("ydr", "rebind-texture") => Some("ydr-rebind-texture"),
        ("ydr", "rebind-shader") => Some("ydr-rebind-shader"),
        ("ydd", "info") => Some("ydd-info"),
        ("ydd", "translate") => Some("ydd-translate"),
        ("ydd", "rebind-texture") => Some("ydd-rebind-texture"),
        ("ydd", "rebind-shader") => Some("ydd-rebind-shader"),
        ("ytd", "info") => Some("ytd-info"),
        ("ytd", "extract-dds") => Some("ytd-dds"),
        ("ytd", "replace-dds") => Some("ytd-replace-dds"),
        ("ytd", "repack-dds") => Some("ytd-repack-dds"),
        ("ytd", "repack-rgba") => Some("ytd-repack-rgba"),
        ("ytd", "rebuild-compact") => Some("ytd-rebuild-compact"),
        ("ybn", "info") => Some("ybn-info"),
        ("ymap", "info") => Some("ymap-info"),
        ("ytyp", "info") => Some("ytyp-info"),
        ("ymf", "info") => Some("ymf-info"),
        ("workspace", "scan") => Some("scan"),
        ("workspace", "deps") => Some("deps"),
        ("workspace", "providers") => Some("providers"),
        ("workspace", "preflight") => Some("preflight"),
        ("workspace", "mlo-audit") => Some("mlo-audit"),
        ("workspace", "extract") => Some("extract"),
        ("workspace", "export") => Some("export"),
        ("workspace", "scene") => Some("scene"),
        ("gta", "discover") => Some("gta.discover"),
        ("gta", "catalog") => Some("gta.catalog"),
        ("gta", "vanilla-index") => Some("vanilla-index"),
        ("rpf", "keys") => Some("rpf.keys"),
        ("rpf", "info") => Some("rpf.info"),
        ("rpf", "list") => Some("rpf.list"),
        ("rpf", "extract") => Some("rpf.extract"),
        ("fivem", "discover") => Some("fivem.discover"),
        _ => None,
    };

    let Some(mapped) = mapped else {
        return args;
    };

    let mut normalized = Vec::with_capacity(args.len() - 1);
    normalized.push(mapped.to_string());
    normalized.extend(args.into_iter().skip(2));
    normalized
}

pub(crate) fn print_success(command: &str, data: Value) -> Result<(), serde_json::Error> {
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": RESPONSE_SCHEMA,
            "schemaVersion": RESPONSE_SCHEMA_VERSION,
            "ok": true,
            "command": command,
            "data": data,
        }))?
    );
    Ok(())
}

fn validation_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn operation_error(error: OperationError) -> io::Error {
    io::Error::new(error.error_kind(), error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_namespaced_commands_without_touching_arguments() {
        assert_eq!(
            normalize_command_args(vec![
                "ydr".into(),
                "translate".into(),
                "a.ydr".into(),
                "1".into(),
                "2".into(),
                "3".into(),
                "b.ydr".into(),
            ]),
            vec!["ydr-translate", "a.ydr", "1", "2", "3", "b.ydr",]
        );
    }

    #[test]
    fn leaves_unknown_namespaces_unchanged() {
        assert_eq!(
            normalize_command_args(vec!["unknown".into(), "thing".into()]),
            vec!["unknown", "thing"]
        );
    }

    #[test]
    fn classifies_contract_errors() {
        let invalid = io::Error::new(io::ErrorKind::InvalidInput, "bad input");
        let unsupported = io::Error::new(io::ErrorKind::Unsupported, "unsupported");
        let validation = io::Error::new(io::ErrorKind::InvalidData, "invalid");

        assert_eq!(classify_exit(&invalid), (2, "invalid_input"));
        assert_eq!(classify_exit(&unsupported), (3, "unsupported"));
        assert_eq!(classify_exit(&validation), (4, "validation_failed"));
    }
}

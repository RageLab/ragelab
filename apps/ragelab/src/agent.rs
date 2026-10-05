use std::{
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    time::Instant,
};

use ragelab_assets::AssetKind;
use ragelab_engine::{
    apply_operation_document, asset_capabilities, asset_type_name, build_vanilla_catalog,
    discover_fivem_legacy, discover_gta_v_legacy, gta_rpf_archive_order, inspect_asset,
    isolated_asset_spatial_context, parse_operation_document, plan_operation_document,
    preview_asset, render_asset_package_bytes_as, render_vanilla_catalog_paths, validate_asset,
    workspace_scene_render_package_with_game_index, workspace_scene_report_with_game_index,
    ymap_spatial_context, EngineError, GtaRpfAssetIndex, GtaRpfWorldBounds, GtaRpfWorldPoint,
    OperationError, PreviewOptions, RenderPackageOptions, SceneAssemblyOptions,
    SceneAssetPreviewSources, SceneGameIndexSource, SceneRpfMount, SpatialContext,
    SpatialProvenance,
};
use ragelab_render::{
    compare_png_files, OffscreenOptions, OffscreenRenderer, Projection, RenderError, RenderView,
    ScreenshotMetadata,
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
            | "render-package"
            | "render.asset"
            | "render.scene"
            | "render.compare"
            | "preflight"
            | "export"
            | "gta.discover"
            | "gta.catalog"
            | "gta.rpf-order"
            | "gta.rpf-index"
            | "gta.world-query"
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
    if let Some(error) = error.downcast_ref::<RenderError>() {
        return match error {
            RenderError::InvalidInput(_) => (2, "invalid_input"),
            RenderError::Unsupported(_) => (3, "unsupported"),
            RenderError::Io(_) | RenderError::Gpu(_) | RenderError::Png(_) => {
                (1, "operation_failed")
            }
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
    game_index: Option<&SceneGameIndexSource>,
    options: SceneAssemblyOptions,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let report = workspace_scene_report_with_game_index(
        workspace,
        ymap_arg,
        fallback_roots,
        rpf_mounts,
        game_index,
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

pub fn render_package(
    scene: &SceneCommandArgs,
    output: &Path,
    options: RenderPackageOptions,
    overwrite: bool,
) -> Result<(), Box<dyn Error>> {
    if output.exists() && !overwrite {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "output already exists: {}; pass --overwrite to replace it",
                output.display()
            ),
        )
        .into());
    }

    let package = workspace_scene_render_package_with_game_index(
        &scene.workspace,
        &scene.ymap,
        SceneAssetPreviewSources {
            fallback_roots: &scene.fallback_roots,
            rpf_mounts: &scene.rpf_mounts,
            game_index: scene.game_index.as_ref(),
        },
        scene.options,
        options,
    )?;
    let report = package.report();
    let bytes = package.encode_binary()?;
    fs::write(output, &bytes)?;

    if scene.json_output {
        print_success(
            "render-package",
            json!({
                "output": output.display().to_string(),
                "report": report,
            }),
        )?;
    } else {
        println!("output: {}", output.display());
        println!("instances: {}", report.summary.instances);
        println!("assets: {}", report.summary.assets);
        println!("ready-assets: {}", report.summary.ready_assets);
        println!("meshes: {}", report.summary.meshes);
        println!("materials: {}", report.summary.materials);
        println!("textures: {}", report.summary.textures);
        println!("blob-bytes: {}", report.blob_bytes);
        println!("encoded-bytes: {}", report.encoded_bytes);
        println!("diagnostics: {}", report.diagnostics.len());
    }

    Ok(())
}

pub struct SceneCommandArgs {
    pub workspace: PathBuf,
    pub ymap: PathBuf,
    pub fallback_roots: Vec<PathBuf>,
    pub rpf_mounts: Vec<SceneRpfMount>,
    pub game_index: Option<SceneGameIndexSource>,
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
    let mut game_root = None;
    let mut game_index = None;
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
            "--game-root" if game_root.is_none() => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                game_root = Some(PathBuf::from(value));
            }
            "--game-index" if game_index.is_none() => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                game_index = Some(PathBuf::from(value));
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
        let keys = rpf_keys.clone().ok_or_else(|| {
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

    let game_index_source = match (game_root, game_index) {
        (None, None) => None,
        (Some(root), Some(index)) => {
            let keys = rpf_keys.clone().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "--rpf-keys is required when --game-index is used",
                )
            })?;
            Some(SceneGameIndexSource::new(root, index, keys))
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "--game-root and --game-index must be provided together",
            ))
        }
    };

    Ok(SceneCommandArgs {
        workspace,
        ymap,
        fallback_roots,
        rpf_mounts: mounts,
        game_index: game_index_source,
        options: max_nodes.map(SceneAssemblyOptions::new).unwrap_or_default(),
        json_output,
    })
}

pub struct RenderPackageCommandArgs {
    pub scene: SceneCommandArgs,
    pub output: PathBuf,
    pub options: RenderPackageOptions,
    pub overwrite: bool,
}

pub fn parse_render_package_args(
    args: impl Iterator<Item = String>,
    usage: &str,
) -> Result<RenderPackageCommandArgs, io::Error> {
    let mut base_args = Vec::new();
    let mut output = None;
    let mut max_assets = None;
    let mut max_blob_bytes = None;
    let mut overwrite = false;
    let mut args = args;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output" => {
                output =
                    Some(PathBuf::from(args.next().ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, usage)
                    })?));
            }
            "--max-assets" => {
                max_assets = Some(parse_preview_limit(args.next(), "--max-assets", usage)?);
            }
            "--max-blob-bytes" => {
                max_blob_bytes = Some(parse_preview_limit(args.next(), "--max-blob-bytes", usage)?);
            }
            "--overwrite" => overwrite = true,
            _ => base_args.push(arg),
        }
    }

    let scene = parse_scene_args(base_args.into_iter(), usage)?;
    let mut options = RenderPackageOptions::default();
    if let Some(value) = max_assets {
        options.max_assets = value;
    }
    if let Some(value) = max_blob_bytes {
        options.max_blob_bytes = value;
    }
    let options = options.validate()?;
    let output = output.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{usage}; --output <file> is required"),
        )
    })?;

    Ok(RenderPackageCommandArgs {
        scene,
        output,
        options,
        overwrite,
    })
}

pub struct RenderAssetCommandArgs {
    pub path: PathBuf,
    pub preview: PreviewOptions,
    pub image: OffscreenOptions,
    pub output: PathBuf,
    pub metadata: Option<PathBuf>,
    pub overwrite: bool,
    pub json_output: bool,
}

pub struct RenderSceneCommandArgs {
    pub scene: SceneCommandArgs,
    pub package: RenderPackageOptions,
    pub image: OffscreenOptions,
    pub output: PathBuf,
    pub metadata: Option<PathBuf>,
    pub overwrite: bool,
}

pub struct RenderCompareCommandArgs {
    pub expected: PathBuf,
    pub actual: PathBuf,
    pub tolerance: u8,
    pub json_output: bool,
}

pub fn parse_render_asset_args(
    args: impl Iterator<Item = String>,
    usage: &str,
) -> Result<RenderAssetCommandArgs, io::Error> {
    let mut args = args;
    let path = args
        .next()
        .filter(|value| !value.starts_with('-'))
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
    let mut preview = PreviewOptions::default();
    let mut image = OffscreenOptions::default();
    let mut output = None;
    let mut metadata = None;
    let mut overwrite = false;
    let mut json_output = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output" => output = Some(parse_path_value(args.next(), usage)?),
            "--metadata" => metadata = Some(parse_path_value(args.next(), usage)?),
            "--overwrite" => overwrite = true,
            "--json" => json_output = true,
            "--transparent" => image.transparent = true,
            "--grid" => image.grid = true,
            "--wireframe" => image.wireframe = true,
            "--bounds" => image.bounds = true,
            "--width" => image.width = parse_u32_option(args.next(), "--width", usage)?,
            "--height" => image.height = parse_u32_option(args.next(), "--height", usage)?,
            "--view" => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                image.view = RenderView::parse(&value).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "--view must be auto/front/back/left/right/top/isometric",
                    )
                })?;
            }
            "--projection" => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                image.projection = Projection::parse(&value).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "--projection must be perspective or orthographic",
                    )
                })?;
            }
            "--drawable-index" if preview.drawable_index.is_none() => {
                preview.drawable_index = Some(parse_preview_index(args.next(), usage)?);
            }
            "--max-primitives" => {
                preview.max_primitives =
                    parse_preview_limit(args.next(), "--max-primitives", usage)?;
            }
            "--max-vertices" => {
                preview.max_vertices = parse_preview_limit(args.next(), "--max-vertices", usage)?;
            }
            "--max-indices" => {
                preview.max_indices = parse_preview_limit(args.next(), "--max-indices", usage)?;
            }
            "--max-shaders" => {
                preview.max_shaders = parse_preview_limit(args.next(), "--max-shaders", usage)?;
            }
            "--max-texture-references" => {
                preview.max_texture_references =
                    parse_preview_limit(args.next(), "--max-texture-references", usage)?;
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{usage}; unknown option: {arg}"),
                ))
            }
        }
    }

    let output = output.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{usage}; --output <file.png> is required"),
        )
    })?;
    validate_png_path(&output)?;
    let image = image
        .validate()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;

    Ok(RenderAssetCommandArgs {
        path,
        preview: preview.validate()?,
        image,
        output,
        metadata,
        overwrite,
        json_output,
    })
}

pub fn parse_render_scene_args(
    args: impl Iterator<Item = String>,
    usage: &str,
) -> Result<RenderSceneCommandArgs, io::Error> {
    let mut args = args;
    let mut scene_args = Vec::new();
    let mut image = OffscreenOptions::default();
    let mut package = RenderPackageOptions::default();
    let mut output = None;
    let mut metadata = None;
    let mut overwrite = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output" => output = Some(parse_path_value(args.next(), usage)?),
            "--metadata" => metadata = Some(parse_path_value(args.next(), usage)?),
            "--overwrite" => overwrite = true,
            "--transparent" => image.transparent = true,
            "--grid" => image.grid = true,
            "--wireframe" => image.wireframe = true,
            "--bounds" => image.bounds = true,
            "--width" => image.width = parse_u32_option(args.next(), "--width", usage)?,
            "--height" => image.height = parse_u32_option(args.next(), "--height", usage)?,
            "--view" => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                image.view = RenderView::parse(&value).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "--view must be auto/front/back/left/right/top/isometric",
                    )
                })?;
            }
            "--projection" => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                image.projection = Projection::parse(&value).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "--projection must be perspective or orthographic",
                    )
                })?;
            }
            "--max-assets" => {
                package.max_assets = parse_preview_limit(args.next(), "--max-assets", usage)?;
            }
            "--max-blob-bytes" => {
                package.max_blob_bytes =
                    parse_preview_limit(args.next(), "--max-blob-bytes", usage)?;
            }
            _ => scene_args.push(arg),
        }
    }

    let output = output.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{usage}; --output <file.png> is required"),
        )
    })?;
    validate_png_path(&output)?;
    let scene = parse_scene_args(scene_args.into_iter(), usage)?;
    let image = image
        .validate()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;

    Ok(RenderSceneCommandArgs {
        scene,
        package: package.validate()?,
        image,
        output,
        metadata,
        overwrite,
    })
}

pub fn parse_render_compare_args(
    args: impl Iterator<Item = String>,
    usage: &str,
) -> Result<RenderCompareCommandArgs, io::Error> {
    let mut args = args;
    let expected = args
        .next()
        .filter(|value| !value.starts_with('-'))
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
    let actual = args
        .next()
        .filter(|value| !value.starts_with('-'))
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
    let mut tolerance = 0_u8;
    let mut json_output = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json_output = true,
            "--tolerance" => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                tolerance = value.parse::<u8>().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "--tolerance must be an integer within 0..=255",
                    )
                })?;
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{usage}; unknown option: {arg}"),
                ))
            }
        }
    }
    Ok(RenderCompareCommandArgs {
        expected,
        actual,
        tolerance,
        json_output,
    })
}

pub fn render_asset(command: &RenderAssetCommandArgs) -> Result<(), Box<dyn Error>> {
    let bytes = fs::read(&command.path)?;
    let asset_type = asset_type_name(&command.path);
    if !matches!(asset_type, "YDR" | "YDD" | "YFT") {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("native model rendering is unavailable for {asset_type}"),
        )
        .into());
    }
    let package = render_asset_package_bytes_as(
        &command.path.display().to_string(),
        asset_type,
        &bytes,
        command.preview,
        &[],
        RenderPackageOptions::default().max_blob_bytes,
    )?;
    let mut renderer = OffscreenRenderer::new()?;
    let image = renderer.render(&package, command.image)?;
    let metadata = ScreenshotMetadata::new(&package, command.image, &image);
    let metadata_path = render_metadata_path(&command.output, command.metadata.as_deref());
    validate_render_outputs(&command.output, &metadata_path, command.overwrite)?;
    image.write_png(&command.output)?;
    metadata.write_json(&metadata_path)?;

    if command.json_output {
        print_success(
            "render.asset",
            json!({
                "output": command.output.display().to_string(),
                "metadata": metadata_path.display().to_string(),
                "screenshot": metadata,
                "package": package.report(),
            }),
        )?;
    } else {
        print_render_summary(&command.output, &metadata_path, &metadata);
    }
    Ok(())
}

pub fn render_scene(command: &RenderSceneCommandArgs) -> Result<(), Box<dyn Error>> {
    let package = workspace_scene_render_package_with_game_index(
        &command.scene.workspace,
        &command.scene.ymap,
        SceneAssetPreviewSources {
            fallback_roots: &command.scene.fallback_roots,
            rpf_mounts: &command.scene.rpf_mounts,
            game_index: command.scene.game_index.as_ref(),
        },
        command.scene.options,
        command.package,
    )?;
    let mut renderer = OffscreenRenderer::new()?;
    let image = renderer.render(&package, command.image)?;
    let metadata = ScreenshotMetadata::new(&package, command.image, &image);
    let metadata_path = render_metadata_path(&command.output, command.metadata.as_deref());
    validate_render_outputs(&command.output, &metadata_path, command.overwrite)?;
    image.write_png(&command.output)?;
    metadata.write_json(&metadata_path)?;

    if command.scene.json_output {
        print_success(
            "render.scene",
            json!({
                "output": command.output.display().to_string(),
                "metadata": metadata_path.display().to_string(),
                "screenshot": metadata,
                "package": package.report(),
            }),
        )?;
    } else {
        print_render_summary(&command.output, &metadata_path, &metadata);
    }
    Ok(())
}

pub fn render_compare(command: &RenderCompareCommandArgs) -> Result<(), Box<dyn Error>> {
    let report = compare_png_files(&command.expected, &command.actual, command.tolerance)?;
    if command.json_output {
        print_success("render.compare", serde_json::to_value(report)?)?;
    } else {
        println!("expected: {}", command.expected.display());
        println!("actual: {}", command.actual.display());
        println!("tolerance: {}", report.tolerance);
        println!("changed-pixels: {}", report.changed_pixels);
        println!("max-channel-delta: {}", report.max_channel_delta);
        println!(
            "mean-absolute-channel-error: {:.6}",
            report.mean_absolute_channel_error
        );
        println!(
            "within-tolerance: {}",
            if report.within_tolerance { "yes" } else { "no" }
        );
    }
    Ok(())
}

fn parse_path_value(value: Option<String>, usage: &str) -> Result<PathBuf, io::Error> {
    value
        .filter(|value| !value.starts_with('-'))
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))
}

fn parse_u32_option(value: Option<String>, label: &str, usage: &str) -> Result<u32, io::Error> {
    let value = value.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
    let parsed = value.parse::<u32>().map_err(|_| {
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

fn validate_png_path(path: &Path) -> Result<(), io::Error> {
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("png"))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "render output must use the .png extension",
        ));
    }
    Ok(())
}

fn render_metadata_path(output: &Path, explicit: Option<&Path>) -> PathBuf {
    if let Some(path) = explicit {
        return path.to_path_buf();
    }
    let mut path = output.to_path_buf();
    let extension = output
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!("{value}.json"))
        .unwrap_or_else(|| "json".into());
    path.set_extension(extension);
    path
}

fn validate_render_outputs(
    output: &Path,
    metadata: &Path,
    overwrite: bool,
) -> Result<(), io::Error> {
    if output == metadata {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "PNG and metadata outputs must be different files",
        ));
    }
    if !overwrite {
        for path in [output, metadata] {
            if path.exists() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "output already exists: {}; pass --overwrite to replace it",
                        path.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn print_render_summary(output: &Path, metadata_path: &Path, metadata: &ScreenshotMetadata) {
    println!("output: {}", output.display());
    println!("metadata: {}", metadata_path.display());
    println!("size: {}x{}", metadata.width, metadata.height);
    println!("view: {}", metadata.view.as_str());
    println!("projection: {}", metadata.projection.as_str());
    println!("instances: {}", metadata.instances);
    println!("assets: {}", metadata.assets);
    println!("meshes: {}", metadata.meshes);
    println!("materials: {}", metadata.materials);
    println!("textures: {}", metadata.textures);
    println!("image-sha256: {}", metadata.image_sha256);
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

pub struct GtaRpfIndexCommandArgs {
    pub root: PathBuf,
    pub keys: PathBuf,
    pub output: PathBuf,
    pub overwrite: bool,
    pub json_output: bool,
}

pub fn parse_gta_rpf_index_args(
    args: impl Iterator<Item = String>,
    usage: &str,
) -> Result<GtaRpfIndexCommandArgs, io::Error> {
    let mut args = args;
    let root = args
        .next()
        .filter(|value| !value.starts_with('-'))
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;

    let mut keys = None;
    let mut output = None;
    let mut overwrite = false;
    let mut json_output = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--keys" if keys.is_none() => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                keys = Some(PathBuf::from(value));
            }
            "--output" if output.is_none() => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
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

    Ok(GtaRpfIndexCommandArgs {
        root,
        keys: keys.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?,
        output: output.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?,
        overwrite,
        json_output,
    })
}

pub fn gta_rpf_index(request: GtaRpfIndexCommandArgs) -> Result<(), Box<dyn Error>> {
    let build = GtaRpfAssetIndex::build(&request.root, &request.keys)?;
    build.index.save(&request.output, request.overwrite)?;
    let bytes = fs::metadata(&request.output)?.len();

    if request.json_output {
        print_success(
            "gta.rpf-index",
            json!({
                "report": build.report,
                "output": request.output.display().to_string(),
                "bytes": bytes,
            }),
        )?;
    } else {
        println!("output: {}", request.output.display());
        println!("bytes: {bytes}");
        println!("ordered-archives: {}", build.report.ordered_archives);
        println!("scanned-archives: {}", build.report.scanned_archives);
        println!("nested-archives: {}", build.report.nested_archives);
        println!("indexed-files: {}", build.report.indexed_files);
        println!("parsed-ytyps: {}", build.report.parsed_ytyps);
        println!("parsed-ymaps: {}", build.report.parsed_ymaps);
        println!("archetypes: {}", build.report.archetypes);
        println!("ytyp-records: {}", build.report.ytyp_records);
        println!("world-maps: {}", build.report.world_maps);
        println!("world-entities: {}", build.report.world_entities);
        println!("file-keys: {}", build.report.file_keys);
        println!("warnings: {}", build.report.warnings.len());
    }

    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub enum GtaWorldQueryShape {
    Radius {
        center: GtaRpfWorldPoint,
        radius: f32,
    },
    Box {
        bounds: GtaRpfWorldBounds,
    },
}

pub struct GtaWorldQueryCommandArgs {
    pub index: PathBuf,
    pub shape: GtaWorldQueryShape,
    pub include_entities: bool,
    pub repeat: usize,
    pub json_output: bool,
}

pub fn parse_gta_world_query_args(
    args: impl Iterator<Item = String>,
    usage: &str,
) -> Result<GtaWorldQueryCommandArgs, io::Error> {
    let mut args = args;
    let index = args
        .next()
        .filter(|value| !value.starts_with('-'))
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;

    let mut point = None;
    let mut radius = None;
    let mut bounds = None;
    let mut include_entities = false;
    let mut repeat = 1usize;
    let mut json_output = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--point" if point.is_none() && bounds.is_none() => {
                point = Some(GtaRpfWorldPoint {
                    x: parse_world_f32(args.next(), usage, "--point x")?,
                    y: parse_world_f32(args.next(), usage, "--point y")?,
                    z: parse_world_f32(args.next(), usage, "--point z")?,
                });
            }
            "--radius" if radius.is_none() && bounds.is_none() => {
                radius = Some(parse_world_f32(args.next(), usage, "--radius")?);
            }
            "--box" if bounds.is_none() && point.is_none() && radius.is_none() => {
                bounds = Some(GtaRpfWorldBounds {
                    min: GtaRpfWorldPoint {
                        x: parse_world_f32(args.next(), usage, "--box minx")?,
                        y: parse_world_f32(args.next(), usage, "--box miny")?,
                        z: parse_world_f32(args.next(), usage, "--box minz")?,
                    },
                    max: GtaRpfWorldPoint {
                        x: parse_world_f32(args.next(), usage, "--box maxx")?,
                        y: parse_world_f32(args.next(), usage, "--box maxy")?,
                        z: parse_world_f32(args.next(), usage, "--box maxz")?,
                    },
                });
            }
            "--entities" if !include_entities => include_entities = true,
            "--repeat" if repeat == 1 => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                repeat = value.parse::<usize>().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{usage}; --repeat must be a positive integer"),
                    )
                })?;
                if repeat == 0 || repeat > 100_000 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{usage}; --repeat must be in 1..=100000"),
                    ));
                }
            }
            "--json" if !json_output => json_output = true,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{usage}; unknown or conflicting option: {arg}"),
                ))
            }
        }
    }

    let shape = match (point, radius, bounds) {
        (Some(center), Some(radius), None) => GtaWorldQueryShape::Radius { center, radius },
        (None, None, Some(bounds)) => GtaWorldQueryShape::Box { bounds },
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{usage}; provide either --point x y z --radius r or --box minx miny minz maxx maxy maxz"),
            ))
        }
    };

    Ok(GtaWorldQueryCommandArgs {
        index,
        shape,
        include_entities,
        repeat,
        json_output,
    })
}

pub fn gta_world_query(request: GtaWorldQueryCommandArgs) -> Result<(), Box<dyn Error>> {
    let load_started = Instant::now();
    let index = GtaRpfAssetIndex::load(&request.index)?;
    let index_load_ms = load_started.elapsed().as_secs_f64() * 1000.0;

    let query_started = Instant::now();
    let mut report = None;
    for _ in 0..request.repeat {
        report = Some(match request.shape {
            GtaWorldQueryShape::Radius { center, radius } => {
                index.query_world_radius(center, radius, request.include_entities)?
            }
            GtaWorldQueryShape::Box { bounds } => {
                index.query_world_box(bounds, request.include_entities)?
            }
        });
    }
    let query_total_ms = query_started.elapsed().as_secs_f64() * 1000.0;
    let query_average_ms = query_total_ms / request.repeat as f64;
    let report = report.ok_or_else(|| io::Error::other("world query did not execute"))?;

    if request.json_output {
        print_success(
            "gta.world-query",
            json!({
                "index": request.index.display().to_string(),
                "indexLoadMs": index_load_ms,
                "repeat": request.repeat,
                "queryTotalMs": query_total_ms,
                "queryAverageMs": query_average_ms,
                "report": report,
            }),
        )?;
    } else {
        println!("index: {}", request.index.display());
        println!("index-load-ms: {index_load_ms:.3}");
        println!("repeat: {}", request.repeat);
        println!("query-average-ms: {query_average_ms:.6}");
        println!("candidate-map-keys: {}", report.candidate_map_keys);
        println!("maps: {}", report.maps.len());
        println!("entities: {}", report.entities.len());
        for map in &report.maps {
            println!(
                "  map 0x{:08X}: {}{}!{}",
                map.map_hash,
                map.provider.archive_relative,
                if map.provider.nested.is_empty() {
                    String::new()
                } else {
                    format!("!/{}", map.provider.nested.join("!/"))
                },
                map.provider.entry
            );
        }
    }
    Ok(())
}

fn parse_world_f32(value: Option<String>, usage: &str, name: &str) -> Result<f32, io::Error> {
    let raw = value.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
    let parsed = raw.parse::<f32>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{usage}; {name} must be a finite number"),
        )
    })?;
    if !parsed.is_finite() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{usage}; {name} must be finite"),
        ));
    }
    Ok(parsed)
}

pub fn parse_gta_rpf_order_args(
    args: impl Iterator<Item = String>,
    usage: &str,
) -> Result<(PathBuf, PathBuf, bool), io::Error> {
    let mut args = args;
    let root = args
        .next()
        .filter(|value| !value.starts_with('-'))
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;

    let mut keys = None;
    let mut json_output = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--keys" if keys.is_none() => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                keys = Some(PathBuf::from(value));
            }
            "--json" if !json_output => json_output = true,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{usage}; unknown option: {arg}"),
                ))
            }
        }
    }

    let keys = keys.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
    Ok((root, keys, json_output))
}

pub fn gta_rpf_order(root: &Path, keys: &Path, json_output: bool) -> Result<(), Box<dyn Error>> {
    let report = gta_rpf_archive_order(root, keys)?;
    if json_output {
        print_success("gta.rpf-order", serde_json::to_value(&report)?)?;
    } else {
        println!("game-root: {}", report.game_root);
        println!("archives: {}", report.archives.len());
        println!("platform-packs: {}", report.platform_packs.len());
        println!("warnings: {}", report.warnings.len());
        for archive in report.archives {
            println!(
                "{:03} [{}] {}",
                archive.load_rank, archive.tier, archive.relative_path
            );
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
        ("workspace", "render-package") => Some("render-package"),
        ("render", "asset") => Some("render.asset"),
        ("render", "scene") => Some("render.scene"),
        ("render", "compare") => Some("render.compare"),
        ("gta", "discover") => Some("gta.discover"),
        ("gta", "catalog") => Some("gta.catalog"),
        ("gta", "rpf-order") => Some("gta.rpf-order"),
        ("gta", "rpf-index") => Some("gta.rpf-index"),
        ("gta", "world-query") => Some("gta.world-query"),
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

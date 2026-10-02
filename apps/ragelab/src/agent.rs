use std::{
    collections::BTreeSet,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
};

use ragelab_assets::{AssetKind, WorkspaceIndex};
use ragelab_engine::{
    apply_operation_document, assemble_ymap_scene, discover_fivem_legacy, discover_gta_v_legacy,
    isolated_asset_spatial_context, parse_operation_document, plan_operation_document,
    ymap_spatial_context, EngineError, OperationError, SceneAssemblyOptions, SceneAssetSelector,
    SceneCollisionState, SceneManifest, SceneResolutionReasonCode, SceneResolutionState,
    SpatialContext, SpatialProvenance,
};
use ragelab_resource::{Rsc7Probe, Rsc7Resource};
use ragelab_ybn::{CollisionShape, YbnCollision};
use ragelab_ydd::{YddDictionary, YddEditSession};
use ragelab_ydr::{YdrDocument, YdrEditSession, YdrModel};
use ragelab_ymap::Ymap;
use ragelab_ymf::Ymf;
use ragelab_ytd::Ytd;
use ragelab_ytyp::{ArchetypeKind, Ytyp};
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
    let bytes = fs::read(path)?;
    let asset_type = asset_type(path);
    let container = inspect_container(&bytes);
    let details = inspect_format(path, &bytes, asset_type)?;

    if json_output {
        print_success(
            "inspect",
            json!({
                "path": path.display().to_string(),
                "type": asset_type,
                "bytes": bytes.len(),
                "container": container,
                "details": details,
            }),
        )?;
    } else {
        println!("file: {}", path.display());
        println!("type: {asset_type}");
        println!("bytes: {}", bytes.len());
        if let Some(container) = container {
            println!(
                "container: {}",
                container
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            );
        }
        println!(
            "status: {}",
            if details.is_null() {
                "container inspected"
            } else {
                "format parsed"
            }
        );
    }

    Ok(())
}

pub fn validate(path: &Path, json_output: bool) -> Result<(), Box<dyn Error>> {
    let bytes = fs::read(path)?;
    let asset_type = asset_type(path);
    let mut checks = Vec::new();

    if Rsc7Probe::parse(&bytes).is_ok() {
        Rsc7Resource::parse(&bytes).map_err(validation_error)?;
        checks.push("rsc7.container");
    }

    match asset_type {
        "YMAP" => {
            Ymap::from_bytes(&bytes).map_err(validation_error)?;
            checks.push("ymap.parse");
        }
        "YTYP" => {
            Ytyp::from_bytes(&bytes).map_err(validation_error)?;
            checks.push("ytyp.parse");
        }
        "YMF" => {
            Ymf::from_bytes(&bytes).map_err(validation_error)?;
            checks.push("ymf.parse");
        }
        "YTD" => {
            Ytd::from_bytes(&bytes).map_err(validation_error)?;
            checks.push("ytd.parse");
        }
        "YBN" => {
            YbnCollision::from_bytes(&bytes).map_err(validation_error)?;
            checks.push("ybn.parse");
        }
        "YDR" => {
            YdrDocument::from_bytes(&bytes).map_err(validation_error)?;
            checks.push("ydr.parse");
        }
        "YDD" => {
            YddDictionary::from_bytes(&bytes).map_err(validation_error)?;
            checks.push("ydd.parse");
        }
        _ if checks.is_empty() => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "no validator is available for file type {} ({})",
                    asset_type,
                    path.display()
                ),
            )
            .into());
        }
        _ => {}
    }

    if json_output {
        print_success(
            "validate",
            json!({
                "path": path.display().to_string(),
                "type": asset_type,
                "valid": true,
                "checks": checks,
            }),
        )?;
    } else {
        println!("file: {}", path.display());
        println!("type: {asset_type}");
        println!("valid: yes");
        for check in checks {
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
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a file: {}", path.display()),
        )
        .into());
    }

    let asset_type = asset_type(path);
    let bytes = fs::read(path)?;
    let operations = operations_for(asset_type, &bytes)?;

    if json_output {
        print_success(
            "capabilities",
            json!({
                "path": path.display().to_string(),
                "type": asset_type,
                "operations": operations,
            }),
        )?;
    } else {
        println!("file: {}", path.display());
        println!("type: {asset_type}");
        println!("operations:");
        for operation in operations {
            let id = operation
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let mode = if operation
                .get("writesAsset")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "write"
            } else {
                "read"
            };
            println!("  {id} [{mode}]");
        }
    }

    Ok(())
}

const DEFAULT_PREVIEW_MAX_PRIMITIVES: usize = 64;
const DEFAULT_PREVIEW_MAX_VERTICES: usize = 10_000;
const DEFAULT_PREVIEW_MAX_INDICES: usize = 30_000;
const DEFAULT_PREVIEW_MAX_SHADERS: usize = 128;
const DEFAULT_PREVIEW_MAX_TEXTURE_REFERENCES: usize = 512;
const DEFAULT_PREVIEW_MAX_CHILDREN: usize = 256;
const DEFAULT_PREVIEW_MAX_MATERIALS: usize = 512;

const HARD_PREVIEW_MAX_PRIMITIVES: usize = 512;
const HARD_PREVIEW_MAX_VERTICES: usize = 100_000;
const HARD_PREVIEW_MAX_INDICES: usize = 300_000;
const HARD_PREVIEW_MAX_SHADERS: usize = 1_024;
const HARD_PREVIEW_MAX_TEXTURE_REFERENCES: usize = 4_096;
const HARD_PREVIEW_MAX_CHILDREN: usize = 4_096;
const HARD_PREVIEW_MAX_MATERIALS: usize = 8_192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewOptions {
    pub drawable_index: Option<usize>,
    pub max_primitives: usize,
    pub max_vertices: usize,
    pub max_indices: usize,
    pub max_shaders: usize,
    pub max_texture_references: usize,
    pub max_children: usize,
    pub max_materials: usize,
}

impl Default for PreviewOptions {
    fn default() -> Self {
        Self {
            drawable_index: None,
            max_primitives: DEFAULT_PREVIEW_MAX_PRIMITIVES,
            max_vertices: DEFAULT_PREVIEW_MAX_VERTICES,
            max_indices: DEFAULT_PREVIEW_MAX_INDICES,
            max_shaders: DEFAULT_PREVIEW_MAX_SHADERS,
            max_texture_references: DEFAULT_PREVIEW_MAX_TEXTURE_REFERENCES,
            max_children: DEFAULT_PREVIEW_MAX_CHILDREN,
            max_materials: DEFAULT_PREVIEW_MAX_MATERIALS,
        }
    }
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
                options.max_primitives = parse_preview_limit(
                    args.next(),
                    "--max-primitives",
                    HARD_PREVIEW_MAX_PRIMITIVES,
                    usage,
                )?;
            }
            "--max-vertices" => {
                options.max_vertices = parse_preview_limit(
                    args.next(),
                    "--max-vertices",
                    HARD_PREVIEW_MAX_VERTICES,
                    usage,
                )?;
            }
            "--max-indices" => {
                options.max_indices = parse_preview_limit(
                    args.next(),
                    "--max-indices",
                    HARD_PREVIEW_MAX_INDICES,
                    usage,
                )?;
            }
            "--max-shaders" => {
                options.max_shaders = parse_preview_limit(
                    args.next(),
                    "--max-shaders",
                    HARD_PREVIEW_MAX_SHADERS,
                    usage,
                )?;
            }
            "--max-texture-references" => {
                options.max_texture_references = parse_preview_limit(
                    args.next(),
                    "--max-texture-references",
                    HARD_PREVIEW_MAX_TEXTURE_REFERENCES,
                    usage,
                )?;
            }
            "--max-children" => {
                options.max_children = parse_preview_limit(
                    args.next(),
                    "--max-children",
                    HARD_PREVIEW_MAX_CHILDREN,
                    usage,
                )?;
            }
            "--max-materials" => {
                options.max_materials = parse_preview_limit(
                    args.next(),
                    "--max-materials",
                    HARD_PREVIEW_MAX_MATERIALS,
                    usage,
                )?;
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{usage}; unknown option: {arg}"),
                ))
            }
        }
    }

    Ok((path, options, json_output))
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
    hard_max: usize,
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
    if hard_max != usize::MAX && parsed > hard_max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{label} exceeds hard limit {hard_max}"),
        ));
    }
    Ok(parsed)
}

pub fn preview(
    path: &Path,
    options: PreviewOptions,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let bytes = fs::read(path)?;
    let asset_type = asset_type(path);

    if asset_type == "YBN" {
        if options.drawable_index.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "--drawable-index is only valid for YDD preview",
            )
            .into());
        }

        let collision = YbnCollision::from_bytes(&bytes).map_err(validation_error)?;
        let data = collision_preview_json(&collision, options);

        if json_output {
            print_success(
                "preview",
                json!({
                    "path": path.display().to_string(),
                    "type": asset_type,
                    "spatial": {
                        "classification": "localOnly",
                        "coordinateConvention": collision.coordinate_convention,
                    },
                    "preview": data,
                }),
            )?;
        } else {
            println!("file: {}", path.display());
            println!("type: {asset_type}");
            println!("children: {}", collision.children.len());
            println!("materials: {}", collision.materials.len());
            println!("mesh-primitives: {}", collision.primitives.len());
            println!("shape-primitives: {}", collision.shape_primitives.len());
            println!("vertices: {}", collision.vertex_count());
            println!("indices: {}", collision.index_count());
            println!("triangles: {}", collision.triangle_count());
            println!("spatial: localOnly");
        }

        return Ok(());
    }

    let (model, selector) = match asset_type {
        "YDR" => {
            if options.drawable_index.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "--drawable-index is only valid for YDD preview",
                )
                .into());
            }
            let document = YdrDocument::from_bytes(&bytes).map_err(validation_error)?;
            (document.model, Value::Null)
        }
        "YDD" => {
            let drawable_index = options.drawable_index.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "YDD preview requires --drawable-index <n>",
                )
            })?;
            let dictionary = YddDictionary::from_bytes(&bytes).map_err(validation_error)?;
            let entry = dictionary.entries().get(drawable_index).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "drawable index {drawable_index} exceeds dictionary size {}",
                        dictionary.entries().len()
                    ),
                )
            })?;
            let document = dictionary
                .document(drawable_index)
                .map_err(validation_error)?;
            (
                document.model,
                json!({
                    "drawableIndex": entry.index,
                    "nameHash": format!("0x{:08X}", entry.name_hash),
                    "name": entry.name,
                }),
            )
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("headless model preview is unavailable for {asset_type}"),
            )
            .into())
        }
    };

    let data = model_preview_json(&model, options, selector);

    if json_output {
        print_success(
            "preview",
            json!({
                "path": path.display().to_string(),
                "type": asset_type,
                "spatial": {
                    "classification": "localOnly",
                    "coordinateConvention": model.coordinate_convention.as_str(),
                },
                "preview": data,
            }),
        )?;
    } else {
        println!("file: {}", path.display());
        println!("type: {asset_type}");
        if let Some(index) = options.drawable_index {
            println!("drawable-index: {index}");
        }
        println!("lod: {}", model.lod.as_str());
        println!("primitives: {}", model.primitives.len());
        println!("vertices: {}", model.vertex_count());
        println!("indices: {}", model.index_count());
        println!("triangles: {}", model.triangle_count());
        println!("spatial: localOnly");
    }

    Ok(())
}

fn model_preview_json(model: &YdrModel, options: PreviewOptions, selector: Value) -> Value {
    let mut remaining_vertices = options.max_vertices;
    let mut remaining_indices = options.max_indices;
    let mut emitted_vertices = 0_usize;
    let mut emitted_indices = 0_usize;
    let mut geometry_omitted = 0_usize;

    let primitives = model
        .primitives
        .iter()
        .take(options.max_primitives)
        .map(|primitive| {
            let fits_geometry = primitive.positions.len() <= remaining_vertices
                && primitive.indices.len() <= remaining_indices;

            let geometry = if fits_geometry {
                remaining_vertices -= primitive.positions.len();
                remaining_indices -= primitive.indices.len();
                emitted_vertices += primitive.positions.len();
                emitted_indices += primitive.indices.len();
                json!({
                    "positions": primitive.positions,
                    "normals": primitive.normals,
                    "uv0": primitive.uv0,
                    "indices": primitive.indices,
                })
            } else {
                geometry_omitted += 1;
                Value::Null
            };

            let winding = primitive.winding_summary().map(|summary| {
                json!({
                    "aligned": summary.aligned,
                    "opposed": summary.opposed,
                    "degenerate": summary.degenerate,
                })
            });

            json!({
                "modelIndex": primitive.model_index,
                "geometryIndex": primitive.geometry_index,
                "shaderIndex": primitive.shader_index,
                "topology": primitive.topology.as_str(),
                "counts": {
                    "vertices": primitive.positions.len(),
                    "indices": primitive.indices.len(),
                    "triangles": primitive.triangle_count(),
                },
                "declaration": {
                    "flags": format!("0x{:08X}", primitive.declaration.flags),
                    "stride": primitive.declaration.stride,
                    "componentCount": primitive.declaration.component_count,
                    "types": format!("0x{:016X}", primitive.declaration.types),
                },
                "winding": winding,
                "geometryIncluded": fits_geometry,
                "geometryOmittedReason": if fits_geometry {
                    Value::Null
                } else {
                    Value::String("preview limits".into())
                },
                "geometry": geometry,
            })
        })
        .collect::<Vec<_>>();

    let mut remaining_texture_references = options.max_texture_references;
    let mut emitted_texture_references = 0_usize;
    let shaders = model
        .shaders
        .iter()
        .take(options.max_shaders)
        .enumerate()
        .map(|(index, shader)| {
            let take = shader
                .texture_references
                .len()
                .min(remaining_texture_references);
            remaining_texture_references -= take;
            emitted_texture_references += take;
            let texture_references = shader
                .texture_references
                .iter()
                .take(take)
                .map(|reference| {
                    json!({
                        "parameterHash": format!("0x{:08X}", reference.parameter_hash),
                        "textureName": reference.texture_name,
                    })
                })
                .collect::<Vec<_>>();

            json!({
                "index": index,
                "nameHash": format!("0x{:08X}", shader.name_hash),
                "fileHash": format!("0x{:08X}", shader.file_hash),
                "textureReferenceCount": shader.texture_references.len(),
                "textureReferences": texture_references,
                "textureReferencesTruncated": take < shader.texture_references.len(),
            })
        })
        .collect::<Vec<_>>();

    json!({
        "selector": selector,
        "name": model.name,
        "lod": model.lod.as_str(),
        "coordinateConvention": model.coordinate_convention.as_str(),
        "bounds": {
            "center": model.bounds.center,
            "radius": model.bounds.radius,
            "min": model.bounds.min,
            "max": model.bounds.max,
        },
        "counts": {
            "shaders": model.shaders.len(),
            "primitives": model.primitives.len(),
            "vertices": model.vertex_count(),
            "indices": model.index_count(),
            "triangles": model.triangle_count(),
        },
        "shaders": shaders,
        "primitives": primitives,
        "limits": {
            "maxShaders": options.max_shaders,
            "maxTextureReferences": options.max_texture_references,
            "maxPrimitives": options.max_primitives,
            "maxVertices": options.max_vertices,
            "maxIndices": options.max_indices,
        },
        "emitted": {
            "shaders": model.shaders.len().min(options.max_shaders),
            "textureReferences": emitted_texture_references,
            "primitives": model.primitives.len().min(options.max_primitives),
            "vertices": emitted_vertices,
            "indices": emitted_indices,
        },
        "truncated": {
            "shaders": model.shaders.len() > options.max_shaders,
            "textureReferences": model
                .shaders
                .iter()
                .map(|shader| shader.texture_references.len())
                .sum::<usize>()
                > options.max_texture_references,
            "primitives": model.primitives.len() > options.max_primitives,
            "geometryOmittedPrimitives": geometry_omitted,
        },
    })
}

fn collision_preview_json(collision: &YbnCollision, options: PreviewOptions) -> Value {
    let positions_included = collision.positions.len() <= options.max_vertices;
    let positions = if positions_included {
        json!(collision.positions)
    } else {
        Value::Null
    };

    let mut remaining_indices = options.max_indices;
    let mut remaining_primitives = options.max_primitives;
    let mut emitted_indices = 0_usize;
    let mut mesh_primitives = Vec::new();

    for primitive in &collision.primitives {
        if remaining_primitives == 0 {
            break;
        }
        remaining_primitives -= 1;

        let indices_included = positions_included && primitive.indices.len() <= remaining_indices;
        let indices = if indices_included {
            remaining_indices -= primitive.indices.len();
            emitted_indices += primitive.indices.len();
            json!(primitive.indices)
        } else {
            Value::Null
        };

        mesh_primitives.push(json!({
            "childIndex": primitive.child_index,
            "materialIndex": primitive.material_index,
            "indexCount": primitive.indices.len(),
            "triangleCount": primitive.indices.len() / 3,
            "indicesIncluded": indices_included,
            "indicesOmittedReason": if indices_included {
                Value::Null
            } else if !positions_included {
                Value::String("vertex preview limits".into())
            } else {
                Value::String("index preview limits".into())
            },
            "indices": indices,
        }));
    }

    let emitted_mesh_primitives = mesh_primitives.len();
    let mut shape_primitives = Vec::new();
    for primitive in &collision.shape_primitives {
        if remaining_primitives == 0 {
            break;
        }
        remaining_primitives -= 1;

        shape_primitives.push(json!({
            "childIndex": primitive.child_index,
            "materialIndex": primitive.material_index,
            "polygonIndex": primitive.polygon_index,
            "shape": collision_shape_json(&primitive.shape),
        }));
    }
    let emitted_shape_primitives = shape_primitives.len();

    let children = collision
        .children
        .iter()
        .take(options.max_children)
        .map(|child| {
            json!({
                "index": child.index,
                "boundsType": child.bounds_type.as_str(),
                "bounds": {
                    "min": child.bounds.min,
                    "max": child.bounds.max,
                    "center": child.bounds.center,
                    "sphereCenter": child.bounds.sphere_center,
                    "sphereRadius": child.bounds.sphere_radius,
                },
                "vertices": child.vertices,
                "triangles": child.triangles,
                "shapePrimitives": child.shape_primitives,
            })
        })
        .collect::<Vec<_>>();

    let materials = collision
        .materials
        .iter()
        .take(options.max_materials)
        .map(|material| {
            json!({
                "index": material.index,
                "childIndex": material.child_index,
                "localIndex": material.local_index,
                "materialType": material.material_type,
                "proceduralId": material.procedural_id,
                "flags": format!("0x{:04X}", material.flags),
                "colorIndex": material.color_index,
            })
        })
        .collect::<Vec<_>>();

    json!({
        "kind": "collision",
        "coordinateConvention": collision.coordinate_convention,
        "bounds": {
            "min": collision.bounds.min,
            "max": collision.bounds.max,
            "center": collision.bounds.center,
            "sphereCenter": collision.bounds.sphere_center,
            "sphereRadius": collision.bounds.sphere_radius,
        },
        "counts": {
            "children": collision.children.len(),
            "materials": collision.materials.len(),
            "meshPrimitives": collision.primitives.len(),
            "shapePrimitives": collision.shape_primitives.len(),
            "vertices": collision.vertex_count(),
            "indices": collision.index_count(),
            "triangles": collision.triangle_count(),
        },
        "children": children,
        "materials": materials,
        "mesh": {
            "positionsIncluded": positions_included,
            "positionsOmittedReason": if positions_included {
                Value::Null
            } else {
                Value::String("vertex preview limits".into())
            },
            "positions": positions,
            "primitives": mesh_primitives,
        },
        "shapePrimitives": shape_primitives,
        "limits": {
            "maxPrimitives": options.max_primitives,
            "maxVertices": options.max_vertices,
            "maxIndices": options.max_indices,
            "maxChildren": options.max_children,
            "maxMaterials": options.max_materials,
        },
        "emitted": {
            "children": collision.children.len().min(options.max_children),
            "materials": collision.materials.len().min(options.max_materials),
            "meshPrimitives": emitted_mesh_primitives,
            "shapePrimitives": emitted_shape_primitives,
            "vertices": if positions_included {
                collision.positions.len()
            } else {
                0
            },
            "indices": emitted_indices,
        },
        "truncated": {
            "children": collision.children.len() > options.max_children,
            "materials": collision.materials.len() > options.max_materials,
            "primitives": collision.primitives.len() + collision.shape_primitives.len()
                > options.max_primitives,
            "vertices": !positions_included,
            "indices": emitted_indices < collision.index_count(),
        },
    })
}

fn collision_shape_json(shape: &CollisionShape) -> Value {
    match shape {
        CollisionShape::Sphere { center, radius } => json!({
            "kind": "sphere",
            "center": center,
            "radius": radius,
        }),
        CollisionShape::Capsule { start, end, radius } => json!({
            "kind": "capsule",
            "start": start,
            "end": end,
            "radius": radius,
        }),
        CollisionShape::Box { corner, edges } => json!({
            "kind": "box",
            "corner": corner,
            "edges": edges,
        }),
        CollisionShape::Cylinder { start, end, radius } => json!({
            "kind": "cylinder",
            "start": start,
            "end": end,
            "radius": radius,
        }),
    }
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
    options: SceneAssemblyOptions,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    if !workspace.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a directory: {}", workspace.display()),
        )
        .into());
    }

    let workspace = workspace.to_path_buf();
    let ymap_path = if ymap_arg.is_absolute() {
        ymap_arg.to_path_buf()
    } else {
        workspace.join(ymap_arg)
    };
    let ymap = Ymap::from_bytes(&fs::read(&ymap_path)?).map_err(validation_error)?;
    let index = WorkspaceIndex::scan(&workspace)?;
    let manifest = assemble_ymap_scene(&index, &ymap_path, &ymap, options);

    if json_output {
        print_success("scene", scene_manifest_json(&manifest))?;
    } else {
        println!("workspace: {}", workspace.display());
        println!("ymap: {}", ymap_path.display());
        println!("entities: {}", manifest.summary.total_entities);
        println!("nodes: {}", manifest.summary.emitted_nodes);
        println!("resolved: {}", manifest.summary.resolved_nodes);
        println!("unresolved: {}", manifest.summary.unresolved_nodes);
        println!("assets: {}", manifest.summary.asset_references);
        println!(
            "truncated: {}",
            if manifest.limits.truncated {
                "yes"
            } else {
                "no"
            }
        );
        println!("warnings: {}", manifest.warnings.len());
    }

    Ok(())
}

pub fn parse_scene_args(
    args: impl Iterator<Item = String>,
    usage: &str,
) -> Result<(PathBuf, PathBuf, SceneAssemblyOptions, bool), io::Error> {
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
    let mut max_nodes = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" if !json_output => json_output = true,
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

    Ok((
        workspace,
        ymap,
        max_nodes.map(SceneAssemblyOptions::new).unwrap_or_default(),
        json_output,
    ))
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

fn scene_manifest_json(manifest: &SceneManifest) -> Value {
    let nodes = manifest
        .nodes
        .iter()
        .map(|node| {
            let transform = node.transform.map(|transform| {
                json!({
                    "translation": transform.translation,
                    "rotation": transform.rotation,
                    "scale": transform.scale,
                })
            });
            let reason = node.reason.as_ref().map(|reason| {
                json!({
                    "code": scene_reason_code(reason.code),
                    "message": reason.message,
                })
            });
            let collision = node.collision.as_ref().map(|collision| {
                json!({
                    "hash": format!("0x{:08X}", collision.hash),
                    "assetRef": collision.asset_ref,
                    "state": scene_collision_state(collision.state),
                    "reason": collision.reason,
                })
            });

            json!({
                "index": node.index,
                "sourceYmap": node.source_ymap.display().to_string(),
                "entityIndex": node.entity_index,
                "archetypeHash": format!("0x{:08X}", node.archetype_hash),
                "providerPath": node
                    .provider_path
                    .as_ref()
                    .map(|path| path.display().to_string()),
                "assetRef": node.asset_ref,
                "assetKind": node.asset_kind.map(|kind| kind.to_string()),
                "transform": transform,
                "resolution": scene_resolution_state(node.resolution),
                "reason": reason,
                "collision": collision,
            })
        })
        .collect::<Vec<_>>();

    let assets = manifest
        .assets
        .iter()
        .map(|asset| {
            let selector = asset.selector.as_ref().map(|selector| match selector {
                SceneAssetSelector::YddDrawable {
                    index,
                    name_hash,
                    name,
                } => json!({
                    "type": "yddDrawable",
                    "index": index,
                    "nameHash": format!("0x{name_hash:08X}"),
                    "name": name,
                }),
            });

            json!({
                "id": asset.id,
                "kind": asset.kind.to_string(),
                "hash": format!("0x{:08X}", asset.hash),
                "path": asset.path.display().to_string(),
                "selector": selector,
            })
        })
        .collect::<Vec<_>>();

    json!({
        "schemaVersion": manifest.schema_version,
        "root": {
            "path": manifest.root.path.display().to_string(),
            "nameHash": manifest
                .root
                .name_hash
                .map(|hash| format!("0x{hash:08X}")),
        },
        "nodes": nodes,
        "assets": assets,
        "summary": {
            "totalEntities": manifest.summary.total_entities,
            "emittedNodes": manifest.summary.emitted_nodes,
            "resolvedNodes": manifest.summary.resolved_nodes,
            "unresolvedNodes": manifest.summary.unresolved_nodes,
            "assetReferences": manifest.summary.asset_references,
        },
        "warnings": manifest.warnings,
        "limits": {
            "maxNodes": manifest.limits.max_nodes,
            "truncated": manifest.limits.truncated,
            "omittedEntities": manifest.limits.omitted_entities,
        },
    })
}

fn scene_resolution_state(state: SceneResolutionState) -> &'static str {
    match state {
        SceneResolutionState::Resolved => "resolved",
        SceneResolutionState::Unresolved => "unresolved",
    }
}

fn scene_collision_state(state: SceneCollisionState) -> &'static str {
    match state {
        SceneCollisionState::LocalOnly => "localOnly",
        SceneCollisionState::Unresolved => "unresolved",
    }
}

fn scene_reason_code(code: SceneResolutionReasonCode) -> &'static str {
    match code {
        SceneResolutionReasonCode::ProviderMissing => "providerMissing",
        SceneResolutionReasonCode::ProviderAmbiguous => "providerAmbiguous",
        SceneResolutionReasonCode::AssetNameMissing => "assetNameMissing",
        SceneResolutionReasonCode::DrawableDictionaryMissing => "drawableDictionaryMissing",
        SceneResolutionReasonCode::AssetMissing => "assetMissing",
        SceneResolutionReasonCode::AssetAmbiguous => "assetAmbiguous",
        SceneResolutionReasonCode::DictionaryUnreadable => "dictionaryUnreadable",
        SceneResolutionReasonCode::DictionaryEntryMissing => "dictionaryEntryMissing",
        SceneResolutionReasonCode::UnsupportedAssetRelation => "unsupportedAssetRelation",
        SceneResolutionReasonCode::InvalidWorldTransform => "invalidWorldTransform",
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
        ("gta", "vanilla-index") => Some("vanilla-index"),
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

fn inspect_container(bytes: &[u8]) -> Option<Value> {
    let probe = Rsc7Probe::parse(bytes).ok()?;
    let resource = Rsc7Resource::parse(bytes).ok();

    Some(json!({
        "kind": "RSC7",
        "version": probe.header.version,
        "systemFlags": format!("0x{:08X}", probe.header.system_flags),
        "graphicsFlags": format!("0x{:08X}", probe.header.graphics_flags),
        "systemSize": probe.header.system_size(),
        "graphicsSize": probe.header.graphics_size(),
        "decompression": resource.is_some(),
    }))
}

fn inspect_format(path: &Path, bytes: &[u8], asset_type: &str) -> Result<Value, Box<dyn Error>> {
    match asset_type {
        "YMAP" => {
            let ymap = Ymap::from_bytes(bytes).map_err(validation_error)?;
            let unique_archetypes = ymap
                .entities
                .iter()
                .map(|entity| entity.archetype_name.0)
                .collect::<BTreeSet<_>>()
                .len();
            Ok(json!({
                "name": optional_hash(ymap.name.map(|value| value.0)),
                "parent": optional_hash(ymap.parent.map(|value| value.0)),
                "entities": ymap.entities.len(),
                "uniqueArchetypes": unique_archetypes,
                "physicsDictionaries": ymap.physics_dictionaries.len(),
            }))
        }
        "YTYP" => {
            let ytyp = Ytyp::from_bytes(bytes).map_err(validation_error)?;
            let mlos = ytyp
                .archetypes
                .iter()
                .filter(|archetype| archetype.kind == ArchetypeKind::Mlo)
                .count();
            Ok(json!({
                "name": optional_hash(ytyp.name.map(|value| value.0)),
                "archetypes": ytyp.archetypes.len(),
                "mloArchetypes": mlos,
                "dependencies": ytyp.dependencies.len(),
            }))
        }
        "YMF" => {
            let ymf = Ymf::from_bytes(bytes).map_err(validation_error)?;
            Ok(json!({
                "format": "PSO",
                "ymaps": ymf.maps.len(),
                "ytypsWithDependencies": ymf.ytyps.len(),
                "interiors": ymf.interiors.len(),
            }))
        }
        "YTD" => {
            let ytd = Ytd::from_bytes(bytes).map_err(validation_error)?;
            let formats = ytd
                .textures
                .iter()
                .map(|texture| texture.format.normalized_name().to_string())
                .collect::<BTreeSet<_>>();
            let textures = ytd
                .textures
                .iter()
                .enumerate()
                .map(|(index, texture)| {
                    json!({
                        "index": index,
                        "name": texture.name,
                        "dictionaryHash": format!("0x{:08X}", texture.dictionary_hash),
                        "nameHash": format!("0x{:08X}", texture.name_hash),
                        "dictionaryHashMatchesName": texture.dictionary_hash_matches_name(),
                        "width": texture.width,
                        "height": texture.height,
                        "depth": texture.depth,
                        "stride": texture.stride,
                        "format": texture.format.normalized_name(),
                        "formatRaw": format!("0x{:08X}", texture.format.raw()),
                        "mipLevels": texture.levels,
                        "usage": texture.usage,
                        "usageFlags": format!("0x{:08X}", texture.usage_flags),
                        "extraFlags": format!("0x{:08X}", texture.extra_flags),
                        "encodedBytes": texture.data_length,
                        "preview": {
                            "topMipRgba": texture.format.supports_rgba_preview(),
                            "classicDds": texture.depth == 1
                                && texture.format.supports_classic_dds(),
                        },
                        "writers": {
                            "rgbaRepack": texture.depth == 1
                                && texture.format.supports_rgba_repack(),
                        },
                    })
                })
                .collect::<Vec<_>>();
            Ok(json!({
                "resourceVersion": ytd.resource_version,
                "textureCount": ytd.textures.len(),
                "formats": formats,
                "textures": textures,
            }))
        }
        "YBN" => {
            let collision = YbnCollision::from_bytes(bytes).map_err(validation_error)?;
            Ok(json!({
                "coordinateConvention": collision.coordinate_convention,
                "children": collision.children.len(),
                "vertices": collision.vertex_count(),
                "indices": collision.index_count(),
                "triangles": collision.triangle_count(),
                "materials": collision.materials.len(),
                "primitives": collision.primitives.len(),
            }))
        }
        "YDR" => {
            let document = YdrDocument::from_bytes(bytes).map_err(validation_error)?;
            let model = &document.model;
            Ok(json!({
                "name": model.name,
                "lod": model.lod.as_str(),
                "coordinateConvention": model.coordinate_convention.as_str(),
                "primitives": model.primitives.len(),
                "vertices": model.vertex_count(),
                "indices": model.index_count(),
                "triangles": model.triangle_count(),
                "shaders": model.shaders.len(),
                "embeddedTextures": document
                    .embedded_textures
                    .as_ref()
                    .map_or(0, |dictionary| dictionary.textures().len()),
            }))
        }
        "YDD" => {
            let dictionary = YddDictionary::from_bytes(bytes).map_err(validation_error)?;
            Ok(json!({
                "drawables": dictionary.entries().len(),
            }))
        }
        _ => {
            let _ = path;
            Ok(Value::Null)
        }
    }
}

fn operations_for(asset_type: &str, bytes: &[u8]) -> Result<Vec<Value>, Box<dyn Error>> {
    let mut operations = vec![
        operation("inspect", false, true, false),
        operation("validate", false, true, false),
        operation("capabilities", false, true, false),
    ];

    match asset_type {
        "YDR" => {
            YdrDocument::from_bytes(bytes).map_err(validation_error)?;
            operations.push(operation("ydr.info", false, false, false));
            operations.push(operation("spatial", false, true, false));
            operations.push(operation_with_availability(
                "preview",
                false,
                true,
                false,
                "available",
                Some("renderer-neutral Legacy YDR model preview is available"),
                &[],
            ));

            match YdrEditSession::from_bytes(bytes) {
                Ok(session) => {
                    let translation = session.rigid_translation_capability();
                    let texture_available = !session.texture_bindings().is_empty();
                    let shader_available =
                        !session.shader_bindings().is_empty() && session.shader_count() > 0;
                    let declarative_available =
                        translation.writable || texture_available || shader_available;
                    let declarative_reason = if declarative_available {
                        Some("one or more declarative YDR writers are available")
                    } else {
                        translation.reason.as_deref().or(Some(
                            "no declarative translation or rebind writer is available for this YDR",
                        ))
                    };
                    let translation_reason = translation.reason.as_deref();
                    operations.push(operation_with_availability(
                        "plan",
                        false,
                        true,
                        false,
                        if declarative_available {
                            "available"
                        } else {
                            "unavailable"
                        },
                        declarative_reason,
                        &[],
                    ));
                    operations.push(operation_with_availability(
                        "apply",
                        true,
                        true,
                        false,
                        if declarative_available {
                            "available"
                        } else {
                            "unavailable"
                        },
                        declarative_reason,
                        &[],
                    ));
                    operations.push(operation_with_availability(
                        "ydr.translate",
                        true,
                        false,
                        false,
                        if translation.writable {
                            "available"
                        } else {
                            "unavailable"
                        },
                        translation_reason,
                        &["delta"],
                    ));

                    operations.push(operation_with_availability(
                        "ydr.rebind-texture",
                        true,
                        false,
                        false,
                        if texture_available {
                            "parameterized"
                        } else {
                            "unavailable"
                        },
                        if texture_available {
                            Some("eligibility depends on the selected source and target bindings")
                        } else {
                            Some("no editable texture bindings were found")
                        },
                        &[
                            "sourceShader",
                            "sourceParameter",
                            "targetShader",
                            "targetParameter",
                        ],
                    ));

                    operations.push(operation_with_availability(
                        "ydr.rebind-shader",
                        true,
                        false,
                        false,
                        if shader_available {
                            "parameterized"
                        } else {
                            "unavailable"
                        },
                        if shader_available {
                            Some("eligibility depends on the selected geometry and target shader")
                        } else {
                            Some("no editable shader bindings were found")
                        },
                        &["modelIndex", "geometryIndex", "targetShaderIndex"],
                    ));
                }
                Err(error) => {
                    let reason = format!("edit session unavailable: {error}");
                    for id in [
                        "plan",
                        "apply",
                        "ydr.translate",
                        "ydr.rebind-texture",
                        "ydr.rebind-shader",
                    ] {
                        operations.push(operation_with_availability(
                            id,
                            id != "plan",
                            matches!(id, "plan" | "apply"),
                            false,
                            "unavailable",
                            Some(&reason),
                            &[],
                        ));
                    }
                }
            }
        }
        "YDD" => {
            let dictionary = YddDictionary::from_bytes(bytes).map_err(validation_error)?;
            let mut translation_available = false;
            let mut texture_available = false;
            let mut shader_available = false;

            for entry in dictionary.entries() {
                if let Ok(session) = YddEditSession::from_bytes(bytes, entry.index) {
                    translation_available |= session.rigid_translation_capability().writable;
                    texture_available |= !session.texture_bindings().is_empty();
                    shader_available |=
                        !session.shader_bindings().is_empty() && session.shader_count() > 0;
                }
            }

            let declarative_available =
                translation_available || texture_available || shader_available;
            operations.extend([
                operation("ydd.info", false, false, false),
                operation("spatial", false, true, false),
                operation_with_availability(
                    "preview",
                    false,
                    true,
                    false,
                    if dictionary.entries().is_empty() {
                        "unavailable"
                    } else {
                        "parameterized"
                    },
                    if dictionary.entries().is_empty() {
                        Some("drawable dictionary contains no entries")
                    } else {
                        Some("requires an explicit drawableIndex")
                    },
                    &["drawableIndex"],
                ),
                operation_with_availability(
                    "plan",
                    false,
                    true,
                    false,
                    if declarative_available {
                        "available"
                    } else {
                        "unavailable"
                    },
                    if declarative_available {
                        Some("one or more declarative YDD writers are available")
                    } else {
                        Some("no writable drawable was found in this YDD")
                    },
                    &[],
                ),
                operation_with_availability(
                    "apply",
                    true,
                    true,
                    false,
                    if declarative_available {
                        "available"
                    } else {
                        "unavailable"
                    },
                    if declarative_available {
                        Some("one or more declarative YDD writers are available")
                    } else {
                        Some("no writable drawable was found in this YDD")
                    },
                    &[],
                ),
                operation_with_availability(
                    "ydd.translate",
                    true,
                    false,
                    false,
                    if translation_available {
                        "parameterized"
                    } else {
                        "unavailable"
                    },
                    if translation_available {
                        Some("eligibility is evaluated for the selected drawable")
                    } else {
                        Some("no drawable currently passes the rigid translation gate")
                    },
                    &["drawableIndex", "delta"],
                ),
                operation_with_availability(
                    "ydd.rebind-texture",
                    true,
                    false,
                    false,
                    if texture_available {
                        "parameterized"
                    } else {
                        "unavailable"
                    },
                    if texture_available {
                        Some("eligibility is evaluated for the selected drawable and bindings")
                    } else {
                        Some("no drawable exposes editable texture bindings")
                    },
                    &[
                        "drawableIndex",
                        "sourceShader",
                        "sourceParameter",
                        "targetShader",
                        "targetParameter",
                    ],
                ),
                operation_with_availability(
                    "ydd.rebind-shader",
                    true,
                    false,
                    false,
                    if shader_available {
                        "parameterized"
                    } else {
                        "unavailable"
                    },
                    if shader_available {
                        Some("eligibility is evaluated for the selected drawable and geometry")
                    } else {
                        Some("no drawable exposes editable shader bindings")
                    },
                    &[
                        "drawableIndex",
                        "modelIndex",
                        "geometryIndex",
                        "targetShaderIndex",
                    ],
                ),
            ]);
        }
        "YTD" => {
            let ytd = Ytd::from_bytes(bytes).map_err(validation_error)?;
            let dds_available = ytd
                .textures
                .iter()
                .any(|texture| texture.depth == 1 && texture.format.supports_classic_dds());
            let rgba_available = ytd
                .textures
                .iter()
                .any(|texture| texture.depth == 1 && texture.format.supports_rgba_repack());
            let compact_result = Ytd::rebuild_legacy_compact(bytes);
            let compact_available = compact_result.is_ok();
            let declarative_available = dds_available || rgba_available || compact_available;
            operations.extend([
                operation("ytd.info", false, false, false),
                operation("spatial", false, true, false),
                operation_with_availability(
                    "plan",
                    false,
                    true,
                    false,
                    if declarative_available {
                        "available"
                    } else {
                        "unavailable"
                    },
                    if declarative_available {
                        Some("one or more declarative Legacy YTD writers are available")
                    } else {
                        Some("no declarative writer is available for this YTD")
                    },
                    &[],
                ),
                operation_with_availability(
                    "apply",
                    true,
                    true,
                    false,
                    if declarative_available {
                        "available"
                    } else {
                        "unavailable"
                    },
                    if declarative_available {
                        Some("one or more declarative Legacy YTD writers are available")
                    } else {
                        Some("no declarative writer is available for this YTD")
                    },
                    &[],
                ),
                operation_with_availability(
                    "ytd.extract-dds",
                    true,
                    false,
                    false,
                    if dds_available {
                        "parameterized"
                    } else {
                        "unavailable"
                    },
                    if dds_available {
                        Some("requires a 2D RGBA8, BC1, or BC3 textureIndex")
                    } else {
                        Some("no texture supports classic DDS export")
                    },
                    &["textureIndex", "output"],
                ),
                operation_with_availability(
                    "ytd.replace-dds",
                    true,
                    false,
                    false,
                    if dds_available {
                        "parameterized"
                    } else {
                        "unavailable"
                    },
                    Some("eligibility depends on the selected Legacy texture and replacement DDS"),
                    &["textureIndex", "replacement"],
                ),
                operation_with_availability(
                    "ytd.repack-dds",
                    true,
                    false,
                    false,
                    if dds_available {
                        "parameterized"
                    } else {
                        "unavailable"
                    },
                    Some("eligibility depends on the selected Legacy texture and replacement DDS"),
                    &["textureIndex", "replacement"],
                ),
                operation_with_availability(
                    "ytd.repack-rgba",
                    true,
                    false,
                    false,
                    if rgba_available {
                        "parameterized"
                    } else {
                        "unavailable"
                    },
                    Some(
                        "requires a 2D RGBA8, BC1, or BC3 target plus dimensions and an external RGBA payload",
                    ),
                    &["textureIndex", "width", "height", "replacement"],
                ),
            ]);

            match compact_result {
                Ok(_) => operations.push(operation_with_availability(
                    "ytd.rebuild-compact",
                    true,
                    false,
                    false,
                    "available",
                    Some("Legacy v13 compact rebuild is supported for this YTD"),
                    &[],
                )),
                Err(error) => operations.push(operation_with_availability(
                    "ytd.rebuild-compact",
                    true,
                    false,
                    false,
                    "unavailable",
                    Some(&error.to_string()),
                    &[],
                )),
            }
        }
        "YBN" => {
            let collision = YbnCollision::from_bytes(bytes).map_err(validation_error)?;
            let editable = collision.shape_primitive_count() > 0;
            let reason = if editable {
                Some("eligibility depends on the selected polygon and requested fields")
            } else {
                Some("no editable sphere, capsule, box, or cylinder polygons were found")
            };
            operations.extend([
                operation_with_availability(
                    "plan",
                    false,
                    true,
                    false,
                    if editable { "available" } else { "unavailable" },
                    reason,
                    &[],
                ),
                operation_with_availability(
                    "apply",
                    true,
                    true,
                    false,
                    if editable { "available" } else { "unavailable" },
                    reason,
                    &[],
                ),
                operation("ybn.info", false, false, false),
                operation("spatial", false, true, false),
                operation_with_availability(
                    "preview",
                    false,
                    true,
                    false,
                    "available",
                    Some("renderer-neutral Legacy YBN collision preview is available"),
                    &[],
                ),
                operation_with_availability(
                    "ybn.edit-polygon",
                    true,
                    false,
                    false,
                    if editable {
                        "parameterized"
                    } else {
                        "unavailable"
                    },
                    reason,
                    &["childIndex", "polygonIndex", "kind"],
                ),
            ]);
        }
        "YMAP" => {
            Ymap::from_bytes(bytes).map_err(validation_error)?;
            operations.extend([
                operation("ymap.info", false, false, false),
                operation("spatial", false, true, false),
                context_operation("workspace.scene", false, true),
                context_operation("workspace.deps", false, false),
                context_operation("workspace.providers", false, false),
                context_operation("workspace.preflight", false, false),
                context_operation("workspace.extract", true, false),
            ]);
        }
        "YTYP" => {
            Ytyp::from_bytes(bytes).map_err(validation_error)?;
            operations.extend([
                operation("ytyp.info", false, false, false),
                operation("spatial", false, true, false),
            ]);
        }
        "YMF" => {
            Ymf::from_bytes(bytes).map_err(validation_error)?;
            operations.push(operation("ymf.info", false, false, false));
        }
        _ => {}
    }

    Ok(operations)
}

fn operation(
    id: &str,
    writes_asset: bool,
    structured_output: bool,
    requires_workspace: bool,
) -> Value {
    operation_with_availability(
        id,
        writes_asset,
        structured_output,
        requires_workspace,
        "available",
        None,
        &[],
    )
}

fn context_operation(id: &str, writes_asset: bool, structured_output: bool) -> Value {
    operation_with_availability(
        id,
        writes_asset,
        structured_output,
        true,
        "contextRequired",
        Some("requires a workspace root"),
        &["workspace"],
    )
}

fn operation_with_availability(
    id: &str,
    writes_asset: bool,
    structured_output: bool,
    requires_workspace: bool,
    availability: &str,
    reason: Option<&str>,
    requires_parameters: &[&str],
) -> Value {
    json!({
        "id": id,
        "writesAsset": writes_asset,
        "structuredOutput": structured_output,
        "requiresWorkspace": requires_workspace,
        "availability": availability,
        "reason": reason,
        "requiresParameters": requires_parameters,
    })
}

fn optional_hash(value: Option<u32>) -> Value {
    value
        .map(|value| Value::String(format!("0x{value:08X}")))
        .unwrap_or(Value::Null)
}

fn asset_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "ymap" => "YMAP",
        "ytyp" => "YTYP",
        "ymf" => "YMF",
        "ydr" => "YDR",
        "ydd" => "YDD",
        "ytd" => "YTD",
        "ybn" => "YBN",
        "yft" => "YFT",
        "ycd" => "YCD",
        "ymt" => "YMT",
        "yld" => "YLD",
        "ynv" => "YNV",
        _ => "UNKNOWN",
    }
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

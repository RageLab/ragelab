use std::{
    error::Error,
    fs, io,
    path::{Path, PathBuf},
};

use ragelab_assets::{AssetKind, WorkspaceIndex};
use ragelab_engine::{
    apply_operation_document, assemble_ymap_scene, asset_capabilities, asset_type_name,
    build_vanilla_catalog, discover_fivem_legacy, discover_gta_v_legacy, inspect_asset,
    isolated_asset_spatial_context, parse_operation_document, plan_operation_document,
    render_vanilla_catalog_paths, validate_asset, ymap_spatial_context, EngineError,
    OperationError, SceneAssemblyOptions, SceneAssetSelector, SceneCollisionState, SceneManifest,
    SceneResolutionReasonCode, SceneResolutionState, SpatialContext, SpatialProvenance,
};
use ragelab_ybn::{CollisionShape, YbnCollision};
use ragelab_ydd::YddDictionary;
use ragelab_ydr::{YdrDocument, YdrModel};
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
    let asset_type = asset_type_name(path);

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

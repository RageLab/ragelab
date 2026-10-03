use std::{collections::BTreeSet, fs, io, path::Path};

use ragelab_resource::{Rsc7Probe, Rsc7Resource};
use ragelab_ybn::YbnCollision;
use ragelab_ydd::{YddDictionary, YddEditSession};
use ragelab_ydr::{YdrDocument, YdrEditSession};
use ragelab_ymap::Ymap;
use ragelab_ymf::Ymf;
use ragelab_ytd::Ytd;
use ragelab_ytyp::{ArchetypeKind, Ytyp};
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetContainerInspection {
    pub kind: &'static str,
    pub version: u32,
    pub system_flags: String,
    pub graphics_flags: String,
    pub system_size: usize,
    pub graphics_size: usize,
    pub decompression: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetInspectionReport {
    pub path: String,
    #[serde(rename = "type")]
    pub asset_type: String,
    pub bytes: usize,
    pub container: Option<AssetContainerInspection>,
    pub details: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetValidationReport {
    pub path: String,
    #[serde(rename = "type")]
    pub asset_type: String,
    pub valid: bool,
    pub checks: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AssetOperationAvailability {
    Available,
    Parameterized,
    ContextRequired,
    Unavailable,
}

impl AssetOperationAvailability {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Parameterized => "parameterized",
            Self::ContextRequired => "contextRequired",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetOperationCapability {
    pub id: String,
    pub writes_asset: bool,
    pub structured_output: bool,
    pub requires_workspace: bool,
    pub availability: AssetOperationAvailability,
    pub reason: Option<String>,
    pub requires_parameters: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetCapabilitiesReport {
    pub path: String,
    #[serde(rename = "type")]
    pub asset_type: String,
    pub operations: Vec<AssetOperationCapability>,
}

pub fn asset_type_name(path: &Path) -> &'static str {
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

pub fn inspect_asset(path: &Path) -> Result<AssetInspectionReport, io::Error> {
    let bytes = fs::read(path)?;
    inspect_asset_bytes(path, &bytes)
}

pub fn inspect_asset_bytes(path: &Path, bytes: &[u8]) -> Result<AssetInspectionReport, io::Error> {
    let asset_type = asset_type_name(path);
    Ok(AssetInspectionReport {
        path: path.display().to_string(),
        asset_type: asset_type.to_string(),
        bytes: bytes.len(),
        container: inspect_container(bytes),
        details: inspect_format(bytes, asset_type)?,
    })
}

pub fn validate_asset(path: &Path) -> Result<AssetValidationReport, io::Error> {
    let bytes = fs::read(path)?;
    validate_asset_bytes(path, &bytes)
}

pub fn validate_asset_bytes(path: &Path, bytes: &[u8]) -> Result<AssetValidationReport, io::Error> {
    let asset_type = asset_type_name(path);
    let mut checks = Vec::new();

    if Rsc7Probe::parse(bytes).is_ok() {
        Rsc7Resource::parse(bytes).map_err(validation_error)?;
        checks.push("rsc7.container".to_string());
    }

    match asset_type {
        "YMAP" => {
            Ymap::from_bytes(bytes).map_err(validation_error)?;
            checks.push("ymap.parse".into());
        }
        "YTYP" => {
            Ytyp::from_bytes(bytes).map_err(validation_error)?;
            checks.push("ytyp.parse".into());
        }
        "YMF" => {
            Ymf::from_bytes(bytes).map_err(validation_error)?;
            checks.push("ymf.parse".into());
        }
        "YTD" => {
            Ytd::from_bytes(bytes).map_err(validation_error)?;
            checks.push("ytd.parse".into());
        }
        "YBN" => {
            YbnCollision::from_bytes(bytes).map_err(validation_error)?;
            checks.push("ybn.parse".into());
        }
        "YDR" => {
            YdrDocument::from_bytes(bytes).map_err(validation_error)?;
            checks.push("ydr.parse".into());
        }
        "YDD" => {
            YddDictionary::from_bytes(bytes).map_err(validation_error)?;
            checks.push("ydd.parse".into());
        }
        _ if checks.is_empty() => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "no validator is available for file type {} ({})",
                    asset_type,
                    path.display()
                ),
            ));
        }
        _ => {}
    }

    Ok(AssetValidationReport {
        path: path.display().to_string(),
        asset_type: asset_type.to_string(),
        valid: true,
        checks,
    })
}

pub fn asset_capabilities(path: &Path) -> Result<AssetCapabilitiesReport, io::Error> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a file: {}", path.display()),
        ));
    }

    let bytes = fs::read(path)?;
    asset_capabilities_bytes(path, &bytes)
}

pub fn asset_capabilities_bytes(
    path: &Path,
    bytes: &[u8],
) -> Result<AssetCapabilitiesReport, io::Error> {
    let asset_type = asset_type_name(path);
    Ok(AssetCapabilitiesReport {
        path: path.display().to_string(),
        asset_type: asset_type.to_string(),
        operations: operations_for(asset_type, bytes)?,
    })
}

fn inspect_container(bytes: &[u8]) -> Option<AssetContainerInspection> {
    let probe = Rsc7Probe::parse(bytes).ok()?;
    let resource = Rsc7Resource::parse(bytes).ok();

    Some(AssetContainerInspection {
        kind: "RSC7",
        version: probe.header.version,
        system_flags: format!("0x{:08X}", probe.header.system_flags),
        graphics_flags: format!("0x{:08X}", probe.header.graphics_flags),
        system_size: probe.header.system_size(),
        graphics_size: probe.header.graphics_size(),
        decompression: resource.is_some(),
    })
}

fn inspect_format(bytes: &[u8], asset_type: &str) -> Result<Value, io::Error> {
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
        _ => Ok(Value::Null),
    }
}

fn operations_for(
    asset_type: &str,
    bytes: &[u8],
) -> Result<Vec<AssetOperationCapability>, io::Error> {
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
                AssetOperationAvailability::Available,
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
                        availability(declarative_available),
                        declarative_reason,
                        &[],
                    ));
                    operations.push(operation_with_availability(
                        "apply",
                        true,
                        true,
                        false,
                        availability(declarative_available),
                        declarative_reason,
                        &[],
                    ));
                    operations.push(operation_with_availability(
                        "ydr.translate",
                        true,
                        false,
                        false,
                        availability(translation.writable),
                        translation_reason,
                        &["delta"],
                    ));
                    operations.push(operation_with_availability(
                        "ydr.rebind-texture",
                        true,
                        false,
                        false,
                        if texture_available {
                            AssetOperationAvailability::Parameterized
                        } else {
                            AssetOperationAvailability::Unavailable
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
                            AssetOperationAvailability::Parameterized
                        } else {
                            AssetOperationAvailability::Unavailable
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
                            AssetOperationAvailability::Unavailable,
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
                        AssetOperationAvailability::Unavailable
                    } else {
                        AssetOperationAvailability::Parameterized
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
                    availability(declarative_available),
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
                    availability(declarative_available),
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
                        AssetOperationAvailability::Parameterized
                    } else {
                        AssetOperationAvailability::Unavailable
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
                        AssetOperationAvailability::Parameterized
                    } else {
                        AssetOperationAvailability::Unavailable
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
                        AssetOperationAvailability::Parameterized
                    } else {
                        AssetOperationAvailability::Unavailable
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
                    availability(declarative_available),
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
                    availability(declarative_available),
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
                        AssetOperationAvailability::Parameterized
                    } else {
                        AssetOperationAvailability::Unavailable
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
                        AssetOperationAvailability::Parameterized
                    } else {
                        AssetOperationAvailability::Unavailable
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
                        AssetOperationAvailability::Parameterized
                    } else {
                        AssetOperationAvailability::Unavailable
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
                        AssetOperationAvailability::Parameterized
                    } else {
                        AssetOperationAvailability::Unavailable
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
                    AssetOperationAvailability::Available,
                    Some("Legacy v13 compact rebuild is supported for this YTD"),
                    &[],
                )),
                Err(error) => {
                    let reason = error.to_string();
                    operations.push(operation_with_availability(
                        "ytd.rebuild-compact",
                        true,
                        false,
                        false,
                        AssetOperationAvailability::Unavailable,
                        Some(&reason),
                        &[],
                    ));
                }
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
                    availability(editable),
                    reason,
                    &[],
                ),
                operation_with_availability(
                    "apply",
                    true,
                    true,
                    false,
                    availability(editable),
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
                    AssetOperationAvailability::Available,
                    Some("renderer-neutral Legacy YBN collision preview is available"),
                    &[],
                ),
                operation_with_availability(
                    "ybn.edit-polygon",
                    true,
                    false,
                    false,
                    if editable {
                        AssetOperationAvailability::Parameterized
                    } else {
                        AssetOperationAvailability::Unavailable
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

fn availability(value: bool) -> AssetOperationAvailability {
    if value {
        AssetOperationAvailability::Available
    } else {
        AssetOperationAvailability::Unavailable
    }
}

fn operation(
    id: &str,
    writes_asset: bool,
    structured_output: bool,
    requires_workspace: bool,
) -> AssetOperationCapability {
    operation_with_availability(
        id,
        writes_asset,
        structured_output,
        requires_workspace,
        AssetOperationAvailability::Available,
        None,
        &[],
    )
}

fn context_operation(
    id: &str,
    writes_asset: bool,
    structured_output: bool,
) -> AssetOperationCapability {
    operation_with_availability(
        id,
        writes_asset,
        structured_output,
        true,
        AssetOperationAvailability::ContextRequired,
        Some("requires a workspace root"),
        &["workspace"],
    )
}

fn operation_with_availability(
    id: &str,
    writes_asset: bool,
    structured_output: bool,
    requires_workspace: bool,
    availability: AssetOperationAvailability,
    reason: Option<&str>,
    requires_parameters: &[&str],
) -> AssetOperationCapability {
    AssetOperationCapability {
        id: id.to_string(),
        writes_asset,
        structured_output,
        requires_workspace,
        availability,
        reason: reason.map(str::to_string),
        requires_parameters: requires_parameters
            .iter()
            .map(|value| (*value).to_string())
            .collect(),
    }
}

fn optional_hash(value: Option<u32>) -> Value {
    value
        .map(|value| Value::String(format!("0x{value:08X}")))
        .unwrap_or(Value::Null)
}

fn validation_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(relative: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/synthetic")
            .join(relative)
    }

    #[test]
    fn inspection_and_validation_reports_match_legacy_ytd_contract() {
        let path = fixture("ytd/simple.ytd");
        let inspection = inspect_asset(&path).unwrap();
        assert_eq!(inspection.asset_type, "YTD");
        assert_eq!(inspection.container.as_ref().unwrap().kind, "RSC7");
        assert_eq!(inspection.details["textureCount"], 1);
        assert_eq!(
            inspection.details["textures"][0]["name"],
            "synthetic_diffuse"
        );

        let validation = validate_asset(&path).unwrap();
        assert!(validation.valid);
        assert_eq!(
            validation.checks,
            vec!["rsc7.container".to_string(), "ytd.parse".to_string()]
        );
    }

    #[test]
    fn capabilities_use_writer_evidence_for_editable_ydr() {
        let report = asset_capabilities(&fixture("ydr/editable.ydr")).unwrap();
        let translate = report
            .operations
            .iter()
            .find(|operation| operation.id == "ydr.translate")
            .unwrap();
        assert_eq!(
            translate.availability,
            AssetOperationAvailability::Available
        );

        let texture = report
            .operations
            .iter()
            .find(|operation| operation.id == "ydr.rebind-texture")
            .unwrap();
        assert_eq!(
            texture.availability,
            AssetOperationAvailability::Parameterized
        );
    }

    #[test]
    fn unsupported_validation_fails_closed() {
        let path = fixture("stream/readme.txt");
        if path.exists() {
            let error = validate_asset(&path).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        }
    }
}

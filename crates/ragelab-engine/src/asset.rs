use std::{collections::BTreeSet, fs, io, path::Path};

use ragelab_resource::{Rsc7Probe, Rsc7Resource};
use ragelab_ybn::YbnCollision;
use ragelab_ydd::{YddDictionary, YddEditSession};
use ragelab_ydr::{YdrDocument, YdrEditSession};
use ragelab_yft::YftDocument;
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
    /// `Some(true)` when an RSC7 container was present and decompressed successfully.
    /// `None` means this asset did not expose an RSC7 container.
    pub container_valid: Option<bool>,
    /// True only when the format-specific parser completed successfully.
    pub format_validated: bool,
    pub valid: bool,
    pub checks: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssetFormat {
    Ymap,
    Ytyp,
    Ymf,
    Ydr,
    Ydd,
    Ytd,
    Ybn,
    Yft,
    Ycd,
    Ymt,
    Yld,
    Ynv,
    Unknown,
}

impl AssetFormat {
    fn from_path(path: &Path) -> Self {
        match path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "ymap" => Self::Ymap,
            "ytyp" => Self::Ytyp,
            "ymf" => Self::Ymf,
            "ydr" => Self::Ydr,
            "ydd" => Self::Ydd,
            "ytd" => Self::Ytd,
            "ybn" => Self::Ybn,
            "yft" => Self::Yft,
            "ycd" => Self::Ycd,
            "ymt" => Self::Ymt,
            "yld" => Self::Yld,
            "ynv" => Self::Ynv,
            _ => Self::Unknown,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Ymap => "YMAP",
            Self::Ytyp => "YTYP",
            Self::Ymf => "YMF",
            Self::Ydr => "YDR",
            Self::Ydd => "YDD",
            Self::Ytd => "YTD",
            Self::Ybn => "YBN",
            Self::Yft => "YFT",
            Self::Ycd => "YCD",
            Self::Ymt => "YMT",
            Self::Yld => "YLD",
            Self::Ynv => "YNV",
            Self::Unknown => "UNKNOWN",
        }
    }

    const fn has_semantic_parser(self) -> bool {
        matches!(
            self,
            Self::Ymap
                | Self::Ytyp
                | Self::Ymf
                | Self::Ydr
                | Self::Ydd
                | Self::Ytd
                | Self::Ybn
                | Self::Yft
        )
    }
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
    AssetFormat::from_path(path).name()
}

pub fn inspect_asset(path: &Path) -> Result<AssetInspectionReport, io::Error> {
    let bytes = fs::read(path)?;
    inspect_asset_bytes(path, &bytes)
}

pub fn inspect_asset_bytes(path: &Path, bytes: &[u8]) -> Result<AssetInspectionReport, io::Error> {
    let format = AssetFormat::from_path(path);
    Ok(AssetInspectionReport {
        path: path.display().to_string(),
        asset_type: format.name().to_string(),
        bytes: bytes.len(),
        container: inspect_container(bytes),
        details: inspect_format(bytes, format)?,
    })
}

pub fn validate_asset(path: &Path) -> Result<AssetValidationReport, io::Error> {
    let bytes = fs::read(path)?;
    validate_asset_bytes(path, &bytes)
}

pub fn validate_asset_bytes(path: &Path, bytes: &[u8]) -> Result<AssetValidationReport, io::Error> {
    let format = AssetFormat::from_path(path);
    let mut checks = Vec::new();

    let container_valid = if Rsc7Probe::parse(bytes).is_ok() {
        Rsc7Resource::parse(bytes).map_err(validation_error)?;
        checks.push("rsc7.container".to_string());
        Some(true)
    } else {
        None
    };

    match validate_format(bytes, format)? {
        Some(check) => checks.push(check.to_string()),
        None => {
            let container_state = if container_valid == Some(true) {
                "the RSC7 container is valid, but"
            } else {
                "no validated RSC7 container is available and"
            };
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "{container_state} no semantic format validator is available for {} ({})",
                    format.name(),
                    path.display()
                ),
            ));
        }
    }

    Ok(AssetValidationReport {
        path: path.display().to_string(),
        asset_type: format.name().to_string(),
        valid: true,
        container_valid,
        format_validated: true,
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
    let format = AssetFormat::from_path(path);
    Ok(AssetCapabilitiesReport {
        path: path.display().to_string(),
        asset_type: format.name().to_string(),
        operations: operations_for(format, bytes)?,
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

fn validate_format(bytes: &[u8], format: AssetFormat) -> Result<Option<&'static str>, io::Error> {
    let check = match format {
        AssetFormat::Ymap => {
            Ymap::from_bytes(bytes).map_err(validation_error)?;
            "ymap.parse"
        }
        AssetFormat::Ytyp => {
            Ytyp::from_bytes(bytes).map_err(validation_error)?;
            "ytyp.parse"
        }
        AssetFormat::Ymf => {
            Ymf::from_bytes(bytes).map_err(validation_error)?;
            "ymf.parse"
        }
        AssetFormat::Ytd => {
            Ytd::from_bytes(bytes).map_err(validation_error)?;
            "ytd.parse"
        }
        AssetFormat::Ybn => {
            YbnCollision::from_bytes(bytes).map_err(validation_error)?;
            "ybn.parse"
        }
        AssetFormat::Ydr => {
            YdrDocument::from_bytes(bytes).map_err(validation_error)?;
            "ydr.parse"
        }
        AssetFormat::Ydd => {
            YddDictionary::from_bytes(bytes).map_err(validation_error)?;
            "ydd.parse"
        }
        AssetFormat::Yft => {
            YftDocument::from_bytes(bytes).map_err(validation_error)?;
            "yft.parse"
        }
        _ => return Ok(None),
    };
    Ok(Some(check))
}

fn inspect_format(bytes: &[u8], format: AssetFormat) -> Result<Value, io::Error> {
    match format {
        AssetFormat::Ymap => {
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
        AssetFormat::Ytyp => {
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
        AssetFormat::Ymf => {
            let ymf = Ymf::from_bytes(bytes).map_err(validation_error)?;
            Ok(json!({
                "format": "PSO",
                "ymaps": ymf.maps.len(),
                "ytypsWithDependencies": ymf.ytyps.len(),
                "interiors": ymf.interiors.len(),
            }))
        }
        AssetFormat::Ytd => {
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
                "authoring": crate::ytd_texture_authoring_report(bytes)?,
            }))
        }
        AssetFormat::Ybn => {
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
        AssetFormat::Ydr => {
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
                "materialAuthoring": crate::ydr_material_authoring_report(bytes)?,
            }))
        }
        AssetFormat::Ydd => {
            let dictionary = YddDictionary::from_bytes(bytes).map_err(validation_error)?;
            Ok(json!({
                "drawables": dictionary.entries().len(),
                "materialAuthoring": crate::ydd_material_authoring_report(bytes)?,
            }))
        }
        AssetFormat::Yft => {
            let document = YftDocument::from_bytes(bytes).map_err(validation_error)?;
            let drawable = document.main_drawable.as_ref();
            Ok(json!({
                "resourceVersion": ragelab_yft::YFT_LEGACY_VERSION,
                "scope": "legacyV162MainDrawable",
                "name": document.name,
                "hasMainDrawable": drawable.is_some(),
                "mainDrawable": drawable.map(|drawable| json!({
                    "name": drawable.model.name,
                    "lod": drawable.model.lod.as_str(),
                    "primitives": drawable.model.primitives.len(),
                    "vertices": drawable.model.vertex_count(),
                    "indices": drawable.model.index_count(),
                    "triangles": drawable.model.triangle_count(),
                    "shaders": drawable.model.shaders.len(),
                })),
                "unsupportedStructures": ["physicsChildren", "damagedVariants", "articulation"],
            }))
        }
        _ => Ok(Value::Null),
    }
}

fn operations_for(
    format: AssetFormat,
    bytes: &[u8],
) -> Result<Vec<AssetOperationCapability>, io::Error> {
    if !format.has_semantic_parser() {
        let container_available =
            Rsc7Probe::parse(bytes).is_ok() && Rsc7Resource::parse(bytes).is_ok();
        let inspect_reason = if container_available {
            "container-only inspection: RSC7 decompression is available, but no semantic format parser is registered"
        } else {
            "no semantic format parser or validated RSC7 container is available"
        };
        return Ok(vec![
            operation_with_availability(
                "inspect",
                false,
                true,
                false,
                availability(container_available),
                Some(inspect_reason),
                &[],
            ),
            operation_with_availability(
                "validate",
                false,
                true,
                false,
                AssetOperationAvailability::Unavailable,
                Some("semantic validation is unsupported until a format-specific parser exists"),
                &[],
            ),
            operation("capabilities", false, true, false),
        ]);
    }

    // The same format dispatch used by validation gates all advertised operations.
    validate_format(bytes, format)?.expect("supported formats have a semantic parser");
    let mut operations = vec![
        operation("inspect", false, true, false),
        operation("validate", false, true, false),
        operation("capabilities", false, true, false),
    ];

    match format {
        AssetFormat::Ydr => {
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
        AssetFormat::Ydd => {
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
        AssetFormat::Ytd => {
            let ytd = Ytd::from_bytes(bytes).map_err(validation_error)?;
            let dds_available = ytd
                .textures
                .iter()
                .any(|texture| texture.depth == 1 && texture.format.supports_classic_dds());
            let png_available = ytd
                .textures
                .iter()
                .any(|texture| texture.depth == 1 && texture.format.supports_rgba_preview());
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
                    "ytd.extract-png",
                    true,
                    false,
                    false,
                    if png_available {
                        AssetOperationAvailability::Parameterized
                    } else {
                        AssetOperationAvailability::Unavailable
                    },
                    if png_available {
                        Some("exports the decoded top mip as portable RGBA PNG without modifying the YTD")
                    } else {
                        Some("no 2D texture supports RGBA top-mip decode")
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
                    "ytd.repack-png",
                    true,
                    false,
                    false,
                    if rgba_available {
                        AssetOperationAvailability::Parameterized
                    } else {
                        AssetOperationAvailability::Unavailable
                    },
                    Some(
                        "PNG is decoded to RGBA8; target Legacy format is preserved and a complete mip chain is regenerated",
                    ),
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
        AssetFormat::Ybn => {
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
        AssetFormat::Ymap => {
            Ymap::from_bytes(bytes).map_err(validation_error)?;
            operations.extend([
                operation("ymap.info", false, false, false),
                operation("spatial", false, true, false),
                operation_with_availability(
                    "plan", false, true, false,
                    AssetOperationAvailability::Available,
                    Some("YMAP authoring plan is supported by the typed operation dispatcher"),
                    &[],
                ),
                operation_with_availability(
                    "apply", true, true, false,
                    AssetOperationAvailability::Available,
                    Some("YMAP authoring apply writes a separate output and performs semantic reopen"),
                    &[],
                ),
                operation_with_availability(
                    "ymap.set-transform", true, false, false,
                    AssetOperationAvailability::Parameterized,
                    Some("requires index, position and rotation; scaleXY and scaleZ are optional"),
                    &["index", "position", "rotation"],
                ),
                operation_with_availability(
                    "ymap.set-property", true, false, false,
                    AssetOperationAvailability::Parameterized,
                    Some("requires index and at least one of archetypeHash, flags or parentIndex"),
                    &["index"],
                ),
                operation_with_availability(
                    "ymap.delete", true, false, false,
                    AssetOperationAvailability::Parameterized,
                    Some("requires a non-empty indices array"),
                    &["indices"],
                ),
                operation_with_availability(
                    "ymap.duplicate", true, false, false,
                    AssetOperationAvailability::Parameterized,
                    Some("requires non-empty indices and a translation vector"),
                    &["indices", "translation"],
                ),
                operation_with_availability(
                    "ymap.create", true, false, false,
                    AssetOperationAvailability::Parameterized,
                    Some("requires templateIndex, archetypeHash, position, rotation and flags; scaleXY, scaleZ and parentIndex are optional"),
                    &["templateIndex", "archetypeHash", "position", "rotation", "flags"],
                ),
                context_operation("workspace.scene", false, true),
                context_operation("workspace.deps", false, false),
                context_operation("workspace.providers", false, false),
                context_operation("workspace.preflight", false, false),
                context_operation("workspace.extract", true, false),
            ]);
        }
        AssetFormat::Ytyp => {
            Ytyp::from_bytes(bytes).map_err(validation_error)?;
            operations.extend([
                operation("ytyp.info", false, false, false),
                operation("spatial", false, true, false),
            ]);
        }
        AssetFormat::Ymf => {
            Ymf::from_bytes(bytes).map_err(validation_error)?;
            operations.push(operation("ymf.info", false, false, false));
        }
        AssetFormat::Yft => {
            YftDocument::from_bytes(bytes).map_err(validation_error)?;
            operations.push(operation("yft.info", false, false, false));
        }
        AssetFormat::Ycd
        | AssetFormat::Ymt
        | AssetFormat::Yld
        | AssetFormat::Ynv
        | AssetFormat::Unknown => {
            unreachable!("container-only formats return before semantic dispatch")
        }
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
    fn validation_report_distinguishes_container_and_semantic_format() {
        let path = fixture("ytd/simple.ytd");
        let report = validate_asset(&path).unwrap();
        assert!(report.valid);
        assert_eq!(report.container_valid, Some(true));
        assert!(report.format_validated);
        assert_eq!(
            report.checks,
            vec!["rsc7.container".to_string(), "ytd.parse".to_string()]
        );
    }

    #[test]
    fn container_only_extensions_never_pass_semantic_validation() {
        let system = vec![0_u8; 0x60];
        let bytes = Rsc7Resource::from_segments(162, 0xA000_0000, 0, &system, &[])
            .unwrap()
            .to_bytes()
            .unwrap();

        for extension in ["ycd", "ymt", "yld", "ynv"] {
            let path = PathBuf::from(format!("synthetic.{extension}"));
            let error = validate_asset_bytes(&path, &bytes).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::Unsupported, "{extension}");
            assert!(
                error.to_string().contains("RSC7 container is valid"),
                "{extension}: {error}"
            );

            let capabilities = asset_capabilities_bytes(&path, &bytes).unwrap();
            let find = |id: &str| {
                capabilities
                    .operations
                    .iter()
                    .find(|operation| operation.id == id)
                    .unwrap()
            };
            assert_eq!(
                find("inspect").availability,
                AssetOperationAvailability::Available
            );
            assert!(find("inspect")
                .reason
                .as_deref()
                .unwrap()
                .contains("container-only"));
            assert_eq!(
                find("validate").availability,
                AssetOperationAvailability::Unavailable
            );
            assert_eq!(
                find("capabilities").availability,
                AssetOperationAvailability::Available
            );
        }
    }

    #[test]
    fn yft_validation_and_inspection_are_limited_to_legacy_v162_main_drawable() {
        let path = fixture("yft/no-main-drawable.yft");
        let validation = validate_asset(&path).unwrap();
        assert!(validation.valid);
        assert_eq!(validation.container_valid, Some(true));
        assert!(validation.format_validated);
        assert!(validation.checks.iter().any(|check| check == "yft.parse"));

        let inspection = inspect_asset(&path).unwrap();
        assert_eq!(inspection.details["resourceVersion"], 162);
        assert_eq!(inspection.details["scope"], "legacyV162MainDrawable");
        assert_eq!(inspection.details["hasMainDrawable"], false);
        assert!(inspection.details["mainDrawable"].is_null());
        assert!(inspection.details["unsupportedStructures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value == "physicsChildren"));

        let capabilities = asset_capabilities(&path).unwrap();
        assert!(capabilities.operations.iter().any(|operation| {
            operation.id == "yft.info"
                && operation.availability == AssetOperationAvailability::Available
        }));
        assert!(!capabilities
            .operations
            .iter()
            .any(|operation| operation.writes_asset));
    }

    #[test]
    fn ymap_capabilities_match_the_authoring_operation_dispatch() {
        let report = asset_capabilities(&fixture("simple.ymap")).unwrap();
        let find = |id: &str| {
            report
                .operations
                .iter()
                .find(|operation| operation.id == id)
                .unwrap()
        };

        assert_eq!(
            find("plan").availability,
            AssetOperationAvailability::Available
        );
        assert!(!find("plan").writes_asset);
        assert_eq!(
            find("apply").availability,
            AssetOperationAvailability::Available
        );
        assert!(find("apply").writes_asset);

        for id in [
            "ymap.set-transform",
            "ymap.set-property",
            "ymap.delete",
            "ymap.duplicate",
            "ymap.create",
        ] {
            assert_eq!(
                find(id).availability,
                AssetOperationAvailability::Parameterized,
                "{id}"
            );
            assert!(find(id).writes_asset, "{id}");
        }
        assert_eq!(
            find("ymap.set-transform").requires_parameters,
            ["index", "position", "rotation"]
        );
        assert_eq!(find("ymap.delete").requires_parameters, ["indices"]);
        assert_eq!(
            find("ymap.duplicate").requires_parameters,
            ["indices", "translation"]
        );
        assert_eq!(
            find("ymap.create").requires_parameters,
            [
                "templateIndex",
                "archetypeHash",
                "position",
                "rotation",
                "flags"
            ]
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

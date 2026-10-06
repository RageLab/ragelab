use std::{collections::BTreeMap, fs, io, path::Path};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use ragelab_hash::joaat;
use ragelab_ybn::{CollisionShape, YbnCollision};
use ragelab_ydd::YddDictionary;
use ragelab_ydr::{YdrDocument, YdrModel};
use ragelab_yft::YftDocument;
use ragelab_ytd::{DecodedTexture, YtdDictionary};
use serde::Serialize;
use serde_json::{json, Value};

use crate::asset_type_name;

pub const DEFAULT_PREVIEW_MAX_PRIMITIVES: usize = 64;
pub const DEFAULT_PREVIEW_MAX_VERTICES: usize = 10_000;
pub const DEFAULT_PREVIEW_MAX_INDICES: usize = 30_000;
pub const DEFAULT_PREVIEW_MAX_SHADERS: usize = 128;
pub const DEFAULT_PREVIEW_MAX_TEXTURE_REFERENCES: usize = 512;
pub const DEFAULT_PREVIEW_MAX_CHILDREN: usize = 256;
pub const DEFAULT_PREVIEW_MAX_MATERIALS: usize = 512;

pub const HARD_PREVIEW_MAX_PRIMITIVES: usize = 512;
pub const HARD_PREVIEW_MAX_VERTICES: usize = 100_000;
pub const HARD_PREVIEW_MAX_INDICES: usize = 300_000;
pub const HARD_PREVIEW_MAX_SHADERS: usize = 1_024;
pub const HARD_PREVIEW_MAX_TEXTURE_REFERENCES: usize = 4_096;
pub const HARD_PREVIEW_MAX_CHILDREN: usize = 4_096;
pub const HARD_PREVIEW_MAX_MATERIALS: usize = 8_192;

pub(crate) const MODEL_DIFFUSE_TEXTURE_LIMIT: usize = 32;
pub(crate) const MODEL_DIFFUSE_TEXTURE_MAX_DIMENSION: u16 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
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

impl PreviewOptions {
    pub fn validate(self) -> Result<Self, io::Error> {
        validate_limit(
            self.max_primitives,
            "--max-primitives",
            HARD_PREVIEW_MAX_PRIMITIVES,
        )?;
        validate_limit(
            self.max_vertices,
            "--max-vertices",
            HARD_PREVIEW_MAX_VERTICES,
        )?;
        validate_limit(self.max_indices, "--max-indices", HARD_PREVIEW_MAX_INDICES)?;
        validate_limit(self.max_shaders, "--max-shaders", HARD_PREVIEW_MAX_SHADERS)?;
        validate_limit(
            self.max_texture_references,
            "--max-texture-references",
            HARD_PREVIEW_MAX_TEXTURE_REFERENCES,
        )?;
        validate_limit(
            self.max_children,
            "--max-children",
            HARD_PREVIEW_MAX_CHILDREN,
        )?;
        validate_limit(
            self.max_materials,
            "--max-materials",
            HARD_PREVIEW_MAX_MATERIALS,
        )?;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewSpatial {
    pub classification: &'static str,
    pub coordinate_convention: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetPreviewReport {
    pub path: String,
    #[serde(rename = "type")]
    pub asset_type: String,
    pub spatial: PreviewSpatial,
    pub preview: Value,
}

pub fn preview_asset(
    path: &Path,
    options: PreviewOptions,
) -> Result<AssetPreviewReport, io::Error> {
    let bytes = fs::read(path)?;
    preview_asset_bytes(path, &bytes, options)
}

pub fn preview_asset_bytes(
    path: &Path,
    bytes: &[u8],
    options: PreviewOptions,
) -> Result<AssetPreviewReport, io::Error> {
    let asset_type = asset_type_name(path);
    preview_asset_bytes_as(&path.display().to_string(), asset_type, bytes, options)
}

pub fn preview_asset_bytes_as(
    path_label: &str,
    asset_type: &str,
    bytes: &[u8],
    options: PreviewOptions,
) -> Result<AssetPreviewReport, io::Error> {
    preview_asset_bytes_as_with_texture_dictionary(path_label, asset_type, bytes, options, None)
}

pub struct PreviewTextureDictionarySource<'a> {
    pub path: &'a str,
    pub bytes: &'a [u8],
    pub source: &'a str,
}

pub(crate) struct ModelPreviewSource {
    pub(crate) model: YdrModel,
    pub(crate) embedded_textures: Option<YtdDictionary>,
    pub(crate) selector: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedDiffuseTexture {
    pub(crate) name: String,
    pub(crate) name_hash: u32,
    pub(crate) source: String,
    pub(crate) source_path: String,
    pub(crate) original_width: u16,
    pub(crate) original_height: u16,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) downscaled: bool,
    pub(crate) rgba: Vec<u8>,
}

pub(crate) struct DiffuseTextureResolution {
    pub(crate) requested: usize,
    pub(crate) resolved: Vec<ResolvedDiffuseTexture>,
    pub(crate) unresolved: Vec<Value>,
    pub(crate) external_dictionary_errors: Vec<Value>,
    pub(crate) truncated: bool,
}

pub fn preview_asset_bytes_as_with_texture_dictionary(
    path_label: &str,
    asset_type: &str,
    bytes: &[u8],
    options: PreviewOptions,
    external_texture_dictionary: Option<(&str, &[u8])>,
) -> Result<AssetPreviewReport, io::Error> {
    let external =
        external_texture_dictionary.map(|(path, bytes)| PreviewTextureDictionarySource {
            path,
            bytes,
            source: "archetypeYtd",
        });
    let external = external.as_ref().into_iter().collect::<Vec<_>>();
    let sources = external
        .iter()
        .map(|source| PreviewTextureDictionarySource {
            path: source.path,
            bytes: source.bytes,
            source: source.source,
        })
        .collect::<Vec<_>>();
    preview_asset_bytes_as_with_texture_dictionaries(
        path_label, asset_type, bytes, options, &sources,
    )
}

pub fn preview_asset_bytes_as_with_texture_dictionaries(
    path_label: &str,
    asset_type: &str,
    bytes: &[u8],
    options: PreviewOptions,
    external_texture_dictionaries: &[PreviewTextureDictionarySource<'_>],
) -> Result<AssetPreviewReport, io::Error> {
    let options = options.validate()?;

    if asset_type == "YBN" {
        if options.drawable_index.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "--drawable-index is only valid for YDD preview",
            ));
        }

        let collision = YbnCollision::from_bytes(bytes).map_err(validation_error)?;
        return Ok(AssetPreviewReport {
            path: path_label.to_string(),
            asset_type: asset_type.to_string(),
            spatial: PreviewSpatial {
                classification: "localOnly",
                coordinate_convention: collision.coordinate_convention.to_string(),
            },
            preview: collision_preview_json(&collision, options),
        });
    }

    let source = parse_model_preview_source(asset_type, bytes, options)?;
    let coordinate_convention = source.model.coordinate_convention.as_str().to_string();
    let mut preview = model_preview_json(&source.model, options, source.selector);
    append_diffuse_texture_preview(
        &mut preview,
        &source.model,
        source.embedded_textures.as_ref(),
        path_label,
        external_texture_dictionaries,
        options,
    );

    Ok(AssetPreviewReport {
        path: path_label.to_string(),
        asset_type: asset_type.to_string(),
        spatial: PreviewSpatial {
            classification: "localOnly",
            coordinate_convention,
        },
        preview,
    })
}

pub(crate) fn parse_model_preview_source(
    asset_type: &str,
    bytes: &[u8],
    options: PreviewOptions,
) -> Result<ModelPreviewSource, io::Error> {
    match asset_type {
        "YDR" => {
            if options.drawable_index.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "--drawable-index is only valid for YDD preview",
                ));
            }
            let document = YdrDocument::from_bytes(bytes).map_err(validation_error)?;
            Ok(ModelPreviewSource {
                model: document.model,
                embedded_textures: document.embedded_textures,
                selector: Value::Null,
            })
        }
        "YFT" => {
            if options.drawable_index.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "--drawable-index is only valid for YDD preview",
                ));
            }
            let fragment = YftDocument::from_bytes(bytes).map_err(validation_error)?;
            let drawable = fragment.main_drawable.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    "YFT fragment does not contain a pristine main drawable",
                )
            })?;
            Ok(ModelPreviewSource {
                model: drawable.model,
                embedded_textures: drawable.embedded_textures,
                selector: json!({
                    "fragmentRole": "mainDrawable",
                    "fragmentName": fragment.name,
                }),
            })
        }
        "YDD" => {
            let drawable_index = options.drawable_index.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "YDD preview requires --drawable-index <n>",
                )
            })?;
            let dictionary = YddDictionary::from_bytes(bytes).map_err(validation_error)?;
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
            Ok(ModelPreviewSource {
                model: document.model,
                embedded_textures: document.embedded_textures,
                selector: json!({
                    "drawableIndex": entry.index,
                    "nameHash": format!("0x{:08X}", entry.name_hash),
                    "name": entry.name,
                }),
            })
        }
        _ => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("headless model preview is unavailable for {asset_type}"),
        )),
    }
}

fn append_diffuse_texture_preview(
    preview: &mut Value,
    model: &YdrModel,
    embedded_textures: Option<&YtdDictionary>,
    embedded_source_path: &str,
    external_texture_dictionaries: &[PreviewTextureDictionarySource<'_>],
    options: PreviewOptions,
) {
    let Some(object) = preview.as_object_mut() else {
        return;
    };

    let resolution = resolve_diffuse_textures(
        model,
        embedded_textures,
        embedded_source_path,
        external_texture_dictionaries,
        options,
    );
    let resolved = resolution
        .resolved
        .iter()
        .map(resolved_diffuse_texture_json)
        .collect::<Vec<_>>();

    object.insert("diffuseTextures".into(), Value::Array(resolved));
    object.insert(
        "diffuseTextureResolution".into(),
        json!({
            "requested": resolution.requested,
            "resolved": resolution.resolved.len(),
            "unresolved": resolution.unresolved,
            "externalDictionaryErrors": resolution.external_dictionary_errors,
            "truncated": resolution.truncated,
            "limits": {
                "maxTextures": MODEL_DIFFUSE_TEXTURE_LIMIT,
                "maxDimension": MODEL_DIFFUSE_TEXTURE_MAX_DIMENSION,
            }
        }),
    );
}

pub(crate) fn resolve_diffuse_textures(
    model: &YdrModel,
    embedded_textures: Option<&YtdDictionary>,
    embedded_source_path: &str,
    external_texture_dictionaries: &[PreviewTextureDictionarySource<'_>],
    options: PreviewOptions,
) -> DiffuseTextureResolution {
    let mut requested = BTreeMap::<u32, String>::new();
    for shader in model.shaders.iter().take(options.max_shaders) {
        if let Some(name) = shader.diffuse_texture_name() {
            requested
                .entry(joaat(name))
                .or_insert_with(|| name.to_string());
        }
    }

    let requested_count = requested.len();
    let truncated = requested_count > MODEL_DIFFUSE_TEXTURE_LIMIT;
    let external = external_texture_dictionaries
        .iter()
        .map(|source| (source, YtdDictionary::from_bytes(source.bytes)))
        .collect::<Vec<_>>();
    let external_dictionary_errors = external
        .iter()
        .filter_map(|(source, dictionary)| {
            dictionary.as_ref().err().map(|error| {
                json!({
                    "source": source.source,
                    "sourcePath": source.path,
                    "message": error.to_string(),
                })
            })
        })
        .collect::<Vec<_>>();

    let mut resolved = Vec::new();
    let mut unresolved = Vec::new();

    for (name_hash, name) in requested.into_iter().take(MODEL_DIFFUSE_TEXTURE_LIMIT) {
        let embedded_index = embedded_textures.and_then(|dictionary| {
            dictionary
                .texture_by_name(&name)
                .or_else(|| dictionary.texture_by_hash(name_hash))
                .map(|texture| texture.index)
        });

        if let (Some(dictionary), Some(index)) = (embedded_textures, embedded_index) {
            match dictionary.decode_top_mip(index) {
                Ok(texture) => resolved.push(resolved_diffuse_texture(
                    &name,
                    name_hash,
                    texture,
                    "embedded",
                    embedded_source_path,
                )),
                Err(error) => unresolved.push(json!({
                    "name": name,
                    "nameHash": format!("0x{name_hash:08X}"),
                    "reason": "embeddedTextureDecodeFailed",
                    "sourcePath": embedded_source_path,
                    "message": error.to_string(),
                })),
            }
            continue;
        }

        let mut matched = false;
        let mut searched_sources = Vec::new();
        for (source, dictionary) in &external {
            searched_sources.push(source.path);
            let Ok(dictionary) = dictionary else {
                continue;
            };
            let Some(index) = dictionary
                .texture_by_name(&name)
                .or_else(|| dictionary.texture_by_hash(name_hash))
                .map(|texture| texture.index)
            else {
                continue;
            };

            matched = true;
            match dictionary.decode_top_mip(index) {
                Ok(texture) => resolved.push(resolved_diffuse_texture(
                    &name,
                    name_hash,
                    texture,
                    source.source,
                    source.path,
                )),
                Err(error) => unresolved.push(json!({
                    "name": name,
                    "nameHash": format!("0x{name_hash:08X}"),
                    "reason": "externalTextureDecodeFailed",
                    "source": source.source,
                    "sourcePath": source.path,
                    "message": error.to_string(),
                })),
            }
            break;
        }

        if !matched {
            unresolved.push(json!({
                "name": name,
                "nameHash": format!("0x{name_hash:08X}"),
                "reason": "textureNotFound",
                "searchedSources": searched_sources,
            }));
        }
    }

    DiffuseTextureResolution {
        requested: requested_count,
        resolved,
        unresolved,
        external_dictionary_errors,
        truncated,
    }
}

fn resolved_diffuse_texture(
    name: &str,
    name_hash: u32,
    texture: DecodedTexture,
    source: &str,
    source_path: &str,
) -> ResolvedDiffuseTexture {
    let original_width = texture.width;
    let original_height = texture.height;
    let bounded = bound_decoded_texture(texture, MODEL_DIFFUSE_TEXTURE_MAX_DIMENSION);

    ResolvedDiffuseTexture {
        name: name.to_string(),
        name_hash,
        source: source.to_string(),
        source_path: source_path.to_string(),
        original_width,
        original_height,
        width: bounded.width,
        height: bounded.height,
        downscaled: bounded.width != original_width || bounded.height != original_height,
        rgba: bounded.rgba,
    }
}

fn resolved_diffuse_texture_json(texture: &ResolvedDiffuseTexture) -> Value {
    json!({
        "name": texture.name,
        "nameHash": format!("0x{:08X}", texture.name_hash),
        "source": texture.source,
        "sourcePath": texture.source_path,
        "originalWidth": texture.original_width,
        "originalHeight": texture.original_height,
        "width": texture.width,
        "height": texture.height,
        "downscaled": texture.downscaled,
        "rgbaEncoding": "base64-rgba8",
        "rgbaBase64": BASE64_STANDARD.encode(&texture.rgba),
    })
}

#[cfg(test)]
fn diffuse_texture_json(
    name: &str,
    name_hash: u32,
    texture: DecodedTexture,
    source: &str,
    source_path: &str,
) -> Value {
    resolved_diffuse_texture_json(&resolved_diffuse_texture(
        name,
        name_hash,
        texture,
        source,
        source_path,
    ))
}

pub(crate) fn bound_decoded_texture(texture: DecodedTexture, max_dimension: u16) -> DecodedTexture {
    let width = u32::from(texture.width);
    let height = u32::from(texture.height);
    let max_dimension = u32::from(max_dimension);

    if width == 0
        || height == 0
        || (width <= max_dimension && height <= max_dimension)
        || texture.rgba.len() != (width as usize) * (height as usize) * 4
    {
        return texture;
    }

    let (target_width, target_height) = if width >= height {
        (max_dimension, ((height * max_dimension) / width).max(1))
    } else {
        (((width * max_dimension) / height).max(1), max_dimension)
    };

    let mut rgba = vec![0_u8; (target_width * target_height * 4) as usize];
    for target_y in 0..target_height {
        let source_y = (target_y * height / target_height).min(height - 1);
        for target_x in 0..target_width {
            let source_x = (target_x * width / target_width).min(width - 1);
            let source_offset = ((source_y * width + source_x) * 4) as usize;
            let target_offset = ((target_y * target_width + target_x) * 4) as usize;
            rgba[target_offset..target_offset + 4]
                .copy_from_slice(&texture.rgba[source_offset..source_offset + 4]);
        }
    }

    DecodedTexture {
        width: target_width as u16,
        height: target_height as u16,
        rgba,
    }
}

fn validate_limit(value: usize, label: &str, hard_max: usize) -> Result<(), io::Error> {
    if value == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{label} must be greater than zero"),
        ));
    }
    if value > hard_max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{label} exceeds hard limit {hard_max}"),
        ));
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
                "diffuseTextureName": shader.diffuse_texture_name(),
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
    fn diffuse_texture_preview_downscales_rgba_deterministically() {
        let texture = DecodedTexture {
            width: 4,
            height: 2,
            rgba: vec![
                1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
                24, 25, 26, 27, 28, 29, 30, 31, 32,
            ],
        };
        let bounded = bound_decoded_texture(texture, 2);
        assert_eq!(bounded.width, 2);
        assert_eq!(bounded.height, 1);
        assert_eq!(bounded.rgba, vec![1, 2, 3, 4, 9, 10, 11, 12]);
    }

    #[test]
    fn diffuse_texture_json_is_bounded_base64_rgba_with_provenance() {
        let value = diffuse_texture_json(
            "test_diffuse",
            0x1234_5678,
            DecodedTexture {
                width: 1,
                height: 1,
                rgba: vec![255, 0, 128, 64],
            },
            "archetypeYtd",
            "rpf://fixture!/textures.ytd",
        );
        assert_eq!(value["name"], "test_diffuse");
        assert_eq!(value["nameHash"], "0x12345678");
        assert_eq!(value["source"], "archetypeYtd");
        assert_eq!(value["sourcePath"], "rpf://fixture!/textures.ytd");
        assert_eq!(value["width"], 1);
        assert_eq!(value["height"], 1);
        assert_eq!(value["rgbaEncoding"], "base64-rgba8");
        assert_eq!(value["rgbaBase64"], "/wCAQA==");
    }

    #[test]
    fn preview_limits_are_enforced_in_the_engine() {
        let error = PreviewOptions {
            max_vertices: HARD_PREVIEW_MAX_VERTICES + 1,
            ..PreviewOptions::default()
        }
        .validate()
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("hard limit 100000"));
    }

    #[test]
    fn ydr_preview_is_renderer_neutral_and_bounded() {
        let path = fixture("ydr/editable.ydr");
        let report = preview_asset(&path, PreviewOptions::default()).unwrap();

        assert_eq!(report.asset_type, "YDR");
        assert_eq!(report.spatial.classification, "localOnly");
        assert_eq!(
            report.preview["coordinateConvention"],
            Value::String("sourceXyzZUp".into())
        );
        assert_eq!(report.preview["counts"]["primitives"], 1);
        assert_eq!(report.preview["counts"]["vertices"], 3);
        assert_eq!(
            report.preview["primitives"][0]["geometry"]["indices"],
            json!([0, 1, 2])
        );

        let bounded = preview_asset(
            &path,
            PreviewOptions {
                max_vertices: 2,
                max_indices: 3,
                ..PreviewOptions::default()
            },
        )
        .unwrap();
        assert_eq!(bounded.preview["primitives"][0]["geometryIncluded"], false);
        assert_eq!(bounded.preview["emitted"]["vertices"], 0);
    }

    #[test]
    fn ydd_preview_requires_explicit_selector() {
        let path = fixture("ydd/editable.ydd");
        let error = preview_asset(&path, PreviewOptions::default()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);

        let report = preview_asset(
            &path,
            PreviewOptions {
                drawable_index: Some(0),
                ..PreviewOptions::default()
            },
        )
        .unwrap();
        assert_eq!(report.asset_type, "YDD");
        assert_eq!(report.preview["selector"]["drawableIndex"], 0);
        assert_eq!(
            report.preview["selector"]["nameHash"],
            Value::String("0x12345678".into())
        );
    }

    #[test]
    fn ybn_preview_is_local_only_and_bounded() {
        let path = fixture("ybn/preview.ybn");
        let report = preview_asset(&path, PreviewOptions::default()).unwrap();

        assert_eq!(report.asset_type, "YBN");
        assert_eq!(report.spatial.classification, "localOnly");
        assert_eq!(report.preview["kind"], "collision");
        assert!(
            report.preview["counts"]["shapePrimitives"]
                .as_u64()
                .unwrap()
                > 0
        );
    }
}

//! Geometry-focused GTA V PC YDR reader.
//!
//! This crate intentionally exposes a renderer-neutral model rather than the
//! raw RAGE drawable object graph. The current scope is static mesh preview:
//! Drawable -> selected LOD -> models -> geometries -> vertex/index buffers.

use std::{error::Error, fmt, sync::Arc};

use ragelab_resource::{ResourceError, Rsc7Resource, SYSTEM_BASE};
use ragelab_ytd::{YtdDictionary, YtdError};

const SUPPORTED_VERSION: u32 = 165;
const YDR_ROOT_SIZE: usize = 0xD0;
const YDR_NAME_OFFSET: u64 = 0xA8;
const MAX_MODELS: usize = 4_096;
const MAX_SHADERS: usize = 4_096;
const MAX_SHADER_PARAMETERS: usize = 255;
const MAX_GEOMETRIES_PER_MODEL: usize = 16_384;
const MAX_VERTICES_PER_GEOMETRY: usize = 2_000_000;
const MAX_INDICES_PER_GEOMETRY: usize = 6_000_000;
const MAX_VERTEX_BYTES: usize = 256 * 1024 * 1024;
const MAX_NAME_LENGTH: usize = 1_024;

/// rage_joaat("diffusesampler"): the primary GTA V albedo texture parameter.
pub const DIFFUSE_SAMPLER_PARAMETER_HASH: u32 = 0xF1FE_2B71;
/// rage_joaat("texturesampler"): legacy albedo fallback when DiffuseSampler is absent.
pub const TEXTURE_SAMPLER_PARAMETER_HASH: u32 = 0x2B51_70FD;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrawableReadLayout {
    pub resource_version: u32,
    pub root_size: usize,
    pub name_offset: u64,
}

impl DrawableReadLayout {
    pub const YDR: Self = Self {
        resource_version: SUPPORTED_VERSION,
        root_size: YDR_ROOT_SIZE,
        name_offset: YDR_NAME_OFFSET,
    };

    pub const YFT_FRAGMENT: Self = Self {
        resource_version: 162,
        root_size: 0x150,
        name_offset: 0x130,
    };
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawableBounds {
    pub center: [f32; 3],
    pub radius: f32,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LodLevel {
    High,
    Medium,
    Low,
    VeryLow,
}

impl LodLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::VeryLow => "veryLow",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinateConvention {
    SourceXyzZUp,
}

impl CoordinateConvention {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SourceXyzZUp => "sourceXyzZUp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveTopology {
    TriangleList,
}

impl PrimitiveTopology {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TriangleList => "triangleList",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WindingSummary {
    pub aligned: usize,
    pub opposed: usize,
    pub degenerate: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexDeclarationSummary {
    pub flags: u32,
    pub stride: u16,
    pub component_count: u8,
    pub types: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShaderTextureReference {
    pub parameter_hash: u32,
    pub texture_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShaderInfo {
    pub name_hash: u32,
    pub file_hash: u32,
    pub texture_references: Vec<ShaderTextureReference>,
}

impl ShaderInfo {
    pub fn diffuse_texture_name(&self) -> Option<&str> {
        self.texture_name_for_parameter(DIFFUSE_SAMPLER_PARAMETER_HASH)
            .or_else(|| self.texture_name_for_parameter(TEXTURE_SAMPLER_PARAMETER_HASH))
    }

    pub fn texture_name_for_parameter(&self, parameter_hash: u32) -> Option<&str> {
        self.texture_references.iter().find_map(|reference| {
            (reference.parameter_hash == parameter_hash)
                .then_some(reference.texture_name.as_deref())
                .flatten()
                .filter(|name| !name.is_empty())
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MeshPrimitive {
    pub model_index: usize,
    pub geometry_index: usize,
    pub shader_index: Option<u16>,
    pub topology: PrimitiveTopology,
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    pub uv0: Option<Vec<[f32; 2]>>,
    pub indices: Vec<u32>,
    pub declaration: VertexDeclarationSummary,
}

impl MeshPrimitive {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn winding_summary(&self) -> Option<WindingSummary> {
        let normals = self.normals.as_ref()?;
        if normals.len() != self.positions.len() {
            return None;
        }

        let mut summary = WindingSummary::default();
        for triangle in self.indices.chunks_exact(3) {
            let p0 = self.positions[triangle[0] as usize];
            let p1 = self.positions[triangle[1] as usize];
            let p2 = self.positions[triangle[2] as usize];
            let edge1 = sub3(p1, p0);
            let edge2 = sub3(p2, p0);
            let face = cross3(edge1, edge2);
            let face_len2 = dot3(face, face);
            if face_len2 <= 1.0e-16 {
                summary.degenerate += 1;
                continue;
            }

            let n0 = normals[triangle[0] as usize];
            let n1 = normals[triangle[1] as usize];
            let n2 = normals[triangle[2] as usize];
            let average = [
                n0[0] + n1[0] + n2[0],
                n0[1] + n1[1] + n2[1],
                n0[2] + n1[2] + n2[2],
            ];
            if dot3(face, average) >= 0.0 {
                summary.aligned += 1;
            } else {
                summary.opposed += 1;
            }
        }
        Some(summary)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct YdrModel {
    pub name: Option<String>,
    pub lod: LodLevel,
    pub coordinate_convention: CoordinateConvention,
    pub bounds: DrawableBounds,
    pub shaders: Vec<ShaderInfo>,
    pub primitives: Vec<MeshPrimitive>,
}

#[derive(Debug, Clone)]
pub struct YdrDocument {
    pub model: YdrModel,
    pub embedded_textures: Option<YtdDictionary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextureBindingKey {
    pub shader_index: usize,
    pub parameter_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditableTextureBinding {
    pub key: TextureBindingKey,
    pub parameter_hash: u32,
    pub texture_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ShaderBindingKey {
    pub model_index: usize,
    pub geometry_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GeometryKey {
    pub model_index: usize,
    pub geometry_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrawableBoundsProvenance {
    pub root_pointer: u64,
    pub center_pointer: u64,
    pub radius_pointer: u64,
    pub min_pointer: u64,
    pub max_pointer: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeometryProvenance {
    pub key: GeometryKey,
    pub model_pointer: u64,
    pub geometry_pointer: u64,
    pub vertex_buffer_pointer: u64,
    pub vertex_data_pointer: u64,
    pub vertex_declaration_pointer: u64,
    pub index_buffer_pointer: u64,
    pub index_data_pointer: u64,
    pub model_bounds_min_pointer: u64,
    pub model_bounds_max_pointer: u64,
    pub geometry_bounds_min_pointer: u64,
    pub geometry_bounds_max_pointer: u64,
    pub vertex_count: usize,
    pub index_count: usize,
    pub stride: u16,
    pub position_offset: usize,
    pub position_component_type: u8,
    pub normal_offset: Option<usize>,
    pub normal_component_type: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditCapability {
    pub writable: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeometryEditCapabilities {
    pub key: GeometryKey,
    pub vertex_positions: EditCapability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditableShaderBinding {
    pub key: ShaderBindingKey,
    pub shader_index: u16,
}

#[derive(Debug, Clone)]
struct TextureBindingLocation {
    binding: EditableTextureBinding,
    texture_pointer: u64,
    texture_name_pointer: u64,
}

#[derive(Debug, Clone, Copy)]
struct ShaderBindingLocation {
    binding: EditableShaderBinding,
    mapping_pointer: u64,
}

#[derive(Debug, Clone)]
pub struct YdrEditSession {
    resource: Rsc7Resource,
    root: u64,
    shader_count: usize,
    shader_bindings: Vec<EditableShaderBinding>,
    shader_locations: Vec<ShaderBindingLocation>,
    texture_bindings: Vec<EditableTextureBinding>,
    texture_locations: Vec<TextureBindingLocation>,
    bounds_provenance: DrawableBoundsProvenance,
    geometry_provenance: Vec<GeometryProvenance>,
    structural_audit_error: Option<String>,
}

impl YdrDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, YdrError> {
        let resource = Arc::new(Rsc7Resource::parse(bytes)?);
        Self::from_shared_resource_at(resource, SYSTEM_BASE)
    }

    pub fn from_shared_resource_at(
        resource: Arc<Rsc7Resource>,
        root: u64,
    ) -> Result<Self, YdrError> {
        Self::from_shared_resource_at_with_layout(resource, root, DrawableReadLayout::YDR)
    }

    pub fn from_shared_resource_at_with_layout(
        resource: Arc<Rsc7Resource>,
        root: u64,
        layout: DrawableReadLayout,
    ) -> Result<Self, YdrError> {
        validate_resource_version_for(&resource, layout.resource_version)?;

        let model = parse_ydr_model(&resource, root, layout)?;
        let shader_group_pointer = resource.read_u64(root + 0x10)?;
        let texture_dictionary_pointer = if shader_group_pointer == 0 {
            0
        } else {
            resource.bytes_at(shader_group_pointer, 0x40)?;
            resource.read_u64(shader_group_pointer + 0x08)?
        };
        let embedded_textures = if texture_dictionary_pointer == 0 {
            None
        } else {
            Some(YtdDictionary::from_shared_resource_at(
                resource,
                texture_dictionary_pointer,
            )?)
        };

        Ok(Self {
            model,
            embedded_textures,
        })
    }
}

impl YdrEditSession {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, YdrError> {
        Self::from_resource_at(Rsc7Resource::parse(bytes)?, SYSTEM_BASE)
    }

    pub fn from_resource_at(resource: Rsc7Resource, root: u64) -> Result<Self, YdrError> {
        validate_resource_version(&resource)?;
        resource.bytes_at(root, 0xD0)?;
        let (shader_count, shader_locations) = collect_shader_binding_locations(&resource, root)?;
        let shader_bindings = shader_locations
            .iter()
            .map(|location| location.binding)
            .collect();
        let texture_locations = collect_texture_binding_locations(&resource, root)?;
        let texture_bindings = texture_locations
            .iter()
            .map(|location| location.binding.clone())
            .collect();
        let bounds_provenance = DrawableBoundsProvenance {
            root_pointer: root,
            center_pointer: root + 0x20,
            radius_pointer: root + 0x2c,
            min_pointer: root + 0x30,
            max_pointer: root + 0x40,
        };
        let (geometry_provenance, structural_audit_error) =
            match collect_geometry_provenance(&resource, root) {
                Ok(provenance) => (provenance, None),
                Err(error) => (Vec::new(), Some(error.to_string())),
            };
        Ok(Self {
            resource,
            root,
            shader_count,
            shader_bindings,
            shader_locations,
            texture_bindings,
            texture_locations,
            bounds_provenance,
            geometry_provenance,
            structural_audit_error,
        })
    }

    pub fn shader_count(&self) -> usize {
        self.shader_count
    }

    pub fn shader_bindings(&self) -> &[EditableShaderBinding] {
        &self.shader_bindings
    }

    pub fn texture_bindings(&self) -> &[EditableTextureBinding] {
        &self.texture_bindings
    }

    pub fn bounds_provenance(&self) -> DrawableBoundsProvenance {
        self.bounds_provenance
    }

    pub fn geometry_provenance(&self) -> &[GeometryProvenance] {
        &self.geometry_provenance
    }

    pub fn structural_audit_error(&self) -> Option<&str> {
        self.structural_audit_error.as_deref()
    }

    pub fn geometry_edit_capabilities(&self) -> Vec<GeometryEditCapabilities> {
        self.geometry_provenance
            .iter()
            .map(|provenance| GeometryEditCapabilities {
                key: provenance.key,
                vertex_positions: EditCapability {
                    writable: false,
                    reason: Some(
                        "vertex storage and model/geometry bounds provenance are known, but normal recomputation and the full culling invariant set are not yet implemented"
                            .into(),
                    ),
                },
            })
            .collect()
    }

    pub fn rigid_translation_capability(&self) -> EditCapability {
        if let Some(error) = self.structural_audit_error() {
            return EditCapability {
                writable: false,
                reason: Some(format!("structural layout audit failed: {error}")),
            };
        }
        let (active_lods, model_count) = match render_model_layout_counts(&self.resource, self.root)
        {
            Ok(counts) => counts,
            Err(error) => {
                return EditCapability {
                    writable: false,
                    reason: Some(format!("render model layout audit failed: {error}")),
                };
            }
        };
        if active_lods != 1 || model_count != 1 {
            return EditCapability {
                writable: false,
                reason: Some(format!(
                    "rigid translation currently requires exactly one active LOD with one model; found {active_lods} active LODs and {model_count} models"
                )),
            };
        }
        if self.geometry_provenance.is_empty() {
            return EditCapability {
                writable: false,
                reason: Some("render model has no geometry provenance".into()),
            };
        }
        if let Err(error) = validate_disjoint_vertex_storage(&self.geometry_provenance) {
            return EditCapability {
                writable: false,
                reason: Some(error.to_string()),
            };
        }
        EditCapability {
            writable: true,
            reason: None,
        }
    }

    pub fn translate_rigid_model(&mut self, delta: [f32; 3]) -> Result<(), YdrError> {
        if !delta.into_iter().all(f32::is_finite) {
            return Err(YdrError::Unsupported(
                "rigid translation delta must contain only finite values".into(),
            ));
        }
        let capability = self.rigid_translation_capability();
        if !capability.writable {
            return Err(YdrError::Unsupported(
                capability
                    .reason
                    .unwrap_or_else(|| "rigid translation is unavailable".into()),
            ));
        }

        let provenance = self.geometry_provenance.clone();
        let mut patches = Vec::new();
        for geometry in &provenance {
            let stride = usize::from(geometry.stride);
            for vertex_index in 0..geometry.vertex_count {
                let vertex_offset = checked_mul(vertex_index, stride, "vertex translation offset")?;
                let position_pointer = geometry
                    .vertex_data_pointer
                    .checked_add(vertex_offset as u64)
                    .and_then(|pointer| pointer.checked_add(geometry.position_offset as u64))
                    .ok_or_else(|| {
                        YdrError::Malformed("vertex translation pointer overflow".into())
                    })?;
                patches.push(translated_vec3_patch(
                    &self.resource,
                    position_pointer,
                    delta,
                    "vertex position",
                )?);
            }
            patches.push(translated_vec3_patch(
                &self.resource,
                geometry.geometry_bounds_min_pointer,
                delta,
                "geometry min bounds",
            )?);
            patches.push(translated_vec3_patch(
                &self.resource,
                geometry.geometry_bounds_max_pointer,
                delta,
                "geometry max bounds",
            )?);
        }

        let first = provenance[0];
        patches.push(translated_vec3_patch(
            &self.resource,
            first.model_bounds_min_pointer,
            delta,
            "model min bounds",
        )?);
        patches.push(translated_vec3_patch(
            &self.resource,
            first.model_bounds_max_pointer,
            delta,
            "model max bounds",
        )?);
        patches.push(translated_vec3_patch(
            &self.resource,
            self.bounds_provenance.center_pointer,
            delta,
            "drawable bounds center",
        )?);
        patches.push(translated_vec3_patch(
            &self.resource,
            self.bounds_provenance.min_pointer,
            delta,
            "drawable min bounds",
        )?);
        patches.push(translated_vec3_patch(
            &self.resource,
            self.bounds_provenance.max_pointer,
            delta,
            "drawable max bounds",
        )?);

        validate_non_overlapping_vec3_patches(&patches)?;
        for (pointer, bytes) in patches {
            self.resource
                .bytes_at_mut(pointer, bytes.len())?
                .copy_from_slice(&bytes);
        }
        Ok(())
    }

    pub fn document(&self) -> Result<YdrDocument, YdrError> {
        YdrDocument::from_shared_resource_at(Arc::new(self.resource.clone()), self.root)
    }

    pub fn rebind_shader(
        &mut self,
        source: ShaderBindingKey,
        target_shader_index: u16,
    ) -> Result<(), YdrError> {
        if usize::from(target_shader_index) >= self.shader_count {
            return Err(YdrError::Unsupported(format!(
                "target shader index {target_shader_index} exceeds shader count {}",
                self.shader_count
            )));
        }
        let source_index = self
            .shader_locations
            .iter()
            .position(|location| location.binding.key == source)
            .ok_or_else(|| {
                YdrError::Unsupported(format!(
                    "shader binding model {} geometry {} was not found",
                    source.model_index, source.geometry_index
                ))
            })?;
        let mapping_pointer = self.shader_locations[source_index].mapping_pointer;
        self.resource
            .bytes_at_mut(mapping_pointer, 2)?
            .copy_from_slice(&target_shader_index.to_le_bytes());
        self.shader_locations[source_index].binding.shader_index = target_shader_index;
        self.shader_bindings[source_index].shader_index = target_shader_index;
        Ok(())
    }

    pub fn rebind_texture(
        &mut self,
        source: TextureBindingKey,
        target: TextureBindingKey,
    ) -> Result<(), YdrError> {
        let source_index = self
            .texture_locations
            .iter()
            .position(|location| location.binding.key == source)
            .ok_or_else(|| {
                YdrError::Unsupported(format!(
                    "texture binding shader {} parameter {} was not found",
                    source.shader_index, source.parameter_index
                ))
            })?;
        let target_index = self
            .texture_locations
            .iter()
            .position(|location| location.binding.key == target)
            .ok_or_else(|| {
                YdrError::Unsupported(format!(
                    "target texture binding shader {} parameter {} was not found",
                    target.shader_index, target.parameter_index
                ))
            })?;

        let source_hash = self.texture_locations[source_index].binding.parameter_hash;
        let target_hash = self.texture_locations[target_index].binding.parameter_hash;
        if source_hash != target_hash {
            return Err(YdrError::Unsupported(format!(
                "texture binding parameter hash mismatch: source 0x{source_hash:08X}, target 0x{target_hash:08X}"
            )));
        }
        let source_pointer = self.texture_locations[source_index].texture_pointer;
        if source_pointer == 0 {
            return Err(YdrError::Unsupported(
                "cannot rebind a null source texture reference safely".into(),
            ));
        }
        let target_name_pointer = self.texture_locations[target_index].texture_name_pointer;
        if target_name_pointer == 0 {
            return Err(YdrError::Unsupported(
                "cannot rebind to a texture reference without an existing name".into(),
            ));
        }
        let source_reference_count = count_pointer_references(&self.resource, source_pointer);
        if source_reference_count != 1 {
            return Err(YdrError::Unsupported(format!(
                "source TextureBase 0x{source_pointer:016X} has {source_reference_count} references in the RSC7 resource and cannot be edited without affecting aliases"
            )));
        }

        let target_name = self.texture_locations[target_index]
            .binding
            .texture_name
            .clone();
        self.resource
            .bytes_at_mut(source_pointer + 0x28, 8)?
            .copy_from_slice(&target_name_pointer.to_le_bytes());
        self.texture_locations[source_index].texture_name_pointer = target_name_pointer;
        self.texture_locations[source_index].binding.texture_name = target_name.clone();
        self.texture_bindings[source_index].texture_name = target_name;
        Ok(())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, YdrError> {
        let expected_document = match self.document() {
            Ok(document) => Some(document),
            Err(YdrError::Unsupported(message))
                if message == "drawable contains no renderable model LOD"
                    || message == "selected drawable LOD contains no geometries" =>
            {
                None
            }
            Err(error) => return Err(error),
        };
        let bytes = self.resource.to_bytes()?;
        let reopened = Self::from_resource_at(Rsc7Resource::parse(&bytes)?, self.root)?;
        if reopened.shader_bindings != self.shader_bindings
            || reopened.texture_bindings != self.texture_bindings
        {
            return Err(YdrError::Malformed(
                "edited YDR binding state changed after semantic re-open".into(),
            ));
        }

        if let Some(expected) = expected_document {
            let reopened_document = reopened.document()?;
            let embedded_textures_match = match (
                expected.embedded_textures.as_ref(),
                reopened_document.embedded_textures.as_ref(),
            ) {
                (None, None) => true,
                (Some(left), Some(right)) => left.textures() == right.textures(),
                _ => false,
            };
            if reopened_document.model != expected.model || !embedded_textures_match {
                return Err(YdrError::Malformed(
                    "edited YDR failed semantic verification after re-open".into(),
                ));
            }
        }
        Ok(bytes)
    }
}

impl YdrModel {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, YdrError> {
        let resource = Rsc7Resource::parse(bytes)?;
        validate_resource_version(&resource)?;
        parse_ydr_model(&resource, SYSTEM_BASE, DrawableReadLayout::YDR)
    }

    pub fn vertex_count(&self) -> usize {
        self.primitives
            .iter()
            .map(|primitive| primitive.positions.len())
            .sum()
    }

    pub fn index_count(&self) -> usize {
        self.primitives
            .iter()
            .map(|primitive| primitive.indices.len())
            .sum()
    }

    pub fn triangle_count(&self) -> usize {
        self.primitives
            .iter()
            .map(MeshPrimitive::triangle_count)
            .sum()
    }
}

fn validate_resource_version(resource: &Rsc7Resource) -> Result<(), YdrError> {
    validate_resource_version_for(resource, SUPPORTED_VERSION)
}

fn validate_resource_version_for(
    resource: &Rsc7Resource,
    expected_version: u32,
) -> Result<(), YdrError> {
    if resource.header.version != expected_version {
        return Err(YdrError::Unsupported(format!(
            "drawable resource version {} is unsupported for this layout; expected GTA V PC version {expected_version}",
            resource.header.version
        )));
    }
    Ok(())
}

fn parse_ydr_model(
    resource: &Rsc7Resource,
    root: u64,
    layout: DrawableReadLayout,
) -> Result<YdrModel, YdrError> {
    // Ensure the complete selected Drawable root layout is addressable.
    resource.bytes_at(root, layout.root_size)?;

    let bounds = DrawableBounds {
        center: read_vec3(resource, root + 0x20)?,
        radius: read_f32(resource, root + 0x2c)?,
        min: read_vec3(resource, root + 0x30)?,
        max: read_vec3(resource, root + 0x40)?,
    };
    validate_bounds(bounds)?;

    let name_pointer = resource.read_u64(root + layout.name_offset)?;
    let name = if name_pointer == 0 {
        None
    } else {
        Some(resource.read_c_string(name_pointer, MAX_NAME_LENGTH)?)
    };

    let shader_group_pointer = resource.read_u64(root + 0x10)?;
    let shaders = parse_shader_group(resource, shader_group_pointer)?;

    let lod_candidates = [
        (LodLevel::High, 0x50_u64),
        (LodLevel::Medium, 0x58),
        (LodLevel::Low, 0x60),
        (LodLevel::VeryLow, 0x68),
    ];

    let mut selected = None;
    for (lod, offset) in lod_candidates {
        let list_pointer = resource.read_u64(root + offset)?;
        if list_pointer == 0 {
            continue;
        }
        let models = read_pointer_list(resource, list_pointer, "drawable model list", MAX_MODELS)?;
        if !models.is_empty() {
            selected = Some((lod, models));
            break;
        }
    }

    let (lod, model_pointers) = selected
        .ok_or_else(|| YdrError::Unsupported("drawable contains no renderable model LOD".into()))?;

    let mut primitives = Vec::new();
    for (model_index, model_pointer) in model_pointers.into_iter().enumerate() {
        parse_model(
            resource,
            model_pointer,
            model_index,
            shaders.len(),
            &mut primitives,
        )?;
    }

    if primitives.is_empty() {
        return Err(YdrError::Unsupported(
            "selected drawable LOD contains no geometries".into(),
        ));
    }

    Ok(YdrModel {
        name,
        lod,
        coordinate_convention: CoordinateConvention::SourceXyzZUp,
        bounds,
        shaders,
        primitives,
    })
}

#[derive(Debug)]
pub enum YdrError {
    Resource(ResourceError),
    Texture(YtdError),
    Malformed(String),
    Unsupported(String),
}

impl fmt::Display for YdrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resource(error) => write!(f, "{error}"),
            Self::Texture(error) => write!(f, "{error}"),
            Self::Malformed(message) => write!(f, "malformed YDR: {message}"),
            Self::Unsupported(message) => write!(f, "unsupported YDR feature: {message}"),
        }
    }
}

impl Error for YdrError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Resource(error) => Some(error),
            Self::Texture(error) => Some(error),
            Self::Malformed(_) | Self::Unsupported(_) => None,
        }
    }
}

impl From<ResourceError> for YdrError {
    fn from(error: ResourceError) -> Self {
        Self::Resource(error)
    }
}

impl From<YtdError> for YdrError {
    fn from(error: YtdError) -> Self {
        Self::Texture(error)
    }
}

#[derive(Debug, Clone, Copy)]
struct Attribute {
    offset: usize,
    component_type: u8,
}

#[derive(Debug, Clone, Copy)]
struct VertexDeclaration {
    summary: VertexDeclarationSummary,
    attributes: [Option<Attribute>; 16],
}

fn parse_shader_group(resource: &Rsc7Resource, pointer: u64) -> Result<Vec<ShaderInfo>, YdrError> {
    if pointer == 0 {
        return Ok(Vec::new());
    }
    resource.bytes_at(pointer, 0x40)?;

    let shader_array_pointer = resource.read_u64(pointer + 0x10)?;
    let shader_count = usize::from(resource.read_u16(pointer + 0x18)?);
    let shader_capacity = usize::from(resource.read_u16(pointer + 0x1a)?);
    validate_count("shader", shader_count, MAX_SHADERS)?;
    if shader_count > shader_capacity {
        return Err(YdrError::Malformed(format!(
            "shader count {shader_count} exceeds capacity {shader_capacity}"
        )));
    }
    if shader_count == 0 {
        return Ok(Vec::new());
    }
    if shader_array_pointer == 0 {
        return Err(YdrError::Malformed(
            "shader group has shaders but a null shader pointer array".into(),
        ));
    }

    let byte_len = checked_mul(shader_count, 8, "shader pointer array")?;
    resource.bytes_at(shader_array_pointer, byte_len)?;
    let mut shaders = Vec::with_capacity(shader_count);
    for shader_index in 0..shader_count {
        let pointer_address = shader_array_pointer
            .checked_add(checked_mul(shader_index, 8, "shader pointer index")? as u64)
            .ok_or_else(|| YdrError::Malformed("shader pointer address overflow".into()))?;
        let shader_pointer = resource.read_u64(pointer_address)?;
        shaders.push(parse_shader(resource, shader_pointer, shader_index)?);
    }
    Ok(shaders)
}

fn parse_shader(
    resource: &Rsc7Resource,
    pointer: u64,
    shader_index: usize,
) -> Result<ShaderInfo, YdrError> {
    if pointer == 0 {
        return Err(YdrError::Malformed(format!(
            "shader {shader_index} has a null pointer"
        )));
    }
    resource.bytes_at(pointer, 0x30)?;

    let parameters_pointer = resource.read_u64(pointer)?;
    let name_hash = resource.read_u32(pointer + 0x08)?;
    let parameter_count = usize::from(resource.read_u8(pointer + 0x10)?);
    let file_hash = resource.read_u32(pointer + 0x18)?;
    validate_count("shader parameter", parameter_count, MAX_SHADER_PARAMETERS)?;

    if parameter_count == 0 {
        return Ok(ShaderInfo {
            name_hash,
            file_hash,
            texture_references: Vec::new(),
        });
    }
    if parameters_pointer == 0 {
        return Err(YdrError::Malformed(format!(
            "shader {shader_index} has {parameter_count} parameters but a null parameter block"
        )));
    }

    let descriptor_bytes = checked_mul(parameter_count, 16, "shader parameter descriptors")?;
    resource.bytes_at(parameters_pointer, descriptor_bytes)?;
    let mut parameters = Vec::with_capacity(parameter_count);
    let mut embedded_vector_bytes = 0_usize;
    for parameter_index in 0..parameter_count {
        let descriptor = parameters_pointer
            .checked_add(checked_mul(parameter_index, 16, "shader parameter index")? as u64)
            .ok_or_else(|| YdrError::Malformed("shader parameter address overflow".into()))?;
        let data_type = resource.read_u8(descriptor)?;
        let data_pointer = resource.read_u64(descriptor + 0x08)?;
        embedded_vector_bytes = embedded_vector_bytes
            .checked_add(checked_mul(
                usize::from(data_type),
                16,
                "shader embedded parameter data",
            )?)
            .ok_or_else(|| YdrError::Malformed("shader parameter data size overflow".into()))?;
        parameters.push((data_type, data_pointer));
    }

    let hash_pointer = parameters_pointer
        .checked_add(descriptor_bytes as u64)
        .and_then(|value| value.checked_add(embedded_vector_bytes as u64))
        .ok_or_else(|| YdrError::Malformed("shader parameter hash address overflow".into()))?;
    let hash_bytes = checked_mul(parameter_count, 4, "shader parameter hashes")?;
    resource.bytes_at(hash_pointer, hash_bytes)?;

    let mut texture_references = Vec::new();
    for (parameter_index, (data_type, data_pointer)) in parameters.into_iter().enumerate() {
        if data_type != 0 {
            continue;
        }
        let parameter_hash_address = hash_pointer
            .checked_add(checked_mul(parameter_index, 4, "shader parameter hash index")? as u64)
            .ok_or_else(|| YdrError::Malformed("shader parameter hash overflow".into()))?;
        let parameter_hash = resource.read_u32(parameter_hash_address)?;
        let texture_name = if data_pointer == 0 {
            None
        } else {
            resource.bytes_at(data_pointer, 0x50)?;
            let name_pointer = resource.read_u64(data_pointer + 0x28)?;
            if name_pointer == 0 {
                None
            } else {
                Some(resource.read_c_string(name_pointer, MAX_NAME_LENGTH)?)
            }
        };
        texture_references.push(ShaderTextureReference {
            parameter_hash,
            texture_name,
        });
    }

    Ok(ShaderInfo {
        name_hash,
        file_hash,
        texture_references,
    })
}

fn collect_shader_binding_locations(
    resource: &Rsc7Resource,
    root: u64,
) -> Result<(usize, Vec<ShaderBindingLocation>), YdrError> {
    let shader_group_pointer = resource.read_u64(root + 0x10)?;
    let shader_count = parse_shader_group(resource, shader_group_pointer)?.len();
    if shader_count == 0 {
        return Ok((0, Vec::new()));
    }

    let lod_candidates = [0x50_u64, 0x58, 0x60, 0x68];
    let mut model_pointers = Vec::new();
    for offset in lod_candidates {
        let list_pointer = resource.read_u64(root + offset)?;
        if list_pointer == 0 {
            continue;
        }
        let models = read_pointer_list(resource, list_pointer, "drawable model list", MAX_MODELS)?;
        if !models.is_empty() {
            model_pointers = models;
            break;
        }
    }

    let mut bindings = Vec::new();
    for (model_index, model_pointer) in model_pointers.into_iter().enumerate() {
        if model_pointer == 0 {
            return Err(YdrError::Malformed(format!(
                "model {model_index} has a null pointer"
            )));
        }
        resource.bytes_at(model_pointer, 0x30)?;
        let geometry_count = usize::from(resource.read_u16(model_pointer + 0x10)?);
        let geometry_capacity = usize::from(resource.read_u16(model_pointer + 0x12)?);
        validate_count("geometry", geometry_count, MAX_GEOMETRIES_PER_MODEL)?;
        if geometry_count > geometry_capacity {
            return Err(YdrError::Malformed(format!(
                "model {model_index} geometry count {geometry_count} exceeds capacity {geometry_capacity}"
            )));
        }
        if geometry_count == 0 {
            continue;
        }
        let mapping_pointer = resource.read_u64(model_pointer + 0x20)?;
        if mapping_pointer == 0 {
            continue;
        }
        resource.bytes_at(
            mapping_pointer,
            checked_mul(geometry_count, 2, "shader mapping")?,
        )?;
        for geometry_index in 0..geometry_count {
            let address = mapping_pointer
                .checked_add(checked_mul(geometry_index, 2, "shader mapping index")? as u64)
                .ok_or_else(|| YdrError::Malformed("shader mapping address overflow".into()))?;
            let shader_index = resource.read_u16(address)?;
            if usize::from(shader_index) >= shader_count {
                return Err(YdrError::Malformed(format!(
                    "model {model_index} geometry {geometry_index} maps to shader {shader_index}, but only {shader_count} shaders exist"
                )));
            }
            bindings.push(ShaderBindingLocation {
                binding: EditableShaderBinding {
                    key: ShaderBindingKey {
                        model_index,
                        geometry_index,
                    },
                    shader_index,
                },
                mapping_pointer: address,
            });
        }
    }

    Ok((shader_count, bindings))
}

fn collect_texture_binding_locations(
    resource: &Rsc7Resource,
    root: u64,
) -> Result<Vec<TextureBindingLocation>, YdrError> {
    let shader_group_pointer = resource.read_u64(root + 0x10)?;
    if shader_group_pointer == 0 {
        return Ok(Vec::new());
    }
    resource.bytes_at(shader_group_pointer, 0x40)?;

    let shader_array_pointer = resource.read_u64(shader_group_pointer + 0x10)?;
    let shader_count = usize::from(resource.read_u16(shader_group_pointer + 0x18)?);
    let shader_capacity = usize::from(resource.read_u16(shader_group_pointer + 0x1a)?);
    validate_count("shader", shader_count, MAX_SHADERS)?;
    if shader_count > shader_capacity {
        return Err(YdrError::Malformed(format!(
            "shader count {shader_count} exceeds capacity {shader_capacity}"
        )));
    }
    if shader_count == 0 {
        return Ok(Vec::new());
    }
    if shader_array_pointer == 0 {
        return Err(YdrError::Malformed(
            "shader group has shaders but a null shader pointer array".into(),
        ));
    }
    resource.bytes_at(
        shader_array_pointer,
        checked_mul(shader_count, 8, "shader pointer array")?,
    )?;

    let mut bindings = Vec::new();
    for shader_index in 0..shader_count {
        let pointer_address = shader_array_pointer
            .checked_add(checked_mul(shader_index, 8, "shader pointer index")? as u64)
            .ok_or_else(|| YdrError::Malformed("shader pointer address overflow".into()))?;
        let shader_pointer = resource.read_u64(pointer_address)?;
        if shader_pointer == 0 {
            return Err(YdrError::Malformed(format!(
                "shader {shader_index} has a null pointer"
            )));
        }
        resource.bytes_at(shader_pointer, 0x30)?;

        let parameters_pointer = resource.read_u64(shader_pointer)?;
        let parameter_count = usize::from(resource.read_u8(shader_pointer + 0x10)?);
        validate_count("shader parameter", parameter_count, MAX_SHADER_PARAMETERS)?;
        if parameter_count == 0 {
            continue;
        }
        if parameters_pointer == 0 {
            return Err(YdrError::Malformed(format!(
                "shader {shader_index} has {parameter_count} parameters but a null parameter block"
            )));
        }

        let descriptor_bytes = checked_mul(parameter_count, 16, "shader parameter descriptors")?;
        resource.bytes_at(parameters_pointer, descriptor_bytes)?;
        let mut parameters = Vec::with_capacity(parameter_count);
        let mut embedded_vector_bytes = 0_usize;
        for parameter_index in 0..parameter_count {
            let descriptor = parameters_pointer
                .checked_add(checked_mul(parameter_index, 16, "shader parameter index")? as u64)
                .ok_or_else(|| YdrError::Malformed("shader parameter address overflow".into()))?;
            let data_type = resource.read_u8(descriptor)?;
            let texture_pointer = resource.read_u64(descriptor + 0x08)?;
            embedded_vector_bytes = embedded_vector_bytes
                .checked_add(checked_mul(
                    usize::from(data_type),
                    16,
                    "shader embedded parameter data",
                )?)
                .ok_or_else(|| YdrError::Malformed("shader parameter data size overflow".into()))?;
            parameters.push((data_type, texture_pointer));
        }

        let hash_pointer = parameters_pointer
            .checked_add(descriptor_bytes as u64)
            .and_then(|value| value.checked_add(embedded_vector_bytes as u64))
            .ok_or_else(|| YdrError::Malformed("shader parameter hash address overflow".into()))?;
        resource.bytes_at(
            hash_pointer,
            checked_mul(parameter_count, 4, "shader parameter hashes")?,
        )?;

        for (parameter_index, (data_type, texture_pointer)) in parameters.into_iter().enumerate() {
            if data_type != 0 {
                continue;
            }
            let parameter_hash_address = hash_pointer
                .checked_add(checked_mul(parameter_index, 4, "shader parameter hash index")? as u64)
                .ok_or_else(|| YdrError::Malformed("shader parameter hash overflow".into()))?;
            let parameter_hash = resource.read_u32(parameter_hash_address)?;
            let (texture_name_pointer, texture_name) = if texture_pointer == 0 {
                (0, None)
            } else {
                resource.bytes_at(texture_pointer, 0x50)?;
                let name_pointer = resource.read_u64(texture_pointer + 0x28)?;
                let name = if name_pointer == 0 {
                    None
                } else {
                    Some(resource.read_c_string(name_pointer, MAX_NAME_LENGTH)?)
                };
                (name_pointer, name)
            };
            bindings.push(TextureBindingLocation {
                binding: EditableTextureBinding {
                    key: TextureBindingKey {
                        shader_index,
                        parameter_index,
                    },
                    parameter_hash,
                    texture_name,
                },
                texture_pointer,
                texture_name_pointer,
            });
        }
    }
    Ok(bindings)
}

fn read_shader_mapping(
    resource: &Rsc7Resource,
    pointer: u64,
    geometry_count: usize,
    shader_count: usize,
    model_index: usize,
) -> Result<Vec<Option<u16>>, YdrError> {
    if geometry_count == 0 {
        return Ok(Vec::new());
    }
    if pointer == 0 {
        return Ok(vec![None; geometry_count]);
    }

    let byte_len = checked_mul(geometry_count, 2, "shader mapping")?;
    resource.bytes_at(pointer, byte_len)?;
    let mut mapping = Vec::with_capacity(geometry_count);
    for geometry_index in 0..geometry_count {
        let address = pointer
            .checked_add(checked_mul(geometry_index, 2, "shader mapping index")? as u64)
            .ok_or_else(|| YdrError::Malformed("shader mapping address overflow".into()))?;
        let shader_index = resource.read_u16(address)?;
        if usize::from(shader_index) >= shader_count {
            return Err(YdrError::Malformed(format!(
                "model {model_index} geometry {geometry_index} maps to shader {shader_index}, but only {shader_count} shaders exist"
            )));
        }
        mapping.push(Some(shader_index));
    }
    Ok(mapping)
}

fn collect_geometry_provenance(
    resource: &Rsc7Resource,
    root: u64,
) -> Result<Vec<GeometryProvenance>, YdrError> {
    let lod_candidates = [0x50_u64, 0x58, 0x60, 0x68];
    let mut model_pointers = Vec::new();
    for offset in lod_candidates {
        let list_pointer = resource.read_u64(root + offset)?;
        if list_pointer == 0 {
            continue;
        }
        let models = read_pointer_list(resource, list_pointer, "drawable model list", MAX_MODELS)?;
        if !models.is_empty() {
            model_pointers = models;
            break;
        }
    }

    let mut provenance = Vec::new();
    for (model_index, model_pointer) in model_pointers.into_iter().enumerate() {
        if model_pointer == 0 {
            return Err(YdrError::Malformed(format!(
                "model {model_index} has a null pointer"
            )));
        }
        resource.bytes_at(model_pointer, 0x30)?;
        let geometries_pointer = resource.read_u64(model_pointer + 0x08)?;
        let geometry_count = usize::from(resource.read_u16(model_pointer + 0x10)?);
        let geometry_capacity = usize::from(resource.read_u16(model_pointer + 0x12)?);
        let model_bounds_pointer = resource.read_u64(model_pointer + 0x18)?;
        validate_count("geometry", geometry_count, MAX_GEOMETRIES_PER_MODEL)?;
        if geometry_count > geometry_capacity {
            return Err(YdrError::Malformed(format!(
                "model {model_index} geometry count {geometry_count} exceeds capacity {geometry_capacity}"
            )));
        }
        if geometry_count == 0 {
            continue;
        }
        if geometries_pointer == 0 {
            return Err(YdrError::Malformed(format!(
                "model {model_index} has {geometry_count} geometries but a null geometry array"
            )));
        }
        if model_bounds_pointer == 0 {
            return Err(YdrError::Unsupported(format!(
                "model {model_index} has geometries but no model/geometry bounds table"
            )));
        }
        resource.bytes_at(
            geometries_pointer,
            checked_mul(geometry_count, 8, "geometry pointer array")?,
        )?;
        resource.bytes_at(
            model_bounds_pointer,
            checked_mul(
                geometry_count
                    .checked_add(1)
                    .ok_or_else(|| YdrError::Malformed("bounds table count overflow".into()))?,
                0x20,
                "model/geometry bounds table",
            )?,
        )?;
        let model_bounds_max_pointer = model_bounds_pointer
            .checked_add(0x10)
            .ok_or_else(|| YdrError::Malformed("model max bounds pointer overflow".into()))?;

        for geometry_index in 0..geometry_count {
            let geometry_pointer = resource.read_u64(
                geometries_pointer
                    .checked_add(checked_mul(geometry_index, 8, "geometry pointer index")? as u64)
                    .ok_or_else(|| {
                        YdrError::Malformed("geometry pointer address overflow".into())
                    })?,
            )?;
            if geometry_pointer == 0 {
                return Err(YdrError::Malformed(format!(
                    "model {model_index} geometry {geometry_index} has a null pointer"
                )));
            }
            resource.bytes_at(geometry_pointer, 0x98)?;

            let vertex_buffer_pointer = resource.read_u64(geometry_pointer + 0x18)?;
            let index_buffer_pointer = resource.read_u64(geometry_pointer + 0x38)?;
            let geometry_index_count = usize::try_from(resource.read_u32(geometry_pointer + 0x58)?)
                .map_err(|_| {
                    YdrError::Malformed("geometry index count does not fit usize".into())
                })?;
            let geometry_vertex_count = usize::from(resource.read_u16(geometry_pointer + 0x60)?);
            let geometry_stride = resource.read_u16(geometry_pointer + 0x70)?;

            let (vertex_count, stride, vertex_data_pointer, declaration) =
                parse_vertex_buffer(resource, vertex_buffer_pointer)?;
            if vertex_count != geometry_vertex_count || stride != geometry_stride {
                return Err(YdrError::Malformed(format!(
                    "model {model_index} geometry {geometry_index} provenance disagrees with geometry vertex metadata"
                )));
            }
            let vertex_declaration_pointer = resource.read_u64(vertex_buffer_pointer + 0x30)?;
            let (index_count, index_data_pointer) =
                parse_index_buffer(resource, index_buffer_pointer)?;
            if index_count != geometry_index_count {
                return Err(YdrError::Malformed(format!(
                    "model {model_index} geometry {geometry_index} provenance disagrees with geometry index metadata"
                )));
            }

            let position_attribute = declaration.attributes[0].ok_or_else(|| {
                YdrError::Unsupported(format!(
                    "model {model_index} geometry {geometry_index} has no position attribute"
                ))
            })?;
            if position_attribute.component_type != 6 {
                return Err(YdrError::Unsupported(format!(
                    "model {model_index} geometry {geometry_index} position component type {} is not Float3",
                    position_attribute.component_type
                )));
            }
            let normal_attribute = declaration.attributes[3];

            let vertex_byte_len = checked_mul(vertex_count, usize::from(stride), "vertex buffer")?;
            resource.bytes_at(vertex_data_pointer, vertex_byte_len)?;
            resource.bytes_at(
                index_data_pointer,
                checked_mul(index_count, 2, "index buffer")?,
            )?;

            let geometry_bounds_offset =
                checked_mul(geometry_index, 0x20, "geometry bounds index")? as u64;
            let geometry_bounds_min_pointer = model_bounds_pointer
                .checked_add(0x20)
                .and_then(|pointer| pointer.checked_add(geometry_bounds_offset))
                .ok_or_else(|| YdrError::Malformed("geometry bounds pointer overflow".into()))?;
            let geometry_bounds_max_pointer = geometry_bounds_min_pointer
                .checked_add(0x10)
                .ok_or_else(|| {
                    YdrError::Malformed("geometry max bounds pointer overflow".into())
                })?;

            provenance.push(GeometryProvenance {
                key: GeometryKey {
                    model_index,
                    geometry_index,
                },
                model_pointer,
                geometry_pointer,
                vertex_buffer_pointer,
                vertex_data_pointer,
                vertex_declaration_pointer,
                index_buffer_pointer,
                index_data_pointer,
                model_bounds_min_pointer: model_bounds_pointer,
                model_bounds_max_pointer,
                geometry_bounds_min_pointer,
                geometry_bounds_max_pointer,
                vertex_count,
                index_count,
                stride,
                position_offset: position_attribute.offset,
                position_component_type: position_attribute.component_type,
                normal_offset: normal_attribute.map(|attribute| attribute.offset),
                normal_component_type: normal_attribute.map(|attribute| attribute.component_type),
            });
        }
    }

    Ok(provenance)
}

fn parse_model(
    resource: &Rsc7Resource,
    pointer: u64,
    model_index: usize,
    shader_count: usize,
    output: &mut Vec<MeshPrimitive>,
) -> Result<(), YdrError> {
    if pointer == 0 {
        return Err(YdrError::Malformed(format!(
            "model {model_index} has a null pointer"
        )));
    }
    resource.bytes_at(pointer, 0x30)?;

    let geometries_pointer = resource.read_u64(pointer + 0x08)?;
    let geometry_count = usize::from(resource.read_u16(pointer + 0x10)?);
    let geometry_capacity = usize::from(resource.read_u16(pointer + 0x12)?);
    let shader_mapping_pointer = resource.read_u64(pointer + 0x20)?;

    validate_count("geometry", geometry_count, MAX_GEOMETRIES_PER_MODEL)?;
    if geometry_count > geometry_capacity {
        return Err(YdrError::Malformed(format!(
            "model {model_index} geometry count {geometry_count} exceeds capacity {geometry_capacity}"
        )));
    }
    if geometry_count == 0 {
        return Ok(());
    }
    if geometries_pointer == 0 {
        return Err(YdrError::Malformed(format!(
            "model {model_index} has {geometry_count} geometries but a null geometry array"
        )));
    }

    let geometry_bytes = checked_mul(geometry_count, 8, "geometry pointer array")?;
    resource.bytes_at(geometries_pointer, geometry_bytes)?;
    let shader_mapping = read_shader_mapping(
        resource,
        shader_mapping_pointer,
        geometry_count,
        shader_count,
        model_index,
    )?;

    for (geometry_index, shader_index) in shader_mapping.iter().copied().enumerate() {
        let offset = checked_mul(geometry_index, 8, "geometry pointer index")?;
        let geometry_pointer =
            resource.read_u64(geometries_pointer.checked_add(offset as u64).ok_or_else(
                || YdrError::Malformed("geometry pointer address overflow".into()),
            )?)?;
        output.push(parse_geometry(
            resource,
            geometry_pointer,
            model_index,
            geometry_index,
            shader_index,
        )?);
    }

    Ok(())
}

fn parse_geometry(
    resource: &Rsc7Resource,
    pointer: u64,
    model_index: usize,
    geometry_index: usize,
    shader_index: Option<u16>,
) -> Result<MeshPrimitive, YdrError> {
    if pointer == 0 {
        return Err(YdrError::Malformed(format!(
            "model {model_index} geometry {geometry_index} has a null pointer"
        )));
    }
    resource.bytes_at(pointer, 0x98)?;

    let vertex_buffer_pointer = resource.read_u64(pointer + 0x18)?;
    let index_buffer_pointer = resource.read_u64(pointer + 0x38)?;
    let geometry_index_count = usize::try_from(resource.read_u32(pointer + 0x58)?)
        .map_err(|_| YdrError::Malformed("geometry index count does not fit usize".into()))?;
    let geometry_vertex_count = usize::from(resource.read_u16(pointer + 0x60)?);
    let geometry_stride = resource.read_u16(pointer + 0x70)?;

    validate_count("vertex", geometry_vertex_count, MAX_VERTICES_PER_GEOMETRY)?;
    validate_count("index", geometry_index_count, MAX_INDICES_PER_GEOMETRY)?;

    let (vertex_count, stride, data_pointer, declaration) =
        parse_vertex_buffer(resource, vertex_buffer_pointer)?;
    if vertex_count != geometry_vertex_count {
        return Err(YdrError::Malformed(format!(
            "model {model_index} geometry {geometry_index} vertex count mismatch: geometry={geometry_vertex_count}, buffer={vertex_count}"
        )));
    }
    if stride != geometry_stride {
        return Err(YdrError::Malformed(format!(
            "model {model_index} geometry {geometry_index} vertex stride mismatch: geometry={geometry_stride}, buffer={stride}"
        )));
    }

    let (index_count, index_pointer) = parse_index_buffer(resource, index_buffer_pointer)?;
    if index_count != geometry_index_count {
        return Err(YdrError::Malformed(format!(
            "model {model_index} geometry {geometry_index} index count mismatch: geometry={geometry_index_count}, buffer={index_count}"
        )));
    }
    if index_count % 3 != 0 {
        return Err(YdrError::Unsupported(format!(
            "model {model_index} geometry {geometry_index} index count {index_count} is not a triangle list"
        )));
    }

    let vertex_byte_len = checked_mul(vertex_count, usize::from(stride), "vertex buffer")?;
    if vertex_byte_len > MAX_VERTEX_BYTES {
        return Err(YdrError::Malformed(format!(
            "vertex buffer is too large: {vertex_byte_len} bytes"
        )));
    }
    let vertex_bytes = resource.bytes_at(data_pointer, vertex_byte_len)?;

    let position_attribute = declaration.attributes[0].ok_or_else(|| {
        YdrError::Unsupported(format!(
            "model {model_index} geometry {geometry_index} has no position attribute"
        ))
    })?;
    if position_attribute.component_type != 6 {
        return Err(YdrError::Unsupported(format!(
            "position component type {} is not supported; Float3 (6) is required",
            position_attribute.component_type
        )));
    }

    let mut positions = Vec::with_capacity(vertex_count);
    for vertex_index in 0..vertex_count {
        positions.push(read_vertex_vec3(
            vertex_bytes,
            vertex_index,
            usize::from(stride),
            position_attribute.offset,
        )?);
    }

    let normals = match declaration.attributes[3] {
        Some(attribute) if attribute.component_type == 6 => {
            let mut values = Vec::with_capacity(vertex_count);
            for vertex_index in 0..vertex_count {
                values.push(read_vertex_vec3(
                    vertex_bytes,
                    vertex_index,
                    usize::from(stride),
                    attribute.offset,
                )?);
            }
            Some(values)
        }
        _ => None,
    };

    let uv0 = match declaration.attributes[6] {
        Some(attribute) if attribute.component_type == 5 => {
            let mut values = Vec::with_capacity(vertex_count);
            for vertex_index in 0..vertex_count {
                values.push(read_vertex_vec2(
                    vertex_bytes,
                    vertex_index,
                    usize::from(stride),
                    attribute.offset,
                )?);
            }
            Some(values)
        }
        Some(attribute) if attribute.component_type == 1 => {
            let mut values = Vec::with_capacity(vertex_count);
            for vertex_index in 0..vertex_count {
                values.push(read_vertex_half2(
                    vertex_bytes,
                    vertex_index,
                    usize::from(stride),
                    attribute.offset,
                )?);
            }
            Some(values)
        }
        _ => None,
    };

    let index_byte_len = checked_mul(index_count, 2, "index buffer")?;
    let index_bytes = resource.bytes_at(index_pointer, index_byte_len)?;
    let mut indices = Vec::with_capacity(index_count);
    for chunk in index_bytes.chunks_exact(2) {
        let index = u16::from_le_bytes([chunk[0], chunk[1]]) as u32;
        if index as usize >= vertex_count {
            return Err(YdrError::Malformed(format!(
                "model {model_index} geometry {geometry_index} index {index} exceeds vertex count {vertex_count}"
            )));
        }
        indices.push(index);
    }

    Ok(MeshPrimitive {
        model_index,
        geometry_index,
        shader_index,
        topology: PrimitiveTopology::TriangleList,
        positions,
        normals,
        uv0,
        indices,
        declaration: declaration.summary,
    })
}

fn parse_vertex_buffer(
    resource: &Rsc7Resource,
    pointer: u64,
) -> Result<(usize, u16, u64, VertexDeclaration), YdrError> {
    if pointer == 0 {
        return Err(YdrError::Malformed(
            "geometry has a null vertex buffer".into(),
        ));
    }
    resource.bytes_at(pointer, 0x80)?;

    let stride = resource.read_u16(pointer + 0x08)?;
    if stride == 0 {
        return Err(YdrError::Malformed("vertex buffer stride is zero".into()));
    }
    let data1 = resource.read_u64(pointer + 0x10)?;
    let vertex_count = usize::try_from(resource.read_u32(pointer + 0x18)?)
        .map_err(|_| YdrError::Malformed("vertex count does not fit usize".into()))?;
    let data2 = resource.read_u64(pointer + 0x20)?;
    let declaration_pointer = resource.read_u64(pointer + 0x30)?;
    let data_pointer = if data1 != 0 { data1 } else { data2 };
    if data_pointer == 0 && vertex_count != 0 {
        return Err(YdrError::Malformed(
            "vertex buffer has vertices but no data pointer".into(),
        ));
    }

    validate_count("vertex", vertex_count, MAX_VERTICES_PER_GEOMETRY)?;
    let declaration = parse_vertex_declaration(resource, declaration_pointer)?;
    if declaration.summary.stride != stride {
        return Err(YdrError::Malformed(format!(
            "vertex declaration stride {} does not match vertex buffer stride {stride}",
            declaration.summary.stride
        )));
    }

    Ok((vertex_count, stride, data_pointer, declaration))
}

fn parse_index_buffer(resource: &Rsc7Resource, pointer: u64) -> Result<(usize, u64), YdrError> {
    if pointer == 0 {
        return Err(YdrError::Malformed(
            "geometry has a null index buffer".into(),
        ));
    }
    resource.bytes_at(pointer, 0x60)?;

    let count = usize::try_from(resource.read_u32(pointer + 0x08)?)
        .map_err(|_| YdrError::Malformed("index count does not fit usize".into()))?;
    let data_pointer = resource.read_u64(pointer + 0x10)?;
    validate_count("index", count, MAX_INDICES_PER_GEOMETRY)?;
    if data_pointer == 0 && count != 0 {
        return Err(YdrError::Malformed(
            "index buffer has indices but no data pointer".into(),
        ));
    }
    Ok((count, data_pointer))
}

fn parse_vertex_declaration(
    resource: &Rsc7Resource,
    pointer: u64,
) -> Result<VertexDeclaration, YdrError> {
    if pointer == 0 {
        return Err(YdrError::Malformed(
            "vertex buffer has no vertex declaration".into(),
        ));
    }
    resource.bytes_at(pointer, 16)?;

    let flags = resource.read_u32(pointer)?;
    let stride = resource.read_u16(pointer + 0x04)?;
    let component_count = resource.read_u8(pointer + 0x07)?;
    let types = resource.read_u64(pointer + 0x08)?;

    let mut attributes = [None; 16];
    let mut offset = 0_usize;
    let mut observed_count = 0_u8;
    for (slot, attribute) in attributes.iter_mut().enumerate() {
        if (flags & (1_u32 << slot)) == 0 {
            continue;
        }
        observed_count = observed_count.saturating_add(1);
        let component_type = ((types >> (slot * 4)) & 0x0f) as u8;
        let size = component_size(component_type).ok_or_else(|| {
            YdrError::Unsupported(format!(
                "vertex component type {component_type} in semantic slot {slot} has unknown size"
            ))
        })?;
        *attribute = Some(Attribute {
            offset,
            component_type,
        });
        offset = offset
            .checked_add(size)
            .ok_or_else(|| YdrError::Malformed("vertex declaration stride overflow".into()))?;
    }

    if observed_count != component_count {
        return Err(YdrError::Malformed(format!(
            "vertex declaration reports {component_count} components but flags contain {observed_count}"
        )));
    }
    if offset != usize::from(stride) {
        return Err(YdrError::Malformed(format!(
            "vertex declaration components occupy {offset} bytes but stride is {stride}"
        )));
    }

    Ok(VertexDeclaration {
        summary: VertexDeclarationSummary {
            flags,
            stride,
            component_count,
            types,
        },
        attributes,
    })
}

fn component_size(component_type: u8) -> Option<usize> {
    match component_type {
        1 => Some(4),  // Half2
        2 => Some(4),  // Float
        3 => Some(8),  // Half4
        5 => Some(8),  // Float2
        6 => Some(12), // Float3
        7 => Some(16), // Float4
        8 => Some(4),  // UByte4
        9 => Some(4),  // Colour
        10 => Some(4), // RGBA8SNorm
        _ => None,
    }
}

fn read_pointer_list(
    resource: &Rsc7Resource,
    pointer: u64,
    label: &str,
    maximum: usize,
) -> Result<Vec<u64>, YdrError> {
    resource.bytes_at(pointer, 16)?;
    let array_pointer = resource.read_u64(pointer)?;
    let count = usize::from(resource.read_u16(pointer + 0x08)?);
    let capacity = usize::from(resource.read_u16(pointer + 0x0a)?);
    validate_count(label, count, maximum)?;
    if count > capacity {
        return Err(YdrError::Malformed(format!(
            "{label} count {count} exceeds capacity {capacity}"
        )));
    }
    if count == 0 {
        return Ok(Vec::new());
    }
    if array_pointer == 0 {
        return Err(YdrError::Malformed(format!(
            "{label} has {count} entries but a null item array"
        )));
    }

    let byte_len = checked_mul(count, 8, label)?;
    resource.bytes_at(array_pointer, byte_len)?;
    let mut output = Vec::with_capacity(count);
    for index in 0..count {
        let offset = checked_mul(index, 8, label)?;
        let address = array_pointer
            .checked_add(offset as u64)
            .ok_or_else(|| YdrError::Malformed(format!("{label} pointer overflow")))?;
        output.push(resource.read_u64(address)?);
    }
    Ok(output)
}

fn validate_count(label: &str, count: usize, maximum: usize) -> Result<(), YdrError> {
    if count > maximum {
        return Err(YdrError::Malformed(format!(
            "{label} count {count} exceeds safety limit {maximum}"
        )));
    }
    Ok(())
}

fn render_model_layout_counts(
    resource: &Rsc7Resource,
    root: u64,
) -> Result<(usize, usize), YdrError> {
    let mut active_lods = 0_usize;
    let mut model_count = 0_usize;
    for offset in [0x50_u64, 0x58, 0x60, 0x68] {
        let list_pointer = resource.read_u64(root + offset)?;
        if list_pointer == 0 {
            continue;
        }
        let models = read_pointer_list(resource, list_pointer, "drawable model list", MAX_MODELS)?;
        if models.is_empty() {
            continue;
        }
        active_lods = active_lods
            .checked_add(1)
            .ok_or_else(|| YdrError::Malformed("active LOD count overflow".into()))?;
        model_count = model_count
            .checked_add(models.len())
            .ok_or_else(|| YdrError::Malformed("render model count overflow".into()))?;
    }
    Ok((active_lods, model_count))
}

fn validate_disjoint_vertex_storage(provenance: &[GeometryProvenance]) -> Result<(), YdrError> {
    let mut ranges = Vec::with_capacity(provenance.len());
    for geometry in provenance {
        let byte_len = checked_mul(
            geometry.vertex_count,
            usize::from(geometry.stride),
            "vertex storage range",
        )?;
        let end = geometry
            .vertex_data_pointer
            .checked_add(byte_len as u64)
            .ok_or_else(|| YdrError::Malformed("vertex storage end overflow".into()))?;
        ranges.push((geometry.vertex_data_pointer, end, geometry.key));
    }
    ranges.sort_by_key(|range| range.0);
    for pair in ranges.windows(2) {
        let left = pair[0];
        let right = pair[1];
        if right.0 < left.1 {
            return Err(YdrError::Unsupported(format!(
                "geometry vertex storage overlaps between model {} geometry {} and model {} geometry {}",
                left.2.model_index,
                left.2.geometry_index,
                right.2.model_index,
                right.2.geometry_index
            )));
        }
    }
    Ok(())
}

fn validate_non_overlapping_vec3_patches(patches: &[(u64, [u8; 12])]) -> Result<(), YdrError> {
    let mut ranges = patches
        .iter()
        .map(|(pointer, bytes)| {
            pointer
                .checked_add(bytes.len() as u64)
                .map(|end| (*pointer, end))
                .ok_or_else(|| YdrError::Malformed("structural patch end overflow".into()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    ranges.sort_by_key(|range| range.0);
    for pair in ranges.windows(2) {
        if pair[1].0 < pair[0].1 {
            return Err(YdrError::Unsupported(format!(
                "structural edit patches overlap at 0x{:016X}..0x{:016X} and 0x{:016X}..0x{:016X}",
                pair[0].0, pair[0].1, pair[1].0, pair[1].1
            )));
        }
    }
    Ok(())
}

fn translated_vec3_patch(
    resource: &Rsc7Resource,
    pointer: u64,
    delta: [f32; 3],
    label: &str,
) -> Result<(u64, [u8; 12]), YdrError> {
    let source = read_vec3(resource, pointer)?;
    let translated = [
        source[0] + delta[0],
        source[1] + delta[1],
        source[2] + delta[2],
    ];
    if !translated.into_iter().all(f32::is_finite) {
        return Err(YdrError::Unsupported(format!(
            "{label} becomes non-finite after rigid translation"
        )));
    }
    let mut bytes = [0_u8; 12];
    for (index, value) in translated.into_iter().enumerate() {
        let start = index * 4;
        bytes[start..start + 4].copy_from_slice(&value.to_le_bytes());
    }
    Ok((pointer, bytes))
}

fn checked_mul(left: usize, right: usize, label: &str) -> Result<usize, YdrError> {
    left.checked_mul(right)
        .ok_or_else(|| YdrError::Malformed(format!("{label} byte length overflow")))
}

fn count_pointer_references(resource: &Rsc7Resource, pointer: u64) -> usize {
    let needle = pointer.to_le_bytes();
    resource
        .system()
        .windows(needle.len())
        .chain(resource.graphics().windows(needle.len()))
        .filter(|window| *window == needle.as_slice())
        .count()
}

fn read_f32(resource: &Rsc7Resource, pointer: u64) -> Result<f32, YdrError> {
    let bytes: [u8; 4] = resource
        .bytes_at(pointer, 4)?
        .try_into()
        .expect("fixed four-byte resource slice");
    Ok(f32::from_le_bytes(bytes))
}

fn read_vec3(resource: &Rsc7Resource, pointer: u64) -> Result<[f32; 3], YdrError> {
    Ok([
        read_f32(resource, pointer)?,
        read_f32(resource, pointer + 4)?,
        read_f32(resource, pointer + 8)?,
    ])
}

fn validate_bounds(bounds: DrawableBounds) -> Result<(), YdrError> {
    let finite = bounds
        .center
        .into_iter()
        .chain(bounds.min)
        .chain(bounds.max)
        .chain([bounds.radius])
        .all(f32::is_finite);
    if !finite || bounds.radius < 0.0 {
        return Err(YdrError::Malformed("drawable bounds are not finite".into()));
    }
    Ok(())
}

fn read_vertex_vec3(
    bytes: &[u8],
    vertex_index: usize,
    stride: usize,
    attribute_offset: usize,
) -> Result<[f32; 3], YdrError> {
    let slice = vertex_attribute(bytes, vertex_index, stride, attribute_offset, 12)?;
    Ok([
        f32::from_le_bytes(slice[0..4].try_into().expect("four bytes")),
        f32::from_le_bytes(slice[4..8].try_into().expect("four bytes")),
        f32::from_le_bytes(slice[8..12].try_into().expect("four bytes")),
    ])
}

fn read_vertex_vec2(
    bytes: &[u8],
    vertex_index: usize,
    stride: usize,
    attribute_offset: usize,
) -> Result<[f32; 2], YdrError> {
    let slice = vertex_attribute(bytes, vertex_index, stride, attribute_offset, 8)?;
    Ok([
        f32::from_le_bytes(slice[0..4].try_into().expect("four bytes")),
        f32::from_le_bytes(slice[4..8].try_into().expect("four bytes")),
    ])
}

fn read_vertex_half2(
    bytes: &[u8],
    vertex_index: usize,
    stride: usize,
    attribute_offset: usize,
) -> Result<[f32; 2], YdrError> {
    let slice = vertex_attribute(bytes, vertex_index, stride, attribute_offset, 4)?;
    Ok([
        half_to_f32(u16::from_le_bytes([slice[0], slice[1]])),
        half_to_f32(u16::from_le_bytes([slice[2], slice[3]])),
    ])
}

fn vertex_attribute(
    bytes: &[u8],
    vertex_index: usize,
    stride: usize,
    attribute_offset: usize,
    attribute_size: usize,
) -> Result<&[u8], YdrError> {
    let vertex_start = checked_mul(vertex_index, stride, "vertex offset")?;
    let start = vertex_start
        .checked_add(attribute_offset)
        .ok_or_else(|| YdrError::Malformed("vertex attribute offset overflow".into()))?;
    let end = start
        .checked_add(attribute_size)
        .ok_or_else(|| YdrError::Malformed("vertex attribute end overflow".into()))?;
    bytes.get(start..end).ok_or_else(|| {
        YdrError::Malformed(format!(
            "vertex attribute [{start}..{end}] exceeds {}-byte buffer",
            bytes.len()
        ))
    })
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn half_to_f32(bits: u16) -> f32 {
    let sign = ((bits >> 15) & 0x1) as u32;
    let exponent = ((bits >> 10) & 0x1f) as u32;
    let fraction = (bits & 0x03ff) as u32;

    let result = match exponent {
        0 => {
            if fraction == 0 {
                sign << 31
            } else {
                let mut mantissa = fraction;
                let mut shift = 0_u32;
                while (mantissa & 0x0400) == 0 {
                    mantissa <<= 1;
                    shift += 1;
                }
                mantissa &= 0x03ff;
                let exponent32 = 127_u32 - 15 - shift + 1;
                (sign << 31) | (exponent32 << 23) | (mantissa << 13)
            }
        }
        0x1f => (sign << 31) | (0xff << 23) | (fraction << 13),
        _ => {
            let exponent32 = exponent + (127 - 15);
            (sign << 31) | (exponent32 << 23) | (fraction << 13)
        }
    };
    f32::from_bits(result)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::{write::DeflateEncoder, Compression};
    use ragelab_resource::{Rsc7Resource, RSC7_MAGIC, SYSTEM_BASE};

    use super::{
        CoordinateConvention, GeometryKey, LodLevel, PrimitiveTopology, ShaderBindingKey,
        ShaderInfo, ShaderTextureReference, TextureBindingKey, YdrDocument, YdrEditSession,
        YdrError, YdrModel, DIFFUSE_SAMPLER_PARAMETER_HASH, TEXTURE_SAMPLER_PARAMETER_HASH,
    };

    const SYSTEM_SIZE: usize = 2048;
    const SYSTEM_FLAGS: u32 = (1 << 26) | 1;
    const DECL_TYPES: u64 = 0x7755_5555_5599_6996;

    fn ptr(offset: usize) -> u64 {
        SYSTEM_BASE + offset as u64
    }

    fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn write_f32(bytes: &mut [u8], offset: usize, value: f32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn build_fixture(mut mutate: impl FnMut(&mut [u8])) -> Vec<u8> {
        let mut system = vec![0_u8; SYSTEM_SIZE];

        // Drawable root.
        write_f32(&mut system, 0x20, 0.5);
        write_f32(&mut system, 0x24, 0.5);
        write_f32(&mut system, 0x28, 0.0);
        write_f32(&mut system, 0x2c, 1.0);
        write_f32(&mut system, 0x30, 0.0);
        write_f32(&mut system, 0x34, 0.0);
        write_f32(&mut system, 0x38, 0.0);
        write_f32(&mut system, 0x40, 1.0);
        write_f32(&mut system, 0x44, 1.0);
        write_f32(&mut system, 0x48, 0.0);
        write_u64(&mut system, 0x50, ptr(0x0d0));
        write_u64(&mut system, 0xa8, ptr(0x340));

        // High LOD ResourcePointerList64.
        write_u64(&mut system, 0x0d0, ptr(0x0e0));
        write_u16(&mut system, 0x0d8, 1);
        write_u16(&mut system, 0x0da, 1);
        write_u64(&mut system, 0x0e0, ptr(0x0f0));

        // DrawableModel.
        write_u64(&mut system, 0x0f0 + 0x08, ptr(0x120));
        write_u16(&mut system, 0x0f0 + 0x10, 1);
        write_u16(&mut system, 0x0f0 + 0x12, 1);
        write_u64(&mut system, 0x0f0 + 0x18, ptr(0x680));
        write_u64(&mut system, 0x120, ptr(0x130));

        // Model bounds followed by one min/max Vector4 storage pair per geometry.
        // The fourth lane is preserved/unknown for structural editing; real source
        // assets can keep it at zero even when other serializers normalize it.
        for base in [0x680, 0x6a0] {
            write_f32(&mut system, base, 0.0);
            write_f32(&mut system, base + 4, 0.0);
            write_f32(&mut system, base + 8, 0.0);
            write_f32(&mut system, base + 12, 0.0);
            write_f32(&mut system, base + 16, 1.0);
            write_f32(&mut system, base + 20, 1.0);
            write_f32(&mut system, base + 24, 0.0);
            write_f32(&mut system, base + 28, 0.0);
        }

        // DrawableGeometry.
        write_u64(&mut system, 0x130 + 0x18, ptr(0x1d0));
        write_u64(&mut system, 0x130 + 0x38, ptr(0x250));
        write_u32(&mut system, 0x130 + 0x58, 3);
        write_u16(&mut system, 0x130 + 0x60, 3);
        write_u16(&mut system, 0x130 + 0x70, 36);
        write_u64(&mut system, 0x130 + 0x78, ptr(0x2c0));

        // VertexBuffer.
        write_u16(&mut system, 0x1d0 + 0x08, 36);
        write_u64(&mut system, 0x1d0 + 0x10, ptr(0x2c0));
        write_u32(&mut system, 0x1d0 + 0x18, 3);
        write_u64(&mut system, 0x1d0 + 0x30, ptr(0x2b0));

        // IndexBuffer.
        write_u32(&mut system, 0x250 + 0x08, 3);
        write_u64(&mut system, 0x250 + 0x10, ptr(0x330));

        // VertexDeclaration: Position Float3, Normal Float3, Colour0, TexCoord0 Float2.
        write_u32(&mut system, 0x2b0, 0x59);
        write_u16(&mut system, 0x2b4, 36);
        system[0x2b7] = 4;
        write_u64(&mut system, 0x2b8, DECL_TYPES);

        let vertices = [
            ([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0]),
            ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0]),
            ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0]),
        ];
        for (index, (position, normal, uv)) in vertices.into_iter().enumerate() {
            let base = 0x2c0 + index * 36;
            for (component, value) in position.into_iter().enumerate() {
                write_f32(&mut system, base + component * 4, value);
            }
            for (component, value) in normal.into_iter().enumerate() {
                write_f32(&mut system, base + 12 + component * 4, value);
            }
            system[base + 24..base + 28].copy_from_slice(&[255, 255, 255, 255]);
            write_f32(&mut system, base + 28, uv[0]);
            write_f32(&mut system, base + 32, uv[1]);
        }
        write_u16(&mut system, 0x330, 0);
        write_u16(&mut system, 0x332, 1);
        write_u16(&mut system, 0x334, 2);
        system[0x340..0x34e].copy_from_slice(b"test_drawable\0");

        mutate(&mut system);

        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&system).expect("compress fixture");
        let compressed = encoder.finish().expect("finish compression");

        let mut output = Vec::new();
        output.extend_from_slice(&RSC7_MAGIC);
        output.extend_from_slice(&165_u32.to_le_bytes());
        output.extend_from_slice(&SYSTEM_FLAGS.to_le_bytes());
        output.extend_from_slice(&0_u32.to_le_bytes());
        output.extend_from_slice(&compressed);
        output
    }

    fn add_two_texture_shader(system: &mut [u8]) {
        // Drawable -> ShaderGroup -> one ShaderFX.
        write_u64(system, 0x10, ptr(0x380));
        write_u64(system, 0x380 + 0x10, ptr(0x3c0));
        write_u16(system, 0x380 + 0x18, 1);
        write_u16(system, 0x380 + 0x1a, 1);
        write_u64(system, 0x3c0, ptr(0x3d0));
        write_u64(system, 0x3d0, ptr(0x400));
        write_u32(system, 0x3d0 + 0x08, 0x1111_2222);
        system[0x3d0 + 0x10] = 2;
        write_u32(system, 0x3d0 + 0x18, 0x3333_4444);
        system[0x3d0 + 0x27] = 2;

        // Two texture ShaderParameter descriptors. Texture parameters have no
        // embedded vector payload, so the hash list starts after 2 * 16 bytes.
        system[0x400] = 0;
        write_u64(system, 0x408, ptr(0x440));
        system[0x410] = 0;
        write_u64(system, 0x418, ptr(0x4a0));
        write_u32(system, 0x420, 0x5555_6666);
        write_u32(system, 0x424, 0x7777_8888);

        // Existing TextureBase objects and their names. The editor may only
        // point an existing descriptor at one of these objects.
        write_u64(system, 0x440 + 0x28, ptr(0x500));
        write_u64(system, 0x4a0 + 0x28, ptr(0x520));
        system[0x500..0x50d].copy_from_slice(b"test_diffuse\0");
        system[0x520..0x52c].copy_from_slice(b"test_normal\0");

        // Geometry 0 -> shader 0.
        write_u64(system, 0x0f0 + 0x20, ptr(0x540));
        write_u16(system, 0x540, 0);
    }

    fn add_second_shader(system: &mut [u8]) {
        write_u16(system, 0x380 + 0x18, 2);
        write_u16(system, 0x380 + 0x1a, 2);
        write_u64(system, 0x3c8, ptr(0x560));
        write_u32(system, 0x560 + 0x08, 0x9999_AAAA);
        write_u32(system, 0x560 + 0x18, 0xBBBB_CCCC);
    }

    #[test]
    fn decodes_static_triangle_geometry() {
        let bytes = build_fixture(|_| {});
        let model = YdrModel::from_bytes(&bytes).expect("valid synthetic YDR");

        assert_eq!(model.name.as_deref(), Some("test_drawable"));
        assert_eq!(model.lod, LodLevel::High);
        assert_eq!(
            model.coordinate_convention,
            CoordinateConvention::SourceXyzZUp
        );
        assert_eq!(model.bounds.min, [0.0, 0.0, 0.0]);
        assert_eq!(model.bounds.max, [1.0, 1.0, 0.0]);
        assert_eq!(model.primitives.len(), 1);
        assert_eq!(model.vertex_count(), 3);
        assert_eq!(model.index_count(), 3);
        assert_eq!(model.triangle_count(), 1);

        let primitive = &model.primitives[0];
        assert_eq!(primitive.topology, PrimitiveTopology::TriangleList);
        let winding = primitive
            .winding_summary()
            .expect("normals should permit winding validation");
        assert_eq!(winding.aligned, 1);
        assert_eq!(winding.opposed, 0);
        assert_eq!(winding.degenerate, 0);
        assert_eq!(
            primitive.positions,
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
        );
        assert_eq!(
            primitive.normals.as_ref().unwrap(),
            &vec![[0.0, 0.0, 1.0]; 3]
        );
        assert_eq!(
            primitive.uv0.as_ref().unwrap(),
            &vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]
        );
        assert_eq!(primitive.indices, vec![0, 1, 2]);
        assert_eq!(primitive.declaration.flags, 0x59);
        assert_eq!(primitive.declaration.stride, 36);
    }

    #[test]
    fn edit_session_preserves_geometry_provenance_but_keeps_vertex_write_locked() {
        let bytes = build_fixture(|_| {});
        let session = YdrEditSession::from_bytes(&bytes).expect("fixture should be editable");

        let bounds = session.bounds_provenance();
        assert_eq!(bounds.root_pointer, ptr(0));
        assert_eq!(bounds.center_pointer, ptr(0x20));
        assert_eq!(bounds.radius_pointer, ptr(0x2c));
        assert_eq!(bounds.min_pointer, ptr(0x30));
        assert_eq!(bounds.max_pointer, ptr(0x40));

        assert_eq!(session.structural_audit_error(), None);
        assert_eq!(session.geometry_provenance().len(), 1);
        let provenance = session.geometry_provenance()[0];
        assert_eq!(
            provenance.key,
            GeometryKey {
                model_index: 0,
                geometry_index: 0,
            }
        );
        assert_eq!(provenance.model_pointer, ptr(0x0f0));
        assert_eq!(provenance.geometry_pointer, ptr(0x130));
        assert_eq!(provenance.vertex_buffer_pointer, ptr(0x1d0));
        assert_eq!(provenance.vertex_declaration_pointer, ptr(0x2b0));
        assert_eq!(provenance.vertex_data_pointer, ptr(0x2c0));
        assert_eq!(provenance.index_buffer_pointer, ptr(0x250));
        assert_eq!(provenance.index_data_pointer, ptr(0x330));
        assert_eq!(provenance.model_bounds_min_pointer, ptr(0x680));
        assert_eq!(provenance.model_bounds_max_pointer, ptr(0x690));
        assert_eq!(provenance.geometry_bounds_min_pointer, ptr(0x6a0));
        assert_eq!(provenance.geometry_bounds_max_pointer, ptr(0x6b0));
        assert_eq!(provenance.vertex_count, 3);
        assert_eq!(provenance.index_count, 3);
        assert_eq!(provenance.stride, 36);
        assert_eq!(provenance.position_offset, 0);
        assert_eq!(provenance.position_component_type, 6);
        assert_eq!(provenance.normal_offset, Some(12));
        assert_eq!(provenance.normal_component_type, Some(6));

        let capabilities = session.geometry_edit_capabilities();
        assert_eq!(capabilities.len(), 1);
        assert_eq!(capabilities[0].key, provenance.key);
        assert!(!capabilities[0].vertex_positions.writable);
        assert!(capabilities[0]
            .vertex_positions
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("normal recomputation")));
    }

    #[test]
    fn rigid_translation_updates_positions_and_xyz_bounds_only() {
        let bytes = build_fixture(|_| {});
        let original = YdrDocument::from_bytes(&bytes).expect("original fixture should parse");
        let original_resource = Rsc7Resource::parse(&bytes).expect("original fixture envelope");
        let mut session = YdrEditSession::from_bytes(&bytes).expect("fixture should be editable");
        assert_eq!(
            session.rigid_translation_capability(),
            super::EditCapability {
                writable: true,
                reason: None,
            }
        );

        session
            .translate_rigid_model([2.0, -3.0, 4.0])
            .expect("single static model should support rigid translation");
        let edited = session.to_bytes().expect("translated YDR should re-encode");
        let reopened = YdrDocument::from_bytes(&edited).expect("translated YDR should reopen");
        let edited_resource = Rsc7Resource::parse(&edited).expect("translated resource envelope");

        assert_eq!(reopened.model.bounds.center, [2.5, -2.5, 4.0]);
        assert_eq!(reopened.model.bounds.min, [2.0, -3.0, 4.0]);
        assert_eq!(reopened.model.bounds.max, [3.0, -2.0, 4.0]);
        assert_eq!(reopened.model.bounds.radius, original.model.bounds.radius);
        assert_eq!(
            reopened.model.primitives[0].positions,
            vec![[2.0, -3.0, 4.0], [3.0, -3.0, 4.0], [2.0, -2.0, 4.0]]
        );
        assert_eq!(
            reopened.model.primitives[0].normals,
            original.model.primitives[0].normals
        );
        assert_eq!(
            reopened.model.primitives[0].uv0,
            original.model.primitives[0].uv0
        );
        assert_eq!(
            reopened.model.primitives[0].indices,
            original.model.primitives[0].indices
        );

        assert_eq!(
            super::read_vec3(&edited_resource, ptr(0x680)).unwrap(),
            [2.0, -3.0, 4.0]
        );
        assert_eq!(
            super::read_vec3(&edited_resource, ptr(0x690)).unwrap(),
            [3.0, -2.0, 4.0]
        );
        assert_eq!(
            super::read_vec3(&edited_resource, ptr(0x6a0)).unwrap(),
            [2.0, -3.0, 4.0]
        );
        assert_eq!(
            super::read_vec3(&edited_resource, ptr(0x6b0)).unwrap(),
            [3.0, -2.0, 4.0]
        );
        assert_eq!(
            super::read_f32(&edited_resource, ptr(0x680 + 12)).unwrap(),
            0.0
        );
        assert_eq!(
            super::read_f32(&edited_resource, ptr(0x690 + 12)).unwrap(),
            0.0
        );
        assert_eq!(
            super::read_f32(&edited_resource, ptr(0x6a0 + 12)).unwrap(),
            0.0
        );
        assert_eq!(
            super::read_f32(&edited_resource, ptr(0x6b0 + 12)).unwrap(),
            0.0
        );

        for vertex_index in 0..3 {
            let tail_pointer = ptr(0x2c0 + vertex_index * 36 + 12);
            assert_eq!(
                edited_resource.bytes_at(tail_pointer, 24).unwrap(),
                original_resource.bytes_at(tail_pointer, 24).unwrap(),
                "normal/color/uv bytes changed for vertex {vertex_index}"
            );
        }
    }

    #[test]
    fn rigid_translation_capability_rejects_multiple_active_lods() {
        let bytes = build_fixture(|system| {
            write_u64(system, 0x58, ptr(0x0d0));
        });
        let session =
            YdrEditSession::from_bytes(&bytes).expect("fixture should remain inspectable");
        let capability = session.rigid_translation_capability();
        assert!(!capability.writable);
        assert!(capability
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("exactly one active LOD")));
    }

    #[test]
    fn rigid_translation_rejects_non_finite_delta_without_mutation() {
        let bytes = build_fixture(|_| {});
        let original = YdrDocument::from_bytes(&bytes).expect("original fixture should parse");
        let mut session = YdrEditSession::from_bytes(&bytes).expect("fixture should be editable");
        assert!(matches!(
            session.translate_rigid_model([f32::NAN, 0.0, 0.0]),
            Err(YdrError::Unsupported(_))
        ));
        assert_eq!(session.document().unwrap().model, original.model);
    }

    #[test]
    fn structural_audit_failure_does_not_disable_binding_edit_session() {
        let bytes = build_fixture(|system| {
            add_two_texture_shader(system);
            let types = (DECL_TYPES & !0x0f) | 0x0f;
            write_u64(system, 0x2b8, types);
        });
        let session = YdrEditSession::from_bytes(&bytes)
            .expect("binding edit session should survive unsupported structural layout");
        assert_eq!(session.geometry_provenance().len(), 0);
        assert!(session
            .structural_audit_error()
            .is_some_and(|message| message.contains("component type 15")));
        assert_eq!(session.texture_bindings().len(), 2);
    }

    #[test]
    fn resolves_diffuse_sampler_before_legacy_texture_sampler() {
        let shader = ShaderInfo {
            name_hash: 0,
            file_hash: 0,
            texture_references: vec![
                ShaderTextureReference {
                    parameter_hash: TEXTURE_SAMPLER_PARAMETER_HASH,
                    texture_name: Some("legacy_albedo".into()),
                },
                ShaderTextureReference {
                    parameter_hash: DIFFUSE_SAMPLER_PARAMETER_HASH,
                    texture_name: Some("primary_albedo".into()),
                },
            ],
        };
        assert_eq!(shader.diffuse_texture_name(), Some("primary_albedo"));

        let fallback = ShaderInfo {
            name_hash: 0,
            file_hash: 0,
            texture_references: vec![ShaderTextureReference {
                parameter_hash: TEXTURE_SAMPLER_PARAMETER_HASH,
                texture_name: Some("legacy_albedo".into()),
            }],
        };
        assert_eq!(fallback.diffuse_texture_name(), Some("legacy_albedo"));
    }

    #[test]
    fn decodes_shader_mapping_and_texture_reference() {
        let bytes = build_fixture(|system| {
            // Drawable -> ShaderGroup.
            write_u64(system, 0x10, ptr(0x380));
            write_u64(system, 0x380 + 0x10, ptr(0x3c0));
            write_u16(system, 0x380 + 0x18, 1);
            write_u16(system, 0x380 + 0x1a, 1);
            write_u64(system, 0x3c0, ptr(0x3d0));

            // One ShaderFX with one texture parameter.
            write_u64(system, 0x3d0, ptr(0x400));
            write_u32(system, 0x3d0 + 0x08, 0x1111_2222);
            system[0x3d0 + 0x10] = 1;
            write_u32(system, 0x3d0 + 0x18, 0x3333_4444);
            system[0x3d0 + 0x27] = 1;

            // ShaderParameter descriptor + parameter-name hash.
            system[0x400] = 0; // texture parameter
            write_u64(system, 0x408, ptr(0x440));
            write_u32(system, 0x410, 0x5555_6666);

            // External TextureBase reference.
            write_u64(system, 0x440 + 0x28, ptr(0x490));
            system[0x490..0x49d].copy_from_slice(b"test_diffuse\0");

            // Geometry 0 -> shader 0.
            write_u64(system, 0x0f0 + 0x20, ptr(0x4b0));
            write_u16(system, 0x4b0, 0);
        });

        let model = YdrModel::from_bytes(&bytes).expect("valid shader-linked YDR");
        assert_eq!(model.shaders.len(), 1);
        assert_eq!(model.shaders[0].name_hash, 0x1111_2222);
        assert_eq!(model.shaders[0].file_hash, 0x3333_4444);
        assert_eq!(model.shaders[0].texture_references.len(), 1);
        assert_eq!(
            model.shaders[0].texture_references[0].parameter_hash,
            0x5555_6666
        );
        assert_eq!(
            model.shaders[0].texture_references[0]
                .texture_name
                .as_deref(),
            Some("test_diffuse")
        );
        assert_eq!(model.primitives[0].shader_index, Some(0));
    }

    #[test]
    fn edit_session_rebinds_geometry_to_existing_shader() {
        let bytes = build_fixture(|system| {
            add_two_texture_shader(system);
            add_second_shader(system);
        });
        let original = YdrDocument::from_bytes(&bytes).expect("original fixture should parse");
        assert_eq!(original.model.shaders.len(), 2);
        assert_eq!(original.model.primitives[0].shader_index, Some(0));

        let mut session = YdrEditSession::from_bytes(&bytes).expect("fixture should be editable");
        assert_eq!(session.shader_count(), 2);
        assert_eq!(session.shader_bindings().len(), 1);
        let key = ShaderBindingKey {
            model_index: 0,
            geometry_index: 0,
        };
        assert_eq!(session.shader_bindings()[0].key, key);
        assert_eq!(session.shader_bindings()[0].shader_index, 0);
        session
            .rebind_shader(key, 1)
            .expect("existing shader index should be reusable");
        assert_eq!(session.shader_bindings()[0].shader_index, 1);

        let edited = session
            .to_bytes()
            .expect("edited resource should re-encode");
        let reopened = YdrDocument::from_bytes(&edited).expect("edited resource should reopen");
        assert_eq!(reopened.model.primitives[0].shader_index, Some(1));
        assert_eq!(reopened.model.bounds, original.model.bounds);
        assert_eq!(
            reopened.model.primitives[0].positions,
            original.model.primitives[0].positions
        );
        assert_eq!(
            reopened.model.primitives[0].indices,
            original.model.primitives[0].indices
        );
        assert_eq!(reopened.model.shaders, original.model.shaders);

        let mut invalid = YdrEditSession::from_bytes(&bytes).expect("fixture should be editable");
        assert!(matches!(
            invalid.rebind_shader(key, 2),
            Err(YdrError::Unsupported(_))
        ));
        assert!(matches!(
            invalid.rebind_shader(
                ShaderBindingKey {
                    model_index: 9,
                    geometry_index: 0,
                },
                1,
            ),
            Err(YdrError::Unsupported(_))
        ));
    }

    #[test]
    fn edit_session_rebinds_to_existing_texture_without_changing_layout() {
        let bytes = build_fixture(|system| {
            add_two_texture_shader(system);
            write_u32(system, 0x424, 0x5555_6666);
        });
        let original = YdrDocument::from_bytes(&bytes).expect("original fixture should parse");
        let mut session = YdrEditSession::from_bytes(&bytes).expect("fixture should be editable");
        assert_eq!(session.texture_bindings().len(), 2);
        assert_eq!(
            session.texture_bindings()[0].texture_name.as_deref(),
            Some("test_diffuse")
        );
        assert_eq!(
            session.texture_bindings()[1].texture_name.as_deref(),
            Some("test_normal")
        );

        let source = TextureBindingKey {
            shader_index: 0,
            parameter_index: 0,
        };
        let target = TextureBindingKey {
            shader_index: 0,
            parameter_index: 1,
        };
        session
            .rebind_texture(source, target)
            .expect("existing texture name should be reusable");
        assert_eq!(
            session.texture_bindings()[0].texture_name.as_deref(),
            Some("test_normal")
        );

        let edited = session
            .to_bytes()
            .expect("edited resource should re-encode");
        let edited_resource = Rsc7Resource::parse(&edited).expect("edited resource envelope");
        assert_eq!(edited_resource.read_u64(ptr(0x408)).unwrap(), ptr(0x440));
        assert_eq!(
            edited_resource.read_u64(ptr(0x440 + 0x28)).unwrap(),
            ptr(0x520)
        );
        assert_eq!(edited_resource.read_u64(ptr(0x418)).unwrap(), ptr(0x4a0));
        assert_eq!(
            edited_resource.read_u64(ptr(0x4a0 + 0x28)).unwrap(),
            ptr(0x520)
        );
        let reopened = YdrDocument::from_bytes(&edited).expect("edited resource should reopen");
        assert_eq!(
            reopened.model.shaders[0].texture_references[0]
                .texture_name
                .as_deref(),
            Some("test_normal")
        );
        assert_eq!(
            reopened.model.shaders[0].texture_references[1]
                .texture_name
                .as_deref(),
            Some("test_normal")
        );
        assert_eq!(
            reopened.model.shaders[0].texture_references[0].parameter_hash,
            0x5555_6666
        );
        assert_eq!(
            reopened.model.shaders[0].texture_references[1].parameter_hash,
            0x5555_6666
        );
        assert_eq!(reopened.model.bounds, original.model.bounds);
        assert_eq!(reopened.model.primitives, original.model.primitives);
    }

    #[test]
    fn edit_session_rejects_texture_rebind_when_source_texture_base_is_shared() {
        let bytes = build_fixture(|system| {
            add_two_texture_shader(system);
            write_u32(system, 0x424, 0x5555_6666);
            write_u64(system, 0x418, ptr(0x440));
        });
        let mut session = YdrEditSession::from_bytes(&bytes).expect("fixture should be editable");
        let error = session
            .rebind_texture(
                TextureBindingKey {
                    shader_index: 0,
                    parameter_index: 0,
                },
                TextureBindingKey {
                    shader_index: 0,
                    parameter_index: 1,
                },
            )
            .expect_err("shared source TextureBase must be rejected");
        assert!(matches!(
            error,
            YdrError::Unsupported(message) if message.contains("has 2 references in the RSC7 resource")
        ));
    }

    #[test]
    fn edit_session_rejects_texture_rebind_across_parameter_semantics() {
        let bytes = build_fixture(add_two_texture_shader);
        let mut session = YdrEditSession::from_bytes(&bytes).expect("fixture should be editable");
        let error = session
            .rebind_texture(
                TextureBindingKey {
                    shader_index: 0,
                    parameter_index: 0,
                },
                TextureBindingKey {
                    shader_index: 0,
                    parameter_index: 1,
                },
            )
            .expect_err("different parameter hashes must not be rebound");
        assert!(matches!(
            error,
            YdrError::Unsupported(message) if message.contains("parameter hash mismatch")
        ));
    }

    #[test]
    fn edit_session_noop_reencode_is_semantically_equivalent() {
        let bytes = build_fixture(add_two_texture_shader);
        let original = YdrDocument::from_bytes(&bytes).expect("original fixture should parse");
        let session = YdrEditSession::from_bytes(&bytes).expect("fixture should be editable");
        let output = session.to_bytes().expect("resource should re-encode");
        let reopened = YdrDocument::from_bytes(&output).expect("re-encoded resource should reopen");
        assert_eq!(reopened.model, original.model);
    }

    #[test]
    fn edit_session_rejects_missing_or_null_texture_targets() {
        let bytes = build_fixture(|system| {
            add_two_texture_shader(system);
            write_u64(system, 0x418, 0);
        });
        let mut session = YdrEditSession::from_bytes(&bytes).expect("fixture should be editable");
        let source = TextureBindingKey {
            shader_index: 0,
            parameter_index: 0,
        };
        let missing = TextureBindingKey {
            shader_index: 0,
            parameter_index: 99,
        };
        assert!(matches!(
            session.rebind_texture(source, missing),
            Err(YdrError::Unsupported(_))
        ));
        let null_target = TextureBindingKey {
            shader_index: 0,
            parameter_index: 1,
        };
        assert!(matches!(
            session.rebind_texture(source, null_target),
            Err(YdrError::Unsupported(_))
        ));
    }

    #[test]
    fn edit_session_rejects_corrupt_parameter_block() {
        let bytes = build_fixture(|system| {
            add_two_texture_shader(system);
            write_u64(system, 0x3d0, 0);
        });
        let error = YdrEditSession::from_bytes(&bytes).expect_err("null parameter block must fail");
        assert!(matches!(
            error,
            YdrError::Malformed(message) if message.contains("null parameter block")
        ));
    }

    #[test]
    fn decodes_embedded_texture_dictionary() {
        let bytes = build_fixture(|system| {
            // Drawable -> ShaderGroup -> embedded TextureDictionary.
            write_u64(system, 0x10, ptr(0x380));
            write_u64(system, 0x380 + 0x08, ptr(0x500));

            // TextureDictionary hash/pointer lists.
            write_u64(system, 0x500 + 0x20, ptr(0x550));
            write_u16(system, 0x500 + 0x28, 1);
            write_u16(system, 0x500 + 0x2a, 1);
            write_u64(system, 0x500 + 0x30, ptr(0x560));
            write_u16(system, 0x500 + 0x38, 1);
            write_u16(system, 0x500 + 0x3a, 1);
            write_u32(system, 0x550, 0x1234_5678);
            write_u64(system, 0x560, ptr(0x580));

            // Embedded legacy Texture: 4x4 DXT1, one mip, solid red.
            write_u64(system, 0x580 + 0x28, ptr(0x620));
            write_u16(system, 0x580 + 0x50, 4);
            write_u16(system, 0x580 + 0x52, 4);
            write_u16(system, 0x580 + 0x54, 1);
            write_u16(system, 0x580 + 0x56, 2);
            write_u32(system, 0x580 + 0x58, 0x3154_5844);
            system[0x580 + 0x5d] = 1;
            write_u64(system, 0x580 + 0x70, ptr(0x640));
            system[0x620..0x62e].copy_from_slice(b"embedded_diff\0");
            system[0x640..0x648].copy_from_slice(&[0x00, 0xf8, 0x00, 0x00, 0, 0, 0, 0]);
        });

        let document = YdrDocument::from_bytes(&bytes).expect("embedded texture YDR should parse");
        let dictionary = document
            .embedded_textures
            .expect("embedded texture dictionary should be present");
        assert_eq!(dictionary.textures().len(), 1);
        assert_eq!(dictionary.textures()[0].name_hash, 0x1234_5678);
        assert_eq!(
            dictionary.textures()[0].name.as_deref(),
            Some("embedded_diff")
        );
        let decoded = dictionary.decode_top_mip(0).expect("decode embedded DXT1");
        assert_eq!(decoded.rgba.len(), 64);
        assert!(decoded
            .rgba
            .chunks_exact(4)
            .all(|pixel| pixel == [255, 0, 0, 255]));
    }

    #[test]
    fn rejects_out_of_range_index() {
        let bytes = build_fixture(|system| write_u16(system, 0x334, 9));
        let error = YdrModel::from_bytes(&bytes).expect_err("invalid index must fail");
        assert!(matches!(error, YdrError::Malformed(message) if message.contains("index 9")));
    }

    #[test]
    fn rejects_geometry_count_above_pointer_capacity() {
        let bytes = build_fixture(|system| write_u16(system, 0x0f0 + 0x10, 2));
        let error = YdrModel::from_bytes(&bytes).expect_err("invalid geometry count must fail");
        assert!(
            matches!(error, YdrError::Malformed(message) if message.contains("exceeds capacity"))
        );
    }

    #[test]
    fn rejects_unknown_active_vertex_component_type() {
        let bytes = build_fixture(|system| {
            // Position semantic slot 0 becomes unknown type 15.
            let types = (DECL_TYPES & !0x0f) | 0x0f;
            write_u64(system, 0x2b8, types);
        });
        let error = YdrModel::from_bytes(&bytes).expect_err("unknown vertex type must fail");
        assert!(
            matches!(error, YdrError::Unsupported(message) if message.contains("component type 15"))
        );
    }

    #[test]
    fn rejects_other_resource_versions() {
        let mut bytes = build_fixture(|_| {});
        bytes[4..8].copy_from_slice(&159_u32.to_le_bytes());
        let error = YdrModel::from_bytes(&bytes).expect_err("unsupported version must fail");
        assert!(matches!(error, YdrError::Unsupported(message) if message.contains("159")));
    }
}

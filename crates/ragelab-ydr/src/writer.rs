//! New-resource geometry writer for GTA V PC YDR v165.
//!
//! This first writer stage intentionally emits a geometry-only Drawable with
//! no ShaderFX group and no embedded TextureDictionary. It is suitable for
//! structural round-trip tests, not yet a claim of in-game renderability.

use super::*;
use ragelab_hash::joaat;
use ragelab_resource::{Rsc7Resource, ADDRESS_SPACE_SIZE, GRAPHICS_BASE};
use ragelab_ytd::{EncodedTexture, TextureFormat, Ytd};
use std::collections::HashSet;

const DRAWABLE_MODEL_LIST_OFFSET: usize = 0x50;
const DRAWABLE_NAME_POINTER_OFFSET: usize = YDR_NAME_OFFSET as usize;
const DRAWABLE_SHADER_GROUP_OFFSET: usize = 0x10;
const MODEL_GEOMETRIES_OFFSET: usize = 0x08;
const MODEL_GEOMETRY_COUNT_OFFSET: usize = 0x10;
const MODEL_GEOMETRY_CAPACITY_OFFSET: usize = 0x12;
const MODEL_BOUNDS_POINTER_OFFSET: usize = 0x18;
const MODEL_SHADER_MAPPING_OFFSET: usize = 0x20;
const MODEL_RENDER_MASK_FLAGS_OFFSET: usize = 0x2C;
const SHADER_FX_SIZE: usize = 0x30;
const SHADER_PARAMETER_DESCRIPTOR_SIZE: usize = 0x10;
const LEGACY_TEXTURE_BASE_SIZE: usize = 0x50;
const MAX_SHADER_PARAMETER_BLOCK_BYTES: usize = 16 * 1024 * 1024;
const GEOMETRY_VERTEX_BUFFER_OFFSET: usize = 0x18;
const GEOMETRY_INDEX_BUFFER_OFFSET: usize = 0x38;
const GEOMETRY_INDEX_COUNT_OFFSET: usize = 0x58;
const GEOMETRY_VERTEX_COUNT_OFFSET: usize = 0x60;
const GEOMETRY_VERTEX_STRIDE_OFFSET: usize = 0x70;
const GEOMETRY_VERTEX_DATA_POINTER_OFFSET: usize = 0x78;

const DRAWABLE_MODEL_LIST_SIZE: usize = 0x10;
const DRAWABLE_MODEL_SIZE: usize = 0x30;
const DRAWABLE_GEOMETRY_SIZE: usize = 0x98;
const VERTEX_BUFFER_SIZE: usize = 0x80;
const INDEX_BUFFER_SIZE: usize = 0x60;
const VERTEX_DECLARATION_SIZE: usize = 0x10;
const BOUNDS_PAIR_SIZE: usize = 0x20;
const SHADER_GROUP_SIZE: usize = 0x40;
const TEXTURE_DICTIONARY_SIZE: usize = 0x40;
const LEGACY_TEXTURE_DESCRIPTOR_SIZE: usize = 0x90;
const MAX_TEXTURES: usize = 16_384;
const MAX_TEXTURE_NAME_BYTES: usize = 4_096;
const MAX_EMBEDDED_TEXTURE_BYTES: usize = 512 * 1024 * 1024;
const MAX_WRITER_VERTEX_COUNT: usize = u16::MAX as usize;

/// Geometry input accepted by the initial YDR v165 structural writer.
///
/// Positions must already use GTA's Z-up coordinate convention. The GLB
/// decoder performs that conversion before this type is populated.
#[derive(Debug, Clone, PartialEq)]
pub struct YdrGeometryInput {
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    pub colors0: Option<Vec<[f32; 4]>>,
    pub uv0: Option<Vec<[f32; 2]>>,
    pub uv1: Option<Vec<[f32; 2]>>,
    pub tangents: Option<Vec<[f32; 4]>>,
    pub indices: Vec<u32>,
    /// Index into the Drawable ShaderGroup, when a verified shader template is supplied.
    pub shader_index: Option<u16>,
}

/// One image to embed in the YDR's TextureDictionary. Input pixels are RGBA8;
/// the writer generates a complete mip chain in the requested supported
/// format before placing the encoded payload in the RSC7 graphics segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YdrTextureInput {
    pub name: String,
    pub width: u16,
    pub height: u16,
    pub rgba8: Vec<u8>,
    pub format: TextureFormat,
}

/// Builds a geometry-only YDR v165. This structural API is retained for
/// callers that intentionally want no texture dictionary.
pub fn build_geometry_only_ydr(
    name: &str,
    geometries: &[YdrGeometryInput],
) -> Result<Vec<u8>, YdrError> {
    build_ydr_with_embedded_textures(name, geometries, &[])
}

/// Builds a fresh YDR v165 RSC7 Drawable with geometry, embedded textures and
/// optional ShaderFX templates copied from a parsed Legacy resource. The
/// supplied shader records are preserved and semantically reopened before return.
pub fn build_ydr_with_embedded_textures(
    name: &str,
    geometries: &[YdrGeometryInput],
    textures: &[YdrTextureInput],
) -> Result<Vec<u8>, YdrError> {
    build_ydr_with_embedded_textures_and_shaders(name, geometries, textures, &[])
}

/// Builds a fresh YDR v165 from geometry, embedded textures and verified Legacy ShaderFX templates.
pub fn build_ydr_with_embedded_textures_and_shaders(
    name: &str,
    geometries: &[YdrGeometryInput],
    textures: &[YdrTextureInput],
    shaders: &[ShaderInfo],
) -> Result<Vec<u8>, YdrError> {
    validate_name(name)?;
    if geometries.is_empty() {
        return Err(YdrError::Malformed(
            "YDR writer requires at least one geometry primitive".into(),
        ));
    }
    if geometries.len() > MAX_GEOMETRIES_PER_MODEL || geometries.len() > usize::from(u16::MAX) {
        return Err(YdrError::Unsupported(format!(
            "geometry count {} exceeds the writer limit",
            geometries.len()
        )));
    }
    if textures.len() > MAX_TEXTURES || textures.len() > usize::from(u16::MAX) {
        return Err(YdrError::Unsupported(format!(
            "texture count {} exceeds the TextureDictionary writer limit",
            textures.len()
        )));
    }

    let prepared = geometries
        .iter()
        .enumerate()
        .map(|(index, geometry)| prepare_geometry(index, geometry))
        .collect::<Result<Vec<_>, _>>()?;
    if shaders.len() > MAX_SHADERS || shaders.len() > usize::from(u16::MAX) {
        return Err(YdrError::Unsupported(format!(
            "shader count {} exceeds the writer limit",
            shaders.len()
        )));
    }
    for (index, geometry) in prepared.iter().enumerate() {
        match (shaders.is_empty(), geometry.shader_index) {
            (true, None) => {}
            (true, Some(_)) => {
                return Err(YdrError::Malformed(format!(
                    "geometry {index} references a shader but no ShaderFX templates were supplied"
                )))
            }
            (false, Some(shader_index)) if usize::from(shader_index) < shaders.len() => {}
            (false, Some(shader_index)) => {
                return Err(YdrError::Malformed(format!(
                    "geometry {index} references shader {shader_index}, but only {} templates were supplied",
                    shaders.len()
                )))
            }
            (false, None) => {
                return Err(YdrError::Malformed(format!(
                    "geometry {index} has no shader mapping while ShaderFX templates are present"
                )))
            }
        }
    }
    for (index, shader) in shaders.iter().enumerate() {
        validate_shader_for_write(index, shader)?;
    }
    let drawable_bounds = combine_bounds(&prepared)?;
    let mut texture_hashes = HashSet::with_capacity(textures.len());
    let mut prepared_textures = Vec::with_capacity(textures.len());
    let mut total_texture_bytes = 0usize;
    for (index, texture) in textures.iter().enumerate() {
        let prepared_texture = prepare_texture(index, texture)?;
        if !texture_hashes.insert(prepared_texture.name_hash) {
            return Err(YdrError::Malformed(format!(
                "texture {index} name hash 0x{:08X} collides with another embedded texture",
                prepared_texture.name_hash
            )));
        }
        total_texture_bytes = total_texture_bytes
            .checked_add(prepared_texture.encoded.data.len())
            .ok_or_else(|| YdrError::Malformed("embedded texture bytes overflow usize".into()))?;
        if total_texture_bytes > MAX_EMBEDDED_TEXTURE_BYTES {
            return Err(YdrError::Unsupported(format!(
                "embedded texture payload exceeds {MAX_EMBEDDED_TEXTURE_BYTES} bytes"
            )));
        }
        prepared_textures.push(prepared_texture);
    }

    let mut arena = SystemArena::default();
    let mut graphics = GraphicsArena::default();
    let (root_offset, _) = arena.allocate(YDR_ROOT_SIZE, 16, "Drawable root")?;
    let (model_list_offset, model_list_pointer) =
        arena.allocate(DRAWABLE_MODEL_LIST_SIZE, 16, "Drawable model list")?;
    let (model_array_offset, model_array_pointer) =
        arena.allocate(8, 8, "Drawable model pointer array")?;
    let (model_offset, model_pointer) = arena.allocate(DRAWABLE_MODEL_SIZE, 16, "DrawableModel")?;
    let geometry_pointer_bytes = checked_writer_mul(geometries.len(), 8, "geometry pointer array")?;
    let (geometry_array_offset, geometry_array_pointer) =
        arena.allocate(geometry_pointer_bytes, 8, "geometry pointer array")?;
    let bounds_table_bytes = checked_writer_mul(
        geometries.len() + 1,
        BOUNDS_PAIR_SIZE,
        "model/geometry bounds table",
    )?;
    let (bounds_table_offset, bounds_table_pointer) =
        arena.allocate(bounds_table_bytes, 16, "model/geometry bounds table")?;
    let (name_offset, name_pointer) = arena.allocate(name.len() + 1, 1, "Drawable name")?;

    arena.write_u64(root_offset + DRAWABLE_SHADER_GROUP_OFFSET, 0)?;
    arena.write_u64(root_offset + DRAWABLE_MODEL_LIST_OFFSET, model_list_pointer)?;
    arena.write_u64(root_offset + DRAWABLE_NAME_POINTER_OFFSET, name_pointer)?;
    arena.write_pointer(model_list_offset, model_array_pointer)?;
    arena.write_u16(model_list_offset + 0x08, 1)?;
    arena.write_u16(model_list_offset + 0x0A, 1)?;
    arena.write_pointer(model_array_offset, model_pointer)?;

    arena.write_pointer(
        model_offset + MODEL_GEOMETRIES_OFFSET,
        geometry_array_pointer,
    )?;
    arena.write_u16(
        model_offset + MODEL_GEOMETRY_COUNT_OFFSET,
        geometries.len() as u16,
    )?;
    arena.write_u16(
        model_offset + MODEL_GEOMETRY_CAPACITY_OFFSET,
        geometries.len() as u16,
    )?;
    // CodeWalker's renderer treats models without bit 0 of RenderMaskFlags
    // as reflection/shadow proxies and skips them in the default Model Viewer.
    arena.write_u16(model_offset + MODEL_RENDER_MASK_FLAGS_OFFSET, 0x0001)?;
    arena.write_pointer(
        model_offset + MODEL_BOUNDS_POINTER_OFFSET,
        bounds_table_pointer,
    )?;
    if !shaders.is_empty() {
        let mapping_bytes = checked_writer_mul(geometries.len(), 2, "model shader mapping")?;
        let (mapping_offset, mapping_pointer) =
            arena.allocate(mapping_bytes, 2, "model shader mapping")?;
        arena.write_pointer(model_offset + MODEL_SHADER_MAPPING_OFFSET, mapping_pointer)?;
        for (index, geometry) in prepared.iter().enumerate() {
            let slot = mapping_offset
                .checked_add(checked_writer_mul(index, 2, "shader mapping slot")?)
                .ok_or_else(|| YdrError::Malformed("shader mapping slot overflow".into()))?;
            arena.write_u16(
                slot,
                geometry.shader_index.expect("validated shader mapping"),
            )?;
        }
    }

    write_bounds_pair(
        &mut arena,
        bounds_table_offset,
        drawable_bounds.min,
        drawable_bounds.max,
    )?;
    write_drawable_bounds(&mut arena, root_offset, drawable_bounds)?;
    if !prepared_textures.is_empty() || !shaders.is_empty() {
        let shader_group_pointer =
            write_shader_group(&mut arena, &mut graphics, &prepared_textures, shaders)?;
        arena.write_pointer(
            root_offset + DRAWABLE_SHADER_GROUP_OFFSET,
            shader_group_pointer,
        )?;
    }

    for (index, geometry) in prepared.iter().enumerate() {
        let geometry_pointer_offset = geometry_array_offset
            .checked_add(checked_writer_mul(index, 8, "geometry pointer index")?)
            .ok_or_else(|| YdrError::Malformed("geometry pointer slot overflow".into()))?;
        let (geometry_offset, geometry_pointer) =
            arena.allocate(DRAWABLE_GEOMETRY_SIZE, 16, "DrawableGeometry")?;
        let (vertex_buffer_offset, vertex_buffer_pointer) =
            arena.allocate(VERTEX_BUFFER_SIZE, 16, "VertexBuffer")?;
        let (index_buffer_offset, index_buffer_pointer) =
            arena.allocate(INDEX_BUFFER_SIZE, 16, "IndexBuffer")?;
        let (declaration_offset, declaration_pointer) =
            arena.allocate(VERTEX_DECLARATION_SIZE, 16, "VertexDeclaration")?;
        let (vertex_data_offset, vertex_data_pointer) =
            arena.allocate(geometry.vertex_data.len(), 16, "vertex data")?;
        let (index_data_offset, index_data_pointer) =
            arena.allocate(geometry.index_data.len(), 16, "index data")?;

        arena.write_pointer(geometry_pointer_offset, geometry_pointer)?;
        arena.write_pointer(
            geometry_offset + GEOMETRY_VERTEX_BUFFER_OFFSET,
            vertex_buffer_pointer,
        )?;
        arena.write_pointer(
            geometry_offset + GEOMETRY_INDEX_BUFFER_OFFSET,
            index_buffer_pointer,
        )?;
        arena.write_u32(
            geometry_offset + GEOMETRY_INDEX_COUNT_OFFSET,
            u32::try_from(geometry.index_count)
                .map_err(|_| YdrError::Unsupported("index count exceeds u32".into()))?,
        )?;
        arena.write_u16(
            geometry_offset + GEOMETRY_VERTEX_COUNT_OFFSET,
            geometry.vertex_count as u16,
        )?;
        arena.write_u16(
            geometry_offset + GEOMETRY_VERTEX_STRIDE_OFFSET,
            geometry.stride,
        )?;
        arena.write_pointer(
            geometry_offset + GEOMETRY_VERTEX_DATA_POINTER_OFFSET,
            vertex_data_pointer,
        )?;

        arena.write_u16(vertex_buffer_offset + 0x08, geometry.stride)?;
        arena.write_pointer(vertex_buffer_offset + 0x10, vertex_data_pointer)?;
        arena.write_u32(vertex_buffer_offset + 0x18, geometry.vertex_count as u32)?;
        arena.write_pointer(vertex_buffer_offset + 0x30, declaration_pointer)?;

        arena.write_u32(index_buffer_offset + 0x08, geometry.index_count as u32)?;
        arena.write_pointer(index_buffer_offset + 0x10, index_data_pointer)?;

        arena.write_u32(declaration_offset, geometry.declaration_flags)?;
        arena.write_u16(declaration_offset + 0x04, geometry.stride)?;
        arena.write_u8(declaration_offset + 0x07, geometry.declaration_components)?;
        arena.write_u64(declaration_offset + 0x08, geometry.declaration_types)?;

        arena.write_bytes(vertex_data_offset, &geometry.vertex_data)?;
        arena.write_bytes(index_data_offset, &geometry.index_data)?;

        let bounds_offset = bounds_table_offset
            .checked_add((index + 1) * BOUNDS_PAIR_SIZE)
            .ok_or_else(|| YdrError::Malformed("geometry bounds offset overflow".into()))?;
        write_bounds_pair(
            &mut arena,
            bounds_offset,
            geometry.bounds.min,
            geometry.bounds.max,
        )?;
    }

    arena.write_bytes(name_offset, name.as_bytes())?;
    arena.write_u8(name_offset + name.len(), 0)?;

    let resource =
        Rsc7Resource::from_segments(SUPPORTED_VERSION, 0, 0, &arena.bytes, &graphics.bytes)?;
    let output = resource.to_bytes()?;
    validate_geometry_only_output(
        &output,
        name,
        &prepared,
        &prepared_textures,
        shaders,
        drawable_bounds,
    )?;
    Ok(output)
}

#[derive(Debug, Clone, Copy)]
struct Bounds {
    min: [f32; 3],
    max: [f32; 3],
}

struct PreparedGeometry {
    vertex_count: usize,
    index_count: usize,
    stride: u16,
    declaration_flags: u32,
    declaration_components: u8,
    declaration_types: u64,
    vertex_data: Vec<u8>,
    index_data: Vec<u8>,
    bounds: Bounds,
    shader_index: Option<u16>,
}

struct PreparedTexture {
    name: String,
    name_hash: u32,
    encoded: EncodedTexture,
}

#[derive(Default)]
struct GraphicsArena {
    bytes: Vec<u8>,
}

impl GraphicsArena {
    fn allocate(
        &mut self,
        size: usize,
        alignment: usize,
        label: &str,
    ) -> Result<(usize, u64), YdrError> {
        if alignment == 0 || !alignment.is_power_of_two() {
            return Err(YdrError::Malformed(format!(
                "{label} requested invalid alignment {alignment}"
            )));
        }
        let mask = alignment - 1;
        let offset = self
            .bytes
            .len()
            .checked_add(mask)
            .map(|value| value & !mask)
            .ok_or_else(|| YdrError::Malformed(format!("{label} offset overflow")))?;
        let end = offset
            .checked_add(size)
            .ok_or_else(|| YdrError::Malformed(format!("{label} size overflow")))?;
        if end as u64 > ADDRESS_SPACE_SIZE {
            return Err(YdrError::Unsupported(format!(
                "{label} exceeds the RSC7 graphics address space"
            )));
        }
        let pointer = GRAPHICS_BASE
            .checked_add(offset as u64)
            .ok_or_else(|| YdrError::Malformed(format!("{label} pointer overflow")))?;
        self.bytes.resize(end, 0);
        Ok((offset, pointer))
    }

    fn write_bytes(&mut self, offset: usize, bytes: &[u8]) -> Result<(), YdrError> {
        let end = offset
            .checked_add(bytes.len())
            .ok_or_else(|| YdrError::Malformed("graphics write range overflow".into()))?;
        let target = self.bytes.get_mut(offset..end).ok_or_else(|| {
            YdrError::Malformed(format!(
                "graphics write range {offset:#x}..{end:#x} is outside the allocated segment"
            ))
        })?;
        target.copy_from_slice(bytes);
        Ok(())
    }
}

#[derive(Default)]
struct SystemArena {
    bytes: Vec<u8>,
}

impl SystemArena {
    fn allocate(
        &mut self,
        size: usize,
        alignment: usize,
        label: &str,
    ) -> Result<(usize, u64), YdrError> {
        if alignment == 0 || !alignment.is_power_of_two() {
            return Err(YdrError::Malformed(format!(
                "{label} requested invalid alignment {alignment}"
            )));
        }
        let mask = alignment - 1;
        let offset = self
            .bytes
            .len()
            .checked_add(mask)
            .map(|value| value & !mask)
            .ok_or_else(|| YdrError::Malformed(format!("{label} offset overflow")))?;
        let end = offset
            .checked_add(size)
            .ok_or_else(|| YdrError::Malformed(format!("{label} size overflow")))?;
        if end as u64 > ADDRESS_SPACE_SIZE {
            return Err(YdrError::Unsupported(format!(
                "{label} exceeds the RSC7 system address space"
            )));
        }
        let pointer = SYSTEM_BASE
            .checked_add(offset as u64)
            .ok_or_else(|| YdrError::Malformed(format!("{label} pointer overflow")))?;
        self.bytes.resize(end, 0);
        Ok((offset, pointer))
    }

    fn write_bytes(&mut self, offset: usize, bytes: &[u8]) -> Result<(), YdrError> {
        let end = offset
            .checked_add(bytes.len())
            .ok_or_else(|| YdrError::Malformed("write range overflow".into()))?;
        let target = self.bytes.get_mut(offset..end).ok_or_else(|| {
            YdrError::Malformed(format!(
                "write range {offset:#x}..{end:#x} is outside the allocated system segment"
            ))
        })?;
        target.copy_from_slice(bytes);
        Ok(())
    }

    fn write_u8(&mut self, offset: usize, value: u8) -> Result<(), YdrError> {
        self.write_bytes(offset, &[value])
    }

    fn write_u16(&mut self, offset: usize, value: u16) -> Result<(), YdrError> {
        self.write_bytes(offset, &value.to_le_bytes())
    }

    fn write_u32(&mut self, offset: usize, value: u32) -> Result<(), YdrError> {
        self.write_bytes(offset, &value.to_le_bytes())
    }

    fn write_u64(&mut self, offset: usize, value: u64) -> Result<(), YdrError> {
        self.write_bytes(offset, &value.to_le_bytes())
    }

    fn write_f32(&mut self, offset: usize, value: f32) -> Result<(), YdrError> {
        if !value.is_finite() {
            return Err(YdrError::Malformed(
                "cannot serialize a non-finite YDR float".into(),
            ));
        }
        self.write_bytes(offset, &value.to_le_bytes())
    }

    fn write_pointer(&mut self, offset: usize, pointer: u64) -> Result<(), YdrError> {
        self.write_u64(offset, pointer)
    }
}

fn prepare_texture(index: usize, input: &YdrTextureInput) -> Result<PreparedTexture, YdrError> {
    if input.name.is_empty()
        || input.name.len() > MAX_TEXTURE_NAME_BYTES
        || input.name.as_bytes().contains(&0)
    {
        return Err(YdrError::Malformed(format!(
            "texture {index} name must contain 1..={MAX_TEXTURE_NAME_BYTES} non-NUL UTF-8 bytes"
        )));
    }
    let encoded =
        Ytd::encode_rgba8_with_mips(input.width, input.height, &input.rgba8, input.format)?;
    if encoded.data.is_empty() || encoded.levels == 0 {
        return Err(YdrError::Malformed(format!(
            "texture {index} encoder returned an empty payload or mip chain"
        )));
    }
    Ok(PreparedTexture {
        name: input.name.clone(),
        name_hash: joaat(&input.name),
        encoded,
    })
}

#[derive(Debug, Clone, Copy)]
struct ShaderParameterLayout {
    descriptor_bytes: usize,
    vector_bytes: usize,
    hash_bytes: usize,
    block_bytes: usize,
}

fn shader_parameter_layout(
    shader_index: usize,
    shader: &ShaderInfo,
) -> Result<ShaderParameterLayout, YdrError> {
    if shader.parameters.len() > MAX_SHADER_PARAMETERS
        || shader.parameters.len() > usize::from(u8::MAX)
    {
        return Err(YdrError::Unsupported(format!(
            "shader {shader_index} has {} parameters; Legacy ShaderFX stores the count as u8",
            shader.parameters.len()
        )));
    }
    let descriptor_bytes = checked_writer_mul(
        shader.parameters.len(),
        SHADER_PARAMETER_DESCRIPTOR_SIZE,
        "ShaderFX parameter descriptors",
    )?;
    let hash_bytes = checked_writer_mul(shader.parameters.len(), 4, "ShaderFX parameter hashes")?;
    let mut vector_bytes = 0usize;
    let mut texture_parameters = 0usize;
    for (parameter_index, parameter) in shader.parameters.iter().enumerate() {
        if parameter.data_type == 0 {
            texture_parameters += 1;
            if !parameter.vector_data.is_empty() {
                return Err(YdrError::Malformed(format!(
                    "shader {shader_index} texture parameter {parameter_index} contains vector payload"
                )));
            }
            let base_name = parameter
                .texture_base
                .as_ref()
                .and_then(|base| base.name.as_deref());
            if parameter.texture_name.as_deref() != base_name {
                return Err(YdrError::Malformed(format!(
                    "shader {shader_index} texture parameter {parameter_index} name does not match its TextureBase"
                )));
            }
        } else {
            if parameter.texture_name.is_some() || parameter.texture_base.is_some() {
                return Err(YdrError::Malformed(format!(
                    "shader {shader_index} vector parameter {parameter_index} unexpectedly contains a TextureBase"
                )));
            }
            if parameter.vector_data.len() != usize::from(parameter.data_type) {
                return Err(YdrError::Malformed(format!(
                    "shader {shader_index} vector parameter {parameter_index} declares {} Vector4 values but stores {}",
                    parameter.data_type,
                    parameter.vector_data.len()
                )));
            }
            vector_bytes = vector_bytes
                .checked_add(checked_writer_mul(
                    parameter.vector_data.len(),
                    16,
                    "ShaderFX vector payload",
                )?)
                .ok_or_else(|| YdrError::Malformed("ShaderFX vector payload overflow".into()))?;
        }
    }
    if texture_parameters != usize::from(shader.texture_parameters_count) {
        return Err(YdrError::Malformed(format!(
            "shader {shader_index} reports {} texture parameters but contains {texture_parameters}",
            shader.texture_parameters_count
        )));
    }
    let parameter_size = descriptor_bytes
        .checked_add(vector_bytes)
        .ok_or_else(|| YdrError::Malformed("ShaderFX parameter size overflow".into()))?;
    if usize::from(shader.parameter_size) != parameter_size {
        return Err(YdrError::Malformed(format!(
            "shader {shader_index} parameterSize {} does not match descriptor/vector size {parameter_size}",
            shader.parameter_size
        )));
    }
    if shader.parameters.is_empty() {
        if shader.parameter_data_size != 0 {
            return Err(YdrError::Malformed(format!(
                "shader {shader_index} has no parameters but parameterDataSize is non-zero"
            )));
        }
        return Ok(ShaderParameterLayout {
            descriptor_bytes,
            vector_bytes,
            hash_bytes,
            block_bytes: 0,
        });
    }
    let base_size = 32usize
        .checked_add(parameter_size)
        .and_then(|value| value.checked_add(hash_bytes))
        .ok_or_else(|| YdrError::Malformed("ShaderFX base block size overflow".into()))?;
    let aligned_size = base_size
        .checked_add(15)
        .map(|value| value & !15)
        .ok_or_else(|| YdrError::Malformed("ShaderFX aligned block size overflow".into()))?;
    if usize::from(shader.parameter_data_size) < aligned_size
        || usize::from(shader.parameter_data_size) % 16 != 0
    {
        return Err(YdrError::Malformed(format!(
            "shader {shader_index} parameterDataSize {} is not a 16-byte aligned size covering the Legacy minimum {aligned_size}",
            shader.parameter_data_size
        )));
    }
    let padding = 32usize
        .checked_add(checked_writer_mul(
            usize::from(shader.parameter_data_size),
            4,
            "ShaderFX parameter data padding",
        )?)
        .ok_or_else(|| YdrError::Malformed("ShaderFX parameter padding overflow".into()))?;
    let block_bytes = parameter_size
        .checked_add(hash_bytes)
        .and_then(|value| value.checked_add(padding))
        .ok_or_else(|| YdrError::Malformed("ShaderFX parameter block size overflow".into()))?;
    if block_bytes > MAX_SHADER_PARAMETER_BLOCK_BYTES {
        return Err(YdrError::Unsupported(format!(
            "shader {shader_index} parameter block exceeds {MAX_SHADER_PARAMETER_BLOCK_BYTES} bytes"
        )));
    }
    Ok(ShaderParameterLayout {
        descriptor_bytes,
        vector_bytes,
        hash_bytes,
        block_bytes,
    })
}

fn validate_shader_for_write(index: usize, shader: &ShaderInfo) -> Result<(), YdrError> {
    if shader.parameters.len() > usize::from(u8::MAX) {
        return Err(YdrError::Unsupported(format!(
            "shader {index} parameter count exceeds the Legacy ShaderFX u8 limit"
        )));
    }
    let _ = shader_parameter_layout(index, shader)?;
    Ok(())
}

fn write_shader_group(
    arena: &mut SystemArena,
    graphics: &mut GraphicsArena,
    textures: &[PreparedTexture],
    shaders: &[ShaderInfo],
) -> Result<u64, YdrError> {
    if textures.is_empty() && shaders.is_empty() {
        return Err(YdrError::Malformed(
            "cannot create an empty ShaderGroup".into(),
        ));
    }
    if shaders.len() > MAX_SHADERS || shaders.len() > usize::from(u16::MAX) {
        return Err(YdrError::Unsupported(format!(
            "shader count {} exceeds the ShaderGroup limit",
            shaders.len()
        )));
    }
    let (group_offset, group_pointer) = arena.allocate(SHADER_GROUP_SIZE, 16, "ShaderGroup")?;
    let dictionary_pointer = if textures.is_empty() {
        0
    } else {
        write_texture_dictionary(arena, graphics, textures)?
    };
    let shader_array_pointer = if shaders.is_empty() {
        0
    } else {
        let array_bytes = checked_writer_mul(shaders.len(), 8, "ShaderFX pointer array")?;
        let (array_offset, array_pointer) =
            arena.allocate(array_bytes, 8, "ShaderFX pointer array")?;
        for (index, shader) in shaders.iter().enumerate() {
            let slot = array_offset
                .checked_add(checked_writer_mul(index, 8, "ShaderFX pointer slot")?)
                .ok_or_else(|| YdrError::Malformed("ShaderFX pointer slot overflow".into()))?;
            let shader_pointer = write_shader_fx(arena, index, shader)?;
            arena.write_pointer(slot, shader_pointer)?;
        }
        array_pointer
    };

    arena.write_pointer(group_offset + 0x08, dictionary_pointer)?;
    arena.write_pointer(group_offset + 0x10, shader_array_pointer)?;
    arena.write_u16(group_offset + 0x18, shaders.len() as u16)?;
    arena.write_u16(group_offset + 0x1A, shaders.len() as u16)?;
    Ok(group_pointer)
}

fn write_shader_fx(
    arena: &mut SystemArena,
    shader_index: usize,
    shader: &ShaderInfo,
) -> Result<u64, YdrError> {
    let layout = shader_parameter_layout(shader_index, shader)?;
    let (header_offset, header_pointer) = arena.allocate(SHADER_FX_SIZE, 16, "ShaderFX")?;
    let parameter_pointer = if shader.parameters.is_empty() {
        0
    } else {
        arena
            .allocate(layout.block_bytes, 16, "ShaderFX parameter block")?
            .1
    };

    arena.write_pointer(header_offset, parameter_pointer)?;
    arena.write_u32(header_offset + 0x08, shader.name_hash)?;
    arena.write_u32(header_offset + 0x0C, shader.unknown_0c)?;
    arena.write_u8(header_offset + 0x10, shader.parameters.len() as u8)?;
    arena.write_u8(header_offset + 0x11, shader.render_bucket)?;
    arena.write_u16(header_offset + 0x12, shader.unknown_12)?;
    arena.write_u16(header_offset + 0x14, shader.parameter_size)?;
    arena.write_u16(header_offset + 0x16, shader.parameter_data_size)?;
    arena.write_u32(header_offset + 0x18, shader.file_hash)?;
    arena.write_u32(header_offset + 0x1C, shader.unknown_1c)?;
    arena.write_u32(header_offset + 0x20, shader.render_bucket_mask)?;
    arena.write_u16(header_offset + 0x24, shader.unknown_24)?;
    arena.write_u8(header_offset + 0x26, shader.unknown_26)?;
    arena.write_u8(header_offset + 0x27, shader.texture_parameters_count)?;
    arena.write_u64(header_offset + 0x28, shader.unknown_28)?;

    if shader.parameters.is_empty() {
        return Ok(header_pointer);
    }

    let parameter_offset = usize::try_from(parameter_pointer - SYSTEM_BASE).map_err(|_| {
        YdrError::Malformed("ShaderFX parameter pointer precedes system base".into())
    })?;
    let mut vector_cursor = 0usize;
    for (parameter_index, parameter) in shader.parameters.iter().enumerate() {
        let descriptor_offset = parameter_offset
            .checked_add(checked_writer_mul(
                parameter_index,
                SHADER_PARAMETER_DESCRIPTOR_SIZE,
                "ShaderFX descriptor index",
            )?)
            .ok_or_else(|| YdrError::Malformed("ShaderFX descriptor offset overflow".into()))?;
        arena.write_u8(descriptor_offset, parameter.data_type)?;
        arena.write_u8(descriptor_offset + 0x01, parameter.unknown_1)?;
        arena.write_u16(descriptor_offset + 0x02, parameter.unknown_2)?;
        arena.write_u32(descriptor_offset + 0x04, parameter.unknown_4)?;

        let data_pointer = if parameter.data_type == 0 {
            match &parameter.texture_base {
                Some(base) => write_texture_base(arena, base)?,
                None => 0,
            }
        } else {
            let vector_offset = layout
                .descriptor_bytes
                .checked_add(vector_cursor)
                .ok_or_else(|| YdrError::Malformed("ShaderFX vector offset overflow".into()))?;
            let vector_pointer = parameter_pointer
                .checked_add(vector_offset as u64)
                .ok_or_else(|| YdrError::Malformed("ShaderFX vector pointer overflow".into()))?;
            for vector in &parameter.vector_data {
                for (component_index, component) in vector.iter().copied().enumerate() {
                    let offset =
                        vector_cursor
                            .checked_add(component_index * 4)
                            .ok_or_else(|| {
                                YdrError::Malformed("ShaderFX vector component overflow".into())
                            })?;
                    arena.write_u32(
                        parameter_offset + layout.descriptor_bytes + offset,
                        component,
                    )?;
                }
                vector_cursor = vector_cursor
                    .checked_add(16)
                    .ok_or_else(|| YdrError::Malformed("ShaderFX vector cursor overflow".into()))?;
            }
            vector_pointer
        };
        arena.write_pointer(descriptor_offset + 0x08, data_pointer)?;
    }

    let expected_vector_bytes = layout.vector_bytes;
    if vector_cursor != expected_vector_bytes {
        return Err(YdrError::Malformed(format!(
            "shader {shader_index} wrote {vector_cursor} vector bytes, expected {expected_vector_bytes}"
        )));
    }
    let hash_offset = layout
        .descriptor_bytes
        .checked_add(layout.vector_bytes)
        .ok_or_else(|| YdrError::Malformed("ShaderFX hash offset overflow".into()))?;
    for (index, parameter) in shader.parameters.iter().enumerate() {
        let offset = parameter_offset
            .checked_add(hash_offset)
            .and_then(|value| {
                checked_writer_mul(index, 4, "ShaderFX parameter hash index")
                    .ok()
                    .and_then(|delta| value.checked_add(delta))
            })
            .ok_or_else(|| YdrError::Malformed("ShaderFX hash address overflow".into()))?;
        arena.write_u32(offset, parameter.parameter_hash)?;
    }
    let _ = layout.hash_bytes;
    Ok(header_pointer)
}

fn write_texture_base(arena: &mut SystemArena, texture: &TextureBaseInfo) -> Result<u64, YdrError> {
    let (offset, pointer) = arena.allocate(LEGACY_TEXTURE_BASE_SIZE, 16, "ShaderFX TextureBase")?;
    arena.write_u32(offset, texture.vft)?;
    arena.write_u32(offset + 0x04, texture.unknown_4)?;
    arena.write_u32(offset + 0x08, texture.unknown_8)?;
    arena.write_u32(offset + 0x0C, texture.unknown_c)?;
    arena.write_u32(offset + 0x10, texture.unknown_10)?;
    arena.write_u32(offset + 0x14, texture.unknown_14)?;
    arena.write_u32(offset + 0x18, texture.unknown_18)?;
    arena.write_u32(offset + 0x1C, texture.unknown_1c)?;
    arena.write_u32(offset + 0x20, texture.unknown_20)?;
    arena.write_u32(offset + 0x24, texture.unknown_24)?;
    if let Some(name) = &texture.name {
        if name.is_empty() || name.len() > MAX_TEXTURE_NAME_BYTES || name.as_bytes().contains(&0) {
            return Err(YdrError::Malformed(
                "ShaderFX TextureBase name is empty, too long or contains NUL".into(),
            ));
        }
        let (name_offset, name_pointer) =
            arena.allocate(name.len() + 1, 1, "ShaderFX TextureBase name")?;
        arena.write_bytes(name_offset, name.as_bytes())?;
        arena.write_u8(name_offset + name.len(), 0)?;
        arena.write_pointer(offset + 0x28, name_pointer)?;
    }
    arena.write_u16(offset + 0x30, texture.unknown_30)?;
    arena.write_u16(offset + 0x32, texture.unknown_32)?;
    arena.write_u32(offset + 0x34, texture.unknown_34)?;
    arena.write_u32(offset + 0x38, texture.unknown_38)?;
    arena.write_u32(offset + 0x3C, texture.unknown_3c)?;
    arena.write_u32(offset + 0x40, texture.usage_data)?;
    arena.write_u32(offset + 0x44, texture.unknown_44)?;
    arena.write_u32(offset + 0x48, texture.extra_flags)?;
    arena.write_u32(offset + 0x4C, texture.unknown_4c)?;
    Ok(pointer)
}

fn write_texture_dictionary(
    arena: &mut SystemArena,
    graphics: &mut GraphicsArena,
    textures: &[PreparedTexture],
) -> Result<u64, YdrError> {
    if textures.is_empty() {
        return Err(YdrError::Malformed(
            "cannot create an empty embedded TextureDictionary".into(),
        ));
    }
    if textures.len() > MAX_TEXTURES || textures.len() > usize::from(u16::MAX) {
        return Err(YdrError::Unsupported(format!(
            "texture count {} exceeds the TextureDictionary limit",
            textures.len()
        )));
    }
    let count = textures.len() as u16;
    let (dictionary_offset, dictionary_pointer) =
        arena.allocate(TEXTURE_DICTIONARY_SIZE, 16, "TextureDictionary")?;
    let hash_bytes = checked_writer_mul(textures.len(), 4, "texture hash array")?;
    let pointer_bytes = checked_writer_mul(textures.len(), 8, "texture pointer array")?;
    let descriptor_bytes = checked_writer_mul(
        textures.len(),
        LEGACY_TEXTURE_DESCRIPTOR_SIZE,
        "texture descriptor array",
    )?;
    let (hash_offset, hash_pointer) = arena.allocate(hash_bytes, 4, "texture hash array")?;
    let (pointer_offset, pointer_pointer) =
        arena.allocate(pointer_bytes, 8, "texture pointer array")?;
    let (descriptor_offset, _) =
        arena.allocate(descriptor_bytes, 16, "texture descriptor array")?;

    arena.write_pointer(dictionary_offset + 0x20, hash_pointer)?;
    arena.write_u16(dictionary_offset + 0x28, count)?;
    arena.write_u16(dictionary_offset + 0x2A, count)?;
    arena.write_pointer(dictionary_offset + 0x30, pointer_pointer)?;
    arena.write_u16(dictionary_offset + 0x38, count)?;
    arena.write_u16(dictionary_offset + 0x3A, count)?;

    for (index, texture) in textures.iter().enumerate() {
        let hash_slot = hash_offset
            .checked_add(checked_writer_mul(index, 4, "texture hash slot")?)
            .ok_or_else(|| YdrError::Malformed("texture hash slot overflow".into()))?;
        let pointer_slot = pointer_offset
            .checked_add(checked_writer_mul(index, 8, "texture pointer slot")?)
            .ok_or_else(|| YdrError::Malformed("texture pointer slot overflow".into()))?;
        let descriptor_slot = descriptor_offset
            .checked_add(checked_writer_mul(
                index,
                LEGACY_TEXTURE_DESCRIPTOR_SIZE,
                "texture descriptor slot",
            )?)
            .ok_or_else(|| YdrError::Malformed("texture descriptor slot overflow".into()))?;
        let descriptor_pointer = SYSTEM_BASE
            .checked_add(descriptor_slot as u64)
            .ok_or_else(|| YdrError::Malformed("texture descriptor pointer overflow".into()))?;
        arena.write_u32(hash_slot, texture.name_hash)?;
        arena.write_pointer(pointer_slot, descriptor_pointer)?;

        let (name_offset, name_pointer) =
            arena.allocate(texture.name.len() + 1, 1, "texture name")?;
        arena.write_bytes(name_offset, texture.name.as_bytes())?;
        arena.write_u8(name_offset + texture.name.len(), 0)?;
        arena.write_pointer(descriptor_slot + 0x28, name_pointer)?;
        // Match the Legacy TextureBase defaults used by CodeWalker when it
        // constructs a texture descriptor from XML (VFT remains zero).
        arena.write_u32(descriptor_slot + 0x04, 1)?;
        arena.write_u16(descriptor_slot + 0x30, 1)?;
        arena.write_u16(descriptor_slot + 0x32, 0)?;
        arena.write_u16(descriptor_slot + 0x50, texture.encoded.width)?;
        arena.write_u16(descriptor_slot + 0x52, texture.encoded.height)?;
        arena.write_u16(descriptor_slot + 0x54, 1)?;
        arena.write_u16(descriptor_slot + 0x56, texture.encoded.stride)?;
        arena.write_u32(descriptor_slot + 0x58, texture.encoded.format.raw())?;
        arena.write_u8(descriptor_slot + 0x5D, texture.encoded.levels)?;

        let (data_offset, data_pointer) =
            graphics.allocate(texture.encoded.data.len(), 16, "embedded texture payload")?;
        graphics.write_bytes(data_offset, &texture.encoded.data)?;
        arena.write_pointer(descriptor_slot + 0x70, data_pointer)?;
    }

    Ok(dictionary_pointer)
}

fn prepare_geometry(index: usize, input: &YdrGeometryInput) -> Result<PreparedGeometry, YdrError> {
    let vertex_count = input.positions.len();
    if vertex_count == 0 || vertex_count > MAX_WRITER_VERTEX_COUNT {
        return Err(YdrError::Unsupported(format!(
            "geometry {index} vertex count {vertex_count} is outside 1..={MAX_WRITER_VERTEX_COUNT}; YDR v165 geometry stores this count as u16"
        )));
    }
    if input.indices.is_empty()
        || input.indices.len() % 3 != 0
        || input.indices.len() > MAX_INDICES_PER_GEOMETRY
    {
        return Err(YdrError::Malformed(format!(
            "geometry {index} must contain a non-empty triangle-list index buffer within the safety limit"
        )));
    }
    if let Some(normals) = &input.normals {
        if normals.len() != vertex_count {
            return Err(YdrError::Malformed(format!(
                "geometry {index} has {} normals for {vertex_count} vertices",
                normals.len()
            )));
        }
    }
    if let Some(colors0) = &input.colors0 {
        if colors0.len() != vertex_count {
            return Err(YdrError::Malformed(format!(
                "geometry {index} has {} COLOR_0 values for {vertex_count} vertices",
                colors0.len()
            )));
        }
    }
    if let Some(uv0) = &input.uv0 {
        if uv0.len() != vertex_count {
            return Err(YdrError::Malformed(format!(
                "geometry {index} has {} UV0 values for {vertex_count} vertices",
                uv0.len()
            )));
        }
    }

    if let Some(tangents) = &input.tangents {
        if tangents.len() != vertex_count {
            return Err(YdrError::Malformed(format!(
                "geometry {index} has {} TANGENT values for {vertex_count} vertices",
                tangents.len()
            )));
        }
    }

    if let Some(uv1) = &input.uv1 {
        if uv1.len() != vertex_count {
            return Err(YdrError::Malformed(format!(
                "geometry {index} has {} UV1 values for {vertex_count} vertices",
                uv1.len()
            )));
        }
    }

    let mut bounds_min = input.positions[0];
    let mut bounds_max = input.positions[0];
    for (vertex_index, position) in input.positions.iter().enumerate() {
        validate_vec3(*position, "POSITION", index, vertex_index)?;
        for axis in 0..3 {
            bounds_min[axis] = bounds_min[axis].min(position[axis]);
            bounds_max[axis] = bounds_max[axis].max(position[axis]);
        }
    }
    if let Some(normals) = &input.normals {
        for (vertex_index, normal) in normals.iter().enumerate() {
            validate_vec3(*normal, "NORMAL", index, vertex_index)?;
        }
    }
    if let Some(colors0) = &input.colors0 {
        for (vertex_index, color) in colors0.iter().enumerate() {
            validate_vec4(*color, "COLOR_0", index, vertex_index)?;
        }
    }
    if let Some(tangents) = &input.tangents {
        for (vertex_index, tangent) in tangents.iter().enumerate() {
            validate_vec4(*tangent, "TANGENT", index, vertex_index)?;
        }
    }
    if let Some(uv0) = &input.uv0 {
        for (vertex_index, uv) in uv0.iter().enumerate() {
            validate_vec2(*uv, "TEXCOORD_0", index, vertex_index)?;
        }
    }

    if let Some(uv1) = &input.uv1 {
        for (vertex_index, uv) in uv1.iter().enumerate() {
            validate_vec2(*uv, "TEXCOORD_1", index, vertex_index)?;
        }
    }

    let mut declaration_flags = 1u32;
    let mut declaration_types = 6u64;
    let mut declaration_components = 1u8;
    let mut stride = 12usize;
    if input.normals.is_some() {
        declaration_flags |= 1 << 3;
        declaration_types |= 6u64 << 12;
        declaration_components += 1;
        stride += 12;
    }
    // CodeWalker's stock model layouts require POSITION, NORMAL, COLOR_0 and
    // TEXCOORD_0. COLOR_0 is a packed RGBA8 UNorm component (4 bytes), not Float4.
    declaration_flags |= 1 << 4;
    declaration_types |= 9u64 << 16;
    declaration_components += 1;
    stride += 4;
    if input.uv0.is_some() {
        declaration_flags |= 1 << 6;
        declaration_types |= 5u64 << 24;
        declaration_components += 1;
        stride += 8;
    }
    if input.uv1.is_some() {
        declaration_flags |= 1 << 7;
        declaration_types |= 5u64 << 28;
        declaration_components += 1;
        stride += 8;
    }
    if input.tangents.is_some() {
        declaration_flags |= 1 << 14;
        declaration_types |= 7u64 << 56;
        declaration_components += 1;
        stride += 16;
    }
    let stride_u16 = u16::try_from(stride)
        .map_err(|_| YdrError::Unsupported("vertex stride exceeds u16".into()))?;
    let vertex_bytes_len = checked_writer_mul(vertex_count, stride, "vertex data")?;
    if vertex_bytes_len > MAX_VERTEX_BYTES {
        return Err(YdrError::Unsupported(format!(
            "geometry {index} vertex data exceeds {MAX_VERTEX_BYTES} bytes"
        )));
    }
    let mut vertex_data = Vec::with_capacity(vertex_bytes_len);
    for vertex_index in 0..vertex_count {
        append_vec3(&mut vertex_data, input.positions[vertex_index]);
        if let Some(normals) = &input.normals {
            append_vec3(&mut vertex_data, normals[vertex_index]);
        }
        let color0 = input
            .colors0
            .as_ref()
            .map_or([1.0, 1.0, 1.0, 1.0], |colors| colors[vertex_index]);
        append_color_rgba8(&mut vertex_data, color0);
        if let Some(uv0) = &input.uv0 {
            append_vec2(&mut vertex_data, uv0[vertex_index]);
        }
        if let Some(uv1) = &input.uv1 {
            append_vec2(&mut vertex_data, uv1[vertex_index]);
        }
        if let Some(tangents) = &input.tangents {
            append_vec4(&mut vertex_data, tangents[vertex_index]);
        }
    }

    let mut index_data = Vec::with_capacity(checked_writer_mul(
        input.indices.len(),
        2,
        "16-bit index data",
    )?);
    for (index_position, value) in input.indices.iter().copied().enumerate() {
        if value as usize >= vertex_count {
            return Err(YdrError::Malformed(format!(
                "geometry {index} index {index_position} references vertex {value}, but only {vertex_count} vertices exist"
            )));
        }
        let value = u16::try_from(value).map_err(|_| {
            YdrError::Unsupported(format!(
                "geometry {index} index {index_position} value {value} exceeds the YDR 16-bit index range"
            ))
        })?;
        index_data.extend_from_slice(&value.to_le_bytes());
    }

    Ok(PreparedGeometry {
        vertex_count,
        index_count: input.indices.len(),
        stride: stride_u16,
        declaration_flags,
        declaration_components,
        declaration_types,
        vertex_data,
        index_data,
        bounds: Bounds {
            min: bounds_min,
            max: bounds_max,
        },
        shader_index: input.shader_index,
    })
}

fn validate_geometry_only_output(
    bytes: &[u8],
    expected_name: &str,
    expected: &[PreparedGeometry],
    expected_textures: &[PreparedTexture],
    expected_shaders: &[ShaderInfo],
    expected_bounds: Bounds,
) -> Result<(), YdrError> {
    let document = YdrDocument::from_bytes(bytes)?;
    if document.model.shaders != expected_shaders {
        return Err(YdrError::Malformed(
            "ShaderFX templates or parameter data changed during semantic reopen".into(),
        ));
    }
    if document.model.name.as_deref() != Some(expected_name) {
        return Err(YdrError::Malformed(
            "geometry-only YDR name changed during semantic reopen".into(),
        ));
    }
    if document.model.primitives.len() != expected.len() {
        return Err(YdrError::Malformed(
            "geometry-only YDR primitive count changed during semantic reopen".into(),
        ));
    }
    if document.model.bounds.min != expected_bounds.min
        || document.model.bounds.max != expected_bounds.max
    {
        return Err(YdrError::Malformed(
            "geometry-only YDR drawable bounds changed during semantic reopen".into(),
        ));
    }
    for (index, (actual, source)) in document.model.primitives.iter().zip(expected).enumerate() {
        if actual.positions.len() != source.vertex_count
            || actual.indices.len() != source.index_count
            || actual.indices.iter().any(|value| *value > u16::MAX as u32)
            || actual.normals.is_some() != (source.declaration_flags & (1 << 3) != 0)
            || actual.colors0.is_some() != (source.declaration_flags & (1 << 4) != 0)
            || actual.uv0.is_some() != (source.declaration_flags & (1 << 6) != 0)
            || actual.uv1.is_some() != (source.declaration_flags & (1 << 7) != 0)
            || actual.tangents.is_some() != (source.declaration_flags & (1 << 14) != 0)
            || actual.shader_index != source.shader_index
        {
            return Err(YdrError::Malformed(format!(
                "geometry-only YDR primitive {index} counts changed during semantic reopen"
            )));
        }
        for vertex in 0..source.vertex_count {
            let start = vertex * usize::from(source.stride);
            let mut cursor = start;
            let expected_position = read_vec3_from_bytes(&source.vertex_data, &mut cursor)?;
            if actual.positions[vertex] != expected_position {
                return Err(YdrError::Malformed(format!(
                    "geometry-only YDR primitive {index} position {vertex} changed during semantic reopen"
                )));
            }
            if let Some(normals) = &actual.normals {
                let expected_normal = read_vec3_from_bytes(&source.vertex_data, &mut cursor)?;
                if normals[vertex] != expected_normal {
                    return Err(YdrError::Malformed(format!(
                        "geometry-only YDR primitive {index} normal {vertex} changed during semantic reopen"
                    )));
                }
            }
            if let Some(colors0) = &actual.colors0 {
                let expected_color = read_color_rgba8_from_bytes(&source.vertex_data, &mut cursor)?;
                if colors0[vertex] != expected_color {
                    return Err(YdrError::Malformed(format!(
                        "geometry-only YDR primitive {index} COLOR_0 vertex {vertex} changed during semantic reopen"
                    )));
                }
            }
            if let Some(uv0) = &actual.uv0 {
                let expected_uv = read_vec2_from_bytes(&source.vertex_data, &mut cursor)?;
                if uv0[vertex] != expected_uv {
                    return Err(YdrError::Malformed(format!(
                        "geometry-only YDR primitive {index} UV0 {vertex} changed during semantic reopen"
                    )));
                }
            }
            if let Some(uv1) = &actual.uv1 {
                let expected_uv1 = read_vec2_from_bytes(&source.vertex_data, &mut cursor)?;
                if uv1[vertex] != expected_uv1 {
                    return Err(YdrError::Malformed(format!(
                        "geometry-only YDR primitive {index} UV1 vertex {vertex} changed during semantic reopen"
                    )));
                }
            }
            if let Some(tangents) = &actual.tangents {
                let expected_tangent = read_vec4_from_bytes(&source.vertex_data, &mut cursor)?;
                if tangents[vertex] != expected_tangent {
                    return Err(YdrError::Malformed(format!(
                        "geometry-only YDR primitive {index} TANGENT vertex {vertex} changed during semantic reopen"
                    )));
                }
            }
        }
        let expected_indices = source
            .index_data
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]) as u32)
            .collect::<Vec<_>>();
        if actual.indices != expected_indices {
            return Err(YdrError::Malformed(format!(
                "geometry-only YDR primitive {index} indices changed during semantic reopen"
            )));
        }
    }

    match (&document.embedded_textures, expected_textures.is_empty()) {
        (None, true) => {}
        (Some(_), true) => {
            return Err(YdrError::Malformed(
                "unexpected embedded TextureDictionary after semantic reopen".into(),
            ))
        }
        (None, false) => {
            return Err(YdrError::Malformed(
                "embedded TextureDictionary is missing after semantic reopen".into(),
            ))
        }
        (Some(dictionary), false) => {
            if dictionary.textures().len() != expected_textures.len() {
                return Err(YdrError::Malformed(
                    "embedded TextureDictionary texture count changed during semantic reopen"
                        .into(),
                ));
            }
            for (index, (actual, source)) in dictionary
                .textures()
                .iter()
                .zip(expected_textures)
                .enumerate()
            {
                if actual.name.as_deref() != Some(source.name.as_str())
                    || actual.name_hash != source.name_hash
                    || actual.width != source.encoded.width
                    || actual.height != source.encoded.height
                    || actual.depth != 1
                    || actual.stride != source.encoded.stride
                    || actual.format != source.encoded.format
                    || actual.levels != source.encoded.levels
                {
                    return Err(YdrError::Malformed(format!(
                        "embedded texture {index} metadata changed during semantic reopen"
                    )));
                }
                match source.encoded.format {
                    TextureFormat::Bc1 | TextureFormat::Bc3 => {
                        let mip_chain = dictionary.compressed_mip_chain(index)?;
                        let payload = mip_chain
                            .mipmaps
                            .iter()
                            .flat_map(|mip| mip.data.iter().copied())
                            .collect::<Vec<_>>();
                        if payload != source.encoded.data {
                            return Err(YdrError::Malformed(format!(
                                "embedded texture {index} compressed mip payload changed during semantic reopen"
                            )));
                        }
                    }
                    TextureFormat::Rgba8 => {
                        let mut width = source.encoded.width;
                        let mut height = source.encoded.height;
                        let mut cursor = 0usize;
                        for mip_index in 0..usize::from(source.encoded.levels) {
                            let decoded = dictionary.decode_mip(index, mip_index)?;
                            let byte_len = usize::from(width)
                                .checked_mul(usize::from(height))
                                .and_then(|pixels| pixels.checked_mul(4))
                                .ok_or_else(|| {
                                    YdrError::Malformed(
                                        "embedded RGBA mip byte length overflow".into(),
                                    )
                                })?;
                            let end = cursor.checked_add(byte_len).ok_or_else(|| {
                                YdrError::Malformed("embedded RGBA mip cursor overflow".into())
                            })?;
                            if decoded.width != width
                                || decoded.height != height
                                || source.encoded.data.get(cursor..end)
                                    != Some(decoded.rgba.as_slice())
                            {
                                return Err(YdrError::Malformed(format!(
                                    "embedded texture {index} RGBA mip {mip_index} changed during semantic reopen"
                                )));
                            }
                            cursor = end;
                            width = (width / 2).max(1);
                            height = (height / 2).max(1);
                        }
                        if cursor != source.encoded.data.len() {
                            return Err(YdrError::Malformed(format!(
                                "embedded texture {index} RGBA mip payload has trailing bytes"
                            )));
                        }
                    }
                    format => {
                        return Err(YdrError::Unsupported(format!(
                            "semantic reopen does not validate embedded texture format {}",
                            format.normalized_name()
                        )))
                    }
                }
            }
        }
    }

    // The edit-session audit also checks per-model and per-geometry bounds-table
    // pointers and disjoint vertex storage, independently of the render reader.
    let _audit = YdrEditSession::from_bytes(bytes)?;
    Ok(())
}

fn write_drawable_bounds(
    arena: &mut SystemArena,
    root_offset: usize,
    bounds: Bounds,
) -> Result<(), YdrError> {
    let center = [
        ((bounds.min[0] as f64 + bounds.max[0] as f64) * 0.5) as f32,
        ((bounds.min[1] as f64 + bounds.max[1] as f64) * 0.5) as f32,
        ((bounds.min[2] as f64 + bounds.max[2] as f64) * 0.5) as f32,
    ];
    let mut radius_squared = 0.0f64;
    for corner in [
        [bounds.min[0], bounds.min[1], bounds.min[2]],
        [bounds.max[0], bounds.min[1], bounds.min[2]],
        [bounds.min[0], bounds.max[1], bounds.min[2]],
        [bounds.min[0], bounds.min[1], bounds.max[2]],
        [bounds.max[0], bounds.max[1], bounds.min[2]],
        [bounds.max[0], bounds.min[1], bounds.max[2]],
        [bounds.min[0], bounds.max[1], bounds.max[2]],
        [bounds.max[0], bounds.max[1], bounds.max[2]],
    ] {
        let dx = corner[0] as f64 - center[0] as f64;
        let dy = corner[1] as f64 - center[1] as f64;
        let dz = corner[2] as f64 - center[2] as f64;
        radius_squared = radius_squared.max(dx * dx + dy * dy + dz * dz);
    }
    let radius = radius_squared.sqrt() as f32;
    for (offset, value) in [
        (0x20, center[0]),
        (0x24, center[1]),
        (0x28, center[2]),
        (0x2C, radius),
        (0x30, bounds.min[0]),
        (0x34, bounds.min[1]),
        (0x38, bounds.min[2]),
        (0x40, bounds.max[0]),
        (0x44, bounds.max[1]),
        (0x48, bounds.max[2]),
    ] {
        arena.write_f32(root_offset + offset, value)?;
    }
    Ok(())
}

fn write_bounds_pair(
    arena: &mut SystemArena,
    offset: usize,
    min: [f32; 3],
    max: [f32; 3],
) -> Result<(), YdrError> {
    for axis in 0..3 {
        arena.write_f32(offset + axis * 4, min[axis])?;
        arena.write_f32(offset + 0x10 + axis * 4, max[axis])?;
    }
    arena.write_f32(offset + 0x0C, 0.0)?;
    arena.write_f32(offset + 0x1C, 0.0)?;
    Ok(())
}

fn combine_bounds(geometries: &[PreparedGeometry]) -> Result<Bounds, YdrError> {
    let first = geometries
        .first()
        .ok_or_else(|| YdrError::Malformed("no geometry bounds to combine".into()))?;
    let mut result = first.bounds;
    for geometry in &geometries[1..] {
        for axis in 0..3 {
            result.min[axis] = result.min[axis].min(geometry.bounds.min[axis]);
            result.max[axis] = result.max[axis].max(geometry.bounds.max[axis]);
        }
    }
    Ok(result)
}

fn validate_name(name: &str) -> Result<(), YdrError> {
    if name.is_empty() || name.len() > MAX_NAME_LENGTH || name.as_bytes().contains(&0) {
        return Err(YdrError::Malformed(format!(
            "Drawable name must contain 1..={MAX_NAME_LENGTH} non-NUL UTF-8 bytes"
        )));
    }
    Ok(())
}

fn validate_vec3(
    value: [f32; 3],
    semantic: &str,
    geometry: usize,
    vertex: usize,
) -> Result<(), YdrError> {
    if value.iter().any(|component| !component.is_finite()) {
        return Err(YdrError::Malformed(format!(
            "geometry {geometry} {semantic} vertex {vertex} contains a non-finite value"
        )));
    }
    Ok(())
}

fn validate_vec2(
    value: [f32; 2],
    semantic: &str,
    geometry: usize,
    vertex: usize,
) -> Result<(), YdrError> {
    if value.iter().any(|component| !component.is_finite()) {
        return Err(YdrError::Malformed(format!(
            "geometry {geometry} {semantic} vertex {vertex} contains a non-finite value"
        )));
    }
    Ok(())
}

fn validate_vec4(
    value: [f32; 4],
    semantic: &str,
    geometry: usize,
    vertex: usize,
) -> Result<(), YdrError> {
    if value.iter().any(|component| !component.is_finite()) {
        return Err(YdrError::Malformed(format!(
            "geometry {geometry} {semantic} vertex {vertex} contains a non-finite value"
        )));
    }
    Ok(())
}

fn append_vec3(output: &mut Vec<u8>, value: [f32; 3]) {
    for component in value {
        output.extend_from_slice(&component.to_le_bytes());
    }
}

fn append_vec4(output: &mut Vec<u8>, value: [f32; 4]) {
    for component in value {
        output.extend_from_slice(&component.to_le_bytes());
    }
}

fn append_color_rgba8(output: &mut Vec<u8>, value: [f32; 4]) {
    for component in value {
        output.push((component.clamp(0.0, 1.0) * 255.0).round() as u8);
    }
}

fn append_vec2(output: &mut Vec<u8>, value: [f32; 2]) {
    for component in value {
        output.extend_from_slice(&component.to_le_bytes());
    }
}

fn read_vec3_from_bytes(bytes: &[u8], cursor: &mut usize) -> Result<[f32; 3], YdrError> {
    let mut result = [0.0; 3];
    for value in &mut result {
        let end = cursor
            .checked_add(4)
            .ok_or_else(|| YdrError::Malformed("vertex read cursor overflow".into()))?;
        let slice = bytes
            .get(*cursor..end)
            .ok_or_else(|| YdrError::Malformed("serialized vertex data is truncated".into()))?;
        *value = f32::from_le_bytes(slice.try_into().expect("four-byte float"));
        *cursor = end;
    }
    Ok(result)
}

fn read_color_rgba8_from_bytes(bytes: &[u8], cursor: &mut usize) -> Result<[f32; 4], YdrError> {
    let end = cursor
        .checked_add(4)
        .ok_or_else(|| YdrError::Malformed("vertex read cursor overflow".into()))?;
    let slice = bytes
        .get(*cursor..end)
        .ok_or_else(|| YdrError::Malformed("serialized COLOR_0 data is truncated".into()))?;
    *cursor = end;
    Ok([
        f32::from(slice[0]) / 255.0,
        f32::from(slice[1]) / 255.0,
        f32::from(slice[2]) / 255.0,
        f32::from(slice[3]) / 255.0,
    ])
}

fn read_vec4_from_bytes(bytes: &[u8], cursor: &mut usize) -> Result<[f32; 4], YdrError> {
    let mut result = [0.0; 4];
    for value in &mut result {
        let end = cursor
            .checked_add(4)
            .ok_or_else(|| YdrError::Malformed("vertex read cursor overflow".into()))?;
        let slice = bytes
            .get(*cursor..end)
            .ok_or_else(|| YdrError::Malformed("serialized vertex data is truncated".into()))?;
        *value = f32::from_le_bytes(slice.try_into().expect("four-byte float"));
        *cursor = end;
    }
    Ok(result)
}

fn read_vec2_from_bytes(bytes: &[u8], cursor: &mut usize) -> Result<[f32; 2], YdrError> {
    let mut result = [0.0; 2];
    for value in &mut result {
        let end = cursor
            .checked_add(4)
            .ok_or_else(|| YdrError::Malformed("vertex read cursor overflow".into()))?;
        let slice = bytes
            .get(*cursor..end)
            .ok_or_else(|| YdrError::Malformed("serialized vertex data is truncated".into()))?;
        *value = f32::from_le_bytes(slice.try_into().expect("four-byte float"));
        *cursor = end;
    }
    Ok(result)
}

fn checked_writer_mul(left: usize, right: usize, label: &str) -> Result<usize, YdrError> {
    left.checked_mul(right)
        .ok_or_else(|| YdrError::Malformed(format!("{label} size overflows usize")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle() -> YdrGeometryInput {
        YdrGeometryInput {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            normals: Some(vec![[0.0, 1.0, 0.0]; 3]),
            colors0: Some(vec![[1.0, 0.5, 0.25, 1.0]; 3]),
            uv0: Some(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]),
            uv1: Some(vec![[0.25, 0.25], [0.75, 0.25], [0.25, 0.75]]),
            tangents: Some(vec![[1.0, 0.0, 0.0, 1.0]; 3]),
            indices: vec![0, 1, 2],
            shader_index: None,
        }
    }

    fn sample_texture_base(name: &str) -> TextureBaseInfo {
        TextureBaseInfo {
            vft: 0x4061_87F8,
            unknown_4: 1,
            unknown_8: 0,
            unknown_c: 0,
            unknown_10: 0,
            unknown_14: 0,
            unknown_18: 0,
            unknown_1c: 0,
            unknown_20: 0,
            unknown_24: 0,
            unknown_30: 1,
            unknown_32: 2,
            unknown_34: 0,
            unknown_38: 0,
            unknown_3c: 0,
            usage_data: 0,
            unknown_44: 0,
            extra_flags: 0,
            unknown_4c: 0,
            name: Some(name.to_owned()),
        }
    }

    fn sample_texture_parameter(
        parameter_hash: u32,
        unknown_1: u8,
        name: &str,
    ) -> ShaderParameterInfo {
        ShaderParameterInfo {
            parameter_hash,
            data_type: 0,
            unknown_1,
            unknown_2: 0,
            unknown_4: 0,
            texture_name: Some(name.to_owned()),
            texture_base: Some(sample_texture_base(name)),
            vector_data: Vec::new(),
        }
    }

    fn sample_vector_parameter(
        parameter_hash: u32,
        unknown_1: u8,
        value: [f32; 4],
    ) -> ShaderParameterInfo {
        ShaderParameterInfo {
            parameter_hash,
            data_type: 1,
            unknown_1,
            unknown_2: 0,
            unknown_4: 0,
            texture_name: None,
            texture_base: None,
            vector_data: vec![value.map(f32::to_bits)],
        }
    }

    fn sample_normal_shader() -> ShaderInfo {
        let parameters = vec![
            sample_texture_parameter(0xF1FE_2B71, 0, "pumpkin_albedo"),
            sample_texture_parameter(0x46B7_C64F, 2, "pumpkin_normal"),
            sample_texture_parameter(0x6087_99C6, 3, "pumpkin_albedo"),
            sample_vector_parameter(0xE943_7406, 167, [1.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0x4620_A35D, 166, [0.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0x32DD_9FF5, 165, [0.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0xF671_2B81, 164, [1.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0xFF11_711D, 163, [1.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0xF418_334F, 162, [0.25, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0x8774_4680, 161, [20.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0x27B9_B2FA, 160, [0.99, 0.0, 0.0, 0.0]),
        ];
        ShaderInfo {
            name_hash: 0x38DD_00DF,
            unknown_0c: 0,
            file_hash: 0x14A7_80FD,
            unknown_12: 0x8000,
            unknown_1c: 0,
            unknown_24: 0,
            unknown_26: 0,
            unknown_28: 0,
            render_bucket: 0,
            parameter_size: 304,
            parameter_data_size: 384,
            render_bucket_mask: 0x0000_FF01,
            texture_parameters_count: 3,
            texture_references: parameters
                .iter()
                .filter(|parameter| parameter.data_type == 0)
                .map(|parameter| ShaderTextureReference {
                    parameter_hash: parameter.parameter_hash,
                    texture_name: parameter.texture_name.clone(),
                })
                .collect(),
            parameters,
        }
    }

    fn sample_spec_shader() -> ShaderInfo {
        let parameters = vec![
            sample_texture_parameter(0xF1FE_2B71, 0, "pole_albedo"),
            sample_texture_parameter(0x6087_99C6, 2, "pole_spec"),
            sample_vector_parameter(0xE943_7406, 168, [1.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0x4620_A35D, 167, [0.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0x32DD_9FF5, 166, [1.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0xFF11_711D, 165, [1.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0xF418_334F, 164, [0.5, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0x8774_4680, 163, [100.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0x27B9_B2FA, 162, [0.75, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0xBA54_C190, 161, [0.0, 1.0, 0.0, 0.0]),
            sample_vector_parameter(0xD79B_FC1E, 160, [1.0, 0.0, 0.0, 0.0]),
        ];
        ShaderInfo {
            name_hash: 0x7204_A391,
            unknown_0c: 0,
            file_hash: 0x85F5_0BE9,
            unknown_12: 0x8000,
            unknown_1c: 0,
            unknown_24: 0,
            unknown_26: 0,
            unknown_28: 0,
            render_bucket: 0,
            parameter_size: 320,
            parameter_data_size: 400,
            render_bucket_mask: 0x0000_FF01,
            texture_parameters_count: 2,
            texture_references: parameters
                .iter()
                .filter(|parameter| parameter.data_type == 0)
                .map(|parameter| ShaderTextureReference {
                    parameter_hash: parameter.parameter_hash,
                    texture_name: parameter.texture_name.clone(),
                })
                .collect(),
            parameters,
        }
    }

    fn sample_palette_texture_parameter(
        parameter_hash: u32,
        unknown_1: u8,
        name: &str,
        usage_data: u32,
    ) -> ShaderParameterInfo {
        let mut parameter = sample_texture_parameter(parameter_hash, unknown_1, name);
        let base = parameter.texture_base.as_mut().unwrap();
        base.vft = 0x4061_7568;
        base.unknown_32 = 0x80;
        base.usage_data = usage_data;
        parameter
    }

    fn sample_normal_spec_tint_shader() -> ShaderInfo {
        let parameters = vec![
            sample_palette_texture_parameter(0xF1FE_2B71, 0, "bollard_diffuse", 0x2001_5874),
            sample_palette_texture_parameter(0xF648_A067, 2, "bollard_palette", 0x2000_2019),
            sample_palette_texture_parameter(0x46B7_C64F, 3, "bollard_normal", 0x2001_5876),
            sample_palette_texture_parameter(0x6087_99C6, 4, "bollard_spec", 0x2001_5877),
            sample_vector_parameter(0xE943_7406, 168, [1.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0x4620_A35D, 167, [0.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0x32DD_9FF5, 166, [1.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0xF671_2B81, 165, [2.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0xFF11_711D, 164, [1.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0xF418_334F, 163, [0.6, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0x8774_4680, 162, [60.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0x27B9_B2FA, 161, [0.9, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0xFDD7_96CF, 160, [0.0, 0.0, 0.0, 0.0]),
        ];
        ShaderInfo {
            name_hash: 0x6CD4_735B,
            unknown_0c: 0,
            file_hash: 0x304A_B527,
            unknown_12: 0x8000,
            unknown_1c: 0,
            unknown_24: 0,
            unknown_26: 0,
            unknown_28: 0,
            render_bucket: 0,
            parameter_size: 352,
            parameter_data_size: 448,
            render_bucket_mask: 0x0000_FF01,
            texture_parameters_count: 4,
            texture_references: parameters
                .iter()
                .filter(|parameter| parameter.data_type == 0)
                .map(|parameter| ShaderTextureReference {
                    parameter_hash: parameter.parameter_hash,
                    texture_name: parameter.texture_name.clone(),
                })
                .collect(),
            parameters,
        }
    }

    #[test]
    fn writes_and_semantically_reopens_normal_spec_tnt_shaderfx_template() {
        let mut geometry = triangle();
        geometry.shader_index = Some(0);
        let shader = sample_normal_spec_tint_shader();
        let textures = [
            texture("bollard_diffuse", TextureFormat::Bc1, [180, 160, 120, 255]),
            texture("bollard_palette", TextureFormat::Bc1, [180, 80, 40, 255]),
            texture("bollard_normal", TextureFormat::Bc1, [128, 128, 255, 255]),
            texture("bollard_spec", TextureFormat::Bc1, [64, 64, 64, 255]),
        ];
        let bytes = build_ydr_with_embedded_textures_and_shaders(
            "writer_normal_spec_tnt",
            &[geometry],
            &textures,
            std::slice::from_ref(&shader),
        )
        .unwrap();

        let document = YdrDocument::from_bytes(&bytes).unwrap();
        assert_eq!(document.model.shaders, vec![shader.clone()]);
        assert_eq!(document.model.shaders[0].name_hash, 0x6CD4_735B);
        assert_eq!(document.model.shaders[0].file_hash, 0x304A_B527);
        assert_eq!(document.model.primitives[0].shader_index, Some(0));
        assert_eq!(
            [0, 1, 2, 3].map(|index| {
                let base = document.model.shaders[0].parameters[index]
                    .texture_base
                    .as_ref()
                    .unwrap();
                (base.vft, base.unknown_32, base.usage_data)
            }),
            [
                (0x4061_7568, 0x80, 0x2001_5874),
                (0x4061_7568, 0x80, 0x2000_2019),
                (0x4061_7568, 0x80, 0x2001_5876),
                (0x4061_7568, 0x80, 0x2001_5877),
            ]
        );
        assert_eq!(
            document.model.shaders[0].parameters[12].vector_data[0],
            [0; 4]
        );
        let dictionary = document.embedded_textures.as_ref().unwrap();
        assert_eq!(dictionary.textures().len(), 4);
        assert!(dictionary.textures().iter().all(|texture| {
            texture.vft == 0
                && texture.unknown_4 == 1
                && texture.unknown_30 == 1
                && texture.unknown_32 == 0
        }));
        assert_eq!(
            dictionary.textures()[0].name.as_deref(),
            Some("bollard_diffuse")
        );
        assert_eq!(
            dictionary.textures()[1].name.as_deref(),
            Some("bollard_palette")
        );
        assert_eq!(
            dictionary.textures()[2].name.as_deref(),
            Some("bollard_normal")
        );
        assert_eq!(
            dictionary.textures()[3].name.as_deref(),
            Some("bollard_spec")
        );

        let session = YdrEditSession::from_bytes(&bytes).unwrap();
        assert_eq!(session.shader_count(), 1);
        assert_eq!(session.shader_bindings().len(), 1);
        assert_eq!(session.shader_bindings()[0].shader_index, 0);
        assert_eq!(session.texture_bindings().len(), 4);
    }

    fn sample_emissive_night_shader() -> ShaderInfo {
        let mut diffuse = sample_texture_parameter(0xF1FE_2B71, 0, "bulb_diffuse");
        diffuse.texture_base.as_mut().unwrap().vft = 0x4061_A8F8;
        let parameters = vec![
            diffuse,
            sample_vector_parameter(0x948A_54F2, 163, [1.0, 0.0, 0.0, 1.0]),
            sample_vector_parameter(0xE943_7406, 162, [1.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0x4620_A35D, 161, [0.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0x5EEB_ED48, 160, [6.0, 0.0, 0.0, 0.0]),
        ];
        ShaderInfo {
            name_hash: 0xB08C_6435,
            unknown_0c: 0,
            file_hash: 0x557D_B6E4,
            unknown_12: 0x8000,
            unknown_1c: 0,
            unknown_24: 0,
            unknown_26: 0,
            unknown_28: 0,
            render_bucket: 0,
            parameter_size: 144,
            parameter_data_size: 208,
            render_bucket_mask: 0x0000_FF01,
            texture_parameters_count: 1,
            texture_references: parameters
                .iter()
                .filter(|parameter| parameter.data_type == 0)
                .map(|parameter| ShaderTextureReference {
                    parameter_hash: parameter.parameter_hash,
                    texture_name: parameter.texture_name.clone(),
                })
                .collect(),
            parameters,
        }
    }

    fn sample_emissive_shader() -> ShaderInfo {
        let mut diffuse = sample_texture_parameter(0xF1FE_2B71, 0, "arena_diffuse");
        let base = diffuse.texture_base.as_mut().unwrap();
        base.vft = 0x4061_9668;
        base.unknown_32 = 0x80;
        base.usage_data = 0x2000_B414;
        let parameters = vec![
            diffuse,
            sample_vector_parameter(0x948A_54F2, 165, [1.0, 0.0, 0.0, 1.0]),
            sample_vector_parameter(0xE943_7406, 164, [1.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0x4620_A35D, 163, [0.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0x5EEB_ED48, 162, [16.0, 0.0, 0.0, 0.0]),
            sample_vector_parameter(0xBA54_C190, 161, [0.0, 1.0, 0.0, 0.0]),
            sample_vector_parameter(0xD79B_FC1E, 160, [1.0, 0.0, 0.0, 0.0]),
        ];
        ShaderInfo {
            name_hash: 0x8F24_5469,
            unknown_0c: 0,
            file_hash: 0x9D18_3303,
            unknown_12: 0x8000,
            unknown_1c: 0,
            unknown_24: 0,
            unknown_26: 0,
            unknown_28: 0,
            render_bucket: 0,
            parameter_size: 208,
            parameter_data_size: 272,
            render_bucket_mask: 0x0000_FF01,
            texture_parameters_count: 1,
            texture_references: parameters
                .iter()
                .filter(|parameter| parameter.data_type == 0)
                .map(|parameter| ShaderTextureReference {
                    parameter_hash: parameter.parameter_hash,
                    texture_name: parameter.texture_name.clone(),
                })
                .collect(),
            parameters,
        }
    }

    #[test]
    fn writes_and_semantically_reopens_emissive_sps_shaderfx_template() {
        let mut geometry = triangle();
        geometry.shader_index = Some(0);
        let shader = sample_emissive_shader();
        let textures = [texture(
            "arena_diffuse",
            TextureFormat::Bc3,
            [255, 255, 255, 255],
        )];
        let bytes = build_ydr_with_embedded_textures_and_shaders(
            "writer_emissive",
            &[geometry],
            &textures,
            std::slice::from_ref(&shader),
        )
        .unwrap();

        let document = YdrDocument::from_bytes(&bytes).unwrap();
        assert_eq!(document.model.shaders, vec![shader]);
        assert_eq!(document.model.shaders[0].name_hash, 0x8F24_5469);
        assert_eq!(document.model.shaders[0].file_hash, 0x9D18_3303);
        assert_eq!(document.model.primitives.len(), 1);
        assert_eq!(document.model.primitives[0].shader_index, Some(0));
        let base = document.model.shaders[0].parameters[0]
            .texture_base
            .as_ref()
            .unwrap();
        assert_eq!(base.name.as_deref(), Some("arena_diffuse"));
        assert_eq!(base.vft, 0x4061_9668);
        assert_eq!(base.unknown_32, 0x80);
        assert_eq!(base.usage_data, 0x2000_B414);
        let dictionary = document.embedded_textures.as_ref().unwrap();
        assert_eq!(dictionary.textures().len(), 1);
        assert_eq!(
            dictionary.textures()[0].name.as_deref(),
            Some("arena_diffuse")
        );

        let session = YdrEditSession::from_bytes(&bytes).unwrap();
        assert_eq!(session.shader_count(), 1);
        assert_eq!(session.shader_bindings().len(), 1);
        assert_eq!(session.shader_bindings()[0].shader_index, 0);
        assert_eq!(session.texture_bindings().len(), 1);
    }

    #[test]
    fn writes_and_semantically_reopens_emissivenight_shaderfx_template() {
        let mut geometry = triangle();
        geometry.shader_index = Some(0);
        let shader = sample_emissive_night_shader();
        let textures = [texture(
            "bulb_diffuse",
            TextureFormat::Bc3,
            [255, 255, 255, 255],
        )];
        let bytes = build_ydr_with_embedded_textures_and_shaders(
            "writer_emissivenight",
            &[geometry],
            &textures,
            std::slice::from_ref(&shader),
        )
        .unwrap();

        let document = YdrDocument::from_bytes(&bytes).unwrap();
        assert_eq!(document.model.shaders, vec![shader]);
        assert_eq!(document.model.shaders[0].name_hash, 0xB08C_6435);
        assert_eq!(document.model.shaders[0].file_hash, 0x557D_B6E4);
        assert_eq!(document.model.primitives.len(), 1);
        assert_eq!(document.model.primitives[0].shader_index, Some(0));
        assert_eq!(
            document.model.shaders[0].parameters[0]
                .texture_base
                .as_ref()
                .and_then(|base| base.name.as_deref()),
            Some("bulb_diffuse")
        );
        assert_eq!(
            document.model.shaders[0].parameters[0]
                .texture_base
                .as_ref()
                .unwrap()
                .vft,
            0x4061_A8F8
        );
        assert_eq!(
            f32::from_bits(document.model.shaders[0].parameters[1].vector_data[0][0]),
            1.0
        );
        assert_eq!(
            f32::from_bits(document.model.shaders[0].parameters[4].vector_data[0][0]),
            6.0
        );
        let dictionary = document.embedded_textures.as_ref().unwrap();
        assert_eq!(dictionary.textures().len(), 1);
        assert_eq!(
            dictionary.textures()[0].name.as_deref(),
            Some("bulb_diffuse")
        );

        let session = YdrEditSession::from_bytes(&bytes).unwrap();
        assert_eq!(session.shader_count(), 1);
        assert_eq!(session.shader_bindings().len(), 1);
        assert_eq!(session.shader_bindings()[0].shader_index, 0);
        assert_eq!(session.texture_bindings().len(), 1);
    }

    #[test]
    fn writes_and_semantically_reopens_spec_sps_shaderfx_template() {
        let mut geometry = triangle();
        geometry.shader_index = Some(0);
        let shader = sample_spec_shader();
        let textures = [
            texture("pole_albedo", TextureFormat::Bc1, [180, 160, 120, 255]),
            texture("pole_spec", TextureFormat::Bc1, [64, 64, 64, 255]),
        ];
        let bytes = build_ydr_with_embedded_textures_and_shaders(
            "writer_spec_shader",
            &[geometry],
            &textures,
            std::slice::from_ref(&shader),
        )
        .unwrap();

        let document = YdrDocument::from_bytes(&bytes).unwrap();
        assert_eq!(document.model.shaders, vec![shader.clone()]);
        assert_eq!(document.model.shaders[0].name_hash, 0x7204_A391);
        assert_eq!(document.model.shaders[0].file_hash, 0x85F5_0BE9);
        assert_eq!(document.model.primitives.len(), 1);
        assert_eq!(document.model.primitives[0].shader_index, Some(0));
        assert_eq!(
            document.model.shaders[0].parameters[0]
                .texture_base
                .as_ref()
                .and_then(|base| base.name.as_deref()),
            Some("pole_albedo")
        );
        assert_eq!(
            document.model.shaders[0].parameters[1]
                .texture_base
                .as_ref()
                .and_then(|base| base.name.as_deref()),
            Some("pole_spec")
        );
        let dictionary = document.embedded_textures.as_ref().unwrap();
        assert_eq!(dictionary.textures().len(), 2);
        assert_eq!(
            dictionary.textures()[0].name.as_deref(),
            Some("pole_albedo")
        );
        assert_eq!(dictionary.textures()[1].name.as_deref(), Some("pole_spec"));

        let session = YdrEditSession::from_bytes(&bytes).unwrap();
        assert_eq!(session.shader_count(), 1);
        assert_eq!(session.shader_bindings().len(), 1);
        assert_eq!(session.shader_bindings()[0].shader_index, 0);
        assert_eq!(session.texture_bindings().len(), 2);
    }

    #[test]
    fn writes_and_semantically_reopens_geometry_only_ydr() {
        let bytes = build_geometry_only_ydr("writer_triangle", &[triangle()]).unwrap();
        let resource = Rsc7Resource::parse(&bytes).unwrap();
        let model_render_mask_flags = u16::from_le_bytes(
            resource.system()[0x11C..0x11E]
                .try_into()
                .expect("DrawableModel RenderMaskFlags field"),
        );
        assert_eq!(
            model_render_mask_flags & 0x0001,
            0x0001,
            "CodeWalker hides models without the final-render bit unless proxy rendering is enabled",
        );
        let document = YdrDocument::from_bytes(&bytes).unwrap();
        assert_eq!(document.model.name.as_deref(), Some("writer_triangle"));
        assert_eq!(document.model.primitives.len(), 1);
        let primitive = &document.model.primitives[0];
        assert_eq!(
            primitive.positions,
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]
        );
        assert_eq!(primitive.normals, Some(vec![[0.0, 1.0, 0.0]; 3]));
        assert_eq!(
            primitive.colors0,
            Some(vec![[1.0, 128.0 / 255.0, 64.0 / 255.0, 1.0]; 3])
        );
        assert_eq!(primitive.declaration.flags, 0x40D9); // CodeWalker's PNCTTX layout.
        assert_eq!(primitive.declaration.stride, 60);
        assert_eq!(primitive.tangents, Some(vec![[1.0, 0.0, 0.0, 1.0]; 3]));
        assert_eq!(
            primitive.uv0,
            Some(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]])
        );
        assert_eq!(
            primitive.uv1,
            Some(vec![[0.25, 0.25], [0.75, 0.25], [0.25, 0.75]])
        );
        assert_eq!(primitive.indices, vec![0, 1, 2]);
        assert_eq!(document.model.bounds.min, [0.0, 0.0, 0.0]);
        assert_eq!(document.model.bounds.max, [1.0, 0.0, 1.0]);
        assert!(document.embedded_textures.is_none());
    }

    #[test]
    fn missing_color0_uses_white_and_codewalker_supported_layouts() {
        for (with_uv1, expected_flags, expected_stride) in [(false, 0x0059, 36), (true, 0x00D9, 44)]
        {
            let mut input = triangle();
            input.colors0 = None;
            input.tangents = None;
            if !with_uv1 {
                input.uv1 = None;
            }

            let bytes = build_geometry_only_ydr("writer_default_colour", &[input]).unwrap();
            let document = YdrDocument::from_bytes(&bytes).unwrap();
            let primitive = &document.model.primitives[0];
            assert_eq!(primitive.declaration.flags, expected_flags);
            assert_eq!(primitive.declaration.stride, expected_stride);
            assert_eq!(
                primitive.colors0,
                Some(vec![[1.0, 1.0, 1.0, 1.0]; 3]),
                "missing GLB COLOR_0 must become opaque white for GTA/CodeWalker layouts",
            );
        }
    }

    fn texture(name: &str, format: TextureFormat, color: [u8; 4]) -> YdrTextureInput {
        YdrTextureInput {
            name: name.to_owned(),
            width: 4,
            height: 4,
            rgba8: color.repeat(16),
            format,
        }
    }

    #[test]
    fn writes_and_semantically_reopens_legacy_shaderfx_template_and_texture_bases() {
        let mut geometry = triangle();
        geometry.shader_index = Some(0);
        let shader = sample_normal_shader();
        let textures = [
            texture("pumpkin_albedo", TextureFormat::Bc1, [255, 96, 16, 255]),
            texture("pumpkin_normal", TextureFormat::Bc1, [128, 128, 255, 255]),
        ];
        let bytes = build_ydr_with_embedded_textures_and_shaders(
            "writer_shader_template",
            &[geometry],
            &textures,
            std::slice::from_ref(&shader),
        )
        .unwrap();

        let document = YdrDocument::from_bytes(&bytes).unwrap();
        assert_eq!(document.model.shaders, vec![shader.clone()]);
        assert_eq!(document.model.primitives.len(), 1);
        assert_eq!(document.model.primitives[0].shader_index, Some(0));
        assert_eq!(
            document.model.shaders[0].parameters[0]
                .texture_base
                .as_ref()
                .and_then(|base| base.name.as_deref()),
            Some("pumpkin_albedo")
        );
        assert_eq!(
            document.model.shaders[0].parameters[1]
                .texture_base
                .as_ref()
                .and_then(|base| base.name.as_deref()),
            Some("pumpkin_normal")
        );
        let dictionary = document.embedded_textures.as_ref().unwrap();
        assert_eq!(dictionary.textures().len(), 2);
        assert_eq!(
            dictionary.textures()[0].name.as_deref(),
            Some("pumpkin_albedo")
        );
        assert_eq!(
            dictionary.textures()[1].name.as_deref(),
            Some("pumpkin_normal")
        );

        let session = YdrEditSession::from_bytes(&bytes).unwrap();
        assert_eq!(session.shader_count(), 1);
        assert_eq!(session.shader_bindings().len(), 1);
        assert_eq!(session.shader_bindings()[0].shader_index, 0);
        assert_eq!(session.texture_bindings().len(), 3);
    }

    #[test]
    fn embeds_bc1_texture_and_reopens_all_mips_from_graphics_segment() {
        let input = texture("pumpkin_albedo", TextureFormat::Bc1, [255, 96, 0, 255]);
        let bytes =
            build_ydr_with_embedded_textures("writer_textured", &[triangle()], &[input]).unwrap();
        let document = YdrDocument::from_bytes(&bytes).unwrap();
        let dictionary = document.embedded_textures.as_ref().unwrap();
        assert_eq!(dictionary.textures().len(), 1);
        let info = &dictionary.textures()[0];
        assert_eq!(info.name.as_deref(), Some("pumpkin_albedo"));
        assert_eq!(info.width, 4);
        assert_eq!(info.height, 4);
        assert_eq!(info.format, TextureFormat::Bc1);
        assert_eq!(info.levels, 3);
        let chain = dictionary.compressed_mip_chain(0).unwrap();
        assert_eq!(chain.mipmaps.len(), 3);
        assert_eq!(chain.mipmaps[0].width, 4);
        assert_eq!(chain.mipmaps[1].width, 2);
        assert_eq!(chain.mipmaps[2].width, 1);
    }

    #[test]
    fn embeds_rgba8_and_bc3_textures_with_complete_mip_chains() {
        let textures = [
            texture("pumpkin_glow", TextureFormat::Rgba8, [255, 128, 16, 255]),
            texture("pumpkin_normal", TextureFormat::Bc3, [128, 128, 255, 255]),
        ];
        let bytes =
            build_ydr_with_embedded_textures("writer_multi_texture", &[triangle()], &textures)
                .unwrap();
        let document = YdrDocument::from_bytes(&bytes).unwrap();
        let dictionary = document.embedded_textures.as_ref().unwrap();
        assert_eq!(dictionary.textures().len(), 2);
        let glow = dictionary.decode_mip(0, 0).unwrap();
        assert_eq!(glow.width, 4);
        assert_eq!(glow.height, 4);
        assert!(glow
            .rgba
            .chunks_exact(4)
            .all(|pixel| pixel == [255, 128, 16, 255]));
        let normal = dictionary.compressed_mip_chain(1).unwrap();
        assert_eq!(normal.mipmaps.len(), 3);
        assert_eq!(normal.format, TextureFormat::Bc3);
    }

    #[test]
    fn writer_rejects_texture_name_hash_collisions_and_invalid_images() {
        let colliding = [
            texture("Pumpkin", TextureFormat::Bc1, [255, 0, 0, 255]),
            texture("pumpkin", TextureFormat::Bc1, [0, 255, 0, 255]),
        ];
        assert!(build_ydr_with_embedded_textures("bad", &[triangle()], &colliding).is_err());

        let invalid = YdrTextureInput {
            name: "bad_image".into(),
            width: 4,
            height: 4,
            rgba8: vec![0; 8],
            format: TextureFormat::Rgba8,
        };
        assert!(build_ydr_with_embedded_textures("bad", &[triangle()], &[invalid]).is_err());
    }

    #[test]
    fn writer_rejects_invalid_index_and_mismatched_attributes() {
        let mut input = triangle();
        input.indices[2] = 4;
        assert!(build_geometry_only_ydr("bad", &[input]).is_err());

        let mut input = triangle();
        input.normals = Some(vec![[0.0, 1.0, 0.0]; 2]);
        assert!(build_geometry_only_ydr("bad", &[input]).is_err());
    }

    #[test]
    fn writer_rejects_non_finite_geometry_and_invalid_names() {
        let mut input = triangle();
        input.positions[0][0] = f32::NAN;
        assert!(build_geometry_only_ydr("bad", &[input]).is_err());

        assert!(build_geometry_only_ydr("bad\0name", &[triangle()]).is_err());
        assert!(build_geometry_only_ydr("", &[triangle()]).is_err());
    }
}

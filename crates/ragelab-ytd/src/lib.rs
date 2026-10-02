//! GTA V texture dictionary (`.ytd`) reader primitives.
//!
//! The initial implementation supports the legacy GTA V PC YTD resource
//! layout (RSC7 resource version 13). Enhanced/Gen9 version 5 uses a different
//! texture layout and is rejected explicitly until it has its own parser path.

use std::{error::Error, fmt, sync::Arc};

use ragelab_hash::joaat;
use ragelab_resource::{
    decode_page_count, encode_page_size_at_least, ResourceError, Rsc7Resource, ADDRESS_SPACE_SIZE,
    GRAPHICS_BASE, SYSTEM_BASE,
};

const LEGACY_RESOURCE_VERSION: u32 = 13;
const TEXTURE_DICTIONARY_SIZE: usize = 0x40;
const LEGACY_TEXTURE_SIZE: usize = 0x90;
const MAX_TEXTURES: usize = 16_384;
const MAX_TEXTURE_NAME_BYTES: usize = 4_096;
const MAX_TEXTURE_DATA_BYTES: usize = 2 * 1024 * 1024 * 1024;
const MAX_DECODED_RGBA_BYTES: usize = 256 * 1024 * 1024;
const MAX_GENERATED_RGBA_MIP_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureFormat {
    Bgra8,
    Bgrx8,
    B5G5R5A1,
    A8,
    Rgba8,
    R8,
    Bc1,
    Bc2,
    Bc3,
    Bc4,
    Bc5,
    Bc7,
    Unknown(u32),
}

impl TextureFormat {
    pub fn from_raw(value: u32) -> Self {
        match value {
            21 => Self::Bgra8,
            22 => Self::Bgrx8,
            25 => Self::B5G5R5A1,
            28 => Self::A8,
            32 => Self::Rgba8,
            50 => Self::R8,
            0x3154_5844 => Self::Bc1,
            0x3354_5844 => Self::Bc2,
            0x3554_5844 => Self::Bc3,
            0x3149_5441 => Self::Bc4,
            0x3249_5441 => Self::Bc5,
            0x2037_4342 => Self::Bc7,
            other => Self::Unknown(other),
        }
    }

    pub fn raw(self) -> u32 {
        match self {
            Self::Bgra8 => 21,
            Self::Bgrx8 => 22,
            Self::B5G5R5A1 => 25,
            Self::A8 => 28,
            Self::Rgba8 => 32,
            Self::R8 => 50,
            Self::Bc1 => 0x3154_5844,
            Self::Bc2 => 0x3354_5844,
            Self::Bc3 => 0x3554_5844,
            Self::Bc4 => 0x3149_5441,
            Self::Bc5 => 0x3249_5441,
            Self::Bc7 => 0x2037_4342,
            Self::Unknown(value) => value,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Bgra8 => "D3DFMT_A8R8G8B8",
            Self::Bgrx8 => "D3DFMT_X8R8G8B8",
            Self::B5G5R5A1 => "D3DFMT_A1R5G5B5",
            Self::A8 => "D3DFMT_A8",
            Self::Rgba8 => "D3DFMT_A8B8G8R8",
            Self::R8 => "D3DFMT_L8",
            Self::Bc1 => "D3DFMT_DXT1",
            Self::Bc2 => "D3DFMT_DXT3",
            Self::Bc3 => "D3DFMT_DXT5",
            Self::Bc4 => "D3DFMT_ATI1",
            Self::Bc5 => "D3DFMT_ATI2",
            Self::Bc7 => "D3DFMT_BC7",
            Self::Unknown(_) => "UNKNOWN",
        }
    }

    pub fn normalized_name(self) -> &'static str {
        match self {
            Self::Bgra8 => "BGRA8",
            Self::Bgrx8 => "BGRX8",
            Self::B5G5R5A1 => "B5G5R5A1",
            Self::A8 => "A8",
            Self::Rgba8 => "RGBA8",
            Self::R8 => "R8",
            Self::Bc1 => "BC1",
            Self::Bc2 => "BC2",
            Self::Bc3 => "BC3",
            Self::Bc4 => "BC4",
            Self::Bc5 => "BC5",
            Self::Bc7 => "BC7",
            Self::Unknown(_) => "UNKNOWN",
        }
    }

    /// Compact preview label retained for the model/texture batch API.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bc1 => "dxt1",
            Self::Bc3 => "dxt5",
            _ => self.normalized_name(),
        }
    }

    pub fn supports_rgba_preview(self) -> bool {
        matches!(
            self,
            Self::Bgra8
                | Self::Bgrx8
                | Self::B5G5R5A1
                | Self::A8
                | Self::Rgba8
                | Self::R8
                | Self::Bc1
                | Self::Bc2
                | Self::Bc3
                | Self::Bc4
                | Self::Bc5
        )
    }

    pub fn supports_classic_dds(self) -> bool {
        matches!(self, Self::Rgba8 | Self::Bc1 | Self::Bc3)
    }

    pub fn supports_rgba_repack(self) -> bool {
        matches!(self, Self::Rgba8 | Self::Bc1 | Self::Bc3)
    }
}

impl fmt::Display for TextureFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(raw) => write!(f, "UNKNOWN(0x{raw:08X})"),
            other => f.write_str(other.name()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureInfo {
    pub dictionary_hash: u32,
    pub name: String,
    pub name_hash: u32,
    pub width: u16,
    pub height: u16,
    pub depth: u16,
    pub stride: u16,
    pub format: TextureFormat,
    pub levels: u8,
    pub usage: u8,
    pub usage_flags: u32,
    pub extra_flags: u32,
    pub data_pointer: u64,
    pub data_length: usize,
}

impl TextureInfo {
    pub fn dictionary_hash_matches_name(&self) -> bool {
        self.dictionary_hash == self.name_hash
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedTexture {
    pub width: u16,
    pub height: u16,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictionaryTextureInfo {
    pub index: usize,
    pub name_hash: u32,
    pub name: Option<String>,
    pub width: u16,
    pub height: u16,
    pub depth: u16,
    pub stride: u16,
    pub format: TextureFormat,
    pub levels: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressedMipLevel {
    pub width: u16,
    pub height: u16,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressedTexture {
    pub name_hash: u32,
    pub name: Option<String>,
    pub width: u16,
    pub height: u16,
    pub format: TextureFormat,
    pub mipmaps: Vec<CompressedMipLevel>,
}

#[derive(Debug, Clone)]
pub struct YtdDictionary {
    textures: Vec<DictionaryTextureInfo>,
    metadata: Vec<TextureInfo>,
    resource: Arc<Rsc7Resource>,
}

impl YtdDictionary {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, YtdError> {
        let resource = parse_legacy_resource(bytes)?;
        Self::from_shared_resource_at(Arc::new(resource), SYSTEM_BASE)
    }

    pub fn from_resource_at(resource: Rsc7Resource, root: u64) -> Result<Self, YtdError> {
        Self::from_shared_resource_at(Arc::new(resource), root)
    }

    /// Parses a legacy TextureDictionary embedded at an arbitrary virtual
    /// address. The enclosing RSC7 resource version is intentionally not
    /// constrained here because YDR version 165 embeds the same dictionary
    /// layout used by standalone legacy YTD version 13 resources.
    pub fn from_shared_resource_at(
        resource: Arc<Rsc7Resource>,
        root: u64,
    ) -> Result<Self, YtdError> {
        resource.bytes_at(root, TEXTURE_DICTIONARY_SIZE)?;
        let hash_list = read_list_header(&resource, at(root, 0x20)?, "texture hash list")?;
        let texture_list = read_list_header(&resource, at(root, 0x30)?, "texture pointer list")?;
        if hash_list.count != texture_list.count {
            return Err(YtdError::Malformed(format!(
                "texture hash count {} does not match texture pointer count {}",
                hash_list.count, texture_list.count
            )));
        }

        let hash_bytes = hash_list
            .count
            .checked_mul(4)
            .ok_or_else(|| malformed("texture hash list size overflows usize"))?;
        if hash_bytes > 0 {
            resource.bytes_at(hash_list.pointer, hash_bytes)?;
        }
        let pointer_bytes = texture_list
            .capacity
            .checked_mul(8)
            .ok_or_else(|| malformed("texture pointer list size overflows usize"))?;
        if pointer_bytes > 0 {
            resource.bytes_at(texture_list.pointer, pointer_bytes)?;
        }

        let mut metadata = Vec::with_capacity(texture_list.count);
        let mut textures = Vec::with_capacity(texture_list.count);
        for index in 0..texture_list.count {
            let hash_pointer = at(hash_list.pointer, index as u64 * 4)?;
            let pointer_pointer = at(texture_list.pointer, index as u64 * 8)?;
            let dictionary_hash = resource.read_u32(hash_pointer)?;
            let texture_pointer = resource.read_u64(pointer_pointer)?;
            if texture_pointer == 0 {
                return Err(YtdError::Malformed(format!(
                    "texture pointer at index {index} is null"
                )));
            }
            let texture = read_texture(&resource, texture_pointer, dictionary_hash, index)?;
            textures.push(DictionaryTextureInfo {
                index,
                name_hash: dictionary_hash,
                name: if texture.name.is_empty() {
                    None
                } else {
                    Some(texture.name.clone())
                },
                width: texture.width,
                height: texture.height,
                depth: texture.depth,
                stride: texture.stride,
                format: texture.format,
                levels: texture.levels,
            });
            metadata.push(texture);
        }

        Ok(Self {
            textures,
            metadata,
            resource,
        })
    }

    pub fn textures(&self) -> &[DictionaryTextureInfo] {
        &self.textures
    }

    pub fn texture_by_hash(&self, hash: u32) -> Option<&DictionaryTextureInfo> {
        self.textures
            .iter()
            .find(|texture| texture.name_hash == hash)
    }

    pub fn texture_by_name(&self, name: &str) -> Option<&DictionaryTextureInfo> {
        self.textures
            .iter()
            .find(|texture| texture.name.as_deref() == Some(name))
    }

    pub fn decode_top_mip(&self, index: usize) -> Result<DecodedTexture, YtdError> {
        let texture = self
            .metadata
            .get(index)
            .ok_or(YtdError::TextureIndexOutOfBounds {
                index,
                textures: self.metadata.len(),
            })?;
        decode_texture_top_mip(&self.resource, texture, index)
    }

    pub fn compressed_mip_chain(&self, index: usize) -> Result<CompressedTexture, YtdError> {
        let texture = self
            .metadata
            .get(index)
            .ok_or(YtdError::TextureIndexOutOfBounds {
                index,
                textures: self.metadata.len(),
            })?;
        if texture.depth != 1 {
            return Err(YtdError::Malformed(format!(
                "texture {index} has depth {}; compressed preview supports 2D textures only",
                texture.depth
            )));
        }
        if !matches!(texture.format, TextureFormat::Bc1 | TextureFormat::Bc3) {
            return Err(YtdError::UnsupportedTextureFormat(texture.format));
        }

        let layout = mip_layout(
            texture.format,
            texture.width,
            texture.height,
            texture.levels,
        )?
        .ok_or(YtdError::UnsupportedTextureFormat(texture.format))?;
        let mut mipmaps = Vec::with_capacity(layout.len());
        for mip in layout {
            let pointer = texture
                .data_pointer
                .checked_add(mip.offset as u64)
                .ok_or_else(|| malformed("compressed mip address overflows u64"))?;
            let data = self.resource.bytes_at(pointer, mip.slice_pitch)?.to_vec();
            mipmaps.push(CompressedMipLevel {
                width: u16::try_from(mip.width)
                    .map_err(|_| malformed("compressed mip width does not fit u16"))?,
                height: u16::try_from(mip.height)
                    .map_err(|_| malformed("compressed mip height does not fit u16"))?,
                data,
            });
        }

        let public = &self.textures[index];
        Ok(CompressedTexture {
            name_hash: public.name_hash,
            name: public.name.clone(),
            width: public.width,
            height: public.height,
            format: public.format,
            mipmaps,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedTexture {
    pub width: u16,
    pub height: u16,
    pub format: TextureFormat,
    pub levels: u8,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
struct ParsedDds<'a> {
    width: u16,
    height: u16,
    format: TextureFormat,
    levels: u8,
    data: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ytd {
    pub resource_version: u32,
    pub file_vft: u32,
    pub file_unknown: u32,
    pub pages_info_pointer: u64,
    pub textures: Vec<TextureInfo>,
}

impl Ytd {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, YtdError> {
        let resource = parse_legacy_resource(bytes)?;
        parse_legacy(&resource)
    }

    /// Rebuilds a legacy v13 YTD into a compact fresh RSC7 layout.
    ///
    /// The output does not reuse source virtual offsets. Dictionary lists,
    /// texture descriptors, names, ResourcePagesInfo and graphics payloads are
    /// laid out again from scratch while preserving texture metadata, opaque
    /// descriptor bytes that are not pointer fields, and encoded image data.
    pub fn rebuild_legacy_compact(bytes: &[u8]) -> Result<Vec<u8>, YtdError> {
        let resource = parse_legacy_resource(bytes)?;
        let ytd = parse_legacy(&resource)?;
        let (rebuilt, source_textures) = rebuild_legacy_resource(&resource, &ytd)?;
        let output = rebuilt.to_bytes().map_err(YtdError::Resource)?;
        validate_compact_rebuild(&output, &ytd, &source_textures)?;
        Ok(output)
    }

    pub fn decode_top_mip_rgba(bytes: &[u8], index: usize) -> Result<DecodedTexture, YtdError> {
        let resource = parse_legacy_resource(bytes)?;
        let ytd = parse_legacy(&resource)?;
        let texture = texture_at(&ytd, index)?;
        decode_texture_top_mip(&resource, texture, index)
    }

    pub fn encoded_texture(bytes: &[u8], index: usize) -> Result<EncodedTexture, YtdError> {
        let resource = parse_legacy_resource(bytes)?;
        let ytd = parse_legacy(&resource)?;
        let texture = texture_at(&ytd, index)?;
        let data = if texture.data_length == 0 {
            Vec::new()
        } else {
            resource
                .bytes_at(texture.data_pointer, texture.data_length)?
                .to_vec()
        };
        Ok(EncodedTexture {
            width: texture.width,
            height: texture.height,
            format: texture.format,
            levels: texture.levels,
            data,
        })
    }

    pub fn texture_dds(bytes: &[u8], index: usize) -> Result<Vec<u8>, YtdError> {
        let resource = parse_legacy_resource(bytes)?;
        let ytd = parse_legacy(&resource)?;
        let texture = texture_at(&ytd, index)?;
        let data = if texture.data_length == 0 {
            &[][..]
        } else {
            resource.bytes_at(texture.data_pointer, texture.data_length)?
        };
        build_dds(texture, data)
    }

    /// Builds a classic RGBA8 DDS from one top-level image and generates a
    /// complete mip chain down to 1x1 with deterministic box filtering.
    ///
    /// This remains the direct uncompressed path. BC1 and BC3 can be generated
    /// through [`Self::bc1_dds_with_generated_mips`] and
    /// [`Self::bc3_dds_with_generated_mips`] when block compression is wanted.
    pub fn rgba8_dds_with_generated_mips(
        width: u16,
        height: u16,
        rgba: &[u8],
    ) -> Result<Vec<u8>, YtdError> {
        let (levels, payload) = generate_rgba8_mip_payload(width, height, rgba)?;
        let texture = TextureInfo {
            dictionary_hash: 0,
            name: String::new(),
            name_hash: 0,
            width,
            height,
            depth: 1,
            stride: legacy_stride(TextureFormat::Rgba8, width)?,
            format: TextureFormat::Rgba8,
            levels,
            usage: 0,
            usage_flags: 0,
            extra_flags: 0,
            data_pointer: 0,
            data_length: payload.len(),
        };
        build_dds(&texture, &payload)
    }

    /// Builds a classic BC1/DXT1 DDS from one top-level RGBA8 image.
    ///
    /// The full mip chain is generated with the same deterministic box filter
    /// used by [`Self::rgba8_dds_with_generated_mips`], then each level is
    /// encoded internally to BC1. Pixels with alpha below 128 use BC1's
    /// one-bit transparent selector; all other pixels are encoded as opaque.
    pub fn bc1_dds_with_generated_mips(
        width: u16,
        height: u16,
        rgba: &[u8],
    ) -> Result<Vec<u8>, YtdError> {
        let (levels, payload) = generate_bc1_mip_payload(width, height, rgba)?;
        let texture = TextureInfo {
            dictionary_hash: 0,
            name: String::new(),
            name_hash: 0,
            width,
            height,
            depth: 1,
            stride: legacy_stride(TextureFormat::Bc1, width)?,
            format: TextureFormat::Bc1,
            levels,
            usage: 0,
            usage_flags: 0,
            extra_flags: 0,
            data_pointer: 0,
            data_length: payload.len(),
        };
        build_dds(&texture, &payload)
    }

    /// Builds a classic BC3/DXT5 DDS from one top-level RGBA8 image.
    ///
    /// The full mip chain uses the deterministic RGBA box filter before each
    /// level is encoded. Color endpoints use BC3's forced four-color mode and
    /// alpha endpoints/selectors are chosen deterministically per 4x4 block.
    pub fn bc3_dds_with_generated_mips(
        width: u16,
        height: u16,
        rgba: &[u8],
    ) -> Result<Vec<u8>, YtdError> {
        let (levels, payload) = generate_bc3_mip_payload(width, height, rgba)?;
        let texture = TextureInfo {
            dictionary_hash: 0,
            name: String::new(),
            name_hash: 0,
            width,
            height,
            depth: 1,
            stride: legacy_stride(TextureFormat::Bc3, width)?,
            format: TextureFormat::Bc3,
            levels,
            usage: 0,
            usage_flags: 0,
            extra_flags: 0,
            data_pointer: 0,
            data_length: payload.len(),
        };
        build_dds(&texture, &payload)
    }

    /// Replaces one legacy 2D texture from raw RGBA8 pixels, generating a
    /// complete mip chain and preserving the target texture's existing format.
    ///
    /// Supported target formats are RGBA8, BC1 and BC3. The generated classic
    /// DDS is passed through the same relocated-repack path as imported DDS
    /// files, so allocation guards and semantic reparse checks remain shared.
    pub fn replace_texture_from_rgba_relocated(
        bytes: &[u8],
        index: usize,
        width: u16,
        height: u16,
        rgba: &[u8],
    ) -> Result<Vec<u8>, YtdError> {
        let ytd = Self::from_bytes(bytes)?;
        let texture = texture_at(&ytd, index)?;
        if texture.depth != 1 {
            return Err(YtdError::ReplacementMismatch(format!(
                "texture {index} has depth {}; RGBA repack supports 2D textures only",
                texture.depth
            )));
        }
        let dds = match texture.format {
            TextureFormat::Rgba8 => Self::rgba8_dds_with_generated_mips(width, height, rgba)?,
            TextureFormat::Bc1 => Self::bc1_dds_with_generated_mips(width, height, rgba)?,
            TextureFormat::Bc3 => Self::bc3_dds_with_generated_mips(width, height, rgba)?,
            format => return Err(YtdError::UnsupportedTextureFormat(format)),
        };
        Self::replace_texture_from_dds_relocated(bytes, index, &dds)
    }

    /// Replaces one legacy texture payload without changing the RSC7 layout.
    ///
    /// The DDS must describe the exact same 2D texture dimensions, format,
    /// mip count and encoded byte length as the existing texture. Metadata,
    /// pointers, page flags and segment sizes are preserved.
    pub fn replace_texture_from_dds(
        bytes: &[u8],
        index: usize,
        dds: &[u8],
    ) -> Result<Vec<u8>, YtdError> {
        let replacement = parse_classic_dds(dds)?;
        let mut resource = parse_legacy_resource(bytes)?;
        let ytd = parse_legacy(&resource)?;
        let texture = texture_at(&ytd, index)?.clone();

        if texture.depth != 1 {
            return Err(YtdError::ReplacementMismatch(format!(
                "texture {index} has depth {}; layout-preserving DDS replacement supports 2D textures only",
                texture.depth
            )));
        }
        if replacement.width != texture.width || replacement.height != texture.height {
            return Err(YtdError::ReplacementMismatch(format!(
                "DDS dimensions {}x{} do not match texture {index} dimensions {}x{}",
                replacement.width, replacement.height, texture.width, texture.height
            )));
        }
        if replacement.format != texture.format {
            return Err(YtdError::ReplacementMismatch(format!(
                "DDS format {} does not match texture {index} format {}",
                replacement.format.normalized_name(),
                texture.format.normalized_name()
            )));
        }
        if replacement.levels != texture.levels {
            return Err(YtdError::ReplacementMismatch(format!(
                "DDS mip count {} does not match texture {index} mip count {}",
                replacement.levels, texture.levels
            )));
        }
        if replacement.data.len() != texture.data_length {
            return Err(YtdError::ReplacementMismatch(format!(
                "DDS payload length {} does not match texture {index} encoded length {}",
                replacement.data.len(),
                texture.data_length
            )));
        }

        resource
            .bytes_at_mut(texture.data_pointer, texture.data_length)?
            .copy_from_slice(replacement.data);
        resource.to_bytes().map_err(YtdError::Resource)
    }

    /// Replaces one legacy 2D texture by relocating its encoded payload.
    ///
    /// Unlike [`Self::replace_texture_from_dds`], dimensions and mip count may
    /// change. The legacy format is deliberately kept identical for this first
    /// repacking path. Existing system/graphics bytes and all other texture
    /// pointers remain stable; the replacement payload is appended after the
    /// original graphics allocation and the graphics segment grows as needed.
    pub fn replace_texture_from_dds_relocated(
        bytes: &[u8],
        index: usize,
        dds: &[u8],
    ) -> Result<Vec<u8>, YtdError> {
        let replacement = parse_classic_dds(dds)?;
        let mut resource = parse_legacy_resource(bytes)?;
        let ytd = parse_legacy(&resource)?;
        let texture = texture_at(&ytd, index)?.clone();

        if texture.depth != 1 {
            return Err(YtdError::ReplacementMismatch(format!(
                "texture {index} has depth {}; relocated DDS replacement supports 2D textures only",
                texture.depth
            )));
        }
        if replacement.format != texture.format {
            return Err(YtdError::ReplacementMismatch(format!(
                "relocated DDS replacement currently preserves format; imported {} does not match texture {index} format {}",
                replacement.format.normalized_name(),
                texture.format.normalized_name()
            )));
        }

        let layout = mip_layout(
            replacement.format,
            replacement.width,
            replacement.height,
            replacement.levels,
        )?
        .ok_or(YtdError::UnsupportedTextureFormat(replacement.format))?;
        let stride = legacy_stride(replacement.format, replacement.width)?;
        let encoded_length = layout.last().map_or(0, |mip| mip.offset + mip.slice_pitch);
        if encoded_length != replacement.data.len() {
            return Err(YtdError::InvalidDds(format!(
                "replacement mip layout describes {encoded_length} bytes but DDS contains {}",
                replacement.data.len()
            )));
        }

        let original_graphics_len = resource.graphics().len();
        let required_graphics_len = original_graphics_len
            .checked_add(replacement.data.len())
            .ok_or_else(|| malformed("replacement graphics allocation overflows usize"))?;
        resource
            .grow_graphics_to_fit_with_page_info(required_graphics_len, ytd.pages_info_pointer)?;

        let data_pointer = GRAPHICS_BASE
            .checked_add(original_graphics_len as u64)
            .ok_or_else(|| malformed("replacement graphics pointer overflows u64"))?;
        resource
            .bytes_at_mut(data_pointer, replacement.data.len())?
            .copy_from_slice(replacement.data);

        let texture_pointer = texture_pointer_at(&resource, index)?;
        write_u16(&mut resource, at(texture_pointer, 0x50)?, replacement.width)?;
        write_u16(
            &mut resource,
            at(texture_pointer, 0x52)?,
            replacement.height,
        )?;
        write_u16(&mut resource, at(texture_pointer, 0x56)?, stride)?;
        write_u32(
            &mut resource,
            at(texture_pointer, 0x58)?,
            replacement.format.raw(),
        )?;
        write_u8(
            &mut resource,
            at(texture_pointer, 0x5D)?,
            replacement.levels,
        )?;
        write_u64(&mut resource, at(texture_pointer, 0x70)?, data_pointer)?;

        let output = resource.to_bytes().map_err(YtdError::Resource)?;
        validate_relocated_replacement(&output, &ytd, index, &replacement, stride, data_pointer)?;
        Ok(output)
    }
}

#[derive(Debug, Clone)]
struct CompactTextureSource {
    metadata: TextureInfo,
    descriptor: Vec<u8>,
    name_bytes: Vec<u8>,
    data: Vec<u8>,
}

fn rebuild_legacy_resource(
    resource: &Rsc7Resource,
    ytd: &Ytd,
) -> Result<(Rsc7Resource, Vec<CompactTextureSource>), YtdError> {
    let texture_count = ytd.textures.len();
    if texture_count > MAX_TEXTURES || texture_count > usize::from(u16::MAX) {
        return Err(malformed(format!(
            "cannot serialize {texture_count} textures; legacy list capacity is u16 and parser limit is {MAX_TEXTURES}"
        )));
    }

    let root_aux = resource.read_u32(at(SYSTEM_BASE, 0x18)?)?;
    let mut sources = Vec::with_capacity(texture_count);
    for (index, texture) in ytd.textures.iter().enumerate() {
        let texture_pointer = texture_pointer_at(resource, index)?;
        let descriptor = resource
            .bytes_at(texture_pointer, LEGACY_TEXTURE_SIZE)?
            .to_vec();
        reject_unmodeled_descriptor_pointers(&descriptor, index)?;
        let name_bytes = texture.name.as_bytes().to_vec();
        if name_bytes.len() > MAX_TEXTURE_NAME_BYTES {
            return Err(malformed(format!(
                "texture {index} name length {} exceeds serializer limit {MAX_TEXTURE_NAME_BYTES}",
                name_bytes.len()
            )));
        }
        let data = if texture.data_length == 0 {
            Vec::new()
        } else {
            resource
                .bytes_at(texture.data_pointer, texture.data_length)?
                .to_vec()
        };
        sources.push(CompactTextureSource {
            metadata: texture.clone(),
            descriptor,
            name_bytes,
            data,
        });
    }

    let pointer_list_offset = TEXTURE_DICTIONARY_SIZE;
    let pointer_list_bytes = checked_mul(texture_count, 8, "texture pointer list bytes")?;
    let hash_list_offset = align_up(
        checked_add(
            pointer_list_offset,
            pointer_list_bytes,
            "texture pointer list end",
        )?,
        16,
    )?;
    let hash_list_bytes = checked_mul(texture_count, 4, "texture hash list bytes")?;
    let descriptor_table_offset = align_up(
        checked_add(hash_list_offset, hash_list_bytes, "texture hash list end")?,
        16,
    )?;
    let descriptor_table_bytes = checked_mul(
        texture_count,
        LEGACY_TEXTURE_SIZE,
        "texture descriptor bytes",
    )?;
    let mut system_cursor = align_up(
        checked_add(
            descriptor_table_offset,
            descriptor_table_bytes,
            "texture descriptor table end",
        )?,
        16,
    )?;

    let mut name_offsets = Vec::with_capacity(texture_count);
    for source in &sources {
        if source.name_bytes.is_empty() {
            name_offsets.push(None);
            continue;
        }
        let offset = system_cursor;
        system_cursor = checked_add(
            system_cursor,
            checked_add(source.name_bytes.len(), 1, "texture name terminator")?,
            "texture name end",
        )?;
        name_offsets.push(Some(offset));
    }
    let pages_info_offset = align_up(system_cursor, 16)?;

    let mut graphics_cursor = 0_usize;
    let mut data_offsets = Vec::with_capacity(texture_count);
    for source in &sources {
        if source.data.is_empty() {
            data_offsets.push(None);
            continue;
        }
        graphics_cursor = align_up(graphics_cursor, 16)?;
        let offset = graphics_cursor;
        graphics_cursor = checked_add(
            graphics_cursor,
            source.data.len(),
            "texture graphics payload end",
        )?;
        data_offsets.push(Some(offset));
    }
    let graphics_used = graphics_cursor;
    let (graphics_layout, _) = encode_page_size_at_least(graphics_used)?;
    let graphics_pages = decode_page_count(graphics_layout);

    let mut system_pages = 0_usize;
    let mut system_used = 0_usize;
    for _ in 0..16 {
        let page_records = checked_add(system_pages, graphics_pages, "page-info record count")?;
        let page_info_len = checked_add(
            16,
            checked_mul(page_records, 8, "page-info records")?,
            "page-info length",
        )?;
        let candidate = checked_add(pages_info_offset, page_info_len, "system payload end")?;
        let (system_layout, _) = encode_page_size_at_least(candidate)?;
        let next_system_pages = decode_page_count(system_layout);
        if next_system_pages == system_pages {
            system_used = candidate;
            break;
        }
        system_pages = next_system_pages;
    }
    if system_used == 0 {
        return Err(malformed(
            "ResourcePagesInfo page-count layout did not converge",
        ));
    }

    let system_page_count = u8::try_from(system_pages)
        .map_err(|_| malformed("system page count does not fit ResourcePagesInfo u8"))?;
    let graphics_page_count = u8::try_from(graphics_pages)
        .map_err(|_| malformed("graphics page count does not fit ResourcePagesInfo u8"))?;
    let texture_count_u16 = u16::try_from(texture_count)
        .map_err(|_| malformed("texture count does not fit legacy u16 list header"))?;

    let mut system = vec![0_u8; system_used];
    let mut graphics = vec![0_u8; graphics_used];

    write_segment_u32(&mut system, 0x00, ytd.file_vft, "dictionary vft")?;
    write_segment_u32(
        &mut system,
        0x04,
        ytd.file_unknown,
        "dictionary unknown field",
    )?;
    write_segment_u64(
        &mut system,
        0x08,
        virtual_pointer(SYSTEM_BASE, pages_info_offset, "ResourcePagesInfo")?,
        "ResourcePagesInfo pointer",
    )?;
    write_segment_u32(&mut system, 0x18, root_aux, "dictionary auxiliary field")?;

    let hash_list_pointer = if texture_count == 0 {
        0
    } else {
        virtual_pointer(SYSTEM_BASE, hash_list_offset, "texture hash list")?
    };
    let pointer_list_pointer = if texture_count == 0 {
        0
    } else {
        virtual_pointer(SYSTEM_BASE, pointer_list_offset, "texture pointer list")?
    };
    write_list_header(
        &mut system,
        0x20,
        hash_list_pointer,
        texture_count_u16,
        "texture hash list",
    )?;
    write_list_header(
        &mut system,
        0x30,
        pointer_list_pointer,
        texture_count_u16,
        "texture pointer list",
    )?;

    for (index, source) in sources.iter().enumerate() {
        let pointer_slot = checked_add(
            pointer_list_offset,
            checked_mul(index, 8, "texture pointer slot")?,
            "texture pointer slot",
        )?;
        let hash_slot = checked_add(
            hash_list_offset,
            checked_mul(index, 4, "texture hash slot")?,
            "texture hash slot",
        )?;
        let descriptor_offset = checked_add(
            descriptor_table_offset,
            checked_mul(index, LEGACY_TEXTURE_SIZE, "texture descriptor offset")?,
            "texture descriptor offset",
        )?;
        let descriptor_pointer =
            virtual_pointer(SYSTEM_BASE, descriptor_offset, "texture descriptor")?;
        write_segment_u64(
            &mut system,
            pointer_slot,
            descriptor_pointer,
            "texture descriptor pointer",
        )?;
        write_segment_u32(
            &mut system,
            hash_slot,
            source.metadata.dictionary_hash,
            "texture dictionary hash",
        )?;
        write_segment_bytes(
            &mut system,
            descriptor_offset,
            &source.descriptor,
            "texture descriptor",
        )?;

        let name_pointer = match name_offsets[index] {
            Some(offset) => {
                write_segment_bytes(&mut system, offset, &source.name_bytes, "texture name")?;
                virtual_pointer(SYSTEM_BASE, offset, "texture name")?
            }
            None => 0,
        };
        write_segment_u64(
            &mut system,
            checked_add(descriptor_offset, 0x28, "texture name pointer field")?,
            name_pointer,
            "texture name pointer",
        )?;

        let data_pointer = match data_offsets[index] {
            Some(offset) => {
                write_segment_bytes(
                    &mut graphics,
                    offset,
                    &source.data,
                    "texture graphics payload",
                )?;
                virtual_pointer(GRAPHICS_BASE, offset, "texture graphics payload")?
            }
            None => 0,
        };
        write_segment_u64(
            &mut system,
            checked_add(descriptor_offset, 0x70, "texture data pointer field")?,
            data_pointer,
            "texture data pointer",
        )?;
    }

    write_segment_u8(
        &mut system,
        checked_add(pages_info_offset, 0x08, "system page-count field")?,
        system_page_count,
        "system page count",
    )?;
    write_segment_u8(
        &mut system,
        checked_add(pages_info_offset, 0x09, "graphics page-count field")?,
        graphics_page_count,
        "graphics page count",
    )?;

    let rebuilt = Rsc7Resource::from_segments(
        ytd.resource_version,
        resource.header.system_flags,
        resource.header.graphics_flags,
        &system,
        &graphics,
    )?;
    if decode_page_count(rebuilt.header.system_flags) != system_pages
        || decode_page_count(rebuilt.header.graphics_flags) != graphics_pages
    {
        return Err(malformed(
            "fresh RSC7 page allocation disagrees with serialized ResourcePagesInfo",
        ));
    }
    Ok((rebuilt, sources))
}

fn validate_compact_rebuild(
    bytes: &[u8],
    before: &Ytd,
    sources: &[CompactTextureSource],
) -> Result<(), YtdError> {
    let resource = parse_legacy_resource(bytes)?;
    let after = parse_legacy(&resource)?;
    if after.resource_version != before.resource_version
        || after.file_vft != before.file_vft
        || after.file_unknown != before.file_unknown
        || after.textures.len() != before.textures.len()
    {
        return Err(malformed(
            "compact rebuild changed dictionary-level semantic metadata",
        ));
    }

    for (index, ((expected, actual), source)) in before
        .textures
        .iter()
        .zip(after.textures.iter())
        .zip(sources.iter())
        .enumerate()
    {
        if !same_texture_semantics(expected, actual) {
            return Err(malformed(format!(
                "compact rebuild changed texture metadata at index {index}"
            )));
        }
        let actual_data = if actual.data_length == 0 {
            &[][..]
        } else {
            resource.bytes_at(actual.data_pointer, actual.data_length)?
        };
        if actual_data != source.data.as_slice() {
            return Err(malformed(format!(
                "compact rebuild changed encoded texture payload at index {index}"
            )));
        }
    }
    Ok(())
}

fn same_texture_semantics(left: &TextureInfo, right: &TextureInfo) -> bool {
    left.dictionary_hash == right.dictionary_hash
        && left.name == right.name
        && left.name_hash == right.name_hash
        && left.width == right.width
        && left.height == right.height
        && left.depth == right.depth
        && left.stride == right.stride
        && left.format == right.format
        && left.levels == right.levels
        && left.usage == right.usage
        && left.usage_flags == right.usage_flags
        && left.extra_flags == right.extra_flags
        && left.data_length == right.data_length
}

fn reject_unmodeled_descriptor_pointers(descriptor: &[u8], index: usize) -> Result<(), YtdError> {
    for offset in (0..=LEGACY_TEXTURE_SIZE - 8).step_by(8) {
        if matches!(offset, 0x28 | 0x70) {
            continue;
        }
        let value = u64::from_le_bytes(
            descriptor[offset..offset + 8]
                .try_into()
                .expect("fixed descriptor qword slice"),
        );
        let in_system = (SYSTEM_BASE..SYSTEM_BASE + ADDRESS_SPACE_SIZE).contains(&value);
        let in_graphics = (GRAPHICS_BASE..GRAPHICS_BASE + ADDRESS_SPACE_SIZE).contains(&value);
        if in_system || in_graphics {
            return Err(malformed(format!(
                "texture {index} descriptor has unmodeled resource pointer 0x{value:016X} at +0x{offset:02X}"
            )));
        }
    }
    Ok(())
}

fn checked_add(left: usize, right: usize, label: &str) -> Result<usize, YtdError> {
    left.checked_add(right)
        .ok_or_else(|| malformed(format!("{label} overflows usize")))
}

fn checked_mul(left: usize, right: usize, label: &str) -> Result<usize, YtdError> {
    left.checked_mul(right)
        .ok_or_else(|| malformed(format!("{label} overflows usize")))
}

fn align_up(value: usize, alignment: usize) -> Result<usize, YtdError> {
    debug_assert!(alignment.is_power_of_two());
    let mask = alignment - 1;
    checked_add(value, mask, "alignment").map(|aligned| aligned & !mask)
}

fn virtual_pointer(base: u64, offset: usize, label: &str) -> Result<u64, YtdError> {
    let offset =
        u64::try_from(offset).map_err(|_| malformed(format!("{label} offset does not fit u64")))?;
    base.checked_add(offset)
        .ok_or_else(|| malformed(format!("{label} pointer overflows u64")))
}

fn write_list_header(
    segment: &mut [u8],
    offset: usize,
    pointer: u64,
    count: u16,
    label: &str,
) -> Result<(), YtdError> {
    write_segment_u64(segment, offset, pointer, label)?;
    write_segment_u16(
        segment,
        checked_add(offset, 0x08, "list count field")?,
        count,
        label,
    )?;
    write_segment_u16(
        segment,
        checked_add(offset, 0x0A, "list capacity field")?,
        count,
        label,
    )?;
    Ok(())
}

fn write_segment_u8(
    segment: &mut [u8],
    offset: usize,
    value: u8,
    label: &str,
) -> Result<(), YtdError> {
    write_segment_bytes(segment, offset, &[value], label)
}

fn write_segment_u16(
    segment: &mut [u8],
    offset: usize,
    value: u16,
    label: &str,
) -> Result<(), YtdError> {
    write_segment_bytes(segment, offset, &value.to_le_bytes(), label)
}

fn write_segment_u32(
    segment: &mut [u8],
    offset: usize,
    value: u32,
    label: &str,
) -> Result<(), YtdError> {
    write_segment_bytes(segment, offset, &value.to_le_bytes(), label)
}

fn write_segment_u64(
    segment: &mut [u8],
    offset: usize,
    value: u64,
    label: &str,
) -> Result<(), YtdError> {
    write_segment_bytes(segment, offset, &value.to_le_bytes(), label)
}

fn write_segment_bytes(
    segment: &mut [u8],
    offset: usize,
    bytes: &[u8],
    label: &str,
) -> Result<(), YtdError> {
    let end = checked_add(offset, bytes.len(), label)?;
    let segment_len = segment.len();
    let target = segment.get_mut(offset..end).ok_or_else(|| {
        malformed(format!(
            "{label} range {offset}..{end} exceeds segment length {segment_len}"
        ))
    })?;
    target.copy_from_slice(bytes);
    Ok(())
}

#[derive(Debug)]
pub enum YtdError {
    Resource(ResourceError),
    UnsupportedResourceVersion(u32),
    TextureIndexOutOfBounds { index: usize, textures: usize },
    UnsupportedTextureFormat(TextureFormat),
    InvalidDds(String),
    InvalidRgba(String),
    ReplacementMismatch(String),
    Malformed(String),
}

impl fmt::Display for YtdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resource(error) => write!(f, "{error}"),
            Self::UnsupportedResourceVersion(version) => write!(
                f,
                "unsupported YTD resource version {version}; legacy GTA V PC YTD version {LEGACY_RESOURCE_VERSION} is currently supported"
            ),
            Self::TextureIndexOutOfBounds { index, textures } => write!(
                f,
                "texture index {index} is out of bounds for YTD with {textures} textures"
            ),
            Self::UnsupportedTextureFormat(format) => write!(
                f,
                "texture format {} is not supported for this operation",
                format.normalized_name()
            ),
            Self::InvalidDds(message) => write!(f, "invalid DDS: {message}"),
            Self::InvalidRgba(message) => write!(f, "invalid RGBA image: {message}"),
            Self::ReplacementMismatch(message) => write!(f, "DDS replacement mismatch: {message}"),
            Self::Malformed(message) => write!(f, "malformed YTD: {message}"),
        }
    }
}

impl Error for YtdError {}

impl From<ResourceError> for YtdError {
    fn from(error: ResourceError) -> Self {
        Self::Resource(error)
    }
}

#[derive(Debug, Clone, Copy)]
struct ListHeader {
    pointer: u64,
    count: usize,
    capacity: usize,
}

fn texture_at(ytd: &Ytd, index: usize) -> Result<&TextureInfo, YtdError> {
    ytd.textures
        .get(index)
        .ok_or(YtdError::TextureIndexOutOfBounds {
            index,
            textures: ytd.textures.len(),
        })
}

fn texture_pointer_at(resource: &Rsc7Resource, index: usize) -> Result<u64, YtdError> {
    let texture_list = read_list_header(resource, at(SYSTEM_BASE, 0x30)?, "texture pointer list")?;
    if index >= texture_list.count {
        return Err(YtdError::TextureIndexOutOfBounds {
            index,
            textures: texture_list.count,
        });
    }
    let pointer_slot = at(texture_list.pointer, index as u64 * 8)?;
    let texture_pointer = resource.read_u64(pointer_slot)?;
    if texture_pointer == 0 {
        return Err(YtdError::Malformed(format!(
            "texture pointer at index {index} is null"
        )));
    }
    resource.bytes_at(texture_pointer, LEGACY_TEXTURE_SIZE)?;
    Ok(texture_pointer)
}

fn write_u8(resource: &mut Rsc7Resource, pointer: u64, value: u8) -> Result<(), YtdError> {
    resource.bytes_at_mut(pointer, 1)?[0] = value;
    Ok(())
}

fn write_u16(resource: &mut Rsc7Resource, pointer: u64, value: u16) -> Result<(), YtdError> {
    resource
        .bytes_at_mut(pointer, 2)?
        .copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn write_u32(resource: &mut Rsc7Resource, pointer: u64, value: u32) -> Result<(), YtdError> {
    resource
        .bytes_at_mut(pointer, 4)?
        .copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn write_u64(resource: &mut Rsc7Resource, pointer: u64, value: u64) -> Result<(), YtdError> {
    resource
        .bytes_at_mut(pointer, 8)?
        .copy_from_slice(&value.to_le_bytes());
    Ok(())
}

fn validate_relocated_replacement(
    bytes: &[u8],
    before: &Ytd,
    index: usize,
    replacement: &ParsedDds<'_>,
    expected_stride: u16,
    expected_data_pointer: u64,
) -> Result<(), YtdError> {
    let resource = parse_legacy_resource(bytes)?;
    let after = parse_legacy(&resource)?;
    if before.resource_version != after.resource_version
        || before.file_vft != after.file_vft
        || before.file_unknown != after.file_unknown
        || before.pages_info_pointer != after.pages_info_pointer
        || before.textures.len() != after.textures.len()
    {
        return Err(YtdError::Malformed(
            "relocated replacement changed dictionary-level metadata".into(),
        ));
    }

    for (texture_index, (original, rewritten)) in before
        .textures
        .iter()
        .zip(after.textures.iter())
        .enumerate()
    {
        if texture_index == index {
            if original.dictionary_hash != rewritten.dictionary_hash
                || original.name != rewritten.name
                || original.name_hash != rewritten.name_hash
                || original.depth != rewritten.depth
                || original.format != rewritten.format
                || original.usage != rewritten.usage
                || original.usage_flags != rewritten.usage_flags
                || original.extra_flags != rewritten.extra_flags
            {
                return Err(YtdError::Malformed(format!(
                    "relocated replacement changed identity/usage metadata for texture {index}"
                )));
            }
        } else if original != rewritten {
            return Err(YtdError::Malformed(format!(
                "relocated replacement changed metadata for unrelated texture {texture_index}"
            )));
        }
    }

    let texture = texture_at(&after, index)?;
    if texture.width != replacement.width
        || texture.height != replacement.height
        || texture.depth != 1
        || texture.stride != expected_stride
        || texture.format != replacement.format
        || texture.levels != replacement.levels
        || texture.data_pointer != expected_data_pointer
        || texture.data_length != replacement.data.len()
    {
        return Err(YtdError::Malformed(format!(
            "relocated replacement failed semantic reparse for texture {index}"
        )));
    }
    let encoded = resource.bytes_at(texture.data_pointer, texture.data_length)?;
    if encoded != replacement.data {
        return Err(YtdError::Malformed(format!(
            "relocated replacement payload mismatch for texture {index}"
        )));
    }
    Ok(())
}

fn parse_legacy_resource(bytes: &[u8]) -> Result<Rsc7Resource, YtdError> {
    let resource = Rsc7Resource::parse(bytes)?;
    if resource.header.version != LEGACY_RESOURCE_VERSION {
        return Err(YtdError::UnsupportedResourceVersion(
            resource.header.version,
        ));
    }
    Ok(resource)
}

fn parse_legacy(resource: &Rsc7Resource) -> Result<Ytd, YtdError> {
    resource.bytes_at(SYSTEM_BASE, TEXTURE_DICTIONARY_SIZE)?;

    let file_vft = resource.read_u32(SYSTEM_BASE)?;
    let file_unknown = resource.read_u32(at(SYSTEM_BASE, 0x04)?)?;
    let pages_info_pointer = resource.read_u64(at(SYSTEM_BASE, 0x08)?)?;
    let hash_list = read_list_header(resource, at(SYSTEM_BASE, 0x20)?, "texture hash list")?;
    let texture_list = read_list_header(resource, at(SYSTEM_BASE, 0x30)?, "texture pointer list")?;

    if hash_list.count != texture_list.count {
        return Err(YtdError::Malformed(format!(
            "texture hash count {} does not match texture pointer count {}",
            hash_list.count, texture_list.count
        )));
    }

    let hash_bytes = hash_list
        .count
        .checked_mul(4)
        .ok_or_else(|| malformed("texture hash list size overflows usize"))?;
    if hash_bytes > 0 {
        resource.bytes_at(hash_list.pointer, hash_bytes)?;
    }

    let pointer_bytes = texture_list
        .capacity
        .checked_mul(8)
        .ok_or_else(|| malformed("texture pointer list size overflows usize"))?;
    if pointer_bytes > 0 {
        resource.bytes_at(texture_list.pointer, pointer_bytes)?;
    }

    let mut textures = Vec::with_capacity(texture_list.count);
    for index in 0..texture_list.count {
        let hash_pointer = at(hash_list.pointer, index as u64 * 4)?;
        let pointer_pointer = at(texture_list.pointer, index as u64 * 8)?;
        let dictionary_hash = resource.read_u32(hash_pointer)?;
        let texture_pointer = resource.read_u64(pointer_pointer)?;
        if texture_pointer == 0 {
            return Err(YtdError::Malformed(format!(
                "texture pointer at index {index} is null"
            )));
        }
        textures.push(read_texture(
            resource,
            texture_pointer,
            dictionary_hash,
            index,
        )?);
    }

    Ok(Ytd {
        resource_version: resource.header.version,
        file_vft,
        file_unknown,
        pages_info_pointer,
        textures,
    })
}

fn read_list_header(
    resource: &Rsc7Resource,
    pointer: u64,
    label: &str,
) -> Result<ListHeader, YtdError> {
    resource.bytes_at(pointer, 16)?;
    let entries_pointer = resource.read_u64(pointer)?;
    let count = usize::from(resource.read_u16(at(pointer, 0x08)?)?);
    let capacity = usize::from(resource.read_u16(at(pointer, 0x0A)?)?);

    if count > capacity {
        return Err(YtdError::Malformed(format!(
            "{label} count {count} exceeds capacity {capacity}"
        )));
    }
    if capacity > MAX_TEXTURES {
        return Err(YtdError::Malformed(format!(
            "{label} capacity {capacity} exceeds parser limit {MAX_TEXTURES}"
        )));
    }
    if count > 0 && entries_pointer == 0 {
        return Err(YtdError::Malformed(format!(
            "{label} has {count} entries but a null data pointer"
        )));
    }

    Ok(ListHeader {
        pointer: entries_pointer,
        count,
        capacity,
    })
}

fn read_texture(
    resource: &Rsc7Resource,
    pointer: u64,
    dictionary_hash: u32,
    index: usize,
) -> Result<TextureInfo, YtdError> {
    resource.bytes_at(pointer, LEGACY_TEXTURE_SIZE)?;

    let name_pointer = resource.read_u64(at(pointer, 0x28)?)?;
    let name = if name_pointer == 0 {
        String::new()
    } else {
        read_bounded_c_string(resource, name_pointer, MAX_TEXTURE_NAME_BYTES).map_err(|error| {
            YtdError::Malformed(format!("texture {index} has invalid name: {error}"))
        })?
    };
    let name_hash = joaat(&name);

    let usage_data = resource.read_u32(at(pointer, 0x40)?)?;
    let extra_flags = resource.read_u32(at(pointer, 0x48)?)?;
    let width = resource.read_u16(at(pointer, 0x50)?)?;
    let height = resource.read_u16(at(pointer, 0x52)?)?;
    let depth = resource.read_u16(at(pointer, 0x54)?)?;
    let stride = resource.read_u16(at(pointer, 0x56)?)?;
    let raw_format = resource.read_u32(at(pointer, 0x58)?)?;
    let format = TextureFormat::from_raw(raw_format);
    let levels = resource.read_u8(at(pointer, 0x5D)?)?;
    let data_pointer = resource.read_u64(at(pointer, 0x70)?)?;
    let data_length = legacy_data_length(format, width, height, stride, levels)?;

    if data_length > MAX_TEXTURE_DATA_BYTES {
        return Err(YtdError::Malformed(format!(
            "texture {index} data length {data_length} exceeds parser limit {MAX_TEXTURE_DATA_BYTES}"
        )));
    }
    if data_length > 0 {
        if data_pointer == 0 {
            return Err(YtdError::Malformed(format!(
                "texture {index} has {data_length} bytes of image data but a null data pointer"
            )));
        }
        resource.bytes_at(data_pointer, data_length)?;
    }

    Ok(TextureInfo {
        dictionary_hash,
        name,
        name_hash,
        width,
        height,
        depth,
        stride,
        format,
        levels,
        usage: (usage_data & 0x1F) as u8,
        usage_flags: usage_data >> 5,
        extra_flags,
        data_pointer,
        data_length,
    })
}

fn decode_texture_top_mip(
    resource: &Rsc7Resource,
    texture: &TextureInfo,
    index: usize,
) -> Result<DecodedTexture, YtdError> {
    let width = usize::from(texture.width);
    let height = usize::from(texture.height);
    if width == 0 || height == 0 {
        return Err(YtdError::Malformed(format!(
            "texture {index} has zero-sized dimensions {}x{}",
            texture.width, texture.height
        )));
    }

    let rgba_len = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| malformed("decoded RGBA byte length overflows usize"))?;
    if rgba_len > MAX_DECODED_RGBA_BYTES {
        return Err(YtdError::Malformed(format!(
            "texture {index} decoded RGBA length {rgba_len} exceeds preview limit {MAX_DECODED_RGBA_BYTES}"
        )));
    }

    let top_mip_len = usize::from(texture.stride)
        .checked_mul(height)
        .ok_or_else(|| malformed("top mip byte length overflows usize"))?;
    if top_mip_len == 0 {
        return Err(YtdError::Malformed(format!(
            "texture {index} has an empty top mip"
        )));
    }
    let encoded = resource.bytes_at(texture.data_pointer, top_mip_len)?;
    let mut rgba = vec![0_u8; rgba_len];

    match texture.format {
        TextureFormat::Bgra8 => decode_bgra8(
            encoded,
            width,
            height,
            usize::from(texture.stride),
            &mut rgba,
            false,
        )?,
        TextureFormat::Bgrx8 => decode_bgra8(
            encoded,
            width,
            height,
            usize::from(texture.stride),
            &mut rgba,
            true,
        )?,
        TextureFormat::B5G5R5A1 => decode_b5g5r5a1(
            encoded,
            width,
            height,
            usize::from(texture.stride),
            &mut rgba,
        )?,
        TextureFormat::A8 => decode_a8(
            encoded,
            width,
            height,
            usize::from(texture.stride),
            &mut rgba,
        )?,
        TextureFormat::Rgba8 => decode_rgba8(
            encoded,
            width,
            height,
            usize::from(texture.stride),
            &mut rgba,
        )?,
        TextureFormat::R8 => decode_r8(
            encoded,
            width,
            height,
            usize::from(texture.stride),
            &mut rgba,
        )?,
        TextureFormat::Bc1 => decode_bc1(
            encoded,
            width,
            height,
            usize::from(texture.stride),
            &mut rgba,
        )?,
        TextureFormat::Bc2 => decode_bc2(
            encoded,
            width,
            height,
            usize::from(texture.stride),
            &mut rgba,
        )?,
        TextureFormat::Bc3 => decode_bc3(
            encoded,
            width,
            height,
            usize::from(texture.stride),
            &mut rgba,
        )?,
        TextureFormat::Bc4 => decode_bc4(
            encoded,
            width,
            height,
            usize::from(texture.stride),
            &mut rgba,
        )?,
        TextureFormat::Bc5 => decode_bc5(
            encoded,
            width,
            height,
            usize::from(texture.stride),
            &mut rgba,
        )?,
        format => return Err(YtdError::UnsupportedTextureFormat(format)),
    }

    Ok(DecodedTexture {
        width: texture.width,
        height: texture.height,
        rgba,
    })
}

fn linear_row_bytes(
    encoded: &[u8],
    width: usize,
    height: usize,
    row_pitch: usize,
    bytes_per_pixel: usize,
    label: &str,
) -> Result<usize, YtdError> {
    let row_bytes = width
        .checked_mul(bytes_per_pixel)
        .ok_or_else(|| malformed(format!("{label} row byte length overflows usize")))?;
    if row_pitch < row_bytes {
        return Err(malformed(format!(
            "{label} row pitch {row_pitch} is smaller than required {row_bytes}"
        )));
    }
    let required = row_pitch
        .checked_mul(height)
        .ok_or_else(|| malformed(format!("{label} payload length overflows usize")))?;
    if encoded.len() < required {
        return Err(malformed(format!(
            "{label} payload is truncated: need {required} bytes, got {}",
            encoded.len()
        )));
    }
    Ok(row_bytes)
}

fn decode_bgra8(
    encoded: &[u8],
    width: usize,
    height: usize,
    row_pitch: usize,
    rgba: &mut [u8],
    force_opaque: bool,
) -> Result<(), YtdError> {
    let label = if force_opaque { "BGRX8" } else { "BGRA8" };
    let row_bytes = linear_row_bytes(encoded, width, height, row_pitch, 4, label)?;
    for y in 0..height {
        let source_row = y * row_pitch;
        for x in 0..width {
            let source = source_row + x * 4;
            let target = (y * width + x) * 4;
            rgba[target] = encoded[source + 2];
            rgba[target + 1] = encoded[source + 1];
            rgba[target + 2] = encoded[source];
            rgba[target + 3] = if force_opaque {
                255
            } else {
                encoded[source + 3]
            };
        }
        debug_assert!(source_row + row_bytes <= encoded.len());
    }
    Ok(())
}

fn decode_b5g5r5a1(
    encoded: &[u8],
    width: usize,
    height: usize,
    row_pitch: usize,
    rgba: &mut [u8],
) -> Result<(), YtdError> {
    linear_row_bytes(encoded, width, height, row_pitch, 2, "B5G5R5A1")?;
    for y in 0..height {
        let source_row = y * row_pitch;
        for x in 0..width {
            let source = source_row + x * 2;
            let value = u16::from_le_bytes([encoded[source], encoded[source + 1]]);
            let target = (y * width + x) * 4;
            rgba[target] = expand_5_to_8(((value >> 10) & 0x1F) as u8);
            rgba[target + 1] = expand_5_to_8(((value >> 5) & 0x1F) as u8);
            rgba[target + 2] = expand_5_to_8((value & 0x1F) as u8);
            rgba[target + 3] = if value & 0x8000 != 0 { 255 } else { 0 };
        }
    }
    Ok(())
}

fn decode_a8(
    encoded: &[u8],
    width: usize,
    height: usize,
    row_pitch: usize,
    rgba: &mut [u8],
) -> Result<(), YtdError> {
    linear_row_bytes(encoded, width, height, row_pitch, 1, "A8")?;
    for y in 0..height {
        let source_row = y * row_pitch;
        for x in 0..width {
            let alpha = encoded[source_row + x];
            let target = (y * width + x) * 4;
            rgba[target..target + 4].copy_from_slice(&[255, 255, 255, alpha]);
        }
    }
    Ok(())
}

fn decode_r8(
    encoded: &[u8],
    width: usize,
    height: usize,
    row_pitch: usize,
    rgba: &mut [u8],
) -> Result<(), YtdError> {
    linear_row_bytes(encoded, width, height, row_pitch, 1, "R8/L8")?;
    for y in 0..height {
        let source_row = y * row_pitch;
        for x in 0..width {
            let value = encoded[source_row + x];
            let target = (y * width + x) * 4;
            rgba[target..target + 4].copy_from_slice(&[value, value, value, 255]);
        }
    }
    Ok(())
}

fn decode_rgba8(
    encoded: &[u8],
    width: usize,
    height: usize,
    row_pitch: usize,
    rgba: &mut [u8],
) -> Result<(), YtdError> {
    let row_bytes = linear_row_bytes(encoded, width, height, row_pitch, 4, "RGBA8")?;
    for y in 0..height {
        let source = y * row_pitch;
        let target = y * row_bytes;
        rgba[target..target + row_bytes].copy_from_slice(&encoded[source..source + row_bytes]);
    }
    Ok(())
}

fn expand_5_to_8(value: u8) -> u8 {
    (value << 3) | (value >> 2)
}

fn decode_bc1(
    encoded: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    rgba: &mut [u8],
) -> Result<(), YtdError> {
    decode_bc_color_blocks(encoded, width, height, stride, 8, rgba, false)
}

fn decode_bc2(
    encoded: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    rgba: &mut [u8],
) -> Result<(), YtdError> {
    decode_bc_color_blocks(encoded, width, height, stride, 16, rgba, true)?;
    let blocks_x = width.div_ceil(4);
    let blocks_y = height.div_ceil(4);
    let row_pitch = stride
        .checked_mul(4)
        .ok_or_else(|| malformed("BC2 row pitch overflows usize"))?;

    for block_y in 0..blocks_y {
        for block_x in 0..blocks_x {
            let offset = block_y * row_pitch + block_x * 16;
            let alpha_bits = u64::from_le_bytes(
                encoded[offset..offset + 8]
                    .try_into()
                    .expect("fixed BC2 alpha slice"),
            );
            for pixel_y in 0..4 {
                let y = block_y * 4 + pixel_y;
                if y >= height {
                    continue;
                }
                for pixel_x in 0..4 {
                    let x = block_x * 4 + pixel_x;
                    if x >= width {
                        continue;
                    }
                    let pixel = pixel_y * 4 + pixel_x;
                    let alpha = ((alpha_bits >> (pixel * 4)) & 0x0F) as u8 * 17;
                    rgba[(y * width + x) * 4 + 3] = alpha;
                }
            }
        }
    }
    Ok(())
}

fn decode_bc4(
    encoded: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    rgba: &mut [u8],
) -> Result<(), YtdError> {
    let blocks_x = width.div_ceil(4);
    let blocks_y = height.div_ceil(4);
    let row_pitch = stride
        .checked_mul(4)
        .ok_or_else(|| malformed("BC4 row pitch overflows usize"))?;
    validate_bc_payload(encoded, blocks_x, blocks_y, row_pitch, 8, "BC4")?;

    for block_y in 0..blocks_y {
        for block_x in 0..blocks_x {
            let offset = block_y * row_pitch + block_x * 8;
            let values = decode_bc_alpha_values(&encoded[offset..offset + 8]);
            for pixel_y in 0..4 {
                let y = block_y * 4 + pixel_y;
                if y >= height {
                    continue;
                }
                for pixel_x in 0..4 {
                    let x = block_x * 4 + pixel_x;
                    if x >= width {
                        continue;
                    }
                    let value = values[pixel_y * 4 + pixel_x];
                    let target = (y * width + x) * 4;
                    rgba[target..target + 4].copy_from_slice(&[value, 0, 0, 255]);
                }
            }
        }
    }
    Ok(())
}

fn decode_bc5(
    encoded: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    rgba: &mut [u8],
) -> Result<(), YtdError> {
    let blocks_x = width.div_ceil(4);
    let blocks_y = height.div_ceil(4);
    let row_pitch = stride
        .checked_mul(4)
        .ok_or_else(|| malformed("BC5 row pitch overflows usize"))?;
    validate_bc_payload(encoded, blocks_x, blocks_y, row_pitch, 16, "BC5")?;

    for block_y in 0..blocks_y {
        for block_x in 0..blocks_x {
            let offset = block_y * row_pitch + block_x * 16;
            let red = decode_bc_alpha_values(&encoded[offset..offset + 8]);
            let green = decode_bc_alpha_values(&encoded[offset + 8..offset + 16]);
            for pixel_y in 0..4 {
                let y = block_y * 4 + pixel_y;
                if y >= height {
                    continue;
                }
                for pixel_x in 0..4 {
                    let x = block_x * 4 + pixel_x;
                    if x >= width {
                        continue;
                    }
                    let pixel = pixel_y * 4 + pixel_x;
                    let target = (y * width + x) * 4;
                    rgba[target..target + 4].copy_from_slice(&[red[pixel], green[pixel], 0, 255]);
                }
            }
        }
    }
    Ok(())
}

fn validate_bc_payload(
    encoded: &[u8],
    blocks_x: usize,
    blocks_y: usize,
    row_pitch: usize,
    block_size: usize,
    label: &str,
) -> Result<(), YtdError> {
    let tight_row = blocks_x
        .checked_mul(block_size)
        .ok_or_else(|| malformed(format!("{label} block-row length overflows usize")))?;
    if row_pitch < tight_row {
        return Err(malformed(format!(
            "{label} row pitch {row_pitch} is smaller than required {tight_row}"
        )));
    }
    let required = row_pitch
        .checked_mul(blocks_y)
        .ok_or_else(|| malformed(format!("{label} payload length overflows usize")))?;
    if encoded.len() < required {
        return Err(malformed(format!(
            "{label} payload is truncated: need {required} bytes, got {}",
            encoded.len()
        )));
    }
    Ok(())
}

fn decode_bc_alpha_values(block: &[u8]) -> [u8; 16] {
    let palette = bc3_alpha_palette(block[0], block[1]);
    let mut indices = 0_u64;
    for (shift, byte) in block[2..8].iter().enumerate() {
        indices |= u64::from(*byte) << (shift * 8);
    }
    let mut values = [0_u8; 16];
    for (pixel, value) in values.iter_mut().enumerate() {
        let index = ((indices >> (pixel * 3)) & 0x07) as usize;
        *value = palette[index];
    }
    values
}

fn decode_bc_color_blocks(
    encoded: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    block_size: usize,
    rgba: &mut [u8],
    force_four_color: bool,
) -> Result<(), YtdError> {
    let blocks_x = width.div_ceil(4);
    let blocks_y = height.div_ceil(4);
    let tight_row = blocks_x
        .checked_mul(block_size)
        .ok_or_else(|| malformed("BC block-row length overflows usize"))?;
    let row_pitch = stride
        .checked_mul(4)
        .ok_or_else(|| malformed("BC row pitch overflows usize"))?;
    if row_pitch < tight_row {
        return Err(malformed(format!(
            "BC row pitch {row_pitch} is smaller than required {tight_row}"
        )));
    }
    let required = row_pitch
        .checked_mul(blocks_y)
        .ok_or_else(|| malformed("BC payload length overflows usize"))?;
    if encoded.len() < required {
        return Err(malformed(format!(
            "BC payload is truncated: need {required} bytes, got {}",
            encoded.len()
        )));
    }

    for block_y in 0..blocks_y {
        for block_x in 0..blocks_x {
            let offset = block_y * row_pitch + block_x * block_size;
            let block = &encoded[offset..offset + block_size];
            let color_offset = block_size - 8;
            let colors = bc1_palette(&block[color_offset..], force_four_color);
            let indices = u32::from_le_bytes(
                block[color_offset + 4..color_offset + 8]
                    .try_into()
                    .expect("fixed BC color-index slice"),
            );

            for pixel_y in 0..4 {
                let y = block_y * 4 + pixel_y;
                if y >= height {
                    continue;
                }
                for pixel_x in 0..4 {
                    let x = block_x * 4 + pixel_x;
                    if x >= width {
                        continue;
                    }
                    let pixel = pixel_y * 4 + pixel_x;
                    let color_index = ((indices >> (pixel * 2)) & 0x03) as usize;
                    let target = (y * width + x) * 4;
                    rgba[target..target + 4].copy_from_slice(&colors[color_index]);
                }
            }
        }
    }
    Ok(())
}

fn decode_bc3(
    encoded: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    rgba: &mut [u8],
) -> Result<(), YtdError> {
    decode_bc_color_blocks(encoded, width, height, stride, 16, rgba, true)?;

    let blocks_x = width.div_ceil(4);
    let blocks_y = height.div_ceil(4);
    let row_pitch = stride
        .checked_mul(4)
        .ok_or_else(|| malformed("BC3 row pitch overflows usize"))?;
    for block_y in 0..blocks_y {
        for block_x in 0..blocks_x {
            let offset = block_y * row_pitch + block_x * 16;
            let block = &encoded[offset..offset + 16];
            let alpha_palette = bc3_alpha_palette(block[0], block[1]);
            let mut alpha_indices = 0_u64;
            for (shift, byte) in block[2..8].iter().enumerate() {
                alpha_indices |= u64::from(*byte) << (shift * 8);
            }

            for pixel_y in 0..4 {
                let y = block_y * 4 + pixel_y;
                if y >= height {
                    continue;
                }
                for pixel_x in 0..4 {
                    let x = block_x * 4 + pixel_x;
                    if x >= width {
                        continue;
                    }
                    let pixel = pixel_y * 4 + pixel_x;
                    let alpha_index = ((alpha_indices >> (pixel * 3)) & 0x07) as usize;
                    rgba[(y * width + x) * 4 + 3] = alpha_palette[alpha_index];
                }
            }
        }
    }
    Ok(())
}

fn bc1_palette(block: &[u8], force_four_color: bool) -> [[u8; 4]; 4] {
    let color0 = u16::from_le_bytes([block[0], block[1]]);
    let color1 = u16::from_le_bytes([block[2], block[3]]);
    let rgb0 = rgb565(color0);
    let rgb1 = rgb565(color1);
    let mut colors = [[0_u8; 4]; 4];
    colors[0] = [rgb0[0], rgb0[1], rgb0[2], 255];
    colors[1] = [rgb1[0], rgb1[1], rgb1[2], 255];

    if force_four_color || color0 > color1 {
        colors[2] = [
            weighted_channel(rgb0[0], rgb1[0], 2, 1, 3),
            weighted_channel(rgb0[1], rgb1[1], 2, 1, 3),
            weighted_channel(rgb0[2], rgb1[2], 2, 1, 3),
            255,
        ];
        colors[3] = [
            weighted_channel(rgb0[0], rgb1[0], 1, 2, 3),
            weighted_channel(rgb0[1], rgb1[1], 1, 2, 3),
            weighted_channel(rgb0[2], rgb1[2], 1, 2, 3),
            255,
        ];
    } else {
        colors[2] = [
            weighted_channel(rgb0[0], rgb1[0], 1, 1, 2),
            weighted_channel(rgb0[1], rgb1[1], 1, 1, 2),
            weighted_channel(rgb0[2], rgb1[2], 1, 1, 2),
            255,
        ];
        colors[3] = [0, 0, 0, 0];
    }
    colors
}

fn bc3_alpha_palette(alpha0: u8, alpha1: u8) -> [u8; 8] {
    let mut alphas = [0_u8; 8];
    alphas[0] = alpha0;
    alphas[1] = alpha1;
    if alpha0 > alpha1 {
        for index in 1..=6 {
            alphas[index + 1] = weighted_channel(alpha0, alpha1, 7 - index, index, 7);
        }
    } else {
        for index in 1..=4 {
            alphas[index + 1] = weighted_channel(alpha0, alpha1, 5 - index, index, 5);
        }
        alphas[6] = 0;
        alphas[7] = 255;
    }
    alphas
}

fn rgb565(value: u16) -> [u8; 3] {
    let red = ((value >> 11) & 0x1F) as u32;
    let green = ((value >> 5) & 0x3F) as u32;
    let blue = (value & 0x1F) as u32;
    [
        ((red * 255 + 15) / 31) as u8,
        ((green * 255 + 31) / 63) as u8,
        ((blue * 255 + 15) / 31) as u8,
    ]
}

fn weighted_channel(
    left: u8,
    right: u8,
    left_weight: usize,
    right_weight: usize,
    divisor: usize,
) -> u8 {
    ((usize::from(left) * left_weight + usize::from(right) * right_weight) / divisor) as u8
}

fn parse_classic_dds(bytes: &[u8]) -> Result<ParsedDds<'_>, YtdError> {
    const DDS_HEADER_BYTES: usize = 128;
    const DDPF_ALPHAPIXELS: u32 = 0x0000_0001;
    const DDPF_FOURCC: u32 = 0x0000_0004;
    const DDPF_RGB: u32 = 0x0000_0040;

    if bytes.len() < DDS_HEADER_BYTES {
        return Err(YtdError::InvalidDds(format!(
            "expected at least {DDS_HEADER_BYTES} bytes, got {}",
            bytes.len()
        )));
    }
    if bytes.get(0..4) != Some(&b"DDS "[..]) {
        return Err(YtdError::InvalidDds("missing `DDS ` magic".into()));
    }
    if read_dds_u32(bytes, 4)? != 124 {
        return Err(YtdError::InvalidDds(
            "classic DDS header size must be 124 bytes".into(),
        ));
    }
    if read_dds_u32(bytes, 76)? != 32 {
        return Err(YtdError::InvalidDds(
            "classic DDS pixel-format size must be 32 bytes".into(),
        ));
    }

    let width_raw = read_dds_u32(bytes, 16)?;
    let height_raw = read_dds_u32(bytes, 12)?;
    if width_raw == 0 || height_raw == 0 {
        return Err(YtdError::InvalidDds(
            "width and height must both be non-zero".into(),
        ));
    }
    let width = u16::try_from(width_raw)
        .map_err(|_| YtdError::InvalidDds(format!("width {width_raw} does not fit legacy YTD")))?;
    let height = u16::try_from(height_raw).map_err(|_| {
        YtdError::InvalidDds(format!("height {height_raw} does not fit legacy YTD"))
    })?;

    let levels_raw = read_dds_u32(bytes, 28)?;
    let levels_raw = if levels_raw == 0 { 1 } else { levels_raw };
    let levels = u8::try_from(levels_raw).map_err(|_| {
        YtdError::InvalidDds(format!("mip count {levels_raw} does not fit legacy YTD"))
    })?;

    let pixel_flags = read_dds_u32(bytes, 80)?;
    let fourcc = bytes
        .get(84..88)
        .ok_or_else(|| YtdError::InvalidDds("truncated FourCC field".into()))?;
    let format = if pixel_flags & DDPF_FOURCC != 0 {
        match fourcc {
            b"DXT1" => TextureFormat::Bc1,
            b"DXT5" => TextureFormat::Bc3,
            b"DX10" => {
                return Err(YtdError::InvalidDds(
                    "DX10-extended DDS is not supported by layout-preserving replacement".into(),
                ));
            }
            other => {
                return Err(YtdError::InvalidDds(format!(
                    "unsupported DDS FourCC {:?}",
                    String::from_utf8_lossy(other)
                )));
            }
        }
    } else {
        let required_flags = DDPF_RGB | DDPF_ALPHAPIXELS;
        let bit_count = read_dds_u32(bytes, 88)?;
        let red_mask = read_dds_u32(bytes, 92)?;
        let green_mask = read_dds_u32(bytes, 96)?;
        let blue_mask = read_dds_u32(bytes, 100)?;
        let alpha_mask = read_dds_u32(bytes, 104)?;
        if pixel_flags & required_flags != required_flags
            || bit_count != 32
            || red_mask != 0x0000_00FF
            || green_mask != 0x0000_FF00
            || blue_mask != 0x00FF_0000
            || alpha_mask != 0xFF00_0000
        {
            return Err(YtdError::InvalidDds(
                "only classic RGBA8 masks emitted by RageLab are supported for uncompressed replacement"
                    .into(),
            ));
        }
        TextureFormat::Rgba8
    };

    let layout = mip_layout(format, width, height, levels)?
        .ok_or(YtdError::UnsupportedTextureFormat(format))?;
    let expected_data = layout.last().map_or(0, |mip| mip.offset + mip.slice_pitch);
    let expected_total = DDS_HEADER_BYTES
        .checked_add(expected_data)
        .ok_or_else(|| YtdError::InvalidDds("DDS byte length overflows usize".into()))?;
    if bytes.len() != expected_total {
        return Err(YtdError::InvalidDds(format!(
            "payload length mismatch: header describes {expected_data} bytes, file contains {}",
            bytes.len().saturating_sub(DDS_HEADER_BYTES)
        )));
    }

    Ok(ParsedDds {
        width,
        height,
        format,
        levels,
        data: &bytes[DDS_HEADER_BYTES..],
    })
}

fn read_dds_u32(bytes: &[u8], offset: usize) -> Result<u32, YtdError> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| YtdError::InvalidDds(format!("truncated u32 at byte offset {offset}")))?;
    Ok(u32::from_le_bytes(
        slice.try_into().expect("fixed four-byte DDS field"),
    ))
}

fn build_dds(texture: &TextureInfo, data: &[u8]) -> Result<Vec<u8>, YtdError> {
    if texture.depth != 1 {
        return Err(YtdError::Malformed(format!(
            "DDS export currently supports 2D textures only; depth is {}",
            texture.depth
        )));
    }
    if texture.width == 0 || texture.height == 0 || texture.levels == 0 {
        return Err(YtdError::Malformed(
            "DDS export requires non-zero width, height and mip count".into(),
        ));
    }
    if !matches!(
        texture.format,
        TextureFormat::Rgba8 | TextureFormat::Bc1 | TextureFormat::Bc3
    ) {
        return Err(YtdError::UnsupportedTextureFormat(texture.format));
    }

    let layout = mip_layout(
        texture.format,
        texture.width,
        texture.height,
        texture.levels,
    )?
    .ok_or(YtdError::UnsupportedTextureFormat(texture.format))?;
    let expected = layout.last().map_or(0, |mip| mip.offset + mip.slice_pitch);
    if data.len() != expected {
        return Err(YtdError::Malformed(format!(
            "DDS source payload length mismatch: expected {expected} bytes, got {}",
            data.len()
        )));
    }

    const DDSD_CAPS: u32 = 0x0000_0001;
    const DDSD_HEIGHT: u32 = 0x0000_0002;
    const DDSD_WIDTH: u32 = 0x0000_0004;
    const DDSD_PITCH: u32 = 0x0000_0008;
    const DDSD_PIXELFORMAT: u32 = 0x0000_1000;
    const DDSD_MIPMAPCOUNT: u32 = 0x0002_0000;
    const DDSD_LINEARSIZE: u32 = 0x0008_0000;
    const DDPF_ALPHAPIXELS: u32 = 0x0000_0001;
    const DDPF_FOURCC: u32 = 0x0000_0004;
    const DDPF_RGB: u32 = 0x0000_0040;
    const DDSCAPS_COMPLEX: u32 = 0x0000_0008;
    const DDSCAPS_TEXTURE: u32 = 0x0000_1000;
    const DDSCAPS_MIPMAP: u32 = 0x0040_0000;

    let top = layout
        .first()
        .ok_or_else(|| malformed("DDS mip layout is empty"))?;
    let compressed = matches!(texture.format, TextureFormat::Bc1 | TextureFormat::Bc3);
    let mut flags = DDSD_CAPS | DDSD_HEIGHT | DDSD_WIDTH | DDSD_PIXELFORMAT;
    flags |= if compressed {
        DDSD_LINEARSIZE
    } else {
        DDSD_PITCH
    };
    if texture.levels > 1 {
        flags |= DDSD_MIPMAPCOUNT;
    }

    let mut caps = DDSCAPS_TEXTURE;
    if texture.levels > 1 {
        caps |= DDSCAPS_COMPLEX | DDSCAPS_MIPMAP;
    }

    let mut output = Vec::with_capacity(128 + data.len());
    output.extend_from_slice(b"DDS ");
    push_u32_le(&mut output, 124);
    push_u32_le(&mut output, flags);
    push_u32_le(&mut output, u32::from(texture.height));
    push_u32_le(&mut output, u32::from(texture.width));
    push_u32_le(
        &mut output,
        u32::try_from(if compressed {
            top.slice_pitch
        } else {
            top.row_pitch
        })
        .map_err(|_| malformed("DDS pitch does not fit u32"))?,
    );
    push_u32_le(&mut output, 0);
    push_u32_le(&mut output, u32::from(texture.levels));
    for _ in 0..11 {
        push_u32_le(&mut output, 0);
    }

    push_u32_le(&mut output, 32);
    match texture.format {
        TextureFormat::Rgba8 => {
            push_u32_le(&mut output, DDPF_RGB | DDPF_ALPHAPIXELS);
            push_u32_le(&mut output, 0);
            push_u32_le(&mut output, 32);
            push_u32_le(&mut output, 0x0000_00FF);
            push_u32_le(&mut output, 0x0000_FF00);
            push_u32_le(&mut output, 0x00FF_0000);
            push_u32_le(&mut output, 0xFF00_0000);
        }
        TextureFormat::Bc1 => {
            push_u32_le(&mut output, DDPF_FOURCC);
            output.extend_from_slice(b"DXT1");
            for _ in 0..5 {
                push_u32_le(&mut output, 0);
            }
        }
        TextureFormat::Bc3 => {
            push_u32_le(&mut output, DDPF_FOURCC);
            output.extend_from_slice(b"DXT5");
            for _ in 0..5 {
                push_u32_le(&mut output, 0);
            }
        }
        format => return Err(YtdError::UnsupportedTextureFormat(format)),
    }
    push_u32_le(&mut output, caps);
    for _ in 0..4 {
        push_u32_le(&mut output, 0);
    }
    debug_assert_eq!(output.len(), 128);
    output.extend_from_slice(data);
    Ok(output)
}

fn push_u32_le(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

fn generate_rgba8_mip_payload(
    width: u16,
    height: u16,
    rgba: &[u8],
) -> Result<(u8, Vec<u8>), YtdError> {
    if width == 0 || height == 0 {
        return Err(YtdError::InvalidRgba(
            "width and height must both be non-zero".into(),
        ));
    }

    let expected_top = usize::from(width)
        .checked_mul(usize::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| YtdError::InvalidRgba("top-mip byte length overflows usize".into()))?;
    if expected_top > MAX_DECODED_RGBA_BYTES {
        return Err(YtdError::InvalidRgba(format!(
            "top mip requires {expected_top} bytes, above the {MAX_DECODED_RGBA_BYTES}-byte limit"
        )));
    }
    if rgba.len() != expected_top {
        return Err(YtdError::InvalidRgba(format!(
            "{}x{} RGBA8 requires {expected_top} bytes, got {}",
            width,
            height,
            rgba.len()
        )));
    }

    let mut current_width = usize::from(width);
    let mut current_height = usize::from(height);
    let mut current = rgba.to_vec();
    let mut payload = Vec::with_capacity(expected_top);
    let mut levels = 0_usize;

    loop {
        let total = payload
            .len()
            .checked_add(current.len())
            .ok_or_else(|| YtdError::InvalidRgba("generated mip payload overflows usize".into()))?;
        if total > MAX_GENERATED_RGBA_MIP_BYTES {
            return Err(YtdError::InvalidRgba(format!(
                "generated mip payload requires {total} bytes, above the {MAX_GENERATED_RGBA_MIP_BYTES}-byte limit"
            )));
        }
        payload.extend_from_slice(&current);
        levels += 1;

        if current_width == 1 && current_height == 1 {
            break;
        }
        let (next_width, next_height, next) =
            downsample_rgba8_box(&current, current_width, current_height)?;
        current_width = next_width;
        current_height = next_height;
        current = next;
    }

    let levels = u8::try_from(levels)
        .map_err(|_| YtdError::InvalidRgba("generated mip count does not fit u8".into()))?;
    Ok((levels, payload))
}

fn generate_bc1_mip_payload(
    width: u16,
    height: u16,
    rgba: &[u8],
) -> Result<(u8, Vec<u8>), YtdError> {
    validate_rgba_shape(usize::from(width), usize::from(height), rgba)?;

    let mut current_width = usize::from(width);
    let mut current_height = usize::from(height);
    let mut current = rgba.to_vec();
    let mut payload = Vec::new();
    let mut levels = 0_usize;

    loop {
        let encoded = encode_bc1_image(&current, current_width, current_height)?;
        let total = payload.len().checked_add(encoded.len()).ok_or_else(|| {
            YtdError::InvalidRgba("generated BC1 mip payload overflows usize".into())
        })?;
        if total > MAX_GENERATED_RGBA_MIP_BYTES {
            return Err(YtdError::InvalidRgba(format!(
                "generated BC1 mip payload requires {total} bytes, above the {MAX_GENERATED_RGBA_MIP_BYTES}-byte limit"
            )));
        }
        payload.extend_from_slice(&encoded);
        levels += 1;

        if current_width == 1 && current_height == 1 {
            break;
        }
        let (next_width, next_height, next) =
            downsample_rgba8_box(&current, current_width, current_height)?;
        current_width = next_width;
        current_height = next_height;
        current = next;
    }

    let levels = u8::try_from(levels)
        .map_err(|_| YtdError::InvalidRgba("generated mip count does not fit u8".into()))?;
    Ok((levels, payload))
}

fn generate_bc3_mip_payload(
    width: u16,
    height: u16,
    rgba: &[u8],
) -> Result<(u8, Vec<u8>), YtdError> {
    validate_rgba_shape(usize::from(width), usize::from(height), rgba)?;

    let mut current_width = usize::from(width);
    let mut current_height = usize::from(height);
    let mut current = rgba.to_vec();
    let mut payload = Vec::new();
    let mut levels = 0_usize;

    loop {
        let encoded = encode_bc3_image(&current, current_width, current_height)?;
        let total = payload.len().checked_add(encoded.len()).ok_or_else(|| {
            YtdError::InvalidRgba("generated BC3 mip payload overflows usize".into())
        })?;
        if total > MAX_GENERATED_RGBA_MIP_BYTES {
            return Err(YtdError::InvalidRgba(format!(
                "generated BC3 mip payload requires {total} bytes, above the {MAX_GENERATED_RGBA_MIP_BYTES}-byte limit"
            )));
        }
        payload.extend_from_slice(&encoded);
        levels += 1;

        if current_width == 1 && current_height == 1 {
            break;
        }
        let (next_width, next_height, next) =
            downsample_rgba8_box(&current, current_width, current_height)?;
        current_width = next_width;
        current_height = next_height;
        current = next;
    }

    let levels = u8::try_from(levels)
        .map_err(|_| YtdError::InvalidRgba("generated mip count does not fit u8".into()))?;
    Ok((levels, payload))
}

fn encode_bc3_image(rgba: &[u8], width: usize, height: usize) -> Result<Vec<u8>, YtdError> {
    validate_rgba_shape(width, height, rgba)?;
    let blocks_x = width.div_ceil(4);
    let blocks_y = height.div_ceil(4);
    let encoded_len = blocks_x
        .checked_mul(blocks_y)
        .and_then(|blocks| blocks.checked_mul(16))
        .ok_or_else(|| YtdError::InvalidRgba("BC3 encoded byte length overflows usize".into()))?;
    let mut output = Vec::with_capacity(encoded_len);

    for block_y in 0..blocks_y {
        for block_x in 0..blocks_x {
            let block = gather_rgba_block(rgba, width, height, block_x, block_y);
            output.extend_from_slice(&encode_bc3_block(&block));
        }
    }
    Ok(output)
}

fn encode_bc1_image(rgba: &[u8], width: usize, height: usize) -> Result<Vec<u8>, YtdError> {
    validate_rgba_shape(width, height, rgba)?;
    let blocks_x = width.div_ceil(4);
    let blocks_y = height.div_ceil(4);
    let encoded_len = blocks_x
        .checked_mul(blocks_y)
        .and_then(|blocks| blocks.checked_mul(8))
        .ok_or_else(|| YtdError::InvalidRgba("BC1 encoded byte length overflows usize".into()))?;
    let mut output = Vec::with_capacity(encoded_len);

    for block_y in 0..blocks_y {
        for block_x in 0..blocks_x {
            let block = gather_rgba_block(rgba, width, height, block_x, block_y);
            output.extend_from_slice(&encode_bc1_block(&block));
        }
    }
    Ok(output)
}

fn validate_rgba_shape(width: usize, height: usize, rgba: &[u8]) -> Result<(), YtdError> {
    if width == 0 || height == 0 {
        return Err(YtdError::InvalidRgba(
            "width and height must both be non-zero".into(),
        ));
    }
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| YtdError::InvalidRgba("RGBA byte length overflows usize".into()))?;
    if expected > MAX_DECODED_RGBA_BYTES {
        return Err(YtdError::InvalidRgba(format!(
            "RGBA image requires {expected} bytes, above the {MAX_DECODED_RGBA_BYTES}-byte limit"
        )));
    }
    if rgba.len() != expected {
        return Err(YtdError::InvalidRgba(format!(
            "{width}x{height} RGBA8 requires {expected} bytes, got {}",
            rgba.len()
        )));
    }
    Ok(())
}

fn gather_rgba_block(
    rgba: &[u8],
    width: usize,
    height: usize,
    block_x: usize,
    block_y: usize,
) -> [[u8; 4]; 16] {
    let mut block = [[0_u8; 4]; 16];
    for pixel_y in 0..4 {
        let source_y = (block_y * 4 + pixel_y).min(height - 1);
        for pixel_x in 0..4 {
            let source_x = (block_x * 4 + pixel_x).min(width - 1);
            let source = (source_y * width + source_x) * 4;
            block[pixel_y * 4 + pixel_x].copy_from_slice(&rgba[source..source + 4]);
        }
    }
    block
}

fn encode_bc1_block(pixels: &[[u8; 4]; 16]) -> [u8; 8] {
    let has_transparency = pixels.iter().any(|pixel| pixel[3] < 128);
    let mut min_rgb = [u8::MAX; 3];
    let mut max_rgb = [u8::MIN; 3];
    let mut color_pixels = 0_usize;

    for pixel in pixels {
        if has_transparency && pixel[3] < 128 {
            continue;
        }
        color_pixels += 1;
        for channel in 0..3 {
            min_rgb[channel] = min_rgb[channel].min(pixel[channel]);
            max_rgb[channel] = max_rgb[channel].max(pixel[channel]);
        }
    }

    if color_pixels == 0 {
        return [0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF];
    }

    let low = pack_rgb565(min_rgb);
    let high = pack_rgb565(max_rgb);
    let (color0, color1) = if has_transparency {
        if low <= high {
            (low, high)
        } else {
            (high, low)
        }
    } else if high >= low {
        (high, low)
    } else {
        (low, high)
    };

    let mut output = [0_u8; 8];
    output[0..2].copy_from_slice(&color0.to_le_bytes());
    output[2..4].copy_from_slice(&color1.to_le_bytes());
    let palette = bc1_palette(&output, false);
    let opaque_palette_len = if has_transparency || color0 <= color1 {
        3
    } else {
        4
    };
    let mut selectors = 0_u32;

    for (pixel_index, pixel) in pixels.iter().enumerate() {
        let selector = if has_transparency && pixel[3] < 128 {
            3_u32
        } else {
            nearest_rgb_palette_index(pixel, &palette[..opaque_palette_len]) as u32
        };
        selectors |= selector << (pixel_index * 2);
    }
    output[4..8].copy_from_slice(&selectors.to_le_bytes());
    output
}

fn encode_bc3_block(pixels: &[[u8; 4]; 16]) -> [u8; 16] {
    let mut output = [0_u8; 16];
    output[0..8].copy_from_slice(&encode_bc3_alpha_block(pixels));
    output[8..16].copy_from_slice(&encode_bc3_color_block(pixels));
    output
}

fn encode_bc3_alpha_block(pixels: &[[u8; 4]; 16]) -> [u8; 8] {
    let alpha0 = pixels.iter().map(|pixel| pixel[3]).max().unwrap_or(0);
    let alpha1 = pixels.iter().map(|pixel| pixel[3]).min().unwrap_or(0);
    let palette = bc3_alpha_palette(alpha0, alpha1);
    let mut selectors = 0_u64;

    for (pixel_index, pixel) in pixels.iter().enumerate() {
        let selector = nearest_alpha_palette_index(pixel[3], &palette) as u64;
        selectors |= selector << (pixel_index * 3);
    }

    let mut output = [0_u8; 8];
    output[0] = alpha0;
    output[1] = alpha1;
    for byte in 0..6 {
        output[2 + byte] = ((selectors >> (byte * 8)) & 0xFF) as u8;
    }
    output
}

fn encode_bc3_color_block(pixels: &[[u8; 4]; 16]) -> [u8; 8] {
    let mut min_rgb = [u8::MAX; 3];
    let mut max_rgb = [u8::MIN; 3];
    for pixel in pixels {
        for channel in 0..3 {
            min_rgb[channel] = min_rgb[channel].min(pixel[channel]);
            max_rgb[channel] = max_rgb[channel].max(pixel[channel]);
        }
    }

    let low = pack_rgb565(min_rgb);
    let high = pack_rgb565(max_rgb);
    let (color0, color1) = if high >= low {
        (high, low)
    } else {
        (low, high)
    };
    let mut output = [0_u8; 8];
    output[0..2].copy_from_slice(&color0.to_le_bytes());
    output[2..4].copy_from_slice(&color1.to_le_bytes());
    let palette = bc1_palette(&output, true);
    let mut selectors = 0_u32;
    for (pixel_index, pixel) in pixels.iter().enumerate() {
        let selector = nearest_rgb_palette_index(pixel, &palette) as u32;
        selectors |= selector << (pixel_index * 2);
    }
    output[4..8].copy_from_slice(&selectors.to_le_bytes());
    output
}

fn nearest_alpha_palette_index(alpha: u8, palette: &[u8; 8]) -> usize {
    let mut best_index = 0_usize;
    let mut best_error = u16::MAX;
    for (index, candidate) in palette.iter().enumerate() {
        let error = u16::from(alpha.abs_diff(*candidate));
        if error < best_error {
            best_error = error;
            best_index = index;
        }
    }
    best_index
}

fn pack_rgb565(rgb: [u8; 3]) -> u16 {
    let red = (u16::from(rgb[0]) * 31 + 127) / 255;
    let green = (u16::from(rgb[1]) * 63 + 127) / 255;
    let blue = (u16::from(rgb[2]) * 31 + 127) / 255;
    (red << 11) | (green << 5) | blue
}

fn nearest_rgb_palette_index(pixel: &[u8; 4], palette: &[[u8; 4]]) -> usize {
    let mut best_index = 0_usize;
    let mut best_error = u32::MAX;
    for (index, color) in palette.iter().enumerate() {
        let mut error = 0_u32;
        for channel in 0..3 {
            let delta = i32::from(pixel[channel]) - i32::from(color[channel]);
            error += (delta * delta) as u32;
        }
        if error < best_error {
            best_error = error;
            best_index = index;
        }
    }
    best_index
}

fn downsample_rgba8_box(
    source: &[u8],
    width: usize,
    height: usize,
) -> Result<(usize, usize, Vec<u8>), YtdError> {
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| YtdError::InvalidRgba("source mip byte length overflows usize".into()))?;
    if width == 0 || height == 0 || source.len() != expected {
        return Err(YtdError::InvalidRgba(format!(
            "invalid source mip shape {width}x{height} for {} bytes",
            source.len()
        )));
    }
    if width == 1 && height == 1 {
        return Ok((1, 1, source.to_vec()));
    }

    let next_width = (width / 2).max(1);
    let next_height = (height / 2).max(1);
    let next_len = next_width
        .checked_mul(next_height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| {
            YtdError::InvalidRgba("downsampled mip byte length overflows usize".into())
        })?;
    let mut output = vec![0_u8; next_len];

    for y in 0..next_height {
        let source_y0 = y * height / next_height;
        let source_y1 = ((y + 1) * height / next_height)
            .max(source_y0 + 1)
            .min(height);
        for x in 0..next_width {
            let source_x0 = x * width / next_width;
            let source_x1 = ((x + 1) * width / next_width).max(source_x0 + 1).min(width);
            let mut sums = [0_u64; 4];
            let mut samples = 0_u64;
            for source_y in source_y0..source_y1 {
                for source_x in source_x0..source_x1 {
                    let offset = (source_y * width + source_x) * 4;
                    for (channel, sum) in sums.iter_mut().enumerate() {
                        *sum += u64::from(source[offset + channel]);
                    }
                    samples += 1;
                }
            }
            let target = (y * next_width + x) * 4;
            for (channel, sum) in sums.into_iter().enumerate() {
                output[target + channel] = ((sum + samples / 2) / samples) as u8;
            }
        }
    }

    Ok((next_width, next_height, output))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MipLevelLayout {
    width: usize,
    height: usize,
    row_pitch: usize,
    slice_pitch: usize,
    offset: usize,
}

fn mip_layout(
    format: TextureFormat,
    width: u16,
    height: u16,
    levels: u8,
) -> Result<Option<Vec<MipLevelLayout>>, YtdError> {
    if matches!(format, TextureFormat::Unknown(_)) {
        return Ok(None);
    }

    let mut out = Vec::with_capacity(usize::from(levels));
    let mut offset = 0_usize;
    for level in 0..levels {
        let shift = u32::from(level).min(usize::BITS - 1);
        let mip_width = (usize::from(width) >> shift).max(1);
        let mip_height = (usize::from(height) >> shift).max(1);
        let (row_pitch, slice_pitch) = format_pitch(format, mip_width, mip_height)?;
        out.push(MipLevelLayout {
            width: mip_width,
            height: mip_height,
            row_pitch,
            slice_pitch,
            offset,
        });
        offset = offset
            .checked_add(slice_pitch)
            .ok_or_else(|| malformed("texture mip byte length overflows usize"))?;
    }
    Ok(Some(out))
}

fn format_pitch(
    format: TextureFormat,
    width: usize,
    height: usize,
) -> Result<(usize, usize), YtdError> {
    let checked_linear = |bytes_per_pixel: usize| -> Result<(usize, usize), YtdError> {
        let row_pitch = width
            .checked_mul(bytes_per_pixel)
            .ok_or_else(|| malformed("texture row pitch overflows usize"))?;
        let slice_pitch = row_pitch
            .checked_mul(height)
            .ok_or_else(|| malformed("texture slice pitch overflows usize"))?;
        Ok((row_pitch, slice_pitch))
    };
    let checked_blocks = |block_bytes: usize| -> Result<(usize, usize), YtdError> {
        let blocks_x = width.div_ceil(4).max(1);
        let blocks_y = height.div_ceil(4).max(1);
        let row_pitch = blocks_x
            .checked_mul(block_bytes)
            .ok_or_else(|| malformed("BC texture row pitch overflows usize"))?;
        let slice_pitch = row_pitch
            .checked_mul(blocks_y)
            .ok_or_else(|| malformed("BC texture slice pitch overflows usize"))?;
        Ok((row_pitch, slice_pitch))
    };

    match format {
        TextureFormat::Bgra8 | TextureFormat::Bgrx8 | TextureFormat::Rgba8 => checked_linear(4),
        TextureFormat::B5G5R5A1 => checked_linear(2),
        TextureFormat::A8 | TextureFormat::R8 => checked_linear(1),
        TextureFormat::Bc1 | TextureFormat::Bc4 => checked_blocks(8),
        TextureFormat::Bc2 | TextureFormat::Bc3 | TextureFormat::Bc5 | TextureFormat::Bc7 => {
            checked_blocks(16)
        }
        TextureFormat::Unknown(raw) => Err(malformed(format!(
            "cannot compute pitch for unknown texture format 0x{raw:08X}"
        ))),
    }
}

fn legacy_stride(format: TextureFormat, width: u16) -> Result<u16, YtdError> {
    let width = usize::from(width);
    let stride = match format {
        TextureFormat::Bgra8 | TextureFormat::Bgrx8 | TextureFormat::Rgba8 => width
            .checked_mul(4)
            .ok_or_else(|| malformed("legacy texture stride overflows usize"))?,
        TextureFormat::B5G5R5A1 => width
            .checked_mul(2)
            .ok_or_else(|| malformed("legacy texture stride overflows usize"))?,
        TextureFormat::A8 | TextureFormat::R8 => width,
        // Legacy Texture.stride stores the logical bytes-per-scanline value,
        // not the minimum block-row byte count. For example a real 4x4 BC1
        // texture has stride=2 while its encoded block row occupies 8 bytes.
        TextureFormat::Bc1 | TextureFormat::Bc4 => width.div_ceil(2),
        TextureFormat::Bc2 | TextureFormat::Bc3 | TextureFormat::Bc5 | TextureFormat::Bc7 => width,
        TextureFormat::Unknown(raw) => {
            return Err(malformed(format!(
                "cannot compute legacy stride for unknown texture format 0x{raw:08X}"
            )))
        }
    };
    u16::try_from(stride).map_err(|_| {
        YtdError::ReplacementMismatch(format!(
            "legacy stride {stride} for {} texture width {width} does not fit u16",
            format.normalized_name()
        ))
    })
}

fn legacy_data_length(
    format: TextureFormat,
    width: u16,
    height: u16,
    stride: u16,
    levels: u8,
) -> Result<usize, YtdError> {
    if let Some(layout) = mip_layout(format, width, height, levels)? {
        return layout.last().map_or(Ok(0), |mip| {
            mip.offset
                .checked_add(mip.slice_pitch)
                .ok_or_else(|| malformed("texture mip byte length overflows usize"))
        });
    }

    let mut mip_length = usize::from(stride)
        .checked_mul(usize::from(height))
        .ok_or_else(|| malformed("top mip byte length overflows usize"))?;
    let mut total = 0_usize;
    for _ in 0..levels {
        total = total
            .checked_add(mip_length)
            .ok_or_else(|| malformed("texture mip byte length overflows usize"))?;
        mip_length /= 4;
    }
    Ok(total)
}

fn read_bounded_c_string(
    resource: &Rsc7Resource,
    pointer: u64,
    max_len: usize,
) -> Result<String, String> {
    let mut bytes = Vec::with_capacity(64);
    for offset in 0..max_len {
        let byte = resource
            .read_u8(
                pointer
                    .checked_add(offset as u64)
                    .ok_or("name pointer overflow")?,
            )
            .map_err(|error| error.to_string())?;
        if byte == 0 {
            return String::from_utf8(bytes).map_err(|error| format!("name is not UTF-8: {error}"));
        }
        bytes.push(byte);
    }
    Err(format!("name is not NUL-terminated within {max_len} bytes"))
}

fn at(base: u64, offset: u64) -> Result<u64, YtdError> {
    base.checked_add(offset)
        .ok_or_else(|| malformed("resource pointer arithmetic overflow"))
}

fn malformed(message: impl Into<String>) -> YtdError {
    YtdError::Malformed(message.into())
}

#[cfg(test)]
mod tests {
    use super::{
        build_dds, decode_a8, decode_b5g5r5a1, decode_bc1, decode_bc2, decode_bc3, decode_bc4,
        decode_bc5, decode_bgra8, decode_r8, downsample_rgba8_box, encode_bc1_block,
        encode_bc1_image, encode_bc3_block, encode_bc3_image, legacy_data_length, legacy_stride,
        TextureFormat, TextureInfo, Ytd,
    };

    #[test]
    fn recognizes_known_legacy_formats() {
        assert_eq!(TextureFormat::from_raw(32), TextureFormat::Rgba8);
        assert_eq!(TextureFormat::from_raw(0x3154_5844), TextureFormat::Bc1);
        assert_eq!(TextureFormat::Unknown(0xDEAD_BEEF).raw(), 0xDEAD_BEEF);
    }

    #[test]
    fn estimates_legacy_mip_chain_size() {
        assert_eq!(
            legacy_data_length(TextureFormat::Rgba8, 4, 8, 16, 3).unwrap(),
            168
        );
        assert_eq!(
            legacy_data_length(TextureFormat::Bc1, 4, 4, 2, 3).unwrap(),
            24
        );
    }

    #[test]
    fn computes_legacy_metadata_stride_independently_from_block_row_pitch() {
        assert_eq!(legacy_stride(TextureFormat::Rgba8, 4).unwrap(), 16);
        assert_eq!(legacy_stride(TextureFormat::Bc1, 4).unwrap(), 2);
        assert_eq!(legacy_stride(TextureFormat::Bc1, 128).unwrap(), 64);
        assert_eq!(legacy_stride(TextureFormat::Bc3, 4).unwrap(), 4);
        assert_eq!(legacy_stride(TextureFormat::Bc3, 128).unwrap(), 128);
    }

    #[test]
    fn writes_classic_bc1_dds_with_full_mip_payload() {
        let texture = TextureInfo {
            dictionary_hash: 0,
            name: "test".into(),
            name_hash: 0,
            width: 4,
            height: 4,
            depth: 1,
            stride: 2,
            format: TextureFormat::Bc1,
            levels: 3,
            usage: 0,
            usage_flags: 0,
            extra_flags: 0,
            data_pointer: 0,
            data_length: 24,
        };
        let payload = (0_u8..24).collect::<Vec<_>>();
        let dds = build_dds(&texture, &payload).unwrap();

        assert_eq!(&dds[0..4], b"DDS ");
        assert_eq!(u32::from_le_bytes(dds[4..8].try_into().unwrap()), 124);
        assert_eq!(u32::from_le_bytes(dds[12..16].try_into().unwrap()), 4);
        assert_eq!(u32::from_le_bytes(dds[16..20].try_into().unwrap()), 4);
        assert_eq!(u32::from_le_bytes(dds[20..24].try_into().unwrap()), 8);
        assert_eq!(u32::from_le_bytes(dds[28..32].try_into().unwrap()), 3);
        assert_eq!(&dds[84..88], b"DXT1");
        assert_eq!(&dds[128..], payload);
    }

    #[test]
    fn generates_complete_rgba8_dds_mip_chain() {
        let top = vec![42_u8; 8 * 4 * 4];
        let dds = Ytd::rgba8_dds_with_generated_mips(8, 4, &top).unwrap();

        assert_eq!(&dds[0..4], b"DDS ");
        assert_eq!(u32::from_le_bytes(dds[12..16].try_into().unwrap()), 4);
        assert_eq!(u32::from_le_bytes(dds[16..20].try_into().unwrap()), 8);
        assert_eq!(u32::from_le_bytes(dds[20..24].try_into().unwrap()), 32);
        assert_eq!(u32::from_le_bytes(dds[28..32].try_into().unwrap()), 4);
        assert_eq!(dds.len(), 128 + 128 + 32 + 8 + 4);
        assert!(dds[128..].iter().all(|byte| *byte == 42));
    }

    #[test]
    fn rgba8_box_filter_includes_odd_dimension_edges() {
        let source = [0_u8, 10, 20, 255, 90, 100, 110, 255, 210, 220, 230, 255];
        let (width, height, output) = downsample_rgba8_box(&source, 3, 1).unwrap();

        assert_eq!((width, height), (1, 1));
        assert_eq!(output, vec![100, 110, 120, 255]);
    }

    #[test]
    fn rejects_invalid_rgba_top_mip_length() {
        let error = Ytd::rgba8_dds_with_generated_mips(4, 4, &[0_u8; 63]).unwrap_err();
        assert!(matches!(error, super::YtdError::InvalidRgba(_)));
    }

    #[test]
    fn encodes_bc1_solid_red_block_deterministically() {
        let pixels = [[255_u8, 0, 0, 255]; 16];
        let encoded = encode_bc1_block(&pixels);

        assert_eq!(encoded, [0x00, 0xF8, 0x00, 0xF8, 0, 0, 0, 0]);
        let mut decoded = vec![0_u8; 4 * 4 * 4];
        decode_bc1(&encoded, 4, 4, 2, &mut decoded).unwrap();
        assert!(decoded
            .chunks_exact(4)
            .all(|pixel| pixel == [255, 0, 0, 255]));
    }

    #[test]
    fn encodes_bc1_fully_transparent_block() {
        let pixels = [[12_u8, 34, 56, 0]; 16];
        let encoded = encode_bc1_block(&pixels);

        assert_eq!(encoded, [0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF]);
        let mut decoded = vec![0_u8; 4 * 4 * 4];
        decode_bc1(&encoded, 4, 4, 2, &mut decoded).unwrap();
        assert!(decoded.chunks_exact(4).all(|pixel| pixel == [0, 0, 0, 0]));
    }

    #[test]
    fn bc1_round_trip_preserves_exact_grayscale_palette() {
        let shades = [0_u8, 85, 170, 255];
        let mut rgba = Vec::with_capacity(4 * 4 * 4);
        for row in 0..4 {
            for column in 0..4 {
                let value = shades[(row + column) % shades.len()];
                rgba.extend_from_slice(&[value, value, value, 255]);
            }
        }

        let encoded = encode_bc1_image(&rgba, 4, 4).unwrap();
        let mut decoded = vec![0_u8; rgba.len()];
        decode_bc1(&encoded, 4, 4, 2, &mut decoded).unwrap();
        assert_eq!(decoded, rgba);
    }

    #[test]
    fn bc1_encoder_clamps_partial_edge_blocks() {
        let rgba = [0_u8, 0, 255, 255].repeat(3 * 5);
        let encoded = encode_bc1_image(&rgba, 3, 5).unwrap();

        assert_eq!(encoded.len(), 16);
        let mut decoded = vec![0_u8; rgba.len()];
        decode_bc1(&encoded, 3, 5, 2, &mut decoded).unwrap();
        assert_eq!(decoded, rgba);
    }

    #[test]
    fn generates_deterministic_bc1_dds_mip_chain() {
        let rgba = [24_u8, 80, 160, 255].repeat(5 * 3);
        let first = Ytd::bc1_dds_with_generated_mips(5, 3, &rgba).unwrap();
        let second = Ytd::bc1_dds_with_generated_mips(5, 3, &rgba).unwrap();

        assert_eq!(first, second);
        assert_eq!(&first[0..4], b"DDS ");
        assert_eq!(&first[84..88], b"DXT1");
        assert_eq!(u32::from_le_bytes(first[28..32].try_into().unwrap()), 3);
        assert_eq!(first.len(), 128 + 16 + 8 + 8);
    }

    #[test]
    fn encodes_bc3_solid_blue_with_half_alpha() {
        let pixels = [[0_u8, 0, 255, 128]; 16];
        let encoded = encode_bc3_block(&pixels);

        let mut decoded = vec![0_u8; 4 * 4 * 4];
        decode_bc3(&encoded, 4, 4, 4, &mut decoded).unwrap();
        assert!(decoded
            .chunks_exact(4)
            .all(|pixel| pixel == [0, 0, 255, 128]));
    }

    #[test]
    fn bc3_alpha_round_trip_preserves_interpolation_palette() {
        let alphas = [255_u8, 0, 218, 182, 145, 109, 72, 36];
        let mut rgba = Vec::with_capacity(4 * 4 * 4);
        for pixel in 0..16 {
            rgba.extend_from_slice(&[0, 0, 255, alphas[pixel % alphas.len()]]);
        }

        let encoded = encode_bc3_image(&rgba, 4, 4).unwrap();
        let mut decoded = vec![0_u8; rgba.len()];
        decode_bc3(&encoded, 4, 4, 4, &mut decoded).unwrap();
        assert_eq!(decoded, rgba);
    }

    #[test]
    fn bc3_encoder_clamps_partial_edge_blocks() {
        let rgba = [0_u8, 255, 0, 64].repeat(3 * 5);
        let encoded = encode_bc3_image(&rgba, 3, 5).unwrap();

        assert_eq!(encoded.len(), 32);
        let mut decoded = vec![0_u8; rgba.len()];
        decode_bc3(&encoded, 3, 5, 4, &mut decoded).unwrap();
        assert_eq!(decoded, rgba);
    }

    #[test]
    fn generates_deterministic_bc3_dds_mip_chain() {
        let rgba = [48_u8, 96, 192, 160].repeat(5 * 3);
        let first = Ytd::bc3_dds_with_generated_mips(5, 3, &rgba).unwrap();
        let second = Ytd::bc3_dds_with_generated_mips(5, 3, &rgba).unwrap();

        assert_eq!(first, second);
        assert_eq!(&first[0..4], b"DDS ");
        assert_eq!(&first[84..88], b"DXT5");
        assert_eq!(u32::from_le_bytes(first[28..32].try_into().unwrap()), 3);
        assert_eq!(first.len(), 128 + 32 + 16 + 16);
    }

    #[test]
    fn decodes_bgra8_and_bgrx8_pixels() {
        let encoded = [3_u8, 2, 1, 4];
        let mut rgba = [0_u8; 4];
        decode_bgra8(&encoded, 1, 1, 4, &mut rgba, false).unwrap();
        assert_eq!(rgba, [1, 2, 3, 4]);

        decode_bgra8(&encoded, 1, 1, 4, &mut rgba, true).unwrap();
        assert_eq!(rgba, [1, 2, 3, 255]);
    }

    #[test]
    fn decodes_b5g5r5a1_pixel() {
        let encoded = 0xFC00_u16.to_le_bytes();
        let mut rgba = [0_u8; 4];
        decode_b5g5r5a1(&encoded, 1, 1, 2, &mut rgba).unwrap();
        assert_eq!(rgba, [255, 0, 0, 255]);
    }

    #[test]
    fn decodes_a8_as_white_with_alpha() {
        let encoded = [128_u8];
        let mut rgba = [0_u8; 4];
        decode_a8(&encoded, 1, 1, 1, &mut rgba).unwrap();
        assert_eq!(rgba, [255, 255, 255, 128]);
    }

    #[test]
    fn decodes_r8_luminance_as_grayscale() {
        let encoded = [64_u8];
        let mut rgba = [0_u8; 4];
        decode_r8(&encoded, 1, 1, 1, &mut rgba).unwrap();
        assert_eq!(rgba, [64, 64, 64, 255]);
    }

    #[test]
    fn decodes_bc1_opaque_red_block() {
        let mut block = [0_u8; 8];
        block[0..2].copy_from_slice(&0xF800_u16.to_le_bytes());
        block[2..4].copy_from_slice(&0x07E0_u16.to_le_bytes());
        let mut rgba = vec![0_u8; 4 * 4 * 4];
        decode_bc1(&block, 4, 4, 2, &mut rgba).unwrap();
        assert!(rgba.chunks_exact(4).all(|pixel| pixel == [255, 0, 0, 255]));
    }

    #[test]
    fn decodes_bc1_transparent_selector() {
        let mut block = [0_u8; 8];
        block[0..2].copy_from_slice(&0_u16.to_le_bytes());
        block[2..4].copy_from_slice(&0xFFFF_u16.to_le_bytes());
        block[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        let mut rgba = vec![0_u8; 4 * 4 * 4];
        decode_bc1(&block, 4, 4, 2, &mut rgba).unwrap();
        assert!(rgba.chunks_exact(4).all(|pixel| pixel == [0, 0, 0, 0]));
    }

    #[test]
    fn decodes_bc2_explicit_alpha_red_block() {
        let mut block = [0_u8; 16];
        block[0..8].fill(0xFF);
        block[8..10].copy_from_slice(&0xF800_u16.to_le_bytes());
        block[10..12].copy_from_slice(&0x07E0_u16.to_le_bytes());
        let mut rgba = vec![0_u8; 4 * 4 * 4];
        decode_bc2(&block, 4, 4, 4, &mut rgba).unwrap();
        assert!(rgba.chunks_exact(4).all(|pixel| pixel == [255, 0, 0, 255]));
    }

    #[test]
    fn decodes_bc3_opaque_blue_block() {
        let mut block = [0_u8; 16];
        block[0] = 255;
        block[1] = 0;
        block[8..10].copy_from_slice(&0x001F_u16.to_le_bytes());
        block[10..12].copy_from_slice(&0_u16.to_le_bytes());
        let mut rgba = vec![0_u8; 4 * 4 * 4];
        decode_bc3(&block, 4, 4, 4, &mut rgba).unwrap();
        assert!(rgba.chunks_exact(4).all(|pixel| pixel == [0, 0, 255, 255]));
    }

    #[test]
    fn decodes_bc4_single_red_channel() {
        let mut block = [0_u8; 8];
        block[0] = 255;
        block[1] = 0;
        let mut rgba = vec![0_u8; 4 * 4 * 4];
        decode_bc4(&block, 4, 4, 2, &mut rgba).unwrap();
        assert!(rgba.chunks_exact(4).all(|pixel| pixel == [255, 0, 0, 255]));
    }

    #[test]
    fn decodes_bc5_red_green_channels() {
        let mut block = [0_u8; 16];
        block[0] = 255;
        block[1] = 0;
        block[8] = 128;
        block[9] = 0;
        let mut rgba = vec![0_u8; 4 * 4 * 4];
        decode_bc5(&block, 4, 4, 4, &mut rgba).unwrap();
        assert!(rgba
            .chunks_exact(4)
            .all(|pixel| pixel == [255, 128, 0, 255]));
    }
}

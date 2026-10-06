//! Parser for the RAGE `META` container embedded in GTA V RSC7 resources.
//!
//! The implementation is intentionally schema-driven: META files carry
//! structure descriptions and data blocks, so higher-level crates can resolve
//! fields without baking every game structure into this crate.

use std::collections::BTreeMap;

use ragelab_resource::{ResourceError, Rsc7Header, Rsc7Resource, SYSTEM_BASE};

pub const META_HEADER_SIZE: usize = 112;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MetaHash(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetaPointer {
    pub raw: u64,
}

impl MetaPointer {
    pub const fn new(raw: u64) -> Self {
        Self { raw }
    }

    /// META block id. Zero represents a null pointer; non-zero ids are 1-based.
    pub const fn block_id(self) -> usize {
        (self.raw & 0x0fff) as usize
    }

    pub const fn block_index(self) -> Option<usize> {
        let id = self.block_id();
        if id == 0 {
            None
        } else {
            Some(id - 1)
        }
    }

    pub const fn offset(self) -> usize {
        ((self.raw >> 12) & 0x000f_ffff) as usize
    }

    pub const fn is_null(self) -> bool {
        self.block_id() == 0
    }

    pub fn from_block_offset(block_id: usize, offset: usize) -> Result<Self, ResourceError> {
        if block_id == 0 || block_id > 0x0fff {
            return Err(ResourceError::Malformed(format!(
                "META block id {block_id} is outside 1..=4095"
            )));
        }
        if offset > 0x000f_ffff {
            return Err(ResourceError::Malformed(format!(
                "META block offset {offset} exceeds 20-bit pointer range"
            )));
        }
        Ok(Self {
            raw: (block_id as u64) | ((offset as u64) << 12),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetaArrayRef {
    pub pointer: MetaPointer,
    pub count1: u16,
    pub count2: u16,
    pub unknown: u32,
}

impl MetaArrayRef {
    pub fn parse(bytes: &[u8], offset: usize) -> Result<Self, ResourceError> {
        Ok(Self {
            pointer: MetaPointer::new(read_u64(bytes, offset)?),
            count1: read_u16(bytes, offset + 8)?,
            count2: read_u16(bytes, offset + 10)?,
            unknown: read_u32(bytes, offset + 12)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaEntryDataType {
    Boolean,
    SignedByte,
    UnsignedByte,
    SignedShort,
    UnsignedShort,
    SignedInt,
    UnsignedInt,
    Float,
    FloatXyz,
    FloatXyzw,
    ByteEnum,
    IntEnum,
    ShortFlags,
    IntFlags1,
    IntFlags2,
    Hash,
    Array,
    ArrayOfChars,
    ArrayOfBytes,
    DataBlockPointer,
    CharPointer,
    StructurePointer,
    Structure,
    Unknown(u8),
}

impl MetaEntryDataType {
    pub const fn from_raw(value: u8) -> Self {
        match value {
            0x01 => Self::Boolean,
            0x10 => Self::SignedByte,
            0x11 => Self::UnsignedByte,
            0x12 => Self::SignedShort,
            0x13 => Self::UnsignedShort,
            0x14 => Self::SignedInt,
            0x15 => Self::UnsignedInt,
            0x21 => Self::Float,
            0x33 => Self::FloatXyz,
            0x34 => Self::FloatXyzw,
            0x60 => Self::ByteEnum,
            0x62 => Self::IntEnum,
            0x64 => Self::ShortFlags,
            0x63 => Self::IntFlags1,
            0x65 => Self::IntFlags2,
            0x4a => Self::Hash,
            0x52 => Self::Array,
            0x40 => Self::ArrayOfChars,
            0x50 => Self::ArrayOfBytes,
            0x59 => Self::DataBlockPointer,
            0x44 => Self::CharPointer,
            0x07 => Self::StructurePointer,
            0x05 => Self::Structure,
            other => Self::Unknown(other),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaStructureEntry {
    pub name: MetaHash,
    pub data_offset: usize,
    pub data_type: MetaEntryDataType,
    pub unknown_9h: u8,
    pub reference_type_index: i16,
    pub reference_key: MetaHash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaStructureInfo {
    pub name: MetaHash,
    pub key: u32,
    pub unknown_8h: u32,
    pub unknown_ch: u32,
    pub structure_size: usize,
    pub unknown_1ch: i16,
    pub entries: Vec<MetaStructureEntry>,
}

impl MetaStructureInfo {
    pub fn field(&self, name: MetaHash) -> Option<&MetaStructureEntry> {
        self.entries.iter().find(|entry| entry.name == name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaEnumEntry {
    pub name: MetaHash,
    pub value: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaEnumInfo {
    pub name: MetaHash,
    pub key: u32,
    pub unknown_14h: i32,
    pub entries: Vec<MetaEnumEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaDataBlock {
    pub structure_name: MetaHash,
    pub data_pointer: u64,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaHeader {
    /// First 16 bytes inherited from the generic RAGE resource-file base.
    pub resource_base_prefix: [u8; 16],
    pub unknown_10h: i32,
    pub unknown_14h: i16,
    pub has_encrypted_strings: u8,
    pub unknown_17h: u8,
    pub unknown_18h: i32,
    /// One-based index into `MetaDocument::blocks`.
    pub root_block_index: i32,
    pub structure_infos_pointer: u64,
    pub enum_infos_pointer: u64,
    pub data_blocks_pointer: u64,
    pub name_pointer: u64,
    pub encrypted_strings_pointer: u64,
    pub structure_infos_count: u16,
    pub enum_infos_count: u16,
    pub data_blocks_count: u16,
    pub unknown_4eh: i16,
    pub trailing_unknowns: [u32; 8],
}

impl MetaHeader {
    fn parse(system: &[u8]) -> Result<Self, ResourceError> {
        if system.len() < META_HEADER_SIZE {
            return Err(ResourceError::TooSmall {
                expected_at_least: META_HEADER_SIZE,
                actual: system.len(),
            });
        }

        Ok(Self {
            resource_base_prefix: system[0..16]
                .try_into()
                .expect("fixed META base-prefix slice"),
            unknown_10h: read_i32(system, 0x10)?,
            unknown_14h: read_i16(system, 0x14)?,
            has_encrypted_strings: read_u8(system, 0x16)?,
            unknown_17h: read_u8(system, 0x17)?,
            unknown_18h: read_i32(system, 0x18)?,
            root_block_index: read_i32(system, 0x1c)?,
            structure_infos_pointer: read_u64(system, 0x20)?,
            enum_infos_pointer: read_u64(system, 0x28)?,
            data_blocks_pointer: read_u64(system, 0x30)?,
            name_pointer: read_u64(system, 0x38)?,
            encrypted_strings_pointer: read_u64(system, 0x40)?,
            structure_infos_count: read_u16(system, 0x48)?,
            enum_infos_count: read_u16(system, 0x4a)?,
            data_blocks_count: read_u16(system, 0x4c)?,
            unknown_4eh: read_i16(system, 0x4e)?,
            trailing_unknowns: [
                read_u32(system, 0x50)?,
                read_u32(system, 0x54)?,
                read_u32(system, 0x58)?,
                read_u32(system, 0x5c)?,
                read_u32(system, 0x60)?,
                read_u32(system, 0x64)?,
                read_u32(system, 0x68)?,
                read_u32(system, 0x6c)?,
            ],
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaDocument {
    pub resource_header: Rsc7Header,
    pub header: MetaHeader,
    pub name: Option<String>,
    pub structures: Vec<MetaStructureInfo>,
    pub enums: Vec<MetaEnumInfo>,
    pub blocks: Vec<MetaDataBlock>,
}

impl MetaDocument {
    pub fn from_rsc7(bytes: &[u8]) -> Result<Self, ResourceError> {
        let resource = Rsc7Resource::parse(bytes)?;
        let header = MetaHeader::parse(resource.system())?;

        let structures = parse_structure_infos(&resource, &header)?;
        let enums = parse_enum_infos(&resource, &header)?;
        let blocks = parse_data_blocks(&resource, &header)?;
        let name = if header.name_pointer == 0 {
            None
        } else {
            Some(resource.read_c_string(header.name_pointer, 16 * 1024)?)
        };

        let document = Self {
            resource_header: resource.header,
            header,
            name,
            structures,
            enums,
            blocks,
        };

        // Validate the root index early because every typed format depends on it.
        let _ = document.root_block()?;
        Ok(document)
    }

    pub fn root_block(&self) -> Result<&MetaDataBlock, ResourceError> {
        if self.header.root_block_index <= 0 {
            return Err(ResourceError::Malformed(format!(
                "META root block index must be 1-based and positive, got {}",
                self.header.root_block_index
            )));
        }

        let index = usize::try_from(self.header.root_block_index - 1)
            .map_err(|_| ResourceError::Malformed("invalid META root block index".into()))?;
        self.blocks.get(index).ok_or_else(|| {
            ResourceError::Malformed(format!(
                "META root block index {} is outside {} data blocks",
                self.header.root_block_index,
                self.blocks.len()
            ))
        })
    }

    pub fn block_by_id(&self, id: usize) -> Option<&MetaDataBlock> {
        id.checked_sub(1).and_then(|index| self.blocks.get(index))
    }

    pub fn block_for_pointer(&self, pointer: MetaPointer) -> Option<&MetaDataBlock> {
        self.block_by_id(pointer.block_id())
    }

    pub fn structure(&self, name: MetaHash) -> Option<&MetaStructureInfo> {
        self.structures.iter().find(|info| info.name == name)
    }

    pub fn root_structure(&self) -> Result<&MetaStructureInfo, ResourceError> {
        let root = self.root_block()?;
        self.structure(root.structure_name).ok_or_else(|| {
            ResourceError::Malformed(format!(
                "META has no structure descriptor for root hash 0x{:08X}",
                root.structure_name.0
            ))
        })
    }

    pub fn resolve_bytes(
        &self,
        pointer: MetaPointer,
        length: usize,
    ) -> Result<&[u8], ResourceError> {
        let block = self.block_for_pointer(pointer).ok_or_else(|| {
            ResourceError::Malformed(format!(
                "META pointer references missing block id {}",
                pointer.block_id()
            ))
        })?;

        checked_slice("META block", &block.data, pointer.offset(), length)
    }

    pub fn resolve_array_bytes(
        &self,
        array: MetaArrayRef,
        element_size: usize,
    ) -> Result<&[u8], ResourceError> {
        let length = usize::from(array.count1)
            .checked_mul(element_size)
            .ok_or_else(|| ResourceError::Malformed("META array length overflow".into()))?;
        self.resolve_bytes(array.pointer, length)
    }

    pub fn resolve_u32_array(&self, array: MetaArrayRef) -> Result<Vec<u32>, ResourceError> {
        if array.count1 == 0 {
            return Ok(Vec::new());
        }

        let bytes = self.resolve_array_bytes(array, 4)?;
        bytes
            .chunks_exact(4)
            .map(|chunk| {
                Ok(u32::from_le_bytes(
                    chunk.try_into().expect("four-byte array element"),
                ))
            })
            .collect()
    }

    pub fn resolve_pointer_array(
        &self,
        array: MetaArrayRef,
    ) -> Result<Vec<MetaPointer>, ResourceError> {
        if array.count1 == 0 {
            return Ok(Vec::new());
        }

        let bytes = self.resolve_array_bytes(array, 8)?;
        bytes
            .chunks_exact(8)
            .map(|chunk| {
                Ok(MetaPointer::new(u64::from_le_bytes(
                    chunk.try_into().expect("eight-byte pointer element"),
                )))
            })
            .collect()
    }

    pub fn field<'a>(
        &'a self,
        structure: &'a MetaStructureInfo,
        field_name: MetaHash,
    ) -> Result<&'a MetaStructureEntry, ResourceError> {
        structure.field(field_name).ok_or_else(|| {
            ResourceError::Malformed(format!(
                "META structure 0x{:08X} has no field 0x{:08X}",
                structure.name.0, field_name.0
            ))
        })
    }

    /// Rewrites selected META data-block payloads while preserving every
    /// existing block id, schema descriptor and untouched payload.
    ///
    /// Replacement payloads are appended to the system segment and existing
    /// data-block descriptors are repointed. Old payload bytes remain
    /// orphaned, so block-id/offset MetaPointers into untouched blocks stay
    /// stable.
    pub fn rewrite_blocks(
        source: &[u8],
        replacements: &[(usize, Vec<u8>)],
    ) -> Result<Vec<u8>, ResourceError> {
        if replacements.is_empty() {
            return Ok(source.to_vec());
        }

        let document = Self::from_rsc7(source)?;
        let mut ordered = BTreeMap::<usize, &[u8]>::new();
        for (block_id, data) in replacements {
            if *block_id == 0 || *block_id > document.blocks.len() {
                return Err(ResourceError::Malformed(format!(
                    "META replacement block id {block_id} is outside 1..={}",
                    document.blocks.len()
                )));
            }
            if ordered.insert(*block_id, data.as_slice()).is_some() {
                return Err(ResourceError::Malformed(format!(
                    "META replacement block id {block_id} was provided more than once"
                )));
            }
            if data.len() > i32::MAX as usize {
                return Err(ResourceError::Malformed(format!(
                    "META replacement block {block_id} exceeds i32 data length"
                )));
            }
        }

        let mut resource = Rsc7Resource::parse(source)?;
        let old_system_len = resource.system().len();
        let mut required = old_system_len;
        for data in ordered.values() {
            required = align_up_16(required)?;
            required = required.checked_add(data.len()).ok_or_else(|| {
                ResourceError::Malformed("META replacement allocation overflows usize".into())
            })?;
        }

        let pages_info_pointer = u64::from_le_bytes(
            document.header.resource_base_prefix[8..16]
                .try_into()
                .expect("fixed ResourceFileBase page-info pointer"),
        );
        if pages_info_pointer == 0 {
            resource.grow_system_to_fit(required)?;
        } else {
            resource.grow_system_to_fit_with_page_info(required, pages_info_pointer)?;
        }

        let mut cursor = old_system_len;
        for (block_id, data) in ordered {
            cursor = align_up_16(cursor)?;
            let data_pointer = SYSTEM_BASE
                .checked_add(cursor as u64)
                .ok_or_else(|| ResourceError::Malformed("META data pointer overflow".into()))?;
            if !data.is_empty() {
                resource
                    .bytes_at_mut(data_pointer, data.len())?
                    .copy_from_slice(data);
            }

            let descriptor_pointer = add_pointer(
                document.header.data_blocks_pointer,
                (block_id - 1).checked_mul(16).ok_or_else(|| {
                    ResourceError::Malformed("META descriptor offset overflow".into())
                })?,
            )?;
            let descriptor = resource.bytes_at_mut(descriptor_pointer, 16)?;
            descriptor[4..8].copy_from_slice(&(data.len() as i32).to_le_bytes());
            descriptor[8..16]
                .copy_from_slice(&(if data.is_empty() { 0 } else { data_pointer }).to_le_bytes());
            cursor = cursor.checked_add(data.len()).ok_or_else(|| {
                ResourceError::Malformed("META replacement cursor overflow".into())
            })?;
        }

        let rewritten = resource.to_bytes()?;
        let reopened = Self::from_rsc7(&rewritten)?;
        for (block_id, expected) in replacements {
            let actual = reopened.block_by_id(*block_id).ok_or_else(|| {
                ResourceError::Malformed(format!(
                    "META rewritten block id {block_id} disappeared after semantic re-open"
                ))
            })?;
            if actual.data != *expected {
                return Err(ResourceError::Malformed(format!(
                    "META rewritten block id {block_id} changed after semantic re-open"
                )));
            }
            if actual.structure_name != document.blocks[*block_id - 1].structure_name {
                return Err(ResourceError::Malformed(format!(
                    "META rewritten block id {block_id} changed structure identity"
                )));
            }
        }

        Ok(rewritten)
    }
}

fn parse_structure_infos(
    resource: &Rsc7Resource,
    header: &MetaHeader,
) -> Result<Vec<MetaStructureInfo>, ResourceError> {
    let count = usize::from(header.structure_infos_count);
    if count == 0 {
        return Ok(Vec::new());
    }
    if header.structure_infos_pointer == 0 {
        return Err(ResourceError::Malformed(
            "META structure info count is non-zero but pointer is null".into(),
        ));
    }

    let mut output = Vec::with_capacity(count);
    for index in 0..count {
        let pointer = add_pointer(header.structure_infos_pointer, index * 32)?;
        let bytes = resource.bytes_at(pointer, 32)?;
        let entries_pointer = read_u64(bytes, 0x10)?;
        let entries_count_raw = read_i16(bytes, 0x1e)?;
        if entries_count_raw < 0 {
            return Err(ResourceError::Malformed(format!(
                "META structure entry count is negative: {entries_count_raw}"
            )));
        }
        let entries_count = entries_count_raw as usize;

        let mut entries = Vec::with_capacity(entries_count);
        for entry_index in 0..entries_count {
            let entry_pointer = add_pointer(entries_pointer, entry_index * 16)?;
            let entry = resource.bytes_at(entry_pointer, 16)?;
            entries.push(MetaStructureEntry {
                name: MetaHash(read_u32(entry, 0)?),
                data_offset: usize::try_from(read_i32(entry, 4)?).map_err(|_| {
                    ResourceError::Malformed("negative META structure field offset".into())
                })?,
                data_type: MetaEntryDataType::from_raw(read_u8(entry, 8)?),
                unknown_9h: read_u8(entry, 9)?,
                reference_type_index: read_i16(entry, 10)?,
                reference_key: MetaHash(read_u32(entry, 12)?),
            });
        }

        let structure_size_raw = read_i32(bytes, 0x18)?;
        if structure_size_raw < 0 {
            return Err(ResourceError::Malformed(format!(
                "negative META structure size: {structure_size_raw}"
            )));
        }

        output.push(MetaStructureInfo {
            name: MetaHash(read_u32(bytes, 0)?),
            key: read_u32(bytes, 4)?,
            unknown_8h: read_u32(bytes, 8)?,
            unknown_ch: read_u32(bytes, 12)?,
            structure_size: structure_size_raw as usize,
            unknown_1ch: read_i16(bytes, 0x1c)?,
            entries,
        });
    }

    Ok(output)
}

fn parse_enum_infos(
    resource: &Rsc7Resource,
    header: &MetaHeader,
) -> Result<Vec<MetaEnumInfo>, ResourceError> {
    let count = usize::from(header.enum_infos_count);
    if count == 0 {
        return Ok(Vec::new());
    }
    if header.enum_infos_pointer == 0 {
        return Err(ResourceError::Malformed(
            "META enum info count is non-zero but pointer is null".into(),
        ));
    }

    let mut output = Vec::with_capacity(count);
    for index in 0..count {
        let pointer = add_pointer(header.enum_infos_pointer, index * 24)?;
        let bytes = resource.bytes_at(pointer, 24)?;
        let entries_pointer = read_u64(bytes, 8)?;
        let entries_count_raw = read_i32(bytes, 16)?;
        if entries_count_raw < 0 {
            return Err(ResourceError::Malformed(format!(
                "META enum entry count is negative: {entries_count_raw}"
            )));
        }

        let mut entries = Vec::with_capacity(entries_count_raw as usize);
        for entry_index in 0..entries_count_raw as usize {
            let entry_pointer = add_pointer(entries_pointer, entry_index * 8)?;
            let entry = resource.bytes_at(entry_pointer, 8)?;
            entries.push(MetaEnumEntry {
                name: MetaHash(read_u32(entry, 0)?),
                value: read_i32(entry, 4)?,
            });
        }

        output.push(MetaEnumInfo {
            name: MetaHash(read_u32(bytes, 0)?),
            key: read_u32(bytes, 4)?,
            unknown_14h: read_i32(bytes, 20)?,
            entries,
        });
    }

    Ok(output)
}

fn parse_data_blocks(
    resource: &Rsc7Resource,
    header: &MetaHeader,
) -> Result<Vec<MetaDataBlock>, ResourceError> {
    let count = usize::from(header.data_blocks_count);
    if count == 0 {
        return Ok(Vec::new());
    }
    if header.data_blocks_pointer == 0 {
        return Err(ResourceError::Malformed(
            "META data block count is non-zero but pointer is null".into(),
        ));
    }

    let mut output = Vec::with_capacity(count);
    for index in 0..count {
        let descriptor_pointer = add_pointer(header.data_blocks_pointer, index * 16)?;
        let descriptor = resource.bytes_at(descriptor_pointer, 16)?;
        let data_length_raw = read_i32(descriptor, 4)?;
        if data_length_raw < 0 {
            return Err(ResourceError::Malformed(format!(
                "negative META data block length: {data_length_raw}"
            )));
        }
        let data_length = data_length_raw as usize;
        let data_pointer = read_u64(descriptor, 8)?;
        let data = if data_length == 0 {
            Vec::new()
        } else {
            resource.bytes_at(data_pointer, data_length)?.to_vec()
        };

        output.push(MetaDataBlock {
            structure_name: MetaHash(read_u32(descriptor, 0)?),
            data_pointer,
            data,
        });
    }

    Ok(output)
}

fn add_pointer(base: u64, offset: usize) -> Result<u64, ResourceError> {
    base.checked_add(offset as u64)
        .ok_or_else(|| ResourceError::Malformed("resource pointer overflow".into()))
}

fn align_up_16(value: usize) -> Result<usize, ResourceError> {
    value
        .checked_add(15)
        .map(|aligned| aligned & !15)
        .ok_or_else(|| ResourceError::Malformed("META alignment overflows usize".into()))
}

pub fn read_u8(bytes: &[u8], offset: usize) -> Result<u8, ResourceError> {
    bytes
        .get(offset)
        .copied()
        .ok_or(ResourceError::OutOfBounds {
            segment: "buffer",
            offset,
            length: 1,
            segment_length: bytes.len(),
        })
}

pub fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ResourceError> {
    let slice = checked_slice("buffer", bytes, offset, 2)?;
    Ok(u16::from_le_bytes(
        slice.try_into().expect("fixed two-byte slice"),
    ))
}

pub fn read_i16(bytes: &[u8], offset: usize) -> Result<i16, ResourceError> {
    Ok(read_u16(bytes, offset)? as i16)
}

pub fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ResourceError> {
    let slice = checked_slice("buffer", bytes, offset, 4)?;
    Ok(u32::from_le_bytes(
        slice.try_into().expect("fixed four-byte slice"),
    ))
}

pub fn read_i32(bytes: &[u8], offset: usize) -> Result<i32, ResourceError> {
    Ok(read_u32(bytes, offset)? as i32)
}

pub fn read_f32(bytes: &[u8], offset: usize) -> Result<f32, ResourceError> {
    Ok(f32::from_bits(read_u32(bytes, offset)?))
}

pub fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ResourceError> {
    let slice = checked_slice("buffer", bytes, offset, 8)?;
    Ok(u64::from_le_bytes(
        slice.try_into().expect("fixed eight-byte slice"),
    ))
}

fn checked_slice<'a>(
    segment_name: &'static str,
    bytes: &'a [u8],
    offset: usize,
    length: usize,
) -> Result<&'a [u8], ResourceError> {
    let end = offset
        .checked_add(length)
        .ok_or(ResourceError::OutOfBounds {
            segment: segment_name,
            offset,
            length,
            segment_length: bytes.len(),
        })?;
    bytes.get(offset..end).ok_or(ResourceError::OutOfBounds {
        segment: segment_name,
        offset,
        length,
        segment_length: bytes.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::{MetaArrayRef, MetaPointer};

    #[test]
    fn meta_pointer_decodes_block_and_offset() {
        let raw = 7_u64 | (0x12345_u64 << 12);
        let pointer = MetaPointer::new(raw);
        assert_eq!(pointer.block_id(), 7);
        assert_eq!(pointer.block_index(), Some(6));
        assert_eq!(pointer.offset(), 0x12345);

        let encoded = MetaPointer::from_block_offset(7, 0x12345).unwrap();
        assert_eq!(encoded.raw, raw);
        assert!(MetaPointer::from_block_offset(0, 0).is_err());
        assert!(MetaPointer::from_block_offset(0x1000, 0).is_err());
        assert!(MetaPointer::from_block_offset(1, 0x10_0000).is_err());
    }

    #[test]
    fn array_ref_is_sixteen_bytes() {
        let pointer = 2_u64 | (0x40_u64 << 12);
        let mut bytes = [0_u8; 16];
        bytes[0..8].copy_from_slice(&pointer.to_le_bytes());
        bytes[8..10].copy_from_slice(&3_u16.to_le_bytes());
        bytes[10..12].copy_from_slice(&3_u16.to_le_bytes());
        bytes[12..16].copy_from_slice(&0_u32.to_le_bytes());

        let array = MetaArrayRef::parse(&bytes, 0).expect("valid array ref");
        assert_eq!(array.pointer.block_id(), 2);
        assert_eq!(array.pointer.offset(), 0x40);
        assert_eq!(array.count1, 3);
    }
}

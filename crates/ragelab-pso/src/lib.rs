//! Minimal PSO (PsoFile) reader/writer primitives.
//!
//! The first consumer is `_manifest.ymf`. The implementation deliberately
//! focuses on the PSIN/PMAP/PSCH profile used by streamed pack manifests while
//! keeping the block/schema types reusable for future PSO-backed formats.

use std::{collections::BTreeMap, error::Error, fmt};

pub const PSIN: u32 = u32::from_be_bytes(*b"PSIN");
pub const PMAP: u32 = u32::from_be_bytes(*b"PMAP");
pub const PSCH: u32 = u32::from_be_bytes(*b"PSCH");
pub const PSIG: u32 = u32::from_be_bytes(*b"PSIG");
pub const CHKS: u32 = u32::from_be_bytes(*b"CHKS");

pub const DATA_UINT: u8 = 0x06;
pub const DATA_STRING: u8 = 0x0B;
pub const DATA_STRUCTURE: u8 = 0x0C;
pub const DATA_ARRAY: u8 = 0x0D;
pub const DATA_ENUM: u8 = 0x0E;
pub const DATA_FLAGS: u8 = 0x0F;
pub const ARRAY_INFO_HASH: u32 = 0x100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PsoError {
    Truncated(&'static str),
    MissingSection(&'static str),
    Malformed(String),
    Unsupported(&'static str),
}

impl fmt::Display for PsoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated(what) => write!(f, "truncated {what}"),
            Self::MissingSection(what) => write!(f, "missing PSO section {what}"),
            Self::Malformed(message) => f.write_str(message),
            Self::Unsupported(what) => write!(f, "unsupported PSO feature: {what}"),
        }
    }
}

impl Error for PsoError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PsoPointer {
    pub block_id: u16,
    pub offset: u32,
}

impl PsoPointer {
    pub const fn is_null(self) -> bool {
        self.block_id == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PsoArrayHeader {
    pub pointer: PsoPointer,
    pub count: u16,
}

impl PsoArrayHeader {
    pub fn decode(bytes: &[u8], offset: usize) -> Result<Self, PsoError> {
        let word = read_u32_be(bytes, offset, "PSO array pointer")?;
        let count = read_u16_be(bytes, offset + 8, "PSO array count")?;
        let count_copy = read_u16_be(bytes, offset + 10, "PSO array count copy")?;
        if count != count_copy {
            return Err(PsoError::Malformed(format!(
                "PSO array count mismatch at 0x{offset:X}: {count} != {count_copy}"
            )));
        }
        Ok(Self {
            pointer: decode_pointer_word(word),
            count,
        })
    }

    pub fn encode(self) -> Result<[u8; 16], PsoError> {
        let word = encode_pointer_word(self.pointer.block_id, self.pointer.offset)?;
        let mut out = [0_u8; 16];
        out[0..4].copy_from_slice(&word.to_be_bytes());
        out[8..10].copy_from_slice(&self.count.to_be_bytes());
        out[10..12].copy_from_slice(&self.count.to_be_bytes());
        Ok(out)
    }
}

pub fn decode_pointer_word(word: u32) -> PsoPointer {
    PsoPointer {
        block_id: (word & 0x0FFF) as u16,
        offset: word >> 12,
    }
}

pub fn encode_pointer_word(block_id: u16, relative_offset: u32) -> Result<u32, PsoError> {
    if block_id > 0x0FFF {
        return Err(PsoError::Malformed(format!(
            "PSO block id {block_id} exceeds 12-bit pointer field"
        )));
    }
    if relative_offset > 0x000F_FFFF {
        return Err(PsoError::Malformed(format!(
            "PSO relative offset 0x{relative_offset:X} exceeds 20-bit pointer field"
        )));
    }
    Ok((relative_offset << 12) | u32::from(block_id))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PsoBlock {
    pub name_hash: u32,
    pub data: Vec<u8>,
}

impl PsoBlock {
    pub fn new(name_hash: u32, data: Vec<u8>) -> Self {
        Self { name_hash, data }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PsoBlockInfo {
    pub id: u16,
    pub name_hash: u32,
    pub offset: usize,
    pub length: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PsoEntry {
    pub name_hash: u32,
    pub type_id: u8,
    pub subtype: u8,
    pub data_offset: u16,
    pub reference_key: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PsoStruct {
    pub name_hash: u32,
    pub length: i32,
    pub entries: Vec<PsoEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PsoEnumEntry {
    pub name_hash: u32,
    pub value: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PsoEnum {
    pub name_hash: u32,
    pub entries: Vec<PsoEnumEntry>,
}

#[derive(Debug, Clone)]
pub struct PsoDocument<'a> {
    bytes: &'a [u8],
    sections: BTreeMap<u32, std::ops::Range<usize>>,
    pub blocks: BTreeMap<u16, PsoBlockInfo>,
    pub root_block_id: u16,
}

impl<'a> PsoDocument<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, PsoError> {
        let sections = parse_sections(bytes)?;
        let pmap_range = sections
            .get(&PMAP)
            .ok_or(PsoError::MissingSection("PMAP"))?;
        let pmap = &bytes[pmap_range.clone()];
        if pmap.len() < 16 {
            return Err(PsoError::Truncated("PMAP header"));
        }
        let root_raw = read_i32_be(pmap, 8, "PMAP root block")?;
        if root_raw <= 0 || root_raw > i32::from(u16::MAX) {
            return Err(PsoError::Malformed(format!(
                "invalid PMAP root block id {root_raw}"
            )));
        }
        let root_block_id = root_raw as u16;
        let count = read_u16_be(pmap, 12, "PMAP block count")? as usize;
        let required = 16_usize
            .checked_add(
                count
                    .checked_mul(16)
                    .ok_or_else(|| PsoError::Malformed("PMAP block count overflow".into()))?,
            )
            .ok_or_else(|| PsoError::Malformed("PMAP size overflow".into()))?;
        if required > pmap.len() {
            return Err(PsoError::Truncated("PMAP block table"));
        }

        let mut blocks = BTreeMap::new();
        for index in 0..count {
            let base = 16 + index * 16;
            let name_hash = read_u32_be(pmap, base, "PMAP block type")?;
            let offset = read_i32_be(pmap, base + 4, "PMAP block offset")?;
            let length = read_i32_be(pmap, base + 12, "PMAP block length")?;
            if offset < 0 || length < 0 {
                return Err(PsoError::Malformed(format!(
                    "negative PMAP block range for block {}",
                    index + 1
                )));
            }
            blocks.insert(
                (index + 1) as u16,
                PsoBlockInfo {
                    id: (index + 1) as u16,
                    name_hash,
                    offset: offset as usize,
                    length: length as usize,
                },
            );
        }

        let doc = Self {
            bytes,
            sections,
            blocks,
            root_block_id,
        };
        // Validate all PMAP ranges against PSIN immediately.
        for info in doc.blocks.values() {
            let _ = doc.block_bytes(info.id)?;
        }
        Ok(doc)
    }

    pub fn section(&self, ident: u32) -> Option<&'a [u8]> {
        self.sections
            .get(&ident)
            .map(|range| &self.bytes[range.clone()])
    }

    pub fn block(&self, id: u16) -> Option<&PsoBlockInfo> {
        self.blocks.get(&id)
    }

    pub fn root_block(&self) -> Result<&PsoBlockInfo, PsoError> {
        self.block(self.root_block_id).ok_or_else(|| {
            PsoError::Malformed(format!("missing root block {}", self.root_block_id))
        })
    }

    pub fn block_bytes(&self, id: u16) -> Result<&'a [u8], PsoError> {
        let psin_range = self
            .sections
            .get(&PSIN)
            .ok_or(PsoError::MissingSection("PSIN"))?;
        let psin = &self.bytes[psin_range.clone()];
        let info = self
            .blocks
            .get(&id)
            .ok_or_else(|| PsoError::Malformed(format!("missing PSO block {id}")))?;
        let end = info
            .offset
            .checked_add(info.length)
            .ok_or_else(|| PsoError::Malformed("PSO block range overflow".into()))?;
        if end > psin.len() {
            return Err(PsoError::Malformed(format!(
                "PSO block {id} range {}..{} exceeds PSIN size {}",
                info.offset,
                end,
                psin.len()
            )));
        }
        Ok(&psin[info.offset..end])
    }

    pub fn array_bytes(
        &self,
        owner: &[u8],
        field_offset: usize,
        stride: usize,
    ) -> Result<Vec<&'a [u8]>, PsoError> {
        let header = PsoArrayHeader::decode(owner, field_offset)?;
        if header.count == 0 {
            if !header.pointer.is_null() {
                return Err(PsoError::Malformed(
                    "zero-count PSO array has a non-null pointer".into(),
                ));
            }
            return Ok(Vec::new());
        }
        if header.pointer.is_null() {
            return Err(PsoError::Malformed(
                "non-empty PSO array has a null pointer".into(),
            ));
        }
        let block = self.block_bytes(header.pointer.block_id)?;
        let start = header.pointer.offset as usize;
        let total = usize::from(header.count)
            .checked_mul(stride)
            .ok_or_else(|| PsoError::Malformed("PSO array size overflow".into()))?;
        let end = start
            .checked_add(total)
            .ok_or_else(|| PsoError::Malformed("PSO array range overflow".into()))?;
        if end > block.len() {
            return Err(PsoError::Malformed(format!(
                "PSO array range {start}..{end} exceeds block size {}",
                block.len()
            )));
        }
        Ok((0..usize::from(header.count))
            .map(|index| {
                let item_start = start + index * stride;
                &block[item_start..item_start + stride]
            })
            .collect())
    }

    pub fn hash_array(&self, owner: &[u8], field_offset: usize) -> Result<Vec<u32>, PsoError> {
        let header = PsoArrayHeader::decode(owner, field_offset)?;
        if header.count == 0 {
            if !header.pointer.is_null() {
                return Err(PsoError::Malformed(
                    "zero-count PSO hash array has a non-null pointer".into(),
                ));
            }
            return Ok(Vec::new());
        }
        if header.pointer.is_null() {
            return Err(PsoError::Malformed(
                "non-empty PSO hash array has a null pointer".into(),
            ));
        }
        let info = self.block(header.pointer.block_id).ok_or_else(|| {
            PsoError::Malformed(format!(
                "missing PSO hash-array block {}",
                header.pointer.block_id
            ))
        })?;
        if info.name_hash != u32::from(DATA_UINT) {
            return Err(PsoError::Malformed(format!(
                "PSO hash array points to block type 0x{:08X}, expected UInt",
                info.name_hash
            )));
        }
        let block = self.block_bytes(header.pointer.block_id)?;
        let start = header.pointer.offset as usize;
        let total = usize::from(header.count)
            .checked_mul(4)
            .ok_or_else(|| PsoError::Malformed("PSO hash array size overflow".into()))?;
        let end = start
            .checked_add(total)
            .ok_or_else(|| PsoError::Malformed("PSO hash array range overflow".into()))?;
        if end > block.len() {
            return Err(PsoError::Malformed(
                "PSO hash array exceeds target block".into(),
            ));
        }
        (0..usize::from(header.count))
            .map(|index| read_u32_be(block, start + index * 4, "PSO hash array item"))
            .collect()
    }
}

pub fn build_pso(
    blocks: &[PsoBlock],
    root_block_id: u16,
    structs: &[PsoStruct],
    enums: &[PsoEnum],
) -> Result<Vec<u8>, PsoError> {
    if blocks.is_empty() {
        return Err(PsoError::Malformed(
            "PSO requires at least one block".into(),
        ));
    }
    if root_block_id == 0 || usize::from(root_block_id) > blocks.len() {
        return Err(PsoError::Malformed(format!(
            "root block id {root_block_id} is out of range"
        )));
    }
    let psin = build_psin(blocks, 1)?;
    let pmap = build_pmap(blocks, root_block_id, 1)?;
    let psch = build_psch(structs, enums)?;
    let mut out = Vec::with_capacity(psin.len() + pmap.len() + psch.len());
    out.extend_from_slice(&psin);
    out.extend_from_slice(&pmap);
    out.extend_from_slice(&psch);
    Ok(out)
}

pub fn build_psch(structs: &[PsoStruct], enums: &[PsoEnum]) -> Result<Vec<u8>, PsoError> {
    let mut items: Vec<(u32, Vec<u8>)> = Vec::with_capacity(structs.len() + enums.len());
    for info in structs {
        let mut chunk = Vec::with_capacity(12 + info.entries.len() * 12);
        chunk.extend_from_slice(&[0, 0]);
        let count = u16::try_from(info.entries.len())
            .map_err(|_| PsoError::Malformed("too many PSO structure entries".into()))?;
        chunk.extend_from_slice(&count.to_be_bytes());
        chunk.extend_from_slice(&info.length.to_be_bytes());
        chunk.extend_from_slice(&[0; 4]);
        for entry in &info.entries {
            chunk.extend_from_slice(&entry.name_hash.to_be_bytes());
            chunk.push(entry.type_id);
            chunk.push(entry.subtype);
            chunk.extend_from_slice(&entry.data_offset.to_be_bytes());
            chunk.extend_from_slice(&entry.reference_key.to_be_bytes());
        }
        items.push((info.name_hash, chunk));
    }
    for info in enums {
        let mut chunk = Vec::with_capacity(4 + info.entries.len() * 8);
        chunk.extend_from_slice(&[1, 0]);
        let count = u16::try_from(info.entries.len())
            .map_err(|_| PsoError::Malformed("too many PSO enum entries".into()))?;
        chunk.extend_from_slice(&count.to_be_bytes());
        for entry in &info.entries {
            chunk.extend_from_slice(&entry.name_hash.to_be_bytes());
            chunk.extend_from_slice(&entry.value.to_be_bytes());
        }
        items.push((info.name_hash, chunk));
    }

    let header_size = 12_usize
        .checked_add(
            items
                .len()
                .checked_mul(8)
                .ok_or_else(|| PsoError::Malformed("PSCH index size overflow".into()))?,
        )
        .ok_or_else(|| PsoError::Malformed("PSCH header overflow".into()))?;
    let mut offset = header_size;
    let mut indexes = Vec::with_capacity(items.len());
    for (hash, chunk) in &items {
        indexes.push((*hash, offset));
        offset = offset
            .checked_add(chunk.len())
            .ok_or_else(|| PsoError::Malformed("PSCH size overflow".into()))?;
    }

    let mut out = Vec::with_capacity(offset);
    out.extend_from_slice(b"PSCH");
    out.extend_from_slice(&(offset as u32).to_be_bytes());
    out.extend_from_slice(&(items.len() as u32).to_be_bytes());
    for (hash, item_offset) in indexes {
        out.extend_from_slice(&hash.to_be_bytes());
        out.extend_from_slice(&(item_offset as i32).to_be_bytes());
    }
    for (_, chunk) in items {
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

fn build_psin(blocks: &[PsoBlock], alignment: usize) -> Result<Vec<u8>, PsoError> {
    let offsets = block_offsets(blocks, alignment)?;
    let section_length = offsets
        .iter()
        .zip(blocks)
        .map(|(offset, block)| offset + block.data.len())
        .max()
        .unwrap_or(16);
    let mut out = Vec::with_capacity(section_length);
    out.extend_from_slice(b"PSIN");
    out.extend_from_slice(&(section_length as u32).to_be_bytes());
    // Runtime YMF profile: eight-byte zero prefix.
    out.extend_from_slice(&[0; 8]);
    for (offset, block) in offsets.into_iter().zip(blocks) {
        if out.len() > offset {
            return Err(PsoError::Malformed("PSIN block offsets overlap".into()));
        }
        out.resize(offset, 0);
        out.extend_from_slice(&block.data);
    }
    Ok(out)
}

fn build_pmap(
    blocks: &[PsoBlock],
    root_block_id: u16,
    alignment: usize,
) -> Result<Vec<u8>, PsoError> {
    let offsets = block_offsets(blocks, alignment)?;
    let count = u16::try_from(blocks.len())
        .map_err(|_| PsoError::Malformed("too many PSO blocks".into()))?;
    let section_length = 16_usize
        .checked_add(blocks.len() * 16)
        .ok_or_else(|| PsoError::Malformed("PMAP size overflow".into()))?;
    let mut out = Vec::with_capacity(section_length);
    out.extend_from_slice(b"PMAP");
    out.extend_from_slice(&(section_length as u32).to_be_bytes());
    out.extend_from_slice(&i32::from(root_block_id).to_be_bytes());
    out.extend_from_slice(&count.to_be_bytes());
    out.extend_from_slice(&0x7070_u16.to_be_bytes());
    for (block, offset) in blocks.iter().zip(offsets) {
        let offset = i32::try_from(offset)
            .map_err(|_| PsoError::Malformed("PSO block offset exceeds i32".into()))?;
        let length = i32::try_from(block.data.len())
            .map_err(|_| PsoError::Malformed("PSO block length exceeds i32".into()))?;
        out.extend_from_slice(&block.name_hash.to_be_bytes());
        out.extend_from_slice(&offset.to_be_bytes());
        out.extend_from_slice(&0_i32.to_be_bytes());
        out.extend_from_slice(&length.to_be_bytes());
    }
    Ok(out)
}

fn block_offsets(blocks: &[PsoBlock], alignment: usize) -> Result<Vec<usize>, PsoError> {
    if alignment == 0 || !alignment.is_power_of_two() {
        return Err(PsoError::Malformed(
            "PSO alignment must be a power of two".into(),
        ));
    }
    let mut current = 16_usize;
    let mut offsets = Vec::with_capacity(blocks.len());
    for block in blocks {
        current = (current + alignment - 1) & !(alignment - 1);
        offsets.push(current);
        current = current
            .checked_add(block.data.len())
            .ok_or_else(|| PsoError::Malformed("PSO block offsets overflow".into()))?;
    }
    Ok(offsets)
}

fn parse_sections(bytes: &[u8]) -> Result<BTreeMap<u32, std::ops::Range<usize>>, PsoError> {
    let mut sections = BTreeMap::new();
    let mut offset = 0_usize;
    while offset < bytes.len() {
        if bytes.len() - offset < 8 {
            return Err(PsoError::Truncated("PSO section header"));
        }
        let ident = read_u32_be(bytes, offset, "PSO section id")?;
        let length = read_u32_be(bytes, offset + 4, "PSO section length")? as usize;
        if length < 8 {
            return Err(PsoError::Malformed(format!(
                "PSO section at 0x{offset:X} has invalid length {length}"
            )));
        }
        let end = offset
            .checked_add(length)
            .ok_or_else(|| PsoError::Malformed("PSO section range overflow".into()))?;
        if end > bytes.len() {
            return Err(PsoError::Truncated("PSO section payload"));
        }
        if sections.insert(ident, offset..end).is_some() {
            return Err(PsoError::Malformed(format!(
                "duplicate PSO section 0x{ident:08X}"
            )));
        }
        offset = end;
    }
    Ok(sections)
}

pub fn read_u32_be(bytes: &[u8], offset: usize, what: &'static str) -> Result<u32, PsoError> {
    let end = offset.checked_add(4).ok_or(PsoError::Truncated(what))?;
    let slice = bytes.get(offset..end).ok_or(PsoError::Truncated(what))?;
    Ok(u32::from_be_bytes(
        slice.try_into().expect("four-byte slice"),
    ))
}

pub fn read_i32_be(bytes: &[u8], offset: usize, what: &'static str) -> Result<i32, PsoError> {
    Ok(read_u32_be(bytes, offset, what)? as i32)
}

pub fn read_u16_be(bytes: &[u8], offset: usize, what: &'static str) -> Result<u16, PsoError> {
    let end = offset.checked_add(2).ok_or(PsoError::Truncated(what))?;
    let slice = bytes.get(offset..end).ok_or(PsoError::Truncated(what))?;
    Ok(u16::from_be_bytes(
        slice.try_into().expect("two-byte slice"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_word_round_trip() {
        let word = encode_pointer_word(7, 0x12345).unwrap();
        assert_eq!(
            decode_pointer_word(word),
            PsoPointer {
                block_id: 7,
                offset: 0x12345,
            }
        );
    }

    #[test]
    fn minimal_document_round_trip() {
        let blocks = vec![PsoBlock::new(0x11223344, vec![1, 2, 3, 4])];
        let bytes = build_pso(&blocks, 1, &[], &[]).unwrap();
        let parsed = PsoDocument::parse(&bytes).unwrap();
        assert_eq!(parsed.root_block().unwrap().name_hash, 0x11223344);
        assert_eq!(parsed.block_bytes(1).unwrap(), &[1, 2, 3, 4]);
    }
}

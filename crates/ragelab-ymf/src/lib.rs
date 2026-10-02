//! Streamed pack manifest (`_manifest.ymf`) model and PSO encoder/decoder.
//!
//! Modern streamed manifests use a PSO `CPackFileMetaData` root. The first
//! implementation focuses on the dependency arrays needed by the map splitter:
//! IMAP/YMAP -> ITYP/YTYP, ITYP/YTYP -> ITYP/YTYP, and MLO interior bounds.

use std::{collections::BTreeSet, error::Error, fmt};

use ragelab_meta::MetaHash;
use ragelab_pso::{
    build_pso, read_i32_be, read_u32_be, PsoArrayHeader, PsoBlock, PsoDocument, PsoEntry, PsoEnum,
    PsoEnumEntry, PsoError, PsoPointer, PsoStruct, ARRAY_INFO_HASH, DATA_ARRAY, DATA_ENUM,
    DATA_FLAGS, DATA_STRING, DATA_STRUCTURE, DATA_UINT,
};

pub const YMF_PSO_ROOT: u32 = 0x93A68A2F;
pub const YMF_PSO_IMAP_DEPENDENCIES: u32 = 0xC11F3EE1;
pub const YMF_PSO_ITYP_DEPENDENCIES: u32 = 0x5A564E50;
pub const YMF_PSO_INTERIOR_BOUNDS: u32 = 0x2C325290;

const FIELD_MAP_DATA_GROUPS: u32 = 0xB52CAE23;
const FIELD_HD_TXD_BINDINGS: u32 = 0xF78AFB23;
const FIELD_IMAP_DEPENDENCIES_LEGACY: u32 = 0x2BDA143F;
const FIELD_IMAP_DEPENDENCIES: u32 = 0xDD4C5CCC;
const FIELD_ITYP_DEPENDENCIES: u32 = 0xD2611C99;
const FIELD_INTERIORS: u32 = 0x38767A8F;
const FIELD_IMAP_NAME: u32 = 0x31AF439F;
const FIELD_ITYP_NAME: u32 = 0xAC445064;
const FIELD_NAME: u32 = 0xACEC22BE;
const FIELD_MANIFEST_FLAGS: u32 = 0x6452A05B;
const FIELD_ITYP_DEP_ARRAY: u32 = 0x8FB42AE6;

const TYPE_MAP_DATA_GROUP: u32 = 0xC25B3923;
const TYPE_HD_TXD_BINDING: u32 = 0x59869C63;
const TYPE_IMAP_DEPENDENCY_LEGACY: u32 = 0xD0AD6E62;
const MANIFEST_FLAGS_ENUM: u32 = 0x6452A05B;
const MANIFEST_FLAGS_NONE: u32 = 0x21569096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ManifestFlags(pub u32);

impl ManifestFlags {
    pub const NONE: Self = Self(0);
    pub const INTERIOR_DATA: Self = Self(1);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YmfMapDependency {
    pub ymap: MetaHash,
    pub ytyps: Vec<MetaHash>,
    pub flags: ManifestFlags,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YmfYtypDependency {
    pub ytyp: MetaHash,
    pub ytyps: Vec<MetaHash>,
    pub flags: ManifestFlags,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YmfInteriorBounds {
    pub name: MetaHash,
    pub bounds: Vec<MetaHash>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ymf {
    pub maps: Vec<YmfMapDependency>,
    pub ytyps: Vec<YmfYtypDependency>,
    pub interiors: Vec<YmfInteriorBounds>,
}

#[derive(Debug)]
pub enum YmfError {
    Pso(PsoError),
    Invalid(String),
}

impl fmt::Display for YmfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pso(error) => write!(f, "PSO: {error}"),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl Error for YmfError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Pso(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

impl From<PsoError> for YmfError {
    fn from(value: PsoError) -> Self {
        Self::Pso(value)
    }
}

impl Ymf {
    pub fn normalize(&mut self) {
        for map in &mut self.maps {
            normalize_hashes(&mut map.ytyps);
        }
        for ytyp in &mut self.ytyps {
            normalize_hashes(&mut ytyp.ytyps);
        }
        for interior in &mut self.interiors {
            normalize_hashes(&mut interior.bounds);
        }
        self.maps.sort_by_key(|entry| entry.ymap.0);
        self.maps.dedup_by_key(|entry| entry.ymap.0);
        self.ytyps.sort_by_key(|entry| entry.ytyp.0);
        self.ytyps.dedup_by_key(|entry| entry.ytyp.0);
        self.interiors.sort_by_key(|entry| entry.name.0);
        self.interiors.dedup_by_key(|entry| entry.name.0);
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, YmfError> {
        let doc = PsoDocument::parse(bytes)?;
        let root_info = doc.root_block()?;
        if root_info.name_hash != YMF_PSO_ROOT {
            return Err(YmfError::Invalid(format!(
                "expected CPackFileMetaData root 0x{YMF_PSO_ROOT:08X}, got 0x{:08X}",
                root_info.name_hash
            )));
        }
        let root = doc.block_bytes(root_info.id)?;
        if root.len() < 96 {
            return Err(YmfError::Invalid(format!(
                "CPackFileMetaData root is {} bytes, expected at least 96",
                root.len()
            )));
        }

        let maps = parse_dependencies(&doc, root, 48, true)?;
        let ytyps = parse_dependencies(&doc, root, 64, false)?;
        let interiors = parse_interiors(&doc, root)?;

        Ok(Self {
            maps: maps
                .into_iter()
                .map(|(name, deps, flags)| YmfMapDependency {
                    ymap: MetaHash(name),
                    ytyps: deps.into_iter().map(MetaHash).collect(),
                    flags: ManifestFlags(flags),
                })
                .collect(),
            ytyps: ytyps
                .into_iter()
                .map(|(name, deps, flags)| YmfYtypDependency {
                    ytyp: MetaHash(name),
                    ytyps: deps.into_iter().map(MetaHash).collect(),
                    flags: ManifestFlags(flags),
                })
                .collect(),
            interiors: interiors
                .into_iter()
                .map(|(name, bounds)| YmfInteriorBounds {
                    name: MetaHash(name),
                    bounds: bounds.into_iter().map(MetaHash).collect(),
                })
                .collect(),
        })
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, YmfError> {
        let mut manifest = self.clone();
        manifest.normalize();

        let mut hash_data = Vec::<u8>::new();
        let mut map_data = Vec::<u8>::with_capacity(manifest.maps.len() * 24);
        let mut ytyp_data = Vec::<u8>::with_capacity(manifest.ytyps.len() * 24);
        let mut interior_data = Vec::<u8>::with_capacity(manifest.interiors.len() * 24);

        // Block order is deterministic: optional shared UInt hash arrays,
        // dependency arrays, interiors, and finally the root.
        let has_hashes = manifest.maps.iter().any(|x| !x.ytyps.is_empty())
            || manifest.ytyps.iter().any(|x| !x.ytyps.is_empty())
            || manifest.interiors.iter().any(|x| !x.bounds.is_empty());
        let hash_block_id = has_hashes.then_some(1_u16);
        let mut next_block_id = if has_hashes { 2_u16 } else { 1_u16 };

        let map_block_id = if manifest.maps.is_empty() {
            None
        } else {
            let id = next_block_id;
            next_block_id += 1;
            Some(id)
        };
        let ytyp_block_id = if manifest.ytyps.is_empty() {
            None
        } else {
            let id = next_block_id;
            next_block_id += 1;
            Some(id)
        };
        let interior_block_id = if manifest.interiors.is_empty() {
            None
        } else {
            let id = next_block_id;
            next_block_id += 1;
            Some(id)
        };
        let root_block_id = next_block_id;

        for item in &manifest.maps {
            append_dependency_item(
                &mut map_data,
                item.ymap.0,
                item.flags,
                &item.ytyps,
                &mut hash_data,
                hash_block_id,
            )?;
        }
        for item in &manifest.ytyps {
            append_dependency_item(
                &mut ytyp_data,
                item.ytyp.0,
                item.flags,
                &item.ytyps,
                &mut hash_data,
                hash_block_id,
            )?;
        }
        for item in &manifest.interiors {
            append_interior_item(&mut interior_data, item, &mut hash_data, hash_block_id)?;
        }

        let mut root = vec![0_u8; 96];
        if let Some(id) = map_block_id {
            write_array_header(&mut root, 48, id, 0, manifest.maps.len())?;
        }
        if let Some(id) = ytyp_block_id {
            write_array_header(&mut root, 64, id, 0, manifest.ytyps.len())?;
        }
        if let Some(id) = interior_block_id {
            write_array_header(&mut root, 80, id, 0, manifest.interiors.len())?;
        }

        let mut blocks = Vec::<PsoBlock>::new();
        if has_hashes {
            blocks.push(PsoBlock::new(u32::from(DATA_UINT), hash_data));
        }
        if map_block_id.is_some() {
            blocks.push(PsoBlock::new(YMF_PSO_IMAP_DEPENDENCIES, map_data));
        }
        if ytyp_block_id.is_some() {
            blocks.push(PsoBlock::new(YMF_PSO_ITYP_DEPENDENCIES, ytyp_data));
        }
        if interior_block_id.is_some() {
            blocks.push(PsoBlock::new(YMF_PSO_INTERIOR_BOUNDS, interior_data));
        }
        blocks.push(PsoBlock::new(YMF_PSO_ROOT, root));

        if usize::from(root_block_id) != blocks.len() {
            return Err(YmfError::Invalid(
                "internal YMF block-id accounting mismatch".into(),
            ));
        }

        let mut structs = vec![root_schema()];
        if map_block_id.is_some() {
            structs.push(dependency_schema(
                YMF_PSO_IMAP_DEPENDENCIES,
                FIELD_IMAP_NAME,
            ));
        }
        if ytyp_block_id.is_some() {
            structs.push(dependency_schema(
                YMF_PSO_ITYP_DEPENDENCIES,
                FIELD_ITYP_NAME,
            ));
        }
        if interior_block_id.is_some() {
            structs.push(interior_schema());
        }
        let enums = if map_block_id.is_some() || ytyp_block_id.is_some() {
            vec![manifest_flags_enum()]
        } else {
            Vec::new()
        };

        Ok(build_pso(&blocks, root_block_id, &structs, &enums)?)
    }
}

fn append_dependency_item(
    output: &mut Vec<u8>,
    name: u32,
    flags: ManifestFlags,
    dependencies: &[MetaHash],
    hash_data: &mut Vec<u8>,
    hash_block_id: Option<u16>,
) -> Result<(), YmfError> {
    let mut item = [0_u8; 24];
    item[0..4].copy_from_slice(&name.to_be_bytes());
    item[4..8].copy_from_slice(&(flags.0 as i32).to_be_bytes());
    if !dependencies.is_empty() {
        let block_id = hash_block_id.ok_or_else(|| {
            YmfError::Invalid("dependency hashes require a UInt PSO block".into())
        })?;
        let relative_offset = u32::try_from(hash_data.len())
            .map_err(|_| YmfError::Invalid("YMF hash block exceeds u32".into()))?;
        for dependency in dependencies {
            hash_data.extend_from_slice(&dependency.0.to_be_bytes());
        }
        let header = PsoArrayHeader {
            pointer: PsoPointer {
                block_id,
                offset: relative_offset,
            },
            count: u16::try_from(dependencies.len())
                .map_err(|_| YmfError::Invalid("too many YMF dependencies".into()))?,
        }
        .encode()?;
        item[8..24].copy_from_slice(&header);
    }
    output.extend_from_slice(&item);
    Ok(())
}

fn append_interior_item(
    output: &mut Vec<u8>,
    interior: &YmfInteriorBounds,
    hash_data: &mut Vec<u8>,
    hash_block_id: Option<u16>,
) -> Result<(), YmfError> {
    let mut item = [0_u8; 24];
    item[0..4].copy_from_slice(&interior.name.0.to_be_bytes());
    if !interior.bounds.is_empty() {
        let block_id = hash_block_id
            .ok_or_else(|| YmfError::Invalid("interior bounds require a UInt PSO block".into()))?;
        let relative_offset = u32::try_from(hash_data.len())
            .map_err(|_| YmfError::Invalid("YMF hash block exceeds u32".into()))?;
        for bound in &interior.bounds {
            hash_data.extend_from_slice(&bound.0.to_be_bytes());
        }
        let header = PsoArrayHeader {
            pointer: PsoPointer {
                block_id,
                offset: relative_offset,
            },
            count: u16::try_from(interior.bounds.len())
                .map_err(|_| YmfError::Invalid("too many interior bounds".into()))?,
        }
        .encode()?;
        item[8..24].copy_from_slice(&header);
    }
    output.extend_from_slice(&item);
    Ok(())
}

fn write_array_header(
    root: &mut [u8],
    offset: usize,
    block_id: u16,
    relative_offset: u32,
    count: usize,
) -> Result<(), YmfError> {
    let count =
        u16::try_from(count).map_err(|_| YmfError::Invalid("YMF root array exceeds u16".into()))?;
    let header = PsoArrayHeader {
        pointer: PsoPointer {
            block_id,
            offset: relative_offset,
        },
        count,
    }
    .encode()?;
    root[offset..offset + 16].copy_from_slice(&header);
    Ok(())
}

fn parse_dependencies(
    doc: &PsoDocument<'_>,
    root: &[u8],
    root_offset: usize,
    is_map: bool,
) -> Result<Vec<(u32, Vec<u32>, u32)>, YmfError> {
    let expected_type = if is_map {
        YMF_PSO_IMAP_DEPENDENCIES
    } else {
        YMF_PSO_ITYP_DEPENDENCIES
    };
    let header = PsoArrayHeader::decode(root, root_offset)?;
    if header.count == 0 {
        return Ok(Vec::new());
    }
    let target = doc.block(header.pointer.block_id).ok_or_else(|| {
        YmfError::Invalid(format!(
            "YMF root array references missing PSO block {}",
            header.pointer.block_id
        ))
    })?;
    if target.name_hash != expected_type {
        return Err(YmfError::Invalid(format!(
            "YMF root array at 0x{root_offset:X} points to 0x{:08X}, expected 0x{expected_type:08X}",
            target.name_hash
        )));
    }
    let items = doc.array_bytes(root, root_offset, 24)?;
    items
        .into_iter()
        .map(|item| {
            let name = read_u32_be(item, 0, "YMF dependency name")?;
            let flags = read_i32_be(item, 4, "YMF dependency flags")? as u32;
            let deps = doc.hash_array(item, 8)?;
            Ok((name, deps, flags))
        })
        .collect()
}

fn parse_interiors(doc: &PsoDocument<'_>, root: &[u8]) -> Result<Vec<(u32, Vec<u32>)>, YmfError> {
    let header = PsoArrayHeader::decode(root, 80)?;
    if header.count == 0 {
        return Ok(Vec::new());
    }
    let target = doc.block(header.pointer.block_id).ok_or_else(|| {
        YmfError::Invalid(format!(
            "YMF interiors reference missing PSO block {}",
            header.pointer.block_id
        ))
    })?;
    if target.name_hash != YMF_PSO_INTERIOR_BOUNDS {
        return Err(YmfError::Invalid(format!(
            "YMF interiors point to 0x{:08X}, expected 0x{YMF_PSO_INTERIOR_BOUNDS:08X}",
            target.name_hash
        )));
    }
    doc.array_bytes(root, 80, 24)?
        .into_iter()
        .map(|item| {
            Ok((
                read_u32_be(item, 0, "YMF interior name")?,
                doc.hash_array(item, 8)?,
            ))
        })
        .collect()
}

fn normalize_hashes(values: &mut Vec<MetaHash>) {
    let mut seen = BTreeSet::new();
    values.retain(|value| value.0 != 0 && seen.insert(value.0));
    values.sort_by_key(|value| value.0);
}

fn root_schema() -> PsoStruct {
    PsoStruct {
        name_hash: YMF_PSO_ROOT,
        length: 96,
        entries: vec![
            PsoEntry {
                name_hash: ARRAY_INFO_HASH,
                type_id: DATA_STRUCTURE,
                subtype: 0,
                data_offset: 0,
                reference_key: TYPE_MAP_DATA_GROUP,
            },
            PsoEntry {
                name_hash: FIELD_MAP_DATA_GROUPS,
                type_id: DATA_ARRAY,
                subtype: 0,
                data_offset: 0,
                reference_key: 0,
            },
            PsoEntry {
                name_hash: ARRAY_INFO_HASH,
                type_id: DATA_STRUCTURE,
                subtype: 0,
                data_offset: 0,
                reference_key: TYPE_HD_TXD_BINDING,
            },
            PsoEntry {
                name_hash: FIELD_HD_TXD_BINDINGS,
                type_id: DATA_ARRAY,
                subtype: 0,
                data_offset: 16,
                reference_key: 2,
            },
            PsoEntry {
                name_hash: ARRAY_INFO_HASH,
                type_id: DATA_STRUCTURE,
                subtype: 0,
                data_offset: 0,
                reference_key: TYPE_IMAP_DEPENDENCY_LEGACY,
            },
            PsoEntry {
                name_hash: FIELD_IMAP_DEPENDENCIES_LEGACY,
                type_id: DATA_ARRAY,
                subtype: 0,
                data_offset: 32,
                reference_key: 4,
            },
            PsoEntry {
                name_hash: ARRAY_INFO_HASH,
                type_id: DATA_STRUCTURE,
                subtype: 0,
                data_offset: 0,
                reference_key: YMF_PSO_IMAP_DEPENDENCIES,
            },
            PsoEntry {
                name_hash: FIELD_IMAP_DEPENDENCIES,
                type_id: DATA_ARRAY,
                subtype: 0,
                data_offset: 48,
                reference_key: 6,
            },
            PsoEntry {
                name_hash: ARRAY_INFO_HASH,
                type_id: DATA_STRUCTURE,
                subtype: 0,
                data_offset: 0,
                reference_key: YMF_PSO_ITYP_DEPENDENCIES,
            },
            PsoEntry {
                name_hash: FIELD_ITYP_DEPENDENCIES,
                type_id: DATA_ARRAY,
                subtype: 0,
                data_offset: 64,
                reference_key: 8,
            },
            PsoEntry {
                name_hash: ARRAY_INFO_HASH,
                type_id: DATA_STRUCTURE,
                subtype: 0,
                data_offset: 0,
                reference_key: YMF_PSO_INTERIOR_BOUNDS,
            },
            PsoEntry {
                name_hash: FIELD_INTERIORS,
                type_id: DATA_ARRAY,
                subtype: 0,
                data_offset: 80,
                reference_key: 10,
            },
        ],
    }
}

fn dependency_schema(type_hash: u32, name_field: u32) -> PsoStruct {
    PsoStruct {
        name_hash: type_hash,
        length: 24,
        entries: vec![
            PsoEntry {
                name_hash: name_field,
                type_id: DATA_STRING,
                subtype: 7,
                data_offset: 0,
                reference_key: 0,
            },
            PsoEntry {
                name_hash: ARRAY_INFO_HASH,
                type_id: DATA_ENUM,
                subtype: 0,
                data_offset: 0,
                reference_key: MANIFEST_FLAGS_ENUM,
            },
            PsoEntry {
                name_hash: FIELD_MANIFEST_FLAGS,
                type_id: DATA_FLAGS,
                subtype: 0,
                data_offset: 4,
                reference_key: 0x00200001,
            },
            PsoEntry {
                name_hash: ARRAY_INFO_HASH,
                type_id: DATA_STRING,
                subtype: 7,
                data_offset: 0,
                reference_key: 0,
            },
            PsoEntry {
                name_hash: FIELD_ITYP_DEP_ARRAY,
                type_id: DATA_ARRAY,
                subtype: 0,
                data_offset: 8,
                reference_key: 3,
            },
        ],
    }
}

fn interior_schema() -> PsoStruct {
    PsoStruct {
        name_hash: YMF_PSO_INTERIOR_BOUNDS,
        length: 24,
        entries: vec![
            PsoEntry {
                name_hash: FIELD_NAME,
                type_id: DATA_STRING,
                subtype: 7,
                data_offset: 0,
                reference_key: 0,
            },
            PsoEntry {
                name_hash: ARRAY_INFO_HASH,
                type_id: DATA_STRING,
                subtype: 7,
                data_offset: 0,
                reference_key: 0,
            },
            PsoEntry {
                name_hash: 0xC496E4A8,
                type_id: DATA_ARRAY,
                subtype: 0,
                data_offset: 8,
                reference_key: 1,
            },
        ],
    }
}

fn manifest_flags_enum() -> PsoEnum {
    PsoEnum {
        name_hash: MANIFEST_FLAGS_ENUM,
        entries: vec![PsoEnumEntry {
            name_hash: MANIFEST_FLAGS_NONE,
            value: 0,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pso_round_trip_for_basic_manifest() {
        let source = Ymf {
            maps: vec![YmfMapDependency {
                ymap: MetaHash(0x11111111),
                ytyps: vec![MetaHash(0x22222222), MetaHash(0x33333333)],
                flags: ManifestFlags::NONE,
            }],
            ytyps: vec![YmfYtypDependency {
                ytyp: MetaHash(0x22222222),
                ytyps: vec![MetaHash(0x44444444)],
                flags: ManifestFlags::NONE,
            }],
            interiors: Vec::new(),
        };
        let bytes = source.to_bytes().unwrap();
        assert!(bytes.starts_with(b"PSIN"));
        let parsed = Ymf::from_bytes(&bytes).unwrap();
        assert_eq!(parsed, source);
    }

    #[test]
    fn pso_round_trip_for_interior() {
        let source = Ymf {
            maps: vec![YmfMapDependency {
                ymap: MetaHash(1),
                ytyps: vec![MetaHash(2)],
                flags: ManifestFlags::INTERIOR_DATA,
            }],
            ytyps: vec![YmfYtypDependency {
                ytyp: MetaHash(2),
                ytyps: vec![],
                flags: ManifestFlags::INTERIOR_DATA,
            }],
            interiors: vec![YmfInteriorBounds {
                name: MetaHash(3),
                bounds: vec![MetaHash(3)],
            }],
        };
        let parsed = Ymf::from_bytes(&source.to_bytes().unwrap()).unwrap();
        assert_eq!(parsed, source);
    }
}

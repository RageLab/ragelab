//! YTYP (`CMapTypes`) domain model and typed META reader.
//!
//! The current reader intentionally focuses on the archetype fields needed by
//! the map dependency resolver. Geometry, extensions and detailed MLO room/
//! portal records remain future milestones; room/portal counts are exposed.

use ragelab_hash::jenkins;
use ragelab_meta::{read_i32, read_u32, MetaArrayRef, MetaDocument, MetaHash, MetaStructureInfo};
use ragelab_resource::ResourceError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchetypeKind {
    Base,
    Time,
    Mlo,
    Unknown(MetaHash),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetType {
    Uninitialized,
    Fragment,
    Drawable,
    DrawableDictionary,
    Assetless,
    Unknown(i32),
}

impl AssetType {
    pub const fn from_raw(value: i32) -> Self {
        match value {
            0 => Self::Uninitialized,
            1 => Self::Fragment,
            2 => Self::Drawable,
            3 => Self::DrawableDictionary,
            4 => Self::Assetless,
            other => Self::Unknown(other),
        }
    }

    /// Primary streamed file extension implied by this archetype type.
    pub const fn primary_extension(self) -> Option<&'static str> {
        match self {
            Self::Fragment => Some("yft"),
            Self::Drawable => Some("ydr"),
            Self::DrawableDictionary => Some("ydd"),
            Self::Uninitialized | Self::Assetless | Self::Unknown(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archetype {
    pub kind: ArchetypeKind,
    pub name: MetaHash,
    pub asset_type: AssetType,
    pub asset_name: Option<MetaHash>,
    pub texture_dictionary: Option<MetaHash>,
    pub physics_dictionary: Option<MetaHash>,
    pub clip_dictionary: Option<MetaHash>,
    pub drawable_dictionary: Option<MetaHash>,
    /// Total embedded entity records in an MLO. Zero for non-MLO archetypes.
    pub mlo_entity_count: usize,
    /// Archetypes referenced by embedded `CEntityDef` records in an MLO.
    /// Empty for non-MLO archetypes. This list is deduplicated by hash.
    pub mlo_entity_archetypes: Vec<MetaHash>,
    /// Number of MLO room records, when exposed by the META descriptor.
    pub mlo_room_count: usize,
    /// Number of MLO portal records, when exposed by the META descriptor.
    pub mlo_portal_count: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ytyp {
    pub name: Option<MetaHash>,
    pub dependencies: Vec<MetaHash>,
    pub archetypes: Vec<Archetype>,
}

impl Ytyp {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ResourceError> {
        let meta = MetaDocument::from_rsc7(bytes)?;
        Self::from_meta(&meta)
    }

    pub fn from_meta(meta: &MetaDocument) -> Result<Self, ResourceError> {
        let root = meta.root_block()?;
        let root_structure = meta.root_structure()?;
        let expected_root = MetaHash(jenkins("CMapTypes"));

        if root.structure_name != expected_root {
            return Err(ResourceError::Malformed(format!(
                "expected CMapTypes root (0x{:08X}), got 0x{:08X}",
                expected_root.0, root.structure_name.0
            )));
        }

        let name = nonzero_hash(read_hash_field(root_structure, &root.data, "name")?);
        let dependencies_offset = optional_field_offset(root_structure, "dependencies");
        let dependencies = if let Some(offset) = dependencies_offset {
            let array = MetaArrayRef::parse(&root.data, offset)?;
            meta.resolve_u32_array(array)?
                .into_iter()
                .filter(|hash| *hash != 0)
                .map(MetaHash)
                .collect()
        } else {
            Vec::new()
        };

        let archetype_array = read_array_field(root_structure, &root.data, "archetypes")?;
        let archetype_pointers = meta.resolve_pointer_array(archetype_array)?;
        let mut archetypes = Vec::with_capacity(archetype_pointers.len());

        for pointer in archetype_pointers {
            if pointer.is_null() {
                continue;
            }

            let block = meta.block_for_pointer(pointer).ok_or_else(|| {
                ResourceError::Malformed(format!(
                    "YTYP archetype pointer references missing META block {}",
                    pointer.block_id()
                ))
            })?;
            let structure = meta.structure(block.structure_name).ok_or_else(|| {
                ResourceError::Malformed(format!(
                    "no META structure info for archetype block type 0x{:08X}",
                    block.structure_name.0
                ))
            })?;
            let bytes = meta.resolve_bytes(pointer, structure.structure_size)?;
            archetypes.push(parse_archetype(meta, structure, bytes)?);
        }

        Ok(Self {
            name,
            dependencies,
            archetypes,
        })
    }
}

fn parse_archetype(
    meta: &MetaDocument,
    structure: &MetaStructureInfo,
    bytes: &[u8],
) -> Result<Archetype, ResourceError> {
    let base_hash = MetaHash(jenkins("CBaseArchetypeDef"));
    let time_hash = MetaHash(jenkins("CTimeArchetypeDef"));
    let mlo_hash = MetaHash(jenkins("CMloArchetypeDef"));

    let kind = if structure.name == base_hash {
        ArchetypeKind::Base
    } else if structure.name == time_hash {
        ArchetypeKind::Time
    } else if structure.name == mlo_hash {
        ArchetypeKind::Mlo
    } else {
        ArchetypeKind::Unknown(structure.name)
    };

    // Derived time/MLO structures repeat the base archetype fields in their
    // META descriptors, so field-name lookup works for all supported kinds.
    let name = MetaHash(read_hash_field(structure, bytes, "name")?);
    if name.0 == 0 {
        return Err(ResourceError::Malformed(format!(
            "archetype 0x{:08X} has a zero name hash",
            structure.name.0
        )));
    }

    let (mlo_entity_count, mlo_entity_archetypes, mlo_room_count, mlo_portal_count) = if kind
        == ArchetypeKind::Mlo
    {
        let (entity_count, entity_archetypes) = read_mlo_entity_archetypes(meta, structure, bytes)?;
        (
            entity_count,
            entity_archetypes,
            read_optional_array_count(structure, bytes, "rooms")?,
            read_optional_array_count(structure, bytes, "portals")?,
        )
    } else {
        (0, Vec::new(), 0, 0)
    };

    Ok(Archetype {
        kind,
        name,
        asset_type: AssetType::from_raw(read_i32_field(structure, bytes, "assetType")?),
        asset_name: optional_hash_field(structure, bytes, "assetName")?,
        texture_dictionary: optional_hash_field(structure, bytes, "textureDictionary")?,
        physics_dictionary: optional_hash_field(structure, bytes, "physicsDictionary")?,
        clip_dictionary: optional_hash_field(structure, bytes, "clipDictionary")?,
        drawable_dictionary: optional_hash_field(structure, bytes, "drawableDictionary")?,
        mlo_entity_count,
        mlo_entity_archetypes,
        mlo_room_count,
        mlo_portal_count,
    })
}

fn read_mlo_entity_archetypes(
    meta: &MetaDocument,
    structure: &MetaStructureInfo,
    bytes: &[u8],
) -> Result<(usize, Vec<MetaHash>), ResourceError> {
    let Some(offset) = optional_field_offset(structure, "entities") else {
        return Ok((0, Vec::new()));
    };
    let array = MetaArrayRef::parse(bytes, offset)?;
    let entity_count = usize::from(array.count1);
    let pointers = meta.resolve_pointer_array(array)?;
    let mut archetypes = Vec::new();

    for pointer in pointers {
        if pointer.is_null() {
            continue;
        }
        let Some(block) = meta.block_for_pointer(pointer) else {
            return Err(ResourceError::Malformed(format!(
                "MLO entity pointer references missing META block {}",
                pointer.block_id()
            )));
        };
        let Some(entity_structure) = meta.structure(block.structure_name) else {
            return Err(ResourceError::Malformed(format!(
                "no META structure info for MLO entity block type 0x{:08X}",
                block.structure_name.0
            )));
        };
        let Some(archetype_offset) = optional_field_offset(entity_structure, "archetypeName")
        else {
            // MLO entity arrays can contain derived entity definitions; if a
            // record does not expose archetypeName it is not useful for asset
            // dependency closure and can be skipped safely.
            continue;
        };
        let entity_bytes = meta.resolve_bytes(pointer, entity_structure.structure_size)?;
        let hash = MetaHash(read_u32(entity_bytes, archetype_offset)?);
        if hash.0 != 0 && !archetypes.contains(&hash) {
            archetypes.push(hash);
        }
    }

    Ok((entity_count, archetypes))
}

fn read_optional_array_count(
    structure: &MetaStructureInfo,
    bytes: &[u8],
    name: &str,
) -> Result<usize, ResourceError> {
    let Some(offset) = optional_field_offset(structure, name) else {
        return Ok(0);
    };
    Ok(usize::from(MetaArrayRef::parse(bytes, offset)?.count1))
}

fn nonzero_hash(value: u32) -> Option<MetaHash> {
    (value != 0).then_some(MetaHash(value))
}

fn field_offset(structure: &MetaStructureInfo, name: &str) -> Result<usize, ResourceError> {
    optional_field_offset(structure, name).ok_or_else(|| {
        let hash = MetaHash(jenkins(name));
        ResourceError::Malformed(format!(
            "structure 0x{:08X} does not define field {name} (0x{:08X})",
            structure.name.0, hash.0
        ))
    })
}

fn optional_field_offset(structure: &MetaStructureInfo, name: &str) -> Option<usize> {
    structure
        .field(MetaHash(jenkins(name)))
        .map(|field| field.data_offset)
}

fn read_hash_field(
    structure: &MetaStructureInfo,
    bytes: &[u8],
    name: &str,
) -> Result<u32, ResourceError> {
    read_u32(bytes, field_offset(structure, name)?)
}

fn optional_hash_field(
    structure: &MetaStructureInfo,
    bytes: &[u8],
    name: &str,
) -> Result<Option<MetaHash>, ResourceError> {
    let Some(offset) = optional_field_offset(structure, name) else {
        return Ok(None);
    };
    Ok(nonzero_hash(read_u32(bytes, offset)?))
}

fn read_i32_field(
    structure: &MetaStructureInfo,
    bytes: &[u8],
    name: &str,
) -> Result<i32, ResourceError> {
    read_i32(bytes, field_offset(structure, name)?)
}

fn read_array_field(
    structure: &MetaStructureInfo,
    bytes: &[u8],
    name: &str,
) -> Result<MetaArrayRef, ResourceError> {
    MetaArrayRef::parse(bytes, field_offset(structure, name)?)
}

#[cfg(test)]
mod tests {
    use super::AssetType;

    #[test]
    fn asset_types_map_to_primary_extensions() {
        assert_eq!(AssetType::Fragment.primary_extension(), Some("yft"));
        assert_eq!(AssetType::Drawable.primary_extension(), Some("ydr"));
        assert_eq!(
            AssetType::DrawableDictionary.primary_extension(),
            Some("ydd")
        );
        assert_eq!(AssetType::Assetless.primary_extension(), None);
    }
}

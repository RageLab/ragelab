//! YTYP (`CMapTypes`) domain model and typed META reader.
//!
//! The current reader intentionally focuses on the archetype fields needed by
//! the map dependency resolver. Geometry, extensions and detailed MLO room/
//! portal records remain future milestones; room/portal counts are exposed.

use ragelab_hash::jenkins;
use ragelab_meta::{
    read_f32, read_i32, read_u32, MetaArrayRef, MetaDocument, MetaHash, MetaStructureInfo,
};
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

#[derive(Debug, Clone, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MloEntity {
    pub index: usize,
    pub archetype_name: MetaHash,
    pub position: Vec3,
    pub rotation: Quat,
    pub scale_xy: Option<f32>,
    pub scale_z: Option<f32>,
    pub flags: u32,
    pub parent_index: Option<i32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MloRoom {
    pub index: usize,
    pub name: String,
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    pub flags: u32,
    pub portal_count: u32,
    pub floor_id: i32,
    pub attached_objects: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MloPortal {
    pub index: usize,
    pub room_from: u32,
    pub room_to: u32,
    pub flags: u32,
    pub mirror_priority: u32,
    pub opacity: u32,
    pub audio_occlusion: u32,
    pub corners: Vec<Vec3>,
    pub attached_objects: Vec<u32>,
}

impl MloPortal {
    pub const fn is_exterior(&self) -> bool {
        self.room_from == 0 || self.room_to == 0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MloEntitySet {
    pub index: usize,
    pub name: MetaHash,
    pub locations: Vec<i32>,
    pub entities: Vec<MloEntity>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MloDef {
    pub archetype_name: MetaHash,
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    pub entities: Vec<MloEntity>,
    pub rooms: Vec<MloRoom>,
    pub portals: Vec<MloPortal>,
    pub entity_sets: Vec<MloEntitySet>,
}

#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ytyp {
    pub name: Option<MetaHash>,
    pub dependencies: Vec<MetaHash>,
    pub archetypes: Vec<Archetype>,
    pub mlos: Vec<MloDef>,
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
        let mut mlos = Vec::new();

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
            let archetype = parse_archetype(meta, structure, bytes)?;
            if archetype.kind == ArchetypeKind::Mlo {
                mlos.push(parse_mlo(meta, structure, bytes, archetype.name)?);
            }
            archetypes.push(archetype);
        }

        Ok(Self {
            name,
            dependencies,
            archetypes,
            mlos,
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

fn parse_mlo(
    meta: &MetaDocument,
    structure: &MetaStructureInfo,
    bytes: &[u8],
    archetype_name: MetaHash,
) -> Result<MloDef, ResourceError> {
    let entities = read_mlo_entities(meta, structure, bytes, "entities")?;
    let rooms = read_mlo_rooms(meta, structure, bytes)?;
    let portals = read_mlo_portals(meta, structure, bytes)?;
    let entity_sets = read_mlo_entity_sets(meta, structure, bytes)?;

    for room in &rooms {
        for entity_index in &room.attached_objects {
            if usize::try_from(*entity_index)
                .ok()
                .map_or(true, |index| index >= entities.len())
            {
                return Err(ResourceError::Malformed(format!(
                    "MLO room {} attached object {} is out of range for {} entities",
                    room.index,
                    entity_index,
                    entities.len()
                )));
            }
        }
    }
    for portal in &portals {
        if usize::try_from(portal.room_from)
            .ok()
            .map_or(true, |index| index > rooms.len())
            || usize::try_from(portal.room_to)
                .ok()
                .map_or(true, |index| index > rooms.len())
        {
            return Err(ResourceError::Malformed(format!(
                "MLO portal {} references rooms {} -> {} but only {} rooms exist",
                portal.index,
                portal.room_from,
                portal.room_to,
                rooms.len()
            )));
        }
        for entity_index in &portal.attached_objects {
            if usize::try_from(*entity_index)
                .ok()
                .map_or(true, |index| index >= entities.len())
            {
                return Err(ResourceError::Malformed(format!(
                    "MLO portal {} attached object {} is out of range for {} entities",
                    portal.index,
                    entity_index,
                    entities.len()
                )));
            }
        }
    }
    for entity_set in &entity_sets {
        if entity_set.locations.len() != entity_set.entities.len() {
            return Err(ResourceError::Malformed(format!(
                "MLO entity set {} has {} room locations for {} entities",
                entity_set.index,
                entity_set.locations.len(),
                entity_set.entities.len()
            )));
        }
    }

    Ok(MloDef {
        archetype_name,
        bounds_min: read_vec3_field(structure, bytes, "bbMin")?,
        bounds_max: read_vec3_field(structure, bytes, "bbMax")?,
        entities,
        rooms,
        portals,
        entity_sets,
    })
}

fn read_mlo_rooms(
    meta: &MetaDocument,
    structure: &MetaStructureInfo,
    bytes: &[u8],
) -> Result<Vec<MloRoom>, ResourceError> {
    let Some((room_structure, room_bytes, count)) =
        resolve_structure_array(meta, structure, bytes, "rooms")?
    else {
        return Ok(Vec::new());
    };
    let mut rooms = Vec::with_capacity(count);
    for (index, record) in room_bytes
        .chunks_exact(room_structure.structure_size)
        .take(count)
        .enumerate()
    {
        rooms.push(MloRoom {
            index,
            name: read_string_field(meta, room_structure, record, "name")?,
            bounds_min: read_vec3_field(room_structure, record, "bbMin")?,
            bounds_max: read_vec3_field(room_structure, record, "bbMax")?,
            flags: read_u32_field(room_structure, record, "flags")?,
            portal_count: read_u32_field(room_structure, record, "portalCount")?,
            floor_id: read_i32_field(room_structure, record, "floorId")?,
            attached_objects: read_u32_array_field(
                meta,
                room_structure,
                record,
                "attachedObjects",
            )?,
        });
    }
    Ok(rooms)
}

fn read_mlo_portals(
    meta: &MetaDocument,
    structure: &MetaStructureInfo,
    bytes: &[u8],
) -> Result<Vec<MloPortal>, ResourceError> {
    let Some((portal_structure, portal_bytes, count)) =
        resolve_structure_array(meta, structure, bytes, "portals")?
    else {
        return Ok(Vec::new());
    };
    let mut portals = Vec::with_capacity(count);
    for (index, record) in portal_bytes
        .chunks_exact(portal_structure.structure_size)
        .take(count)
        .enumerate()
    {
        portals.push(MloPortal {
            index,
            room_from: read_u32_field(portal_structure, record, "roomFrom")?,
            room_to: read_u32_field(portal_structure, record, "roomTo")?,
            flags: read_u32_field(portal_structure, record, "flags")?,
            mirror_priority: read_u32_field(portal_structure, record, "mirrorPriority")?,
            opacity: read_u32_field(portal_structure, record, "opacity")?,
            audio_occlusion: read_u32_field(portal_structure, record, "audioOcclusion")?,
            corners: read_vec3_array_field(meta, portal_structure, record, "corners", 16)?,
            attached_objects: read_u32_array_field(
                meta,
                portal_structure,
                record,
                "attachedObjects",
            )?,
        });
    }
    Ok(portals)
}

fn read_mlo_entity_sets(
    meta: &MetaDocument,
    structure: &MetaStructureInfo,
    bytes: &[u8],
) -> Result<Vec<MloEntitySet>, ResourceError> {
    let Some((set_structure, set_bytes, count)) =
        resolve_structure_array(meta, structure, bytes, "entitySets")?
    else {
        return Ok(Vec::new());
    };
    let mut sets = Vec::with_capacity(count);
    for (index, record) in set_bytes
        .chunks_exact(set_structure.structure_size)
        .take(count)
        .enumerate()
    {
        sets.push(MloEntitySet {
            index,
            name: MetaHash(read_hash_field(set_structure, record, "name")?),
            locations: read_i32_array_field(meta, set_structure, record, "locations")?,
            entities: read_mlo_entities(meta, set_structure, record, "entities")?,
        });
    }
    Ok(sets)
}

fn read_mlo_entities(
    meta: &MetaDocument,
    structure: &MetaStructureInfo,
    bytes: &[u8],
    field_name: &str,
) -> Result<Vec<MloEntity>, ResourceError> {
    let Some(offset) = optional_field_offset(structure, field_name) else {
        return Ok(Vec::new());
    };
    let array = MetaArrayRef::parse(bytes, offset)?;
    let pointers = meta.resolve_pointer_array(array)?;
    let mut entities = Vec::with_capacity(pointers.len());
    for (index, pointer) in pointers.into_iter().enumerate() {
        if pointer.is_null() {
            continue;
        }
        let block = meta.block_for_pointer(pointer).ok_or_else(|| {
            ResourceError::Malformed(format!(
                "MLO entity pointer references missing META block {}",
                pointer.block_id()
            ))
        })?;
        let entity_structure = meta.structure(block.structure_name).ok_or_else(|| {
            ResourceError::Malformed(format!(
                "no META structure info for MLO entity block type 0x{:08X}",
                block.structure_name.0
            ))
        })?;
        let entity_bytes = meta.resolve_bytes(pointer, entity_structure.structure_size)?;
        entities.push(parse_mlo_entity(index, entity_structure, entity_bytes)?);
    }
    Ok(entities)
}

fn parse_mlo_entity(
    index: usize,
    structure: &MetaStructureInfo,
    bytes: &[u8],
) -> Result<MloEntity, ResourceError> {
    let parent_index = read_i32_field(structure, bytes, "parentIndex")?;
    Ok(MloEntity {
        index,
        archetype_name: MetaHash(read_hash_field(structure, bytes, "archetypeName")?),
        position: read_vec3_field(structure, bytes, "position")?,
        rotation: read_quat_field(structure, bytes, "rotation")?,
        scale_xy: read_optional_f32_field(structure, bytes, "scaleXY")?,
        scale_z: read_optional_f32_field(structure, bytes, "scaleZ")?,
        flags: read_u32_field(structure, bytes, "flags")?,
        parent_index: (parent_index >= 0).then_some(parent_index),
    })
}

type ResolvedStructureArray<'a> = (&'a MetaStructureInfo, &'a [u8], usize);

fn resolve_structure_array<'a>(
    meta: &'a MetaDocument,
    structure: &MetaStructureInfo,
    bytes: &[u8],
    field_name: &str,
) -> Result<Option<ResolvedStructureArray<'a>>, ResourceError> {
    let Some(offset) = optional_field_offset(structure, field_name) else {
        return Ok(None);
    };
    let array = MetaArrayRef::parse(bytes, offset)?;
    let count = usize::from(array.count1);
    if count == 0 {
        return Ok(None);
    }
    let block = meta.block_for_pointer(array.pointer).ok_or_else(|| {
        ResourceError::Malformed(format!(
            "MLO {field_name} array references missing META block {}",
            array.pointer.block_id()
        ))
    })?;
    let element_structure = meta.structure(block.structure_name).ok_or_else(|| {
        ResourceError::Malformed(format!(
            "MLO {field_name} block 0x{:08X} has no META structure descriptor",
            block.structure_name.0
        ))
    })?;
    let array_bytes = meta.resolve_array_bytes(array, element_structure.structure_size)?;
    Ok(Some((element_structure, array_bytes, count)))
}

fn read_string_field(
    meta: &MetaDocument,
    structure: &MetaStructureInfo,
    bytes: &[u8],
    field_name: &str,
) -> Result<String, ResourceError> {
    let offset = field_offset(structure, field_name)?;
    let array = MetaArrayRef::parse(bytes, offset)?;
    if array.count1 == 0 {
        return Ok(String::new());
    }
    let raw = meta.resolve_array_bytes(array, 1)?;
    let end = raw.iter().position(|byte| *byte == 0).unwrap_or(raw.len());
    Ok(String::from_utf8_lossy(&raw[..end]).into_owned())
}

fn read_i32_array_field(
    meta: &MetaDocument,
    structure: &MetaStructureInfo,
    bytes: &[u8],
    field: &str,
) -> Result<Vec<i32>, ResourceError> {
    Ok(read_u32_array_field(meta, structure, bytes, field)?
        .into_iter()
        .map(|value| value as i32)
        .collect())
}

fn read_u32_array_field(
    meta: &MetaDocument,
    structure: &MetaStructureInfo,
    bytes: &[u8],
    field_name: &str,
) -> Result<Vec<u32>, ResourceError> {
    let Some(offset) = optional_field_offset(structure, field_name) else {
        return Ok(Vec::new());
    };
    meta.resolve_u32_array(MetaArrayRef::parse(bytes, offset)?)
}

fn read_vec3_array_field(
    meta: &MetaDocument,
    structure: &MetaStructureInfo,
    bytes: &[u8],
    field_name: &str,
    element_size: usize,
) -> Result<Vec<Vec3>, ResourceError> {
    let Some(offset) = optional_field_offset(structure, field_name) else {
        return Ok(Vec::new());
    };
    let array = MetaArrayRef::parse(bytes, offset)?;
    let raw = meta.resolve_array_bytes(array, element_size)?;
    raw.chunks_exact(element_size)
        .map(|record| read_vec3(record, 0))
        .collect()
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

fn read_u32_field(
    structure: &MetaStructureInfo,
    bytes: &[u8],
    name: &str,
) -> Result<u32, ResourceError> {
    read_u32(bytes, field_offset(structure, name)?)
}

fn read_optional_f32_field(
    structure: &MetaStructureInfo,
    bytes: &[u8],
    name: &str,
) -> Result<Option<f32>, ResourceError> {
    let Some(offset) = optional_field_offset(structure, name) else {
        return Ok(None);
    };
    Ok(Some(read_f32(bytes, offset)?))
}

fn read_vec3_field(
    structure: &MetaStructureInfo,
    bytes: &[u8],
    name: &str,
) -> Result<Vec3, ResourceError> {
    read_vec3(bytes, field_offset(structure, name)?)
}

fn read_vec3(bytes: &[u8], offset: usize) -> Result<Vec3, ResourceError> {
    Ok(Vec3 {
        x: read_f32(bytes, offset)?,
        y: read_f32(bytes, offset + 4)?,
        z: read_f32(bytes, offset + 8)?,
    })
}

fn read_quat_field(
    structure: &MetaStructureInfo,
    bytes: &[u8],
    name: &str,
) -> Result<Quat, ResourceError> {
    let offset = field_offset(structure, name)?;
    Ok(Quat {
        x: read_f32(bytes, offset)?,
        y: read_f32(bytes, offset + 4)?,
        z: read_f32(bytes, offset + 8)?,
        w: read_f32(bytes, offset + 12)?,
    })
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

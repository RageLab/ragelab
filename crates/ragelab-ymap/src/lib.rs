//! YMAP domain model and the first typed reader built on top of `ragelab-meta`.
//!
//! This phase intentionally decodes only fields needed by the map-splitting
//! workflow: map identity/parent, entity archetypes and physics dictionaries.

use ragelab_hash::jenkins;
use ragelab_meta::{
    read_f32, read_i32, read_u32, MetaArrayRef, MetaDocument, MetaHash, MetaStructureInfo,
};
use ragelab_resource::ResourceError;

#[derive(Debug, Clone, Copy, PartialEq)]
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
pub struct YmapEntity {
    pub archetype_name: MetaHash,
    pub position: Vec3,
    pub rotation: Quat,
    pub flags: u32,
    /// `None` corresponds to the common on-disk sentinel `-1`.
    pub parent_index: Option<i32>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ymap {
    pub name: Option<MetaHash>,
    pub parent: Option<MetaHash>,
    pub entities: Vec<YmapEntity>,
    pub physics_dictionaries: Vec<MetaHash>,
    pub entities_extents_min: Option<Vec3>,
    pub entities_extents_max: Option<Vec3>,
    pub streaming_extents_min: Option<Vec3>,
    pub streaming_extents_max: Option<Vec3>,
}

impl Ymap {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ResourceError> {
        let meta = MetaDocument::from_rsc7(bytes)?;
        Self::from_meta(&meta)
    }

    pub fn from_meta(meta: &MetaDocument) -> Result<Self, ResourceError> {
        let root = meta.root_block()?;
        let root_structure = meta.root_structure()?;

        let expected_root = MetaHash(jenkins("CMapData"));
        if root.structure_name != expected_root {
            return Err(ResourceError::Malformed(format!(
                "expected CMapData root (0x{:08X}), got 0x{:08X}",
                expected_root.0, root.structure_name.0
            )));
        }

        let name = read_hash_field(root_structure, &root.data, "name")?;
        let parent = read_hash_field(root_structure, &root.data, "parent")?;

        let physics_array = read_array_field(root_structure, &root.data, "physicsDictionaries")?;
        let physics_dictionaries = meta
            .resolve_u32_array(physics_array)?
            .into_iter()
            .filter(|hash| *hash != 0)
            .map(MetaHash)
            .collect();

        let entity_array = read_array_field(root_structure, &root.data, "entities")?;
        let entity_pointers = meta.resolve_pointer_array(entity_array)?;
        let mut entities = Vec::with_capacity(entity_pointers.len());

        for pointer in entity_pointers {
            if pointer.is_null() {
                continue;
            }

            let block = meta.block_for_pointer(pointer).ok_or_else(|| {
                ResourceError::Malformed(format!(
                    "YMAP entity pointer references missing META block {}",
                    pointer.block_id()
                ))
            })?;
            let structure = meta.structure(block.structure_name).ok_or_else(|| {
                ResourceError::Malformed(format!(
                    "no META structure info for entity block type 0x{:08X}",
                    block.structure_name.0
                ))
            })?;
            let entity_size = structure.structure_size;
            let entity_bytes = meta.resolve_bytes(pointer, entity_size)?;
            entities.push(parse_entity(structure, entity_bytes)?);
        }

        Ok(Self {
            name: nonzero_hash(name),
            parent: nonzero_hash(parent),
            entities,
            physics_dictionaries,
            entities_extents_min: read_optional_vec3_field(
                root_structure,
                &root.data,
                "entitiesExtentsMin",
            )?,
            entities_extents_max: read_optional_vec3_field(
                root_structure,
                &root.data,
                "entitiesExtentsMax",
            )?,
            streaming_extents_min: read_optional_vec3_field(
                root_structure,
                &root.data,
                "streamingExtentsMin",
            )?,
            streaming_extents_max: read_optional_vec3_field(
                root_structure,
                &root.data,
                "streamingExtentsMax",
            )?,
        })
    }
}

fn parse_entity(structure: &MetaStructureInfo, bytes: &[u8]) -> Result<YmapEntity, ResourceError> {
    let archetype_name = read_hash_field(structure, bytes, "archetypeName")?;
    let flags = read_u32_field(structure, bytes, "flags")?;
    let parent_index = read_i32_field(structure, bytes, "parentIndex")?;

    let position_offset = field_offset(structure, "position")?;
    let rotation_offset = field_offset(structure, "rotation")?;

    Ok(YmapEntity {
        archetype_name: MetaHash(archetype_name),
        flags,
        parent_index: if parent_index < 0 {
            None
        } else {
            Some(parent_index)
        },
        position: Vec3 {
            x: read_f32(bytes, position_offset)?,
            y: read_f32(bytes, position_offset + 4)?,
            z: read_f32(bytes, position_offset + 8)?,
        },
        rotation: Quat {
            x: read_f32(bytes, rotation_offset)?,
            y: read_f32(bytes, rotation_offset + 4)?,
            z: read_f32(bytes, rotation_offset + 8)?,
            w: read_f32(bytes, rotation_offset + 12)?,
        },
    })
}

fn nonzero_hash(value: u32) -> Option<MetaHash> {
    (value != 0).then_some(MetaHash(value))
}

fn field_offset(structure: &MetaStructureInfo, name: &str) -> Result<usize, ResourceError> {
    let hash = MetaHash(jenkins(name));
    structure
        .field(hash)
        .map(|field| field.data_offset)
        .ok_or_else(|| {
            ResourceError::Malformed(format!(
                "structure 0x{:08X} does not define field {name} (0x{:08X})",
                structure.name.0, hash.0
            ))
        })
}

fn read_hash_field(
    structure: &MetaStructureInfo,
    bytes: &[u8],
    name: &str,
) -> Result<u32, ResourceError> {
    read_u32(bytes, field_offset(structure, name)?)
}

fn read_u32_field(
    structure: &MetaStructureInfo,
    bytes: &[u8],
    name: &str,
) -> Result<u32, ResourceError> {
    read_u32(bytes, field_offset(structure, name)?)
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

fn read_optional_vec3_field(
    structure: &MetaStructureInfo,
    bytes: &[u8],
    name: &str,
) -> Result<Option<Vec3>, ResourceError> {
    let hash = MetaHash(jenkins(name));
    let Some(field) = structure.field(hash) else {
        return Ok(None);
    };
    let offset = field.data_offset;
    Ok(Some(Vec3 {
        x: read_f32(bytes, offset)?,
        y: read_f32(bytes, offset + 4)?,
        z: read_f32(bytes, offset + 8)?,
    }))
}

#[cfg(test)]
mod tests {
    use super::nonzero_hash;

    #[test]
    fn zero_hash_is_none() {
        assert_eq!(nonzero_hash(0), None);
        assert!(nonzero_hash(1).is_some());
    }
}

//! YMAP domain model and the first typed reader built on top of `ragelab-meta`.
//!
//! This phase intentionally decodes only fields needed by the map-splitting
//! workflow: map identity/parent, entity archetypes and physics dictionaries.

use std::collections::{BTreeMap, BTreeSet};

use ragelab_hash::jenkins;
use ragelab_meta::{
    read_f32, read_i32, read_u32, MetaArrayRef, MetaDocument, MetaHash, MetaPointer,
    MetaStructureInfo,
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
    pub scale_xy: Option<f32>,
    pub scale_z: Option<f32>,
    pub flags: u32,
    /// `None` corresponds to the common on-disk sentinel `-1`.
    pub parent_index: Option<i32>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ymap {
    pub name: Option<MetaHash>,
    pub parent: Option<MetaHash>,
    pub flags: Option<u32>,
    pub content_flags: Option<u32>,
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
            flags: read_optional_u32_field(root_structure, &root.data, "flags")?,
            content_flags: read_optional_u32_field(root_structure, &root.data, "contentFlags")?,
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

const YMAP_EDIT_HISTORY_LIMIT: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub enum YmapEditCommand {
    SetTransform {
        index: usize,
        position: Vec3,
        rotation: Quat,
        scale_xy: Option<f32>,
        scale_z: Option<f32>,
    },
    SetProperties {
        index: usize,
        archetype_name: Option<MetaHash>,
        flags: Option<u32>,
        parent_index: Option<Option<i32>>,
    },
    Delete {
        indices: Vec<usize>,
    },
    Duplicate {
        indices: Vec<usize>,
        translation: Vec3,
    },
    CreateFromTemplate {
        template_index: usize,
        archetype_name: MetaHash,
        position: Vec3,
        rotation: Quat,
        scale_xy: Option<f32>,
        scale_z: Option<f32>,
        flags: u32,
        parent_index: Option<i32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YmapEditTransactionResult {
    pub revision: u64,
    pub commands: usize,
    pub entities_before: usize,
    pub entities_after: usize,
    pub undo_depth: usize,
    pub redo_depth: usize,
    pub dirty: bool,
}

#[derive(Debug, Clone)]
pub struct YmapEditSession {
    baseline: Vec<u8>,
    current: Vec<u8>,
    undo: Vec<Vec<u8>>,
    redo: Vec<Vec<u8>>,
    revision: u64,
}

impl YmapEditSession {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ResourceError> {
        Ymap::from_bytes(bytes)?;
        Ok(Self {
            baseline: bytes.to_vec(),
            current: bytes.to_vec(),
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.current
    }

    pub fn document(&self) -> Result<Ymap, ResourceError> {
        Ymap::from_bytes(&self.current)
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn dirty(&self) -> bool {
        self.current != self.baseline
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    pub fn mark_saved(&mut self) {
        self.baseline.clone_from(&self.current);
    }

    pub fn apply_transaction(
        &mut self,
        commands: &[YmapEditCommand],
    ) -> Result<YmapEditTransactionResult, ResourceError> {
        if commands.is_empty() {
            return Err(ResourceError::Malformed(
                "YMAP edit transaction must contain at least one command".into(),
            ));
        }

        let before = Ymap::from_bytes(&self.current)?;
        let previous = self.current.clone();
        let mut working = previous.clone();
        for command in commands {
            working = apply_ymap_edit_command(&working, command)?;
        }
        let after = Ymap::from_bytes(&working)?;

        self.push_undo(previous);
        self.current = working;
        self.redo.clear();
        self.revision = self.revision.saturating_add(1);

        Ok(YmapEditTransactionResult {
            revision: self.revision,
            commands: commands.len(),
            entities_before: before.entities.len(),
            entities_after: after.entities.len(),
            undo_depth: self.undo.len(),
            redo_depth: self.redo.len(),
            dirty: self.dirty(),
        })
    }

    pub fn undo(&mut self) -> Result<YmapEditTransactionResult, ResourceError> {
        let previous = self.undo.pop().ok_or_else(|| {
            ResourceError::Malformed("YMAP edit history has no transaction to undo".into())
        })?;
        let before = Ymap::from_bytes(&self.current)?;
        let current = std::mem::replace(&mut self.current, previous);
        let after = Ymap::from_bytes(&self.current)?;
        self.redo.push(current);
        self.revision = self.revision.saturating_add(1);
        Ok(YmapEditTransactionResult {
            revision: self.revision,
            commands: 0,
            entities_before: before.entities.len(),
            entities_after: after.entities.len(),
            undo_depth: self.undo.len(),
            redo_depth: self.redo.len(),
            dirty: self.dirty(),
        })
    }

    pub fn redo(&mut self) -> Result<YmapEditTransactionResult, ResourceError> {
        let next = self.redo.pop().ok_or_else(|| {
            ResourceError::Malformed("YMAP edit history has no transaction to redo".into())
        })?;
        let before = Ymap::from_bytes(&self.current)?;
        let current = std::mem::replace(&mut self.current, next);
        let after = Ymap::from_bytes(&self.current)?;
        self.push_undo(current);
        self.revision = self.revision.saturating_add(1);
        Ok(YmapEditTransactionResult {
            revision: self.revision,
            commands: 0,
            entities_before: before.entities.len(),
            entities_after: after.entities.len(),
            undo_depth: self.undo.len(),
            redo_depth: self.redo.len(),
            dirty: self.dirty(),
        })
    }

    pub fn revert(&mut self) -> Result<YmapEditTransactionResult, ResourceError> {
        let before = Ymap::from_bytes(&self.current)?;
        let previous = std::mem::replace(&mut self.current, self.baseline.clone());
        let after = Ymap::from_bytes(&self.current)?;
        if previous != self.current {
            self.push_undo(previous);
            self.redo.clear();
            self.revision = self.revision.saturating_add(1);
        }
        Ok(YmapEditTransactionResult {
            revision: self.revision,
            commands: 0,
            entities_before: before.entities.len(),
            entities_after: after.entities.len(),
            undo_depth: self.undo.len(),
            redo_depth: self.redo.len(),
            dirty: self.dirty(),
        })
    }

    fn push_undo(&mut self, bytes: Vec<u8>) {
        if self.undo.len() == YMAP_EDIT_HISTORY_LIMIT {
            self.undo.remove(0);
        }
        self.undo.push(bytes);
    }
}

pub fn apply_ymap_edit_command(
    source: &[u8],
    command: &YmapEditCommand,
) -> Result<Vec<u8>, ResourceError> {
    let meta = MetaDocument::from_rsc7(source)?;
    let layout = YmapEditLayout::from_meta(&meta)?;

    let rewritten = match command {
        YmapEditCommand::SetTransform {
            index,
            position,
            rotation,
            scale_xy,
            scale_z,
        } => {
            validate_vec3(*position, "YMAP entity position")?;
            validate_quat(*rotation)?;
            if let Some(value) = scale_xy {
                validate_f32(*value, "YMAP entity scaleXY")?;
            }
            if let Some(value) = scale_z {
                validate_f32(*value, "YMAP entity scaleZ")?;
            }

            let pointer = layout.entity_pointer(*index)?;
            let structure = layout.entity_structure(pointer)?;
            let mut replacements = BTreeMap::new();
            let block = replacement_block(&mut replacements, &meta, pointer.block_id())?;
            let entity = mutable_entity_bytes(block, pointer, structure.structure_size)?;
            write_vec3_field(structure, entity, "position", *position)?;
            write_quat_field(structure, entity, "rotation", *rotation)?;
            if let Some(value) = scale_xy {
                write_f32_field(structure, entity, "scaleXY", *value)?;
            }
            if let Some(value) = scale_z {
                write_f32_field(structure, entity, "scaleZ", *value)?;
            }
            rewrite_ymap_blocks(source, replacements)?
        }
        YmapEditCommand::SetProperties {
            index,
            archetype_name,
            flags,
            parent_index,
        } => {
            if archetype_name.is_none() && flags.is_none() && parent_index.is_none() {
                return Err(ResourceError::Malformed(
                    "YMAP property edit must change at least one property".into(),
                ));
            }
            if let Some(Some(parent)) = parent_index {
                let parent = usize::try_from(*parent).map_err(|_| {
                    ResourceError::Malformed("YMAP parent index must not be negative".into())
                })?;
                if parent >= layout.entity_pointers.len() || parent == *index {
                    return Err(ResourceError::Malformed(format!(
                        "YMAP parent index {parent} is invalid for entity {index}"
                    )));
                }
            }

            let pointer = layout.entity_pointer(*index)?;
            let structure = layout.entity_structure(pointer)?;
            let mut replacements = BTreeMap::new();
            let block = replacement_block(&mut replacements, &meta, pointer.block_id())?;
            let entity = mutable_entity_bytes(block, pointer, structure.structure_size)?;
            if let Some(value) = archetype_name {
                write_u32_field(structure, entity, "archetypeName", value.0)?;
            }
            if let Some(value) = flags {
                write_u32_field(structure, entity, "flags", *value)?;
            }
            if let Some(value) = parent_index {
                write_i32_field(structure, entity, "parentIndex", value.unwrap_or(-1))?;
            }
            rewrite_ymap_blocks(source, replacements)?
        }
        YmapEditCommand::Delete { indices } => {
            let deleted = normalized_indices(indices, layout.entity_pointers.len(), "delete")?;
            if deleted.is_empty() {
                return Err(ResourceError::Malformed(
                    "YMAP delete requires at least one entity index".into(),
                ));
            }
            delete_entities(source, &meta, &layout, &deleted)?
        }
        YmapEditCommand::Duplicate {
            indices,
            translation,
        } => {
            validate_vec3(*translation, "YMAP duplicate translation")?;
            let selected = normalized_indices(indices, layout.entity_pointers.len(), "duplicate")?;
            if selected.is_empty() {
                return Err(ResourceError::Malformed(
                    "YMAP duplicate requires at least one entity index".into(),
                ));
            }
            duplicate_entities(source, &meta, &layout, &selected, *translation)?
        }
        YmapEditCommand::CreateFromTemplate {
            template_index,
            archetype_name,
            position,
            rotation,
            scale_xy,
            scale_z,
            flags,
            parent_index,
        } => {
            validate_vec3(*position, "YMAP created entity position")?;
            validate_quat(*rotation)?;
            if let Some(value) = scale_xy {
                validate_f32(*value, "YMAP created entity scaleXY")?;
            }
            if let Some(value) = scale_z {
                validate_f32(*value, "YMAP created entity scaleZ")?;
            }
            if let Some(parent) = parent_index {
                let parent = usize::try_from(*parent).map_err(|_| {
                    ResourceError::Malformed("YMAP parent index must not be negative".into())
                })?;
                if parent >= layout.entity_pointers.len() {
                    return Err(ResourceError::Malformed(format!(
                        "YMAP create parent index {parent} is outside {} entities",
                        layout.entity_pointers.len()
                    )));
                }
            }
            create_entity_from_template(
                source,
                &meta,
                &layout,
                *template_index,
                *archetype_name,
                *position,
                *rotation,
                *scale_xy,
                *scale_z,
                *flags,
                *parent_index,
            )?
        }
    };

    Ymap::from_bytes(&rewritten)?;
    Ok(rewritten)
}

struct YmapEditLayout<'a> {
    meta: &'a MetaDocument,
    root_block_id: usize,
    entities_field_offset: usize,
    entity_array: MetaArrayRef,
    entity_pointers: Vec<MetaPointer>,
}

impl<'a> YmapEditLayout<'a> {
    fn from_meta(meta: &'a MetaDocument) -> Result<Self, ResourceError> {
        let root_block_id = usize::try_from(meta.header.root_block_index).map_err(|_| {
            ResourceError::Malformed("YMAP root block index is not positive".into())
        })?;
        let root = meta.root_block()?;
        let root_structure = meta.root_structure()?;
        let entities_field_offset = field_offset(root_structure, "entities")?;
        let entity_array = MetaArrayRef::parse(&root.data, entities_field_offset)?;
        if entity_array.count1 != entity_array.count2 {
            return Err(ResourceError::Malformed(format!(
                "YMAP entity array count/capacity mismatch {}/{} is not supported for authoring",
                entity_array.count1, entity_array.count2
            )));
        }
        let entity_pointers = meta.resolve_pointer_array(entity_array)?;
        if entity_pointers.iter().any(|pointer| pointer.is_null()) {
            return Err(ResourceError::Malformed(
                "YMAP entity array contains null pointers; authoring fails closed".into(),
            ));
        }
        Ok(Self {
            meta,
            root_block_id,
            entities_field_offset,
            entity_array,
            entity_pointers,
        })
    }

    fn entity_pointer(&self, index: usize) -> Result<MetaPointer, ResourceError> {
        self.entity_pointers.get(index).copied().ok_or_else(|| {
            ResourceError::Malformed(format!(
                "YMAP entity index {index} is outside {} entities",
                self.entity_pointers.len()
            ))
        })
    }

    fn entity_structure(
        &self,
        pointer: MetaPointer,
    ) -> Result<&'a MetaStructureInfo, ResourceError> {
        let block = self.meta.block_for_pointer(pointer).ok_or_else(|| {
            ResourceError::Malformed(format!(
                "YMAP entity pointer references missing block {}",
                pointer.block_id()
            ))
        })?;
        self.meta.structure(block.structure_name).ok_or_else(|| {
            ResourceError::Malformed(format!(
                "YMAP entity block type 0x{:08X} has no structure descriptor",
                block.structure_name.0
            ))
        })
    }

    fn entity_bytes(&self, pointer: MetaPointer) -> Result<&'a [u8], ResourceError> {
        let structure = self.entity_structure(pointer)?;
        self.meta.resolve_bytes(pointer, structure.structure_size)
    }
}

fn delete_entities(
    source: &[u8],
    meta: &MetaDocument,
    layout: &YmapEditLayout<'_>,
    deleted: &BTreeSet<usize>,
) -> Result<Vec<u8>, ResourceError> {
    let mut old_to_new = BTreeMap::new();
    let mut next_index = 0_usize;
    for old_index in 0..layout.entity_pointers.len() {
        if !deleted.contains(&old_index) {
            old_to_new.insert(old_index, next_index);
            next_index += 1;
        }
    }

    let mut replacements = BTreeMap::new();
    for &old_index in old_to_new.keys() {
        let pointer = layout.entity_pointer(old_index)?;
        let structure = layout.entity_structure(pointer)?;
        let current = parse_entity(structure, layout.entity_bytes(pointer)?)?;
        let Some(parent) = current.parent_index else {
            continue;
        };
        let parent = usize::try_from(parent).map_err(|_| {
            ResourceError::Malformed("YMAP entity parent index unexpectedly negative".into())
        })?;
        if deleted.contains(&parent) {
            return Err(ResourceError::Malformed(format!(
                "cannot delete YMAP entity {parent}; surviving entity {old_index} references it as parent"
            )));
        }
        let mapped_parent = *old_to_new.get(&parent).ok_or_else(|| {
            ResourceError::Malformed(format!(
                "YMAP entity {old_index} references invalid parent index {parent}"
            ))
        })?;
        if mapped_parent != parent {
            let block = replacement_block(&mut replacements, meta, pointer.block_id())?;
            let entity = mutable_entity_bytes(block, pointer, structure.structure_size)?;
            write_i32_field(
                structure,
                entity,
                "parentIndex",
                i32::try_from(mapped_parent).map_err(|_| {
                    ResourceError::Malformed("YMAP remapped parent index exceeds i32".into())
                })?,
            )?;
        }
    }

    let pointers = layout
        .entity_pointers
        .iter()
        .enumerate()
        .filter_map(|(index, pointer)| (!deleted.contains(&index)).then_some(*pointer))
        .collect::<Vec<_>>();
    rewrite_entity_pointer_array(source, meta, layout, replacements, &pointers)
}

fn duplicate_entities(
    source: &[u8],
    meta: &MetaDocument,
    layout: &YmapEditLayout<'_>,
    selected: &BTreeSet<usize>,
    translation: Vec3,
) -> Result<Vec<u8>, ResourceError> {
    let original_count = layout.entity_pointers.len();
    let selected_order = selected.iter().copied().collect::<Vec<_>>();
    let duplicate_indices = selected_order
        .iter()
        .enumerate()
        .map(|(ordinal, source_index)| (*source_index, original_count + ordinal))
        .collect::<BTreeMap<_, _>>();

    let mut replacements = BTreeMap::new();
    let mut duplicated_pointers = Vec::with_capacity(selected_order.len());

    for source_index in selected_order {
        let pointer = layout.entity_pointer(source_index)?;
        let structure = layout.entity_structure(pointer)?;
        let mut entity = layout.entity_bytes(pointer)?.to_vec();
        let current = parse_entity(structure, &entity)?;
        let translated = Vec3 {
            x: finite_add(current.position.x, translation.x, "duplicate position x")?,
            y: finite_add(current.position.y, translation.y, "duplicate position y")?,
            z: finite_add(current.position.z, translation.z, "duplicate position z")?,
        };
        write_vec3_field(structure, &mut entity, "position", translated)?;
        if let Some(parent) = current.parent_index {
            let parent = usize::try_from(parent).map_err(|_| {
                ResourceError::Malformed("YMAP entity parent index unexpectedly negative".into())
            })?;
            if let Some(mapped_parent) = duplicate_indices.get(&parent) {
                write_i32_field(
                    structure,
                    &mut entity,
                    "parentIndex",
                    i32::try_from(*mapped_parent).map_err(|_| {
                        ResourceError::Malformed("YMAP duplicated parent index exceeds i32".into())
                    })?,
                )?;
            }
        }

        let block = replacement_block(&mut replacements, meta, pointer.block_id())?;
        let offset = append_aligned(block, 16, &entity)?;
        duplicated_pointers.push(MetaPointer::from_block_offset(pointer.block_id(), offset)?);
    }

    let mut pointers = layout.entity_pointers.clone();
    pointers.extend(duplicated_pointers);
    rewrite_entity_pointer_array(source, meta, layout, replacements, &pointers)
}

#[allow(clippy::too_many_arguments)]
fn create_entity_from_template(
    source: &[u8],
    meta: &MetaDocument,
    layout: &YmapEditLayout<'_>,
    template_index: usize,
    archetype_name: MetaHash,
    position: Vec3,
    rotation: Quat,
    scale_xy: Option<f32>,
    scale_z: Option<f32>,
    flags: u32,
    parent_index: Option<i32>,
) -> Result<Vec<u8>, ResourceError> {
    let pointer = layout.entity_pointer(template_index)?;
    let structure = layout.entity_structure(pointer)?;
    let mut entity = layout.entity_bytes(pointer)?.to_vec();
    write_u32_field(structure, &mut entity, "archetypeName", archetype_name.0)?;
    write_u32_field(structure, &mut entity, "flags", flags)?;
    write_vec3_field(structure, &mut entity, "position", position)?;
    write_quat_field(structure, &mut entity, "rotation", rotation)?;
    if let Some(value) = scale_xy {
        write_f32_field(structure, &mut entity, "scaleXY", value)?;
    }
    if let Some(value) = scale_z {
        write_f32_field(structure, &mut entity, "scaleZ", value)?;
    }
    write_i32_field(
        structure,
        &mut entity,
        "parentIndex",
        parent_index.unwrap_or(-1),
    )?;

    let mut replacements = BTreeMap::new();
    let block = replacement_block(&mut replacements, meta, pointer.block_id())?;
    let offset = append_aligned(block, 16, &entity)?;
    let created = MetaPointer::from_block_offset(pointer.block_id(), offset)?;

    let mut pointers = layout.entity_pointers.clone();
    pointers.push(created);
    rewrite_entity_pointer_array(source, meta, layout, replacements, &pointers)
}

fn rewrite_entity_pointer_array(
    source: &[u8],
    meta: &MetaDocument,
    layout: &YmapEditLayout<'_>,
    mut replacements: BTreeMap<usize, Vec<u8>>,
    pointers: &[MetaPointer],
) -> Result<Vec<u8>, ResourceError> {
    if pointers.len() > u16::MAX as usize {
        return Err(ResourceError::Malformed(format!(
            "YMAP entity count {} exceeds u16 META array capacity",
            pointers.len()
        )));
    }
    let pointer_block_id = layout.entity_array.pointer.block_id();
    if pointer_block_id == 0 {
        return Err(ResourceError::Malformed(
            "YMAP entity array has a null pointer block; resizing is unsupported".into(),
        ));
    }

    let mut pointer_bytes = Vec::with_capacity(pointers.len() * 8);
    for pointer in pointers {
        pointer_bytes.extend_from_slice(&pointer.raw.to_le_bytes());
    }
    let pointer_block = replacement_block(&mut replacements, meta, pointer_block_id)?;
    let pointer_offset = append_aligned(pointer_block, 8, &pointer_bytes)?;
    let array_pointer = MetaPointer::from_block_offset(pointer_block_id, pointer_offset)?;

    let root = replacement_block(&mut replacements, meta, layout.root_block_id)?;
    write_meta_array_ref(
        root,
        layout.entities_field_offset,
        array_pointer,
        pointers.len() as u16,
        layout.entity_array.unknown,
    )?;

    rewrite_ymap_blocks(source, replacements)
}

fn rewrite_ymap_blocks(
    source: &[u8],
    replacements: BTreeMap<usize, Vec<u8>>,
) -> Result<Vec<u8>, ResourceError> {
    let replacements = replacements.into_iter().collect::<Vec<_>>();
    let rewritten = MetaDocument::rewrite_blocks(source, &replacements)?;
    Ymap::from_bytes(&rewritten)?;
    Ok(rewritten)
}

fn replacement_block<'a>(
    replacements: &'a mut BTreeMap<usize, Vec<u8>>,
    meta: &MetaDocument,
    block_id: usize,
) -> Result<&'a mut Vec<u8>, ResourceError> {
    if let std::collections::btree_map::Entry::Vacant(entry) = replacements.entry(block_id) {
        let block = meta.block_by_id(block_id).ok_or_else(|| {
            ResourceError::Malformed(format!("META block {block_id} does not exist"))
        })?;
        entry.insert(block.data.clone());
    }
    Ok(replacements
        .get_mut(&block_id)
        .expect("replacement inserted above"))
}

fn mutable_entity_bytes(
    block: &mut [u8],
    pointer: MetaPointer,
    structure_size: usize,
) -> Result<&mut [u8], ResourceError> {
    let start = pointer.offset();
    let end = start
        .checked_add(structure_size)
        .ok_or_else(|| ResourceError::Malformed("YMAP entity byte range overflows usize".into()))?;
    let block_len = block.len();
    block.get_mut(start..end).ok_or(ResourceError::OutOfBounds {
        segment: "META entity block",
        offset: start,
        length: structure_size,
        segment_length: block_len,
    })
}

fn normalized_indices(
    indices: &[usize],
    entity_count: usize,
    operation: &str,
) -> Result<BTreeSet<usize>, ResourceError> {
    let set = indices.iter().copied().collect::<BTreeSet<_>>();
    if let Some(index) = set.iter().find(|index| **index >= entity_count) {
        return Err(ResourceError::Malformed(format!(
            "YMAP {operation} entity index {index} is outside {entity_count} entities"
        )));
    }
    Ok(set)
}

fn append_aligned(
    block: &mut Vec<u8>,
    alignment: usize,
    data: &[u8],
) -> Result<usize, ResourceError> {
    debug_assert!(alignment.is_power_of_two());
    let mask = alignment - 1;
    let offset = block
        .len()
        .checked_add(mask)
        .map(|value| value & !mask)
        .ok_or_else(|| ResourceError::Malformed("YMAP block alignment overflows usize".into()))?;
    if offset > block.len() {
        block.resize(offset, 0);
    }
    let end = offset
        .checked_add(data.len())
        .ok_or_else(|| ResourceError::Malformed("YMAP block append overflows usize".into()))?;
    block.resize(end, 0);
    block[offset..end].copy_from_slice(data);
    Ok(offset)
}

fn write_meta_array_ref(
    bytes: &mut [u8],
    offset: usize,
    pointer: MetaPointer,
    count: u16,
    unknown: u32,
) -> Result<(), ResourceError> {
    write_bytes(bytes, offset, &pointer.raw.to_le_bytes())?;
    write_bytes(bytes, offset + 8, &count.to_le_bytes())?;
    write_bytes(bytes, offset + 10, &count.to_le_bytes())?;
    write_bytes(bytes, offset + 12, &unknown.to_le_bytes())
}

fn write_vec3_field(
    structure: &MetaStructureInfo,
    bytes: &mut [u8],
    name: &str,
    value: Vec3,
) -> Result<(), ResourceError> {
    let offset = field_offset(structure, name)?;
    write_f32_at(bytes, offset, value.x)?;
    write_f32_at(bytes, offset + 4, value.y)?;
    write_f32_at(bytes, offset + 8, value.z)
}

fn write_quat_field(
    structure: &MetaStructureInfo,
    bytes: &mut [u8],
    name: &str,
    value: Quat,
) -> Result<(), ResourceError> {
    let offset = field_offset(structure, name)?;
    write_f32_at(bytes, offset, value.x)?;
    write_f32_at(bytes, offset + 4, value.y)?;
    write_f32_at(bytes, offset + 8, value.z)?;
    write_f32_at(bytes, offset + 12, value.w)
}

fn write_u32_field(
    structure: &MetaStructureInfo,
    bytes: &mut [u8],
    name: &str,
    value: u32,
) -> Result<(), ResourceError> {
    write_bytes(bytes, field_offset(structure, name)?, &value.to_le_bytes())
}

fn write_i32_field(
    structure: &MetaStructureInfo,
    bytes: &mut [u8],
    name: &str,
    value: i32,
) -> Result<(), ResourceError> {
    write_bytes(bytes, field_offset(structure, name)?, &value.to_le_bytes())
}

fn write_f32_field(
    structure: &MetaStructureInfo,
    bytes: &mut [u8],
    name: &str,
    value: f32,
) -> Result<(), ResourceError> {
    write_f32_at(bytes, field_offset(structure, name)?, value)
}

fn write_f32_at(bytes: &mut [u8], offset: usize, value: f32) -> Result<(), ResourceError> {
    write_bytes(bytes, offset, &value.to_le_bytes())
}

fn write_bytes(bytes: &mut [u8], offset: usize, value: &[u8]) -> Result<(), ResourceError> {
    let end = offset
        .checked_add(value.len())
        .ok_or_else(|| ResourceError::Malformed("YMAP field write range overflows usize".into()))?;
    let length = bytes.len();
    let destination = bytes
        .get_mut(offset..end)
        .ok_or(ResourceError::OutOfBounds {
            segment: "YMAP structure",
            offset,
            length: value.len(),
            segment_length: length,
        })?;
    destination.copy_from_slice(value);
    Ok(())
}

fn validate_vec3(value: Vec3, label: &str) -> Result<(), ResourceError> {
    validate_f32(value.x, label)?;
    validate_f32(value.y, label)?;
    validate_f32(value.z, label)
}

fn validate_quat(value: Quat) -> Result<(), ResourceError> {
    validate_f32(value.x, "YMAP entity rotation")?;
    validate_f32(value.y, "YMAP entity rotation")?;
    validate_f32(value.z, "YMAP entity rotation")?;
    validate_f32(value.w, "YMAP entity rotation")?;
    let length_squared =
        value.x * value.x + value.y * value.y + value.z * value.z + value.w * value.w;
    if !length_squared.is_finite() || length_squared <= 1.0e-12 {
        return Err(ResourceError::Malformed(
            "YMAP entity rotation quaternion must be non-zero".into(),
        ));
    }
    Ok(())
}

fn validate_f32(value: f32, label: &str) -> Result<(), ResourceError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(ResourceError::Malformed(format!("{label} must be finite")))
    }
}

fn finite_add(left: f32, right: f32, label: &str) -> Result<f32, ResourceError> {
    let value = left + right;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(ResourceError::Malformed(format!("{label} overflowed f32")))
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
        scale_xy: read_optional_f32_field(structure, bytes, "scaleXY")?,
        scale_z: read_optional_f32_field(structure, bytes, "scaleZ")?,
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

fn read_optional_u32_field(
    structure: &MetaStructureInfo,
    bytes: &[u8],
    name: &str,
) -> Result<Option<u32>, ResourceError> {
    let hash = MetaHash(jenkins(name));
    let Some(field) = structure.field(hash) else {
        return Ok(None);
    };
    Ok(Some(read_u32(bytes, field.data_offset)?))
}

fn read_optional_f32_field(
    structure: &MetaStructureInfo,
    bytes: &[u8],
    name: &str,
) -> Result<Option<f32>, ResourceError> {
    let hash = MetaHash(jenkins(name));
    let Some(field) = structure.field(hash) else {
        return Ok(None);
    };
    Ok(Some(read_f32(bytes, field.data_offset)?))
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
    use ragelab_hash::jenkins;
    use ragelab_meta::{MetaDocument, MetaHash};

    use super::{nonzero_hash, Quat, Vec3, Ymap, YmapEditCommand, YmapEditSession};

    const SIMPLE_YMAP: &[u8] = include_bytes!("../../../fixtures/synthetic/simple.ymap");

    #[test]
    fn zero_hash_is_none() {
        assert_eq!(nonzero_hash(0), None);
        assert!(nonzero_hash(1).is_some());
    }

    #[test]
    fn synthetic_entity_exposes_legacy_scale_fields() {
        let ymap = Ymap::from_bytes(SIMPLE_YMAP).expect("synthetic YMAP should parse");
        let entity = &ymap.entities[0];
        assert_eq!(entity.scale_xy, Some(1.25));
        assert_eq!(entity.scale_z, Some(0.75));
    }

    #[test]
    fn edit_session_preserves_untouched_blocks_and_undo_redo_bytes() {
        let original_meta = MetaDocument::from_rsc7(SIMPLE_YMAP).unwrap();
        let mut session = YmapEditSession::from_bytes(SIMPLE_YMAP).unwrap();

        let transaction = session
            .apply_transaction(&[
                YmapEditCommand::SetTransform {
                    index: 0,
                    position: Vec3 {
                        x: 10.0,
                        y: 20.0,
                        z: 30.0,
                    },
                    rotation: Quat {
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                        w: 1.0,
                    },
                    scale_xy: Some(1.5),
                    scale_z: Some(2.0),
                },
                YmapEditCommand::SetProperties {
                    index: 0,
                    archetype_name: Some(MetaHash(jenkins("edited_archetype"))),
                    flags: Some(0x1234_5678),
                    parent_index: None,
                },
            ])
            .unwrap();
        assert_eq!(transaction.commands, 2);
        assert!(transaction.dirty);
        assert_eq!(session.undo_depth(), 1);
        assert_eq!(session.redo_depth(), 0);

        let edited = session.document().unwrap();
        assert_eq!(edited.entities.len(), 1);
        assert_eq!(
            edited.entities[0].archetype_name,
            MetaHash(jenkins("edited_archetype"))
        );
        assert_eq!(edited.entities[0].flags, 0x1234_5678);
        assert_eq!(
            edited.entities[0].position,
            Vec3 {
                x: 10.0,
                y: 20.0,
                z: 30.0
            }
        );
        assert_eq!(edited.entities[0].scale_xy, Some(1.5));
        assert_eq!(edited.entities[0].scale_z, Some(2.0));

        let edited_bytes = session.bytes().to_vec();
        let edited_meta = MetaDocument::from_rsc7(&edited_bytes).unwrap();
        assert_eq!(edited_meta.structures, original_meta.structures);
        assert_eq!(edited_meta.enums, original_meta.enums);
        assert_eq!(edited_meta.blocks.len(), original_meta.blocks.len());
        assert_eq!(edited_meta.blocks[0].data, original_meta.blocks[0].data);
        assert_eq!(edited_meta.blocks[1].data, original_meta.blocks[1].data);
        assert_ne!(edited_meta.blocks[2].data, original_meta.blocks[2].data);
        assert_eq!(edited_meta.blocks[3].data, original_meta.blocks[3].data);

        session.undo().unwrap();
        assert_eq!(session.bytes(), SIMPLE_YMAP);
        assert!(!session.dirty());
        assert_eq!(session.redo_depth(), 1);

        session.redo().unwrap();
        assert_eq!(session.bytes(), edited_bytes.as_slice());
        assert!(session.dirty());
    }

    #[test]
    fn duplicate_create_delete_and_parent_guards_semantically_reopen() {
        let mut session = YmapEditSession::from_bytes(SIMPLE_YMAP).unwrap();

        session
            .apply_transaction(&[YmapEditCommand::Duplicate {
                indices: vec![0],
                translation: Vec3 {
                    x: 5.0,
                    y: -2.0,
                    z: 1.0,
                },
            }])
            .unwrap();
        let duplicated = session.document().unwrap();
        assert_eq!(duplicated.entities.len(), 2);
        assert_eq!(
            duplicated.entities[1].position,
            Vec3 {
                x: 6.0,
                y: 0.0,
                z: 4.0
            }
        );

        session
            .apply_transaction(&[YmapEditCommand::SetProperties {
                index: 1,
                archetype_name: None,
                flags: None,
                parent_index: Some(Some(0)),
            }])
            .unwrap();
        let before_rejected_delete = session.bytes().to_vec();
        assert!(session
            .apply_transaction(&[YmapEditCommand::Delete { indices: vec![0] }])
            .is_err());
        assert_eq!(session.bytes(), before_rejected_delete.as_slice());

        session
            .apply_transaction(&[YmapEditCommand::Delete { indices: vec![1] }])
            .unwrap();
        assert_eq!(session.document().unwrap().entities.len(), 1);

        session
            .apply_transaction(&[YmapEditCommand::CreateFromTemplate {
                template_index: 0,
                archetype_name: MetaHash(jenkins("placed_prop")),
                position: Vec3 {
                    x: -4.0,
                    y: 8.0,
                    z: 2.5,
                },
                rotation: Quat {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0,
                },
                scale_xy: Some(1.0),
                scale_z: Some(1.0),
                flags: 7,
                parent_index: None,
            }])
            .unwrap();
        let created = session.document().unwrap();
        assert_eq!(created.entities.len(), 2);
        assert_eq!(
            created.entities[1].archetype_name,
            MetaHash(jenkins("placed_prop"))
        );
        assert_eq!(created.entities[1].flags, 7);
        assert_eq!(created.entities[1].parent_index, None);
        assert_eq!(
            created.entities[1].position,
            Vec3 {
                x: -4.0,
                y: 8.0,
                z: 2.5
            }
        );

        let reopened = Ymap::from_bytes(session.bytes()).unwrap();
        assert_eq!(reopened, created);
        let meta = MetaDocument::from_rsc7(session.bytes()).unwrap();
        assert_eq!(meta.blocks.len(), 4);
        assert_eq!(
            meta.blocks[3].data,
            MetaDocument::from_rsc7(SIMPLE_YMAP).unwrap().blocks[3].data
        );
    }
}

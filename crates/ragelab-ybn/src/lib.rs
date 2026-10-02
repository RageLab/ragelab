//! GTA V PC YBN collision preview reader.
//!
//! This crate models a renderer-neutral GTA V PC resource-version-43 subset:
//! primitive bounds, `BoundGeometry`/`BoundGeometryBVH` polygons, and the
//! common `BoundComposite` child layout used by streamed world collision. Raw
//! resource pointers remain private.

use std::{collections::BTreeMap, error::Error, fmt};

use ragelab_resource::{ResourceError, Rsc7Resource, SYSTEM_BASE};

const SUPPORTED_VERSION: u32 = 43;
const BOUNDS_SIZE: usize = 0x70;
const EXTENDED_PRIMITIVE_SIZE: usize = 0x80;
const COMPOSITE_SIZE: usize = 0xB0;
const GEOMETRY_SIZE: usize = 0x130;
const GEOMETRY_BVH_SIZE: usize = 0x150;
const MATRIX_SIZE: usize = 64;
const VERTEX_SIZE: usize = 6;
const POLYGON_SIZE: usize = 16;
const MATERIAL_SIZE: usize = 8;

const MAX_CHILDREN: usize = 16_384;
const MAX_VERTICES: usize = 2_000_000;
const MAX_POLYGONS: usize = 4_000_000;
const MAX_TOTAL_VERTICES: usize = 4_000_000;
const MAX_TOTAL_INDICES: usize = 24_000_000;
const MAX_MATERIALS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundsType {
    Sphere,
    Capsule,
    Box,
    Geometry,
    GeometryBvh,
    Composite,
    Disc,
    Cylinder,
    Cloth,
    Other(u8),
}

impl BoundsType {
    pub fn from_raw(raw: u8) -> Self {
        match raw {
            0 => Self::Sphere,
            1 => Self::Capsule,
            3 => Self::Box,
            4 => Self::Geometry,
            8 => Self::GeometryBvh,
            10 => Self::Composite,
            12 => Self::Disc,
            13 => Self::Cylinder,
            15 => Self::Cloth,
            other => Self::Other(other),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sphere => "sphere",
            Self::Capsule => "capsule",
            Self::Box => "box",
            Self::Geometry => "geometry",
            Self::GeometryBvh => "geometryBvh",
            Self::Composite => "composite",
            Self::Disc => "disc",
            Self::Cylinder => "cylinder",
            Self::Cloth => "cloth",
            Self::Other(_) => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CollisionBounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub center: [f32; 3],
    pub sphere_center: [f32; 3],
    pub sphere_radius: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollisionMaterial {
    pub index: usize,
    pub child_index: usize,
    pub local_index: u8,
    pub material_type: u8,
    pub procedural_id: u8,
    pub flags: u16,
    pub color_index: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollisionPrimitive {
    pub child_index: usize,
    pub material_index: usize,
    pub indices: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CollisionShape {
    Sphere {
        center: [f32; 3],
        radius: f32,
    },
    Capsule {
        start: [f32; 3],
        end: [f32; 3],
        radius: f32,
    },
    Box {
        corner: [f32; 3],
        edges: [[f32; 3]; 3],
    },
    Cylinder {
        start: [f32; 3],
        end: [f32; 3],
        radius: f32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct CollisionShapePrimitive {
    pub child_index: usize,
    pub material_index: usize,
    pub polygon_index: Option<usize>,
    pub shape: CollisionShape,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CollisionChild {
    pub index: usize,
    pub bounds_type: BoundsType,
    pub bounds: CollisionBounds,
    pub vertices: usize,
    pub triangles: usize,
    pub shape_primitives: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct YbnCollision {
    pub coordinate_convention: &'static str,
    pub bounds: CollisionBounds,
    pub positions: Vec<[f32; 3]>,
    pub primitives: Vec<CollisionPrimitive>,
    pub shape_primitives: Vec<CollisionShapePrimitive>,
    pub materials: Vec<CollisionMaterial>,
    pub children: Vec<CollisionChild>,
}

impl YbnCollision {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, YbnError> {
        let resource = Rsc7Resource::parse(bytes)?;
        if resource.header.version != SUPPORTED_VERSION {
            return Err(YbnError::Unsupported(format!(
                "YBN resource version {} is unsupported; expected GTA V PC version {SUPPORTED_VERSION}",
                resource.header.version
            )));
        }
        parse_collision(&resource, SYSTEM_BASE)
    }

    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn index_count(&self) -> usize {
        self.primitives
            .iter()
            .map(|primitive| primitive.indices.len())
            .sum()
    }

    pub fn triangle_count(&self) -> usize {
        self.index_count() / 3
    }

    pub fn shape_primitive_count(&self) -> usize {
        self.shape_primitives.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum YbnPolygonKind {
    Sphere,
    Capsule,
    Box,
    Cylinder,
}

impl YbnPolygonKind {
    fn from_raw(raw: u8) -> Option<Self> {
        match raw {
            1 => Some(Self::Sphere),
            2 => Some(Self::Capsule),
            3 => Some(Self::Box),
            4 => Some(Self::Cylinder),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Sphere => "sphere",
            Self::Capsule => "capsule",
            Self::Box => "box",
            Self::Cylinder => "cylinder",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct YbnPolygonEdit {
    pub child_index: usize,
    pub polygon_index: usize,
    pub expected_kind: YbnPolygonKind,
    /// Desired radius in the same normalized/source XYZ space returned by [`YbnCollision`].
    /// Only sphere/capsule/cylinder polygons expose a radius field.
    pub radius: Option<f32>,
    /// Existing material slot local to the geometry child.
    pub local_material: Option<u8>,
}

#[derive(Debug)]
pub enum YbnError {
    Resource(ResourceError),
    Malformed(String),
    Unsupported(String),
}

impl fmt::Display for YbnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resource(error) => write!(f, "{error}"),
            Self::Malformed(message) => write!(f, "malformed YBN: {message}"),
            Self::Unsupported(message) => write!(f, "unsupported YBN feature: {message}"),
        }
    }
}

impl Error for YbnError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Resource(error) => Some(error),
            Self::Malformed(_) | Self::Unsupported(_) => None,
        }
    }
}

impl From<ResourceError> for YbnError {
    fn from(error: ResourceError) -> Self {
        Self::Resource(error)
    }
}

#[derive(Debug, Clone, Copy)]
struct Transform {
    c1: [f32; 3],
    c2: [f32; 3],
    c3: [f32; 3],
    c4: [f32; 3],
}

impl Transform {
    const IDENTITY: Self = Self {
        c1: [1.0, 0.0, 0.0],
        c2: [0.0, 1.0, 0.0],
        c3: [0.0, 0.0, 1.0],
        c4: [0.0, 0.0, 0.0],
    };

    fn apply(self, value: [f32; 3]) -> [f32; 3] {
        [
            value[0] * self.c1[0] + value[1] * self.c2[0] + value[2] * self.c3[0] + self.c4[0],
            value[0] * self.c1[1] + value[1] * self.c2[1] + value[2] * self.c3[1] + self.c4[1],
            value[0] * self.c1[2] + value[1] * self.c2[2] + value[2] * self.c3[2] + self.c4[2],
        ]
    }

    fn apply_vector(self, value: [f32; 3]) -> [f32; 3] {
        [
            value[0] * self.c1[0] + value[1] * self.c2[0] + value[2] * self.c3[0],
            value[0] * self.c1[1] + value[1] * self.c2[1] + value[2] * self.c3[1],
            value[0] * self.c1[2] + value[1] * self.c2[2] + value[2] * self.c3[2],
        ]
    }

    fn uniform_radial_scale(self) -> Option<f32> {
        let lengths = [length(self.c1), length(self.c2), length(self.c3)];
        if lengths
            .iter()
            .any(|value| !value.is_finite() || *value <= f32::EPSILON)
        {
            return None;
        }
        let scale = (lengths[0] + lengths[1] + lengths[2]) / 3.0;
        let scale_tolerance = scale.max(1.0) * 1.0e-4;
        if lengths
            .iter()
            .any(|value| (*value - scale).abs() > scale_tolerance)
        {
            return None;
        }
        let dot_tolerance = scale * scale * 1.0e-4;
        if dot(self.c1, self.c2).abs() > dot_tolerance
            || dot(self.c1, self.c3).abs() > dot_tolerance
            || dot(self.c2, self.c3).abs() > dot_tolerance
        {
            return None;
        }
        Some(scale)
    }
}

#[derive(Debug, Clone, Copy)]
struct GeometryEditLocation {
    bounds_type: BoundsType,
    transform: Transform,
    polygons_pointer: u64,
    polygons_count: usize,
    polygon_materials_pointer: u64,
    materials_count: usize,
}

#[derive(Debug)]
struct ParseOutput {
    positions: Vec<[f32; 3]>,
    primitives: Vec<CollisionPrimitive>,
    shape_primitives: Vec<CollisionShapePrimitive>,
    materials: Vec<CollisionMaterial>,
    children: Vec<CollisionChild>,
}

/// Applies a conservative layout-preserving edit to existing geometry polygon records.
///
/// This writer never reallocates resource pages, changes vertex topology, edits primitive
/// bounds, or changes polygon kinds. It only patches radius fields that already exist and
/// existing same-child polygon-material slots, then reparses the generated YBN and rejects
/// the result if any untargeted normalized collision data changed.
pub fn repack_polygon_edits(bytes: &[u8], edits: &[YbnPolygonEdit]) -> Result<Vec<u8>, YbnError> {
    if edits.is_empty() {
        return Ok(bytes.to_vec());
    }

    let mut resource = Rsc7Resource::parse(bytes)?;
    if resource.header.version != SUPPORTED_VERSION {
        return Err(YbnError::Unsupported(format!(
            "YBN resource version {} is unsupported; expected GTA V PC version {SUPPORTED_VERSION}",
            resource.header.version
        )));
    }
    let original = parse_collision(&resource, SYSTEM_BASE)?;
    let locations = collect_geometry_edit_locations(&resource, SYSTEM_BASE)?;
    let mut seen = BTreeMap::<(usize, usize), ()>::new();

    for edit in edits {
        if seen
            .insert((edit.child_index, edit.polygon_index), ())
            .is_some()
        {
            return Err(YbnError::Malformed(format!(
                "duplicate polygon edit for child {} polygon {}",
                edit.child_index, edit.polygon_index
            )));
        }

        let location = locations.get(&edit.child_index).ok_or_else(|| {
            YbnError::Malformed(format!(
                "child {} is not editable geometry",
                edit.child_index
            ))
        })?;
        if edit.polygon_index >= location.polygons_count {
            return Err(YbnError::Malformed(format!(
                "child {} polygon {} is out of range for {} polygons",
                edit.child_index, edit.polygon_index, location.polygons_count
            )));
        }

        let polygon_pointer = indexed(
            location.polygons_pointer,
            edit.polygon_index,
            POLYGON_SIZE,
            "polygon edit",
        )?;
        let polygon_type = resource.read_u8(polygon_pointer)? & 7;
        let actual_kind = YbnPolygonKind::from_raw(polygon_type).ok_or_else(|| {
            YbnError::Unsupported(format!(
                "child {} polygon {} type {polygon_type} is not an editable shape polygon",
                edit.child_index, edit.polygon_index
            ))
        })?;
        if actual_kind != edit.expected_kind {
            return Err(YbnError::Malformed(format!(
                "child {} polygon {} is {}, but edit expected {}",
                edit.child_index,
                edit.polygon_index,
                actual_kind.as_str(),
                edit.expected_kind.as_str()
            )));
        }

        if let Some(radius) = edit.radius {
            if actual_kind == YbnPolygonKind::Box {
                return Err(YbnError::Unsupported(format!(
                    "child {} polygon {} box geometry does not expose a radius field",
                    edit.child_index, edit.polygon_index
                )));
            }
            if !radius.is_finite() || radius <= 0.0 {
                return Err(YbnError::Malformed(format!(
                    "child {} polygon {} radius must be finite and greater than zero",
                    edit.child_index, edit.polygon_index
                )));
            }
            let scale = radial_scale(location.transform, edit.child_index, location.bounds_type)?;
            let raw_radius = radius / scale;
            if !raw_radius.is_finite() || raw_radius <= 0.0 {
                return Err(YbnError::Malformed(format!(
                    "child {} polygon {} inverse-scaled radius is invalid",
                    edit.child_index, edit.polygon_index
                )));
            }
            let radius_pointer = polygon_pointer
                .checked_add(4)
                .ok_or_else(|| YbnError::Malformed("polygon radius pointer overflow".into()))?;
            resource
                .bytes_at_mut(radius_pointer, 4)?
                .copy_from_slice(&raw_radius.to_le_bytes());
        }

        if let Some(local_material) = edit.local_material {
            let effective_material_count = location.materials_count.max(1);
            if usize::from(local_material) >= effective_material_count {
                return Err(YbnError::Malformed(format!(
                    "child {} polygon {} material {} is out of range for {} materials",
                    edit.child_index, edit.polygon_index, local_material, effective_material_count
                )));
            }
            if location.polygon_materials_pointer == 0 {
                if local_material != 0 {
                    return Err(YbnError::Unsupported(format!(
                        "child {} has no polygon-material array; only implicit material 0 is writable",
                        edit.child_index
                    )));
                }
            } else {
                let material_pointer = location
                    .polygon_materials_pointer
                    .checked_add(edit.polygon_index as u64)
                    .ok_or_else(|| {
                        YbnError::Malformed("polygon material pointer overflow".into())
                    })?;
                resource.bytes_at_mut(material_pointer, 1)?[0] = local_material;
            }
        }
    }

    let output = resource.to_bytes()?;
    let edited = YbnCollision::from_bytes(&output)?;
    validate_edit_round_trip(&original, &edited, edits)?;
    Ok(output)
}

fn collect_geometry_edit_locations(
    resource: &Rsc7Resource,
    root: u64,
) -> Result<BTreeMap<usize, GeometryEditLocation>, YbnError> {
    let root_type = BoundsType::from_raw(resource.read_u8(root + 0x10)?);
    let mut locations = BTreeMap::new();
    match root_type {
        BoundsType::Geometry | BoundsType::GeometryBvh => {
            locations.insert(
                0,
                geometry_edit_location(resource, root, 0, Transform::IDENTITY)?,
            );
        }
        BoundsType::Composite => {
            resource.bytes_at(root, COMPOSITE_SIZE)?;
            let children_pointer = resource.read_u64(root + 0x70)?;
            let transform1_pointer = resource.read_u64(root + 0x78)?;
            let transform2_pointer = resource.read_u64(root + 0x80)?;
            let count1 = usize::from(resource.read_u16(root + 0xa0)?);
            let count2 = usize::from(resource.read_u16(root + 0xa2)?);
            if count1 != count2 {
                return Err(YbnError::Malformed(format!(
                    "composite child counts disagree: {count1} vs {count2}"
                )));
            }
            if count1 > MAX_CHILDREN {
                return Err(YbnError::Malformed(format!(
                    "composite child count {count1} exceeds safety limit {MAX_CHILDREN}"
                )));
            }
            if count1 > 0 && children_pointer == 0 {
                return Err(YbnError::Malformed(
                    "composite has children but a null child pointer array".into(),
                ));
            }
            let transform_pointer = if transform1_pointer != 0 {
                transform1_pointer
            } else {
                transform2_pointer
            };
            for child_index in 0..count1 {
                let child_address = indexed(children_pointer, child_index, 8, "child pointer")?;
                let child_pointer = resource.read_u64(child_address)?;
                if child_pointer == 0 {
                    return Err(YbnError::Malformed(format!(
                        "composite child {child_index} has a null pointer"
                    )));
                }
                let transform = if transform_pointer == 0 {
                    Transform::IDENTITY
                } else {
                    read_transform(
                        resource,
                        indexed(
                            transform_pointer,
                            child_index,
                            MATRIX_SIZE,
                            "child transform",
                        )?,
                    )?
                };
                let child_type = BoundsType::from_raw(resource.read_u8(child_pointer + 0x10)?);
                if matches!(child_type, BoundsType::Geometry | BoundsType::GeometryBvh) {
                    locations.insert(
                        child_index,
                        geometry_edit_location(resource, child_pointer, child_index, transform)?,
                    );
                }
            }
        }
        BoundsType::Sphere | BoundsType::Capsule | BoundsType::Box | BoundsType::Cylinder => {}
        other => {
            return Err(YbnError::Unsupported(format!(
                "root bounds type {} is not supported by the polygon writer",
                other.as_str()
            )))
        }
    }
    Ok(locations)
}

fn geometry_edit_location(
    resource: &Rsc7Resource,
    pointer: u64,
    child_index: usize,
    transform: Transform,
) -> Result<GeometryEditLocation, YbnError> {
    let bounds_type = BoundsType::from_raw(resource.read_u8(pointer + 0x10)?);
    let block_size = match bounds_type {
        BoundsType::Geometry => GEOMETRY_SIZE,
        BoundsType::GeometryBvh => GEOMETRY_BVH_SIZE,
        other => {
            return Err(YbnError::Malformed(format!(
                "child {child_index} is {}, not editable geometry",
                other.as_str()
            )))
        }
    };
    resource.bytes_at(pointer, block_size)?;
    let polygons_pointer = resource.read_u64(pointer + 0x88)?;
    let polygons_count = usize::try_from(resource.read_u32(pointer + 0xd4)?)
        .map_err(|_| YbnError::Malformed("polygon count does not fit usize".into()))?;
    let polygon_materials_pointer = resource.read_u64(pointer + 0x118)?;
    let materials_count = usize::from(resource.read_u8(pointer + 0x120)?);
    validate_count("polygon", polygons_count, MAX_POLYGONS)?;
    validate_count("material", materials_count, MAX_MATERIALS)?;
    if polygons_count > 0 && polygons_pointer == 0 {
        return Err(YbnError::Malformed(format!(
            "child {child_index} has polygons but a null polygon pointer"
        )));
    }
    if polygons_count > 0 {
        resource.bytes_at(
            polygons_pointer,
            checked_mul(polygons_count, POLYGON_SIZE, "polygon byte length")?,
        )?;
        if polygon_materials_pointer != 0 {
            resource.bytes_at(polygon_materials_pointer, polygons_count)?;
        }
    }
    Ok(GeometryEditLocation {
        bounds_type,
        transform,
        polygons_pointer,
        polygons_count,
        polygon_materials_pointer,
        materials_count,
    })
}

fn validate_edit_round_trip(
    original: &YbnCollision,
    edited: &YbnCollision,
    edits: &[YbnPolygonEdit],
) -> Result<(), YbnError> {
    let mut expected = original.clone();
    for edit in edits {
        let expected_index = expected
            .shape_primitives
            .iter()
            .position(|primitive| {
                primitive.child_index == edit.child_index
                    && primitive.polygon_index == Some(edit.polygon_index)
            })
            .ok_or_else(|| {
                YbnError::Malformed(format!(
                    "child {} polygon {} is not a normalized shape primitive",
                    edit.child_index, edit.polygon_index
                ))
            })?;
        let edited_primitive = edited
            .shape_primitives
            .iter()
            .find(|primitive| {
                primitive.child_index == edit.child_index
                    && primitive.polygon_index == Some(edit.polygon_index)
            })
            .ok_or_else(|| {
                YbnError::Malformed(format!(
                    "edited child {} polygon {} disappeared after round-trip",
                    edit.child_index, edit.polygon_index
                ))
            })?;
        if shape_kind(&edited_primitive.shape) != edit.expected_kind {
            return Err(YbnError::Malformed(format!(
                "edited child {} polygon {} changed polygon kind",
                edit.child_index, edit.polygon_index
            )));
        }

        if let Some(radius) = edit.radius {
            let actual_radius = shape_radius(&edited_primitive.shape).ok_or_else(|| {
                YbnError::Malformed(format!(
                    "edited child {} polygon {} lost its radius field",
                    edit.child_index, edit.polygon_index
                ))
            })?;
            let tolerance = radius.abs().max(1.0) * 1.0e-5;
            if (actual_radius - radius).abs() > tolerance {
                return Err(YbnError::Malformed(format!(
                    "edited child {} polygon {} radius round-trip mismatch: requested {radius}, got {actual_radius}",
                    edit.child_index, edit.polygon_index
                )));
            }
            set_shape_radius(
                &mut expected.shape_primitives[expected_index].shape,
                actual_radius,
            )?;
        }

        if let Some(local_material) = edit.local_material {
            let material_index = expected
                .materials
                .iter()
                .position(|material| {
                    material.child_index == edit.child_index
                        && material.local_index == local_material
                })
                .ok_or_else(|| {
                    YbnError::Malformed(format!(
                        "child {} local material {} is missing after round-trip",
                        edit.child_index, local_material
                    ))
                })?;
            expected.shape_primitives[expected_index].material_index = material_index;
        }
    }

    if &expected != edited {
        return Err(YbnError::Malformed(
            "YBN semantic round-trip changed untargeted collision data".into(),
        ));
    }
    Ok(())
}

fn shape_kind(shape: &CollisionShape) -> YbnPolygonKind {
    match shape {
        CollisionShape::Sphere { .. } => YbnPolygonKind::Sphere,
        CollisionShape::Capsule { .. } => YbnPolygonKind::Capsule,
        CollisionShape::Box { .. } => YbnPolygonKind::Box,
        CollisionShape::Cylinder { .. } => YbnPolygonKind::Cylinder,
    }
}

fn shape_radius(shape: &CollisionShape) -> Option<f32> {
    match shape {
        CollisionShape::Sphere { radius, .. }
        | CollisionShape::Capsule { radius, .. }
        | CollisionShape::Cylinder { radius, .. } => Some(*radius),
        CollisionShape::Box { .. } => None,
    }
}

fn set_shape_radius(shape: &mut CollisionShape, radius: f32) -> Result<(), YbnError> {
    match shape {
        CollisionShape::Sphere { radius: value, .. }
        | CollisionShape::Capsule { radius: value, .. }
        | CollisionShape::Cylinder { radius: value, .. } => {
            *value = radius;
            Ok(())
        }
        CollisionShape::Box { .. } => Err(YbnError::Malformed(
            "cannot assign radius to box primitive".into(),
        )),
    }
}

fn parse_collision(resource: &Rsc7Resource, root: u64) -> Result<YbnCollision, YbnError> {
    let bounds = read_bounds(resource, root)?;
    let root_type = BoundsType::from_raw(resource.read_u8(root + 0x10)?);
    let mut output = ParseOutput {
        positions: Vec::new(),
        primitives: Vec::new(),
        shape_primitives: Vec::new(),
        materials: Vec::new(),
        children: Vec::new(),
    };

    match root_type {
        BoundsType::Composite => parse_composite(resource, root, &mut output)?,
        BoundsType::Geometry | BoundsType::GeometryBvh => {
            parse_geometry(resource, root, 0, Transform::IDENTITY, &mut output)?;
        }
        BoundsType::Sphere | BoundsType::Capsule | BoundsType::Box | BoundsType::Cylinder => {
            parse_primitive_bound(resource, root, 0, Transform::IDENTITY, &mut output)?;
        }
        other => {
            return Err(YbnError::Unsupported(format!(
                "root bounds type {} is not supported by the collision preview",
                other.as_str()
            )))
        }
    }

    if output.positions.len() > MAX_TOTAL_VERTICES {
        return Err(YbnError::Malformed(format!(
            "flattened collision has {} vertices, above safety limit {MAX_TOTAL_VERTICES}",
            output.positions.len()
        )));
    }
    let total_indices: usize = output
        .primitives
        .iter()
        .map(|primitive| primitive.indices.len())
        .sum();
    if total_indices > MAX_TOTAL_INDICES {
        return Err(YbnError::Malformed(format!(
            "flattened collision has {total_indices} indices, above safety limit {MAX_TOTAL_INDICES}"
        )));
    }

    Ok(YbnCollision {
        coordinate_convention: "sourceXyzZUp",
        bounds,
        positions: output.positions,
        primitives: output.primitives,
        shape_primitives: output.shape_primitives,
        materials: output.materials,
        children: output.children,
    })
}

fn parse_composite(
    resource: &Rsc7Resource,
    root: u64,
    output: &mut ParseOutput,
) -> Result<(), YbnError> {
    resource.bytes_at(root, COMPOSITE_SIZE)?;
    let children_pointer = resource.read_u64(root + 0x70)?;
    let transform1_pointer = resource.read_u64(root + 0x78)?;
    let transform2_pointer = resource.read_u64(root + 0x80)?;
    let count1 = usize::from(resource.read_u16(root + 0xa0)?);
    let count2 = usize::from(resource.read_u16(root + 0xa2)?);
    if count1 != count2 {
        return Err(YbnError::Malformed(format!(
            "composite child counts disagree: {count1} vs {count2}"
        )));
    }
    if count1 > MAX_CHILDREN {
        return Err(YbnError::Malformed(format!(
            "composite child count {count1} exceeds safety limit {MAX_CHILDREN}"
        )));
    }
    if count1 == 0 {
        return Ok(());
    }
    if children_pointer == 0 {
        return Err(YbnError::Malformed(
            "composite has children but a null child pointer array".into(),
        ));
    }
    resource.bytes_at(
        children_pointer,
        checked_mul(count1, 8, "child pointer array")?,
    )?;

    let transform_pointer = if transform1_pointer != 0 {
        transform1_pointer
    } else {
        transform2_pointer
    };
    if transform_pointer != 0 {
        resource.bytes_at(
            transform_pointer,
            checked_mul(count1, MATRIX_SIZE, "child transform array")?,
        )?;
    }

    for child_index in 0..count1 {
        let child_address = indexed(children_pointer, child_index, 8, "child pointer")?;
        let child_pointer = resource.read_u64(child_address)?;
        if child_pointer == 0 {
            return Err(YbnError::Malformed(format!(
                "composite child {child_index} has a null pointer"
            )));
        }
        let transform = if transform_pointer == 0 {
            Transform::IDENTITY
        } else {
            read_transform(
                resource,
                indexed(
                    transform_pointer,
                    child_index,
                    MATRIX_SIZE,
                    "child transform",
                )?,
            )?
        };
        let child_type = BoundsType::from_raw(resource.read_u8(child_pointer + 0x10)?);
        match child_type {
            BoundsType::Geometry | BoundsType::GeometryBvh => {
                parse_geometry(resource, child_pointer, child_index, transform, output)?;
            }
            BoundsType::Sphere | BoundsType::Capsule | BoundsType::Box | BoundsType::Cylinder => {
                parse_primitive_bound(resource, child_pointer, child_index, transform, output)?;
            }
            other => {
                return Err(YbnError::Unsupported(format!(
                    "composite child {child_index} uses unsupported bounds type {}",
                    other.as_str()
                )))
            }
        }
    }
    Ok(())
}

fn parse_primitive_bound(
    resource: &Rsc7Resource,
    pointer: u64,
    child_index: usize,
    transform: Transform,
    output: &mut ParseOutput,
) -> Result<(), YbnError> {
    let bounds_type = BoundsType::from_raw(resource.read_u8(pointer + 0x10)?);
    let block_size = match bounds_type {
        BoundsType::Sphere | BoundsType::Box => BOUNDS_SIZE,
        BoundsType::Capsule | BoundsType::Cylinder => EXTENDED_PRIMITIVE_SIZE,
        other => {
            return Err(YbnError::Unsupported(format!(
                "child {child_index} is {}, not a supported primitive bound",
                other.as_str()
            )))
        }
    };
    resource.bytes_at(pointer, block_size)?;
    let bounds = read_bounds(resource, pointer)?;
    let material_index = append_bound_material(resource, pointer, child_index, output)?;

    let shape = match bounds_type {
        BoundsType::Sphere => {
            let scale = radial_scale(transform, child_index, bounds_type)?;
            CollisionShape::Sphere {
                center: transform.apply(bounds.sphere_center),
                radius: checked_radius(bounds.sphere_radius * scale, "sphere radius")?,
            }
        }
        BoundsType::Capsule => {
            let radius = read_f32(resource, pointer + 0x2c)?;
            checked_radius(radius, "capsule margin/radius")?;
            if bounds.sphere_radius < radius {
                return Err(YbnError::Malformed(format!(
                    "child {child_index} capsule sphere radius {} is smaller than margin/radius {radius}",
                    bounds.sphere_radius
                )));
            }
            let extent = bounds.sphere_radius - radius;
            let start = [
                bounds.sphere_center[0],
                bounds.sphere_center[1] - extent,
                bounds.sphere_center[2],
            ];
            let end = [
                bounds.sphere_center[0],
                bounds.sphere_center[1] + extent,
                bounds.sphere_center[2],
            ];
            let scale = radial_scale(transform, child_index, bounds_type)?;
            CollisionShape::Capsule {
                start: transform.apply(start),
                end: transform.apply(end),
                radius: checked_radius(radius * scale, "transformed capsule radius")?,
            }
        }
        BoundsType::Box => {
            let extent = sub(bounds.max, bounds.min);
            CollisionShape::Box {
                corner: transform.apply(bounds.min),
                edges: [
                    transform.apply_vector([extent[0], 0.0, 0.0]),
                    transform.apply_vector([0.0, extent[1], 0.0]),
                    transform.apply_vector([0.0, 0.0, extent[2]]),
                ],
            }
        }
        BoundsType::Cylinder => {
            let extent = sub(bounds.max, bounds.min);
            let half_length = extent[1].abs() * 0.5;
            let radius = extent[0].abs() * 0.5;
            let start = [
                bounds.sphere_center[0],
                bounds.sphere_center[1] - half_length,
                bounds.sphere_center[2],
            ];
            let end = [
                bounds.sphere_center[0],
                bounds.sphere_center[1] + half_length,
                bounds.sphere_center[2],
            ];
            let scale = radial_scale(transform, child_index, bounds_type)?;
            CollisionShape::Cylinder {
                start: transform.apply(start),
                end: transform.apply(end),
                radius: checked_radius(radius * scale, "transformed cylinder radius")?,
            }
        }
        _ => unreachable!("primitive bound type matched above"),
    };
    validate_shape(&shape)?;
    output.shape_primitives.push(CollisionShapePrimitive {
        child_index,
        material_index,
        polygon_index: None,
        shape,
    });
    output.children.push(CollisionChild {
        index: child_index,
        bounds_type,
        bounds,
        vertices: 0,
        triangles: 0,
        shape_primitives: 1,
    });
    Ok(())
}

fn append_bound_material(
    resource: &Rsc7Resource,
    pointer: u64,
    child_index: usize,
    output: &mut ParseOutput,
) -> Result<usize, YbnError> {
    let index = output.materials.len();
    output.materials.push(CollisionMaterial {
        index,
        child_index,
        local_index: 0,
        material_type: resource.read_u8(pointer + 0x4c)?,
        procedural_id: resource.read_u8(pointer + 0x4d)?,
        flags: 0,
        color_index: resource.read_u8(pointer + 0x5d)?,
    });
    Ok(index)
}

fn parse_geometry(
    resource: &Rsc7Resource,
    pointer: u64,
    child_index: usize,
    transform: Transform,
    output: &mut ParseOutput,
) -> Result<(), YbnError> {
    let bounds_type = BoundsType::from_raw(resource.read_u8(pointer + 0x10)?);
    let block_size = match bounds_type {
        BoundsType::Geometry => GEOMETRY_SIZE,
        BoundsType::GeometryBvh => GEOMETRY_BVH_SIZE,
        other => {
            return Err(YbnError::Unsupported(format!(
                "child {child_index} is {}, not geometry",
                other.as_str()
            )))
        }
    };
    resource.bytes_at(pointer, block_size)?;
    let child_bounds = read_bounds(resource, pointer)?;

    let polygons_pointer = resource.read_u64(pointer + 0x88)?;
    let quantum = read_vec3(resource, pointer + 0x90)?;
    let center = read_vec3(resource, pointer + 0xa0)?;
    let vertices_pointer = resource.read_u64(pointer + 0xb0)?;
    let vertices_count = usize::try_from(resource.read_u32(pointer + 0xd0)?)
        .map_err(|_| YbnError::Malformed("vertex count does not fit usize".into()))?;
    let polygons_count = usize::try_from(resource.read_u32(pointer + 0xd4)?)
        .map_err(|_| YbnError::Malformed("polygon count does not fit usize".into()))?;
    let materials_pointer = resource.read_u64(pointer + 0xf0)?;
    let polygon_materials_pointer = resource.read_u64(pointer + 0x118)?;
    let materials_count = usize::from(resource.read_u8(pointer + 0x120)?);

    validate_count("vertex", vertices_count, MAX_VERTICES)?;
    validate_count("polygon", polygons_count, MAX_POLYGONS)?;
    validate_count("material", materials_count, MAX_MATERIALS)?;
    validate_vec3("quantum", quantum)?;
    validate_vec3("geometry center", center)?;

    if vertices_count > 0 && vertices_pointer == 0 {
        return Err(YbnError::Malformed(format!(
            "child {child_index} has vertices but a null vertex pointer"
        )));
    }
    if polygons_count > 0 && polygons_pointer == 0 {
        return Err(YbnError::Malformed(format!(
            "child {child_index} has polygons but a null polygon pointer"
        )));
    }

    let vertex_bytes = checked_mul(vertices_count, VERTEX_SIZE, "vertex byte length")?;
    let vertices = if vertices_count == 0 {
        &[][..]
    } else {
        resource.bytes_at(vertices_pointer, vertex_bytes)?
    };
    let base_vertex = output.positions.len();
    output
        .positions
        .try_reserve(vertices_count)
        .map_err(|_| YbnError::Malformed("vertex allocation failed".into()))?;
    for vertex in vertices.chunks_exact(VERTEX_SIZE) {
        let x = i16::from_le_bytes([vertex[0], vertex[1]]) as f32 * quantum[0] + center[0];
        let y = i16::from_le_bytes([vertex[2], vertex[3]]) as f32 * quantum[1] + center[1];
        let z = i16::from_le_bytes([vertex[4], vertex[5]]) as f32 * quantum[2] + center[2];
        let position = transform.apply([x, y, z]);
        validate_vec3("transformed vertex", position)?;
        output.positions.push(position);
    }

    let material_base = output.materials.len();
    if materials_count == 0 {
        output.materials.push(CollisionMaterial {
            index: material_base,
            child_index,
            local_index: 0,
            material_type: resource.read_u8(pointer + 0x4c)?,
            procedural_id: resource.read_u8(pointer + 0x4d)?,
            flags: 0,
            color_index: resource.read_u8(pointer + 0x5d)?,
        });
    } else {
        if materials_pointer == 0 {
            return Err(YbnError::Malformed(format!(
                "child {child_index} declares {materials_count} materials but has a null material pointer"
            )));
        }
        let bytes = resource.bytes_at(
            materials_pointer,
            checked_mul(
                materials_count.max(4),
                MATERIAL_SIZE,
                "material byte length",
            )?,
        )?;
        for local_index in 0..materials_count {
            let offset = local_index * MATERIAL_SIZE;
            let data1 = u32::from_le_bytes(
                bytes[offset..offset + 4]
                    .try_into()
                    .expect("material dword"),
            );
            let data2 = u32::from_le_bytes(
                bytes[offset + 4..offset + 8]
                    .try_into()
                    .expect("material dword"),
            );
            output.materials.push(CollisionMaterial {
                index: material_base + local_index,
                child_index,
                local_index: local_index as u8,
                material_type: (data1 & 0xff) as u8,
                procedural_id: ((data1 >> 8) & 0xff) as u8,
                flags: (((data1 >> 24) & 0xff) | ((data2 & 0xff) << 8)) as u16,
                color_index: ((data2 >> 8) & 0xff) as u8,
            });
        }
    }
    let effective_material_count = materials_count.max(1);

    let polygon_bytes = checked_mul(polygons_count, POLYGON_SIZE, "polygon byte length")?;
    let polygons = if polygons_count == 0 {
        &[][..]
    } else {
        resource.bytes_at(polygons_pointer, polygon_bytes)?
    };
    let polygon_materials = if polygons_count == 0 || polygon_materials_pointer == 0 {
        None
    } else {
        Some(resource.bytes_at(polygon_materials_pointer, polygons_count)?)
    };

    let mut grouped = BTreeMap::<u8, Vec<u32>>::new();
    let shape_base = output.shape_primitives.len();
    let mut triangle_polygons = 0_usize;
    for polygon_index in 0..polygons_count {
        let offset = polygon_index * POLYGON_SIZE;
        let polygon = &polygons[offset..offset + POLYGON_SIZE];
        let polygon_type = polygon[0] & 7;
        let local_material = polygon_materials
            .map(|bytes| bytes[polygon_index])
            .unwrap_or(0);
        if usize::from(local_material) >= effective_material_count {
            return Err(YbnError::Malformed(format!(
                "child {child_index} polygon {polygon_index} references material {local_material}, but child has {effective_material_count} materials"
            )));
        }
        let material_index = material_base + usize::from(local_material);

        match polygon_type {
            0 => {
                let raw_indices = [
                    u16::from_le_bytes([polygon[4], polygon[5]]),
                    u16::from_le_bytes([polygon[6], polygon[7]]),
                    u16::from_le_bytes([polygon[8], polygon[9]]),
                ];
                let mut indices = [0_u32; 3];
                for (slot, raw) in raw_indices.into_iter().enumerate() {
                    let local = usize::from(raw & 0x7fff);
                    let global = checked_global_vertex(
                        base_vertex,
                        vertices_count,
                        local,
                        child_index,
                        polygon_index,
                    )?;
                    indices[slot] = u32::try_from(global).map_err(|_| {
                        YbnError::Malformed("global vertex index exceeds u32".into())
                    })?;
                }
                grouped.entry(local_material).or_default().extend(indices);
                triangle_polygons += 1;
            }
            1 => {
                let local = usize::from(u16::from_le_bytes([polygon[2], polygon[3]]));
                let center = geometry_position(
                    &output.positions,
                    base_vertex,
                    vertices_count,
                    local,
                    child_index,
                    polygon_index,
                )?;
                let radius = f32::from_le_bytes(polygon[4..8].try_into().expect("polygon radius"));
                let scale = radial_scale(transform, child_index, bounds_type)?;
                let shape = CollisionShape::Sphere {
                    center,
                    radius: checked_radius(radius * scale, "polygon sphere radius")?,
                };
                validate_shape(&shape)?;
                output.shape_primitives.push(CollisionShapePrimitive {
                    child_index,
                    material_index,
                    polygon_index: Some(polygon_index),
                    shape,
                });
            }
            2 | 4 => {
                let local1 = usize::from(u16::from_le_bytes([polygon[2], polygon[3]]));
                let local2 = usize::from(u16::from_le_bytes([polygon[8], polygon[9]]));
                let start = geometry_position(
                    &output.positions,
                    base_vertex,
                    vertices_count,
                    local1,
                    child_index,
                    polygon_index,
                )?;
                let end = geometry_position(
                    &output.positions,
                    base_vertex,
                    vertices_count,
                    local2,
                    child_index,
                    polygon_index,
                )?;
                let radius = f32::from_le_bytes(polygon[4..8].try_into().expect("polygon radius"));
                let scale = radial_scale(transform, child_index, bounds_type)?;
                let radius = checked_radius(radius * scale, "polygon axial radius")?;
                let shape = if polygon_type == 2 {
                    CollisionShape::Capsule { start, end, radius }
                } else {
                    CollisionShape::Cylinder { start, end, radius }
                };
                validate_shape(&shape)?;
                output.shape_primitives.push(CollisionShapePrimitive {
                    child_index,
                    material_index,
                    polygon_index: Some(polygon_index),
                    shape,
                });
            }
            3 => {
                let mut points = [[0.0_f32; 3]; 4];
                for (slot, offset) in [4_usize, 6, 8, 10].into_iter().enumerate() {
                    let raw = i16::from_le_bytes([polygon[offset], polygon[offset + 1]]);
                    if raw < 0 {
                        return Err(YbnError::Malformed(format!(
                            "child {child_index} polygon {polygon_index} box references negative vertex {raw}"
                        )));
                    }
                    points[slot] = geometry_position(
                        &output.positions,
                        base_vertex,
                        vertices_count,
                        raw as usize,
                        child_index,
                        polygon_index,
                    )?;
                }
                let axial = mul(
                    sub(add(points[2], points[3]), add(points[0], points[1])),
                    0.5,
                );
                let shape = CollisionShape::Box {
                    corner: points[0],
                    edges: [
                        axial,
                        sub(sub(points[2], axial), points[0]),
                        sub(sub(points[3], axial), points[0]),
                    ],
                };
                validate_shape(&shape)?;
                output.shape_primitives.push(CollisionShapePrimitive {
                    child_index,
                    material_index,
                    polygon_index: Some(polygon_index),
                    shape,
                });
            }
            other => {
                return Err(YbnError::Unsupported(format!(
                    "child {child_index} polygon {polygon_index} uses unknown polygon type {other}"
                )));
            }
        }
    }

    for (local_material, indices) in grouped {
        output.primitives.push(CollisionPrimitive {
            child_index,
            material_index: material_base + usize::from(local_material),
            indices,
        });
    }
    output.children.push(CollisionChild {
        index: child_index,
        bounds_type,
        bounds: child_bounds,
        vertices: vertices_count,
        triangles: triangle_polygons,
        shape_primitives: output.shape_primitives.len() - shape_base,
    });
    Ok(())
}

fn read_bounds(resource: &Rsc7Resource, pointer: u64) -> Result<CollisionBounds, YbnError> {
    resource.bytes_at(pointer, BOUNDS_SIZE)?;
    let bounds = CollisionBounds {
        min: read_vec3(resource, pointer + 0x30)?,
        max: read_vec3(resource, pointer + 0x20)?,
        center: read_vec3(resource, pointer + 0x40)?,
        sphere_center: read_vec3(resource, pointer + 0x50)?,
        sphere_radius: read_f32(resource, pointer + 0x14)?,
    };
    validate_vec3("bounds min", bounds.min)?;
    validate_vec3("bounds max", bounds.max)?;
    validate_vec3("bounds center", bounds.center)?;
    validate_vec3("bounds sphere center", bounds.sphere_center)?;
    if !bounds.sphere_radius.is_finite() || bounds.sphere_radius < 0.0 {
        return Err(YbnError::Malformed(format!(
            "invalid bounds sphere radius {}",
            bounds.sphere_radius
        )));
    }
    for axis in 0..3 {
        if bounds.min[axis] > bounds.max[axis] {
            return Err(YbnError::Malformed(format!(
                "bounds min exceeds max on axis {axis}"
            )));
        }
    }
    Ok(bounds)
}

fn read_transform(resource: &Rsc7Resource, pointer: u64) -> Result<Transform, YbnError> {
    resource.bytes_at(pointer, MATRIX_SIZE)?;
    let transform = Transform {
        c1: read_vec3(resource, pointer)?,
        c2: read_vec3(resource, pointer + 0x10)?,
        c3: read_vec3(resource, pointer + 0x20)?,
        c4: read_vec3(resource, pointer + 0x30)?,
    };
    validate_vec3("transform basis 1", transform.c1)?;
    validate_vec3("transform basis 2", transform.c2)?;
    validate_vec3("transform basis 3", transform.c3)?;
    validate_vec3("transform translation", transform.c4)?;
    Ok(transform)
}

fn read_vec3(resource: &Rsc7Resource, pointer: u64) -> Result<[f32; 3], YbnError> {
    Ok([
        read_f32(resource, pointer)?,
        read_f32(resource, pointer + 4)?,
        read_f32(resource, pointer + 8)?,
    ])
}

fn read_f32(resource: &Rsc7Resource, pointer: u64) -> Result<f32, YbnError> {
    Ok(f32::from_bits(resource.read_u32(pointer)?))
}

fn dot(left: [f32; 3], right: [f32; 3]) -> f32 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

fn length(value: [f32; 3]) -> f32 {
    dot(value, value).sqrt()
}

fn add(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] + right[0], left[1] + right[1], left[2] + right[2]]
}

fn sub(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn mul(value: [f32; 3], scalar: f32) -> [f32; 3] {
    [value[0] * scalar, value[1] * scalar, value[2] * scalar]
}

fn checked_global_vertex(
    base_vertex: usize,
    vertices_count: usize,
    local: usize,
    child_index: usize,
    polygon_index: usize,
) -> Result<usize, YbnError> {
    if local >= vertices_count {
        return Err(YbnError::Malformed(format!(
            "child {child_index} polygon {polygon_index} references vertex {local}, but child has {vertices_count} vertices"
        )));
    }
    base_vertex
        .checked_add(local)
        .ok_or_else(|| YbnError::Malformed("global vertex index overflow".into()))
}

fn geometry_position(
    positions: &[[f32; 3]],
    base_vertex: usize,
    vertices_count: usize,
    local: usize,
    child_index: usize,
    polygon_index: usize,
) -> Result<[f32; 3], YbnError> {
    let global = checked_global_vertex(
        base_vertex,
        vertices_count,
        local,
        child_index,
        polygon_index,
    )?;
    positions
        .get(global)
        .copied()
        .ok_or_else(|| YbnError::Malformed("decoded geometry vertex is missing".into()))
}

fn radial_scale(
    transform: Transform,
    child_index: usize,
    bounds_type: BoundsType,
) -> Result<f32, YbnError> {
    transform.uniform_radial_scale().ok_or_else(|| {
        YbnError::Unsupported(format!(
            "child {child_index} {} uses a non-uniform or skewed transform that cannot preserve a radial primitive",
            bounds_type.as_str()
        ))
    })
}

fn checked_radius(value: f32, label: &str) -> Result<f32, YbnError> {
    if value.is_finite() && value >= 0.0 {
        Ok(value)
    } else {
        Err(YbnError::Malformed(format!("invalid {label} {value}")))
    }
}

fn validate_shape(shape: &CollisionShape) -> Result<(), YbnError> {
    match shape {
        CollisionShape::Sphere { center, radius } => {
            validate_vec3("sphere center", *center)?;
            checked_radius(*radius, "sphere radius")?;
        }
        CollisionShape::Capsule { start, end, radius } => {
            validate_vec3("capsule start", *start)?;
            validate_vec3("capsule end", *end)?;
            checked_radius(*radius, "capsule radius")?;
        }
        CollisionShape::Box { corner, edges } => {
            validate_vec3("box corner", *corner)?;
            for edge in edges {
                validate_vec3("box edge", *edge)?;
            }
        }
        CollisionShape::Cylinder { start, end, radius } => {
            validate_vec3("cylinder start", *start)?;
            validate_vec3("cylinder end", *end)?;
            checked_radius(*radius, "cylinder radius")?;
        }
    }
    Ok(())
}

fn validate_vec3(label: &str, value: [f32; 3]) -> Result<(), YbnError> {
    if value.iter().all(|component| component.is_finite()) {
        Ok(())
    } else {
        Err(YbnError::Malformed(format!(
            "{label} contains non-finite values"
        )))
    }
}

fn validate_count(label: &str, count: usize, max: usize) -> Result<(), YbnError> {
    if count <= max {
        Ok(())
    } else {
        Err(YbnError::Malformed(format!(
            "{label} count {count} exceeds safety limit {max}"
        )))
    }
}

fn checked_mul(left: usize, right: usize, label: &str) -> Result<usize, YbnError> {
    left.checked_mul(right)
        .ok_or_else(|| YbnError::Malformed(format!("{label} overflow")))
}

fn indexed(base: u64, index: usize, stride: usize, label: &str) -> Result<u64, YbnError> {
    let offset = checked_mul(index, stride, label)?;
    base.checked_add(offset as u64)
        .ok_or_else(|| YbnError::Malformed(format!("{label} address overflow")))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::{write::DeflateEncoder, Compression};
    use ragelab_resource::{Rsc7Resource, RSC7_MAGIC, SYSTEM_BASE};

    use super::{
        repack_polygon_edits, BoundsType, CollisionShape, YbnCollision, YbnError, YbnPolygonEdit,
        YbnPolygonKind,
    };

    const SYSTEM_SIZE: usize = 2048;
    const SYSTEM_FLAGS: u32 = (1 << 26) | 1;

    fn ptr(offset: usize) -> u64 {
        SYSTEM_BASE + offset as u64
    }

    fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_i16(bytes: &mut [u8], offset: usize, value: i16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn write_f32(bytes: &mut [u8], offset: usize, value: f32) {
        write_u32(bytes, offset, value.to_bits());
    }

    fn write_vec3(bytes: &mut [u8], offset: usize, value: [f32; 3]) {
        for (axis, component) in value.into_iter().enumerate() {
            write_f32(bytes, offset + axis * 4, component);
        }
    }

    fn write_vertex(bytes: &mut [u8], offset: usize, value: [i16; 3]) {
        write_i16(bytes, offset, value[0]);
        write_i16(bytes, offset + 2, value[1]);
        write_i16(bytes, offset + 4, value[2]);
    }

    fn write_bounds(bytes: &mut [u8], offset: usize, bounds_type: u8) {
        bytes[offset + 0x10] = bounds_type;
        write_f32(bytes, offset + 0x14, 2.0);
        write_vec3(bytes, offset + 0x20, [2.0, 3.0, 1.0]);
        write_vec3(bytes, offset + 0x30, [-2.0, -3.0, -1.0]);
        write_vec3(bytes, offset + 0x40, [0.0, 0.0, 0.0]);
        write_vec3(bytes, offset + 0x50, [0.0, 0.0, 0.0]);
    }

    fn write_primitive_material(bytes: &mut [u8], offset: usize) {
        bytes[offset + 0x4c] = 69;
        bytes[offset + 0x4d] = 7;
        bytes[offset + 0x5d] = 5;
    }

    fn build_fixture(mutate: impl FnOnce(&mut [u8])) -> Vec<u8> {
        let mut system = vec![0_u8; SYSTEM_SIZE];
        write_bounds(&mut system, 0, 10);

        // Composite arrays.
        write_u64(&mut system, 0x70, ptr(0x400));
        write_u64(&mut system, 0x78, ptr(0x410));
        write_u16(&mut system, 0xa0, 1);
        write_u16(&mut system, 0xa2, 1);
        write_u64(&mut system, 0x400, ptr(0x200));

        // Identity Matrix4F_s, with flags occupying each fourth dword.
        write_vec3(&mut system, 0x410, [1.0, 0.0, 0.0]);
        write_vec3(&mut system, 0x420, [0.0, 1.0, 0.0]);
        write_vec3(&mut system, 0x430, [0.0, 0.0, 1.0]);
        write_vec3(&mut system, 0x440, [0.0, 0.0, 0.0]);

        // GeometryBVH child.
        write_bounds(&mut system, 0x200, 8);
        write_u64(&mut system, 0x200 + 0x88, ptr(0x500));
        write_vec3(&mut system, 0x200 + 0x90, [1.0, 1.0, 1.0]);
        write_vec3(&mut system, 0x200 + 0xa0, [10.0, 20.0, 30.0]);
        write_u64(&mut system, 0x200 + 0xb0, ptr(0x480));
        write_u32(&mut system, 0x200 + 0xd0, 3);
        write_u32(&mut system, 0x200 + 0xd4, 1);
        write_u64(&mut system, 0x200 + 0xf0, ptr(0x520));
        write_u64(&mut system, 0x200 + 0x118, ptr(0x540));
        system[0x200 + 0x120] = 1;

        write_i16(&mut system, 0x480, 0);
        write_i16(&mut system, 0x482, 0);
        write_i16(&mut system, 0x484, 0);
        write_i16(&mut system, 0x486, 1);
        write_i16(&mut system, 0x488, 0);
        write_i16(&mut system, 0x48a, 0);
        write_i16(&mut system, 0x48c, 0);
        write_i16(&mut system, 0x48e, 1);
        write_i16(&mut system, 0x490, 0);

        // Triangle: area/type dword then three indices and three edge refs.
        write_u16(&mut system, 0x504, 0);
        write_u16(&mut system, 0x506, 1);
        write_u16(&mut system, 0x508, 2);
        write_u16(&mut system, 0x50a, 0xffff);
        write_u16(&mut system, 0x50c, 0xffff);
        write_u16(&mut system, 0x50e, 0xffff);

        // Material type 69, procedural 7, flags low byte 3, colour index 5.
        write_u32(&mut system, 0x520, 69 | (7 << 8) | (3 << 24));
        write_u32(&mut system, 0x524, 5 << 8);
        system[0x540] = 0;

        mutate(&mut system);

        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&system).expect("compress YBN fixture");
        let compressed = encoder.finish().expect("finish YBN fixture");
        let mut output = Vec::new();
        output.extend_from_slice(&RSC7_MAGIC);
        output.extend_from_slice(&43_u32.to_le_bytes());
        output.extend_from_slice(&SYSTEM_FLAGS.to_le_bytes());
        output.extend_from_slice(&0_u32.to_le_bytes());
        output.extend_from_slice(&compressed);
        output
    }

    #[test]
    fn decodes_composite_triangle_geometry() {
        let bytes = build_fixture(|_| {});
        let collision = YbnCollision::from_bytes(&bytes).expect("valid synthetic YBN");
        assert_eq!(collision.vertex_count(), 3);
        assert_eq!(collision.triangle_count(), 1);
        assert_eq!(
            collision.positions,
            vec![[10.0, 20.0, 30.0], [11.0, 20.0, 30.0], [10.0, 21.0, 30.0]]
        );
        assert_eq!(collision.primitives.len(), 1);
        assert_eq!(collision.primitives[0].indices, vec![0, 1, 2]);
        assert_eq!(collision.materials.len(), 1);
        assert_eq!(collision.materials[0].material_type, 69);
        assert_eq!(collision.materials[0].procedural_id, 7);
        assert_eq!(collision.materials[0].flags, 3);
        assert_eq!(collision.materials[0].color_index, 5);
        assert_eq!(collision.children[0].bounds_type, BoundsType::GeometryBvh);
    }

    #[test]
    fn applies_composite_child_transform() {
        let bytes = build_fixture(|system| {
            write_vec3(system, 0x440, [100.0, -50.0, 5.0]);
        });
        let collision = YbnCollision::from_bytes(&bytes).expect("transformed YBN");
        assert_eq!(collision.positions[0], [110.0, -30.0, 35.0]);
    }

    #[test]
    fn decodes_root_sphere_bound() {
        let bytes = build_fixture(|system| {
            write_bounds(system, 0, 0);
            write_f32(system, 0x14, 1.25);
            write_vec3(system, 0x50, [1.0, 2.0, 3.0]);
            write_primitive_material(system, 0);
        });
        let collision = YbnCollision::from_bytes(&bytes).expect("sphere root");
        assert_eq!(collision.vertex_count(), 0);
        assert_eq!(collision.triangle_count(), 0);
        assert_eq!(collision.shape_primitive_count(), 1);
        assert_eq!(collision.children[0].bounds_type, BoundsType::Sphere);
        assert_eq!(collision.children[0].shape_primitives, 1);
        assert_eq!(collision.materials[0].material_type, 69);
        assert_eq!(collision.materials[0].procedural_id, 7);
        assert_eq!(collision.materials[0].color_index, 5);
        assert_eq!(
            collision.shape_primitives[0].shape,
            CollisionShape::Sphere {
                center: [1.0, 2.0, 3.0],
                radius: 1.25,
            }
        );
    }

    #[test]
    fn decodes_composite_capsule_bound() {
        let bytes = build_fixture(|system| {
            write_bounds(system, 0x200, 1);
            write_f32(system, 0x200 + 0x2c, 0.5);
            write_vec3(system, 0x200 + 0x50, [3.0, 4.0, 5.0]);
            write_primitive_material(system, 0x200);
        });
        let collision = YbnCollision::from_bytes(&bytes).expect("capsule child");
        assert_eq!(collision.children[0].bounds_type, BoundsType::Capsule);
        assert_eq!(
            collision.shape_primitives[0].shape,
            CollisionShape::Capsule {
                start: [3.0, 2.5, 5.0],
                end: [3.0, 5.5, 5.0],
                radius: 0.5,
            }
        );
    }

    #[test]
    fn decodes_composite_box_bound() {
        let bytes = build_fixture(|system| {
            write_bounds(system, 0x200, 3);
            write_primitive_material(system, 0x200);
        });
        let collision = YbnCollision::from_bytes(&bytes).expect("box child");
        assert_eq!(collision.children[0].bounds_type, BoundsType::Box);
        assert_eq!(
            collision.shape_primitives[0].shape,
            CollisionShape::Box {
                corner: [-2.0, -3.0, -1.0],
                edges: [[4.0, 0.0, 0.0], [0.0, 6.0, 0.0], [0.0, 0.0, 2.0]],
            }
        );
    }

    #[test]
    fn decodes_composite_cylinder_bound() {
        let bytes = build_fixture(|system| {
            write_bounds(system, 0x200, 13);
            write_primitive_material(system, 0x200);
        });
        let collision = YbnCollision::from_bytes(&bytes).expect("cylinder child");
        assert_eq!(collision.children[0].bounds_type, BoundsType::Cylinder);
        assert_eq!(
            collision.shape_primitives[0].shape,
            CollisionShape::Cylinder {
                start: [0.0, -3.0, 0.0],
                end: [0.0, 3.0, 0.0],
                radius: 2.0,
            }
        );
    }

    #[test]
    fn rejects_non_uniform_radial_primitive_transform() {
        let bytes = build_fixture(|system| {
            write_bounds(system, 0x200, 0);
            write_primitive_material(system, 0x200);
            write_vec3(system, 0x410, [2.0, 0.0, 0.0]);
        });
        let error = YbnCollision::from_bytes(&bytes).expect_err("non-uniform sphere transform");
        assert!(matches!(error, YbnError::Unsupported(message) if message.contains("non-uniform")));
    }

    #[test]
    fn rejects_truncated_extended_primitive_block() {
        let bytes = build_fixture(|system| {
            write_u64(system, 0x400, ptr(0x790));
            write_bounds(system, 0x790, 1);
            write_f32(system, 0x790 + 0x2c, 0.5);
            write_primitive_material(system, 0x790);
        });
        let error = YbnCollision::from_bytes(&bytes).expect_err("truncated capsule block");
        assert!(matches!(error, YbnError::Resource(_)));
    }

    #[test]
    fn decodes_sphere_polygon() {
        let bytes = build_fixture(|system| {
            write_u16(system, 0x500, 1);
            write_u16(system, 0x502, 1);
            write_f32(system, 0x504, 1.5);
        });
        let collision = YbnCollision::from_bytes(&bytes).expect("sphere polygon");
        assert_eq!(collision.triangle_count(), 0);
        assert_eq!(collision.shape_primitive_count(), 1);
        assert_eq!(collision.children[0].triangles, 0);
        assert_eq!(collision.children[0].shape_primitives, 1);
        assert_eq!(collision.shape_primitives[0].polygon_index, Some(0));
        assert_eq!(
            collision.shape_primitives[0].shape,
            CollisionShape::Sphere {
                center: [11.0, 20.0, 30.0],
                radius: 1.5,
            }
        );
    }

    #[test]
    fn decodes_capsule_polygon() {
        let bytes = build_fixture(|system| {
            write_u16(system, 0x500, 2);
            write_u16(system, 0x502, 0);
            write_f32(system, 0x504, 0.75);
            write_u16(system, 0x508, 2);
        });
        let collision = YbnCollision::from_bytes(&bytes).expect("capsule polygon");
        assert_eq!(
            collision.shape_primitives[0].shape,
            CollisionShape::Capsule {
                start: [10.0, 20.0, 30.0],
                end: [10.0, 21.0, 30.0],
                radius: 0.75,
            }
        );
    }

    #[test]
    fn decodes_box_polygon() {
        let bytes = build_fixture(|system| {
            write_u32(system, 0x200 + 0xd0, 4);
            write_vertex(system, 0x480, [0, 0, 0]);
            write_vertex(system, 0x486, [0, 1, 1]);
            write_vertex(system, 0x48c, [1, 0, 1]);
            write_vertex(system, 0x492, [1, 1, 0]);
            write_u32(system, 0x500, 3);
            write_i16(system, 0x504, 0);
            write_i16(system, 0x506, 1);
            write_i16(system, 0x508, 2);
            write_i16(system, 0x50a, 3);
        });
        let collision = YbnCollision::from_bytes(&bytes).expect("box polygon");
        assert_eq!(
            collision.shape_primitives[0].shape,
            CollisionShape::Box {
                corner: [10.0, 20.0, 30.0],
                edges: [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]],
            }
        );
    }

    #[test]
    fn decodes_cylinder_polygon() {
        let bytes = build_fixture(|system| {
            write_u16(system, 0x500, 4);
            write_u16(system, 0x502, 0);
            write_f32(system, 0x504, 0.25);
            write_u16(system, 0x508, 1);
        });
        let collision = YbnCollision::from_bytes(&bytes).expect("cylinder polygon");
        assert_eq!(
            collision.shape_primitives[0].shape,
            CollisionShape::Cylinder {
                start: [10.0, 20.0, 30.0],
                end: [11.0, 20.0, 30.0],
                radius: 0.25,
            }
        );
    }

    #[test]
    fn repacks_polygon_radius_and_material_with_semantic_round_trip() {
        let bytes = build_fixture(|system| {
            write_u16(system, 0x500, 1);
            write_u16(system, 0x502, 1);
            write_f32(system, 0x504, 1.5);
            system[0x200 + 0x120] = 2;
            write_u32(system, 0x528, 70 | (8 << 8));
            write_u32(system, 0x52c, 6 << 8);
            system[0x540] = 0;
        });
        let original_resource = Rsc7Resource::parse(&bytes).expect("parse source resource");
        let edited_bytes = repack_polygon_edits(
            &bytes,
            &[YbnPolygonEdit {
                child_index: 0,
                polygon_index: 0,
                expected_kind: YbnPolygonKind::Sphere,
                radius: Some(2.75),
                local_material: Some(1),
            }],
        )
        .expect("repack sphere polygon");
        let edited_resource = Rsc7Resource::parse(&edited_bytes).expect("reparse edited resource");

        assert_eq!(edited_resource.header, original_resource.header);
        assert_eq!(edited_resource.graphics(), original_resource.graphics());
        let mut expected_system = original_resource.system().to_vec();
        write_f32(&mut expected_system, 0x504, 2.75);
        expected_system[0x540] = 1;
        assert_eq!(edited_resource.system(), expected_system.as_slice());

        let collision = YbnCollision::from_bytes(&edited_bytes).expect("read edited YBN");
        assert_eq!(
            collision.shape_primitives[0].shape,
            CollisionShape::Sphere {
                center: [11.0, 20.0, 30.0],
                radius: 2.75,
            }
        );
        let material = &collision.materials[collision.shape_primitives[0].material_index];
        assert_eq!(material.local_index, 1);
        assert_eq!(material.material_type, 70);
    }

    #[test]
    fn repacks_normalized_radius_through_uniform_child_scale() {
        let bytes = build_fixture(|system| {
            write_u16(system, 0x500, 2);
            write_u16(system, 0x502, 0);
            write_f32(system, 0x504, 0.5);
            write_u16(system, 0x508, 2);
            write_vec3(system, 0x410, [2.0, 0.0, 0.0]);
            write_vec3(system, 0x420, [0.0, 2.0, 0.0]);
            write_vec3(system, 0x430, [0.0, 0.0, 2.0]);
        });
        let edited_bytes = repack_polygon_edits(
            &bytes,
            &[YbnPolygonEdit {
                child_index: 0,
                polygon_index: 0,
                expected_kind: YbnPolygonKind::Capsule,
                radius: Some(3.0),
                local_material: None,
            }],
        )
        .expect("repack scaled capsule");
        let resource = Rsc7Resource::parse(&edited_bytes).expect("reparse edited resource");
        assert_eq!(
            f32::from_le_bytes(resource.system()[0x504..0x508].try_into().unwrap()),
            1.5
        );
        let collision = YbnCollision::from_bytes(&edited_bytes).expect("read edited YBN");
        assert!(matches!(
            collision.shape_primitives[0].shape,
            CollisionShape::Capsule { radius, .. } if (radius - 3.0).abs() < 1.0e-5
        ));
    }

    #[test]
    fn polygon_writer_rejects_kind_radius_and_material_mismatches() {
        let bytes = build_fixture(|system| {
            write_u32(system, 0x200 + 0xd0, 4);
            write_vertex(system, 0x492, [1, 1, 0]);
            write_u32(system, 0x500, 3);
            write_i16(system, 0x504, 0);
            write_i16(system, 0x506, 1);
            write_i16(system, 0x508, 2);
            write_i16(system, 0x50a, 3);
        });
        let kind_error = repack_polygon_edits(
            &bytes,
            &[YbnPolygonEdit {
                child_index: 0,
                polygon_index: 0,
                expected_kind: YbnPolygonKind::Sphere,
                radius: None,
                local_material: None,
            }],
        )
        .expect_err("stale kind must reject");
        assert!(
            matches!(kind_error, YbnError::Malformed(message) if message.contains("expected sphere"))
        );

        let radius_error = repack_polygon_edits(
            &bytes,
            &[YbnPolygonEdit {
                child_index: 0,
                polygon_index: 0,
                expected_kind: YbnPolygonKind::Box,
                radius: Some(1.0),
                local_material: None,
            }],
        )
        .expect_err("box radius must reject");
        assert!(
            matches!(radius_error, YbnError::Unsupported(message) if message.contains("does not expose a radius"))
        );

        let material_error = repack_polygon_edits(
            &bytes,
            &[YbnPolygonEdit {
                child_index: 0,
                polygon_index: 0,
                expected_kind: YbnPolygonKind::Box,
                radius: None,
                local_material: Some(1),
            }],
        )
        .expect_err("out of range material must reject");
        assert!(
            matches!(material_error, YbnError::Malformed(message) if message.contains("out of range"))
        );
    }

    #[test]
    fn polygon_writer_rejects_invalid_radius_out_of_range_index_and_corrupt_pointer() {
        let bytes = build_fixture(|system| {
            write_u16(system, 0x500, 1);
            write_u16(system, 0x502, 0);
            write_f32(system, 0x504, 1.0);
        });

        for radius in [0.0, f32::NAN] {
            let error = repack_polygon_edits(
                &bytes,
                &[YbnPolygonEdit {
                    child_index: 0,
                    polygon_index: 0,
                    expected_kind: YbnPolygonKind::Sphere,
                    radius: Some(radius),
                    local_material: None,
                }],
            )
            .expect_err("invalid radius must reject");
            assert!(
                matches!(error, YbnError::Malformed(message) if message.contains("radius must be finite and greater than zero"))
            );
        }

        let index_error = repack_polygon_edits(
            &bytes,
            &[YbnPolygonEdit {
                child_index: 0,
                polygon_index: 1,
                expected_kind: YbnPolygonKind::Sphere,
                radius: Some(2.0),
                local_material: None,
            }],
        )
        .expect_err("out of range polygon must reject");
        assert!(
            matches!(index_error, YbnError::Malformed(message) if message.contains("out of range"))
        );

        let corrupt = build_fixture(|system| {
            write_u16(system, 0x500, 1);
            write_u16(system, 0x502, 0);
            write_f32(system, 0x504, 1.0);
            write_u64(system, 0x200 + 0x118, ptr(SYSTEM_SIZE + 16));
        });
        let corrupt_error = repack_polygon_edits(
            &corrupt,
            &[YbnPolygonEdit {
                child_index: 0,
                polygon_index: 0,
                expected_kind: YbnPolygonKind::Sphere,
                radius: None,
                local_material: Some(0),
            }],
        )
        .expect_err("corrupt polygon-material pointer must reject");
        assert!(matches!(
            corrupt_error,
            YbnError::Resource(_) | YbnError::Malformed(_)
        ));
    }

    #[test]
    fn polygon_writer_rejects_duplicate_and_non_geometry_targets() {
        let bytes = build_fixture(|system| {
            write_u16(system, 0x500, 1);
            write_u16(system, 0x502, 0);
            write_f32(system, 0x504, 1.0);
        });
        let edit = YbnPolygonEdit {
            child_index: 0,
            polygon_index: 0,
            expected_kind: YbnPolygonKind::Sphere,
            radius: Some(2.0),
            local_material: None,
        };
        let duplicate_error =
            repack_polygon_edits(&bytes, &[edit, edit]).expect_err("duplicate edit must reject");
        assert!(
            matches!(duplicate_error, YbnError::Malformed(message) if message.contains("duplicate polygon edit"))
        );

        let primitive_root = build_fixture(|system| {
            write_bounds(system, 0, 0);
            write_primitive_material(system, 0);
        });
        let target_error = repack_polygon_edits(&primitive_root, &[edit])
            .expect_err("primitive bound is not geometry");
        assert!(
            matches!(target_error, YbnError::Malformed(message) if message.contains("not editable geometry"))
        );
    }

    #[test]
    fn rejects_unknown_polygon_variant() {
        let bytes = build_fixture(|system| {
            system[0x500] = 5;
        });
        let error = YbnCollision::from_bytes(&bytes).expect_err("unknown polygon variant");
        assert!(matches!(error, YbnError::Unsupported(message) if message.contains("type 5")));
    }

    #[test]
    fn rejects_invalid_polygon_radius() {
        let bytes = build_fixture(|system| {
            write_u16(system, 0x500, 1);
            write_u16(system, 0x502, 0);
            write_f32(system, 0x504, f32::NAN);
        });
        let error = YbnCollision::from_bytes(&bytes).expect_err("invalid sphere radius");
        assert!(matches!(error, YbnError::Malformed(message) if message.contains("radius")));
    }

    #[test]
    fn rejects_negative_box_polygon_vertex() {
        let bytes = build_fixture(|system| {
            write_u32(system, 0x500, 3);
            write_i16(system, 0x504, -1);
        });
        let error = YbnCollision::from_bytes(&bytes).expect_err("negative box vertex");
        assert!(
            matches!(error, YbnError::Malformed(message) if message.contains("negative vertex"))
        );
    }

    #[test]
    fn rejects_out_of_range_primitive_vertex() {
        let bytes = build_fixture(|system| {
            write_u16(system, 0x500, 1);
            write_u16(system, 0x502, 3);
            write_f32(system, 0x504, 1.0);
        });
        let error = YbnCollision::from_bytes(&bytes).expect_err("bad primitive vertex index");
        assert!(matches!(error, YbnError::Malformed(message) if message.contains("vertex 3")));
    }

    #[test]
    fn rejects_out_of_range_vertex_index() {
        let bytes = build_fixture(|system| {
            write_u16(system, 0x508, 3);
        });
        let error = YbnCollision::from_bytes(&bytes).expect_err("bad vertex index");
        assert!(matches!(error, YbnError::Malformed(message) if message.contains("vertex 3")));
    }

    #[test]
    fn rejects_other_resource_versions() {
        let mut bytes = build_fixture(|_| {});
        bytes[4..8].copy_from_slice(&159_u32.to_le_bytes());
        let error = YbnCollision::from_bytes(&bytes).expect_err("Gen9 explicit");
        assert!(matches!(error, YbnError::Unsupported(message) if message.contains("159")));
    }
}

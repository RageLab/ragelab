//! Transport-neutral spatial policy for workspace assets and YMAP instances.
//!
//! World placement is emitted only from explicit format evidence. Local drawable/collision
//! bounds, filenames, directory proximity, and resolver naming conventions never become world
//! coordinates here. Callers that cannot prove a placement must preserve a fail-closed state.

use std::path::PathBuf;

use ragelab_assets::AssetKind;
use ragelab_ymap::{Ymap, YmapEntity};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpatialClassification {
    WorldSpatial,
    LocalOnly,
    NonSpatial,
    Unresolved,
    Ambiguous,
    Unsupported,
}

impl SpatialClassification {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorldSpatial => "worldSpatial",
            Self::LocalOnly => "localOnly",
            Self::NonSpatial => "nonSpatial",
            Self::Unresolved => "unresolved",
            Self::Ambiguous => "ambiguous",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpatialReasonCode {
    LocalCoordinatesOnly,
    NonSpatialAsset,
    NoProvenWorldPlacement,
    AmbiguousWorldPlacement,
    UnsupportedAssetKind,
    InvalidWorldTransform,
}

impl SpatialReasonCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalCoordinatesOnly => "localCoordinatesOnly",
            Self::NonSpatialAsset => "nonSpatialAsset",
            Self::NoProvenWorldPlacement => "noProvenWorldPlacement",
            Self::AmbiguousWorldPlacement => "ambiguousWorldPlacement",
            Self::UnsupportedAssetKind => "unsupportedAssetKind",
            Self::InvalidWorldTransform => "invalidWorldTransform",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpatialReason {
    pub code: SpatialReasonCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpatialAabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl SpatialAabb {
    pub fn new(min: [f32; 3], max: [f32; 3]) -> Option<Self> {
        if !min
            .iter()
            .chain(max.iter())
            .all(|component| component.is_finite())
        {
            return None;
        }
        if (0..3).any(|axis| min[axis] > max[axis]) {
            return None;
        }
        Some(Self { min, max })
    }

    pub fn center(self) -> [f32; 3] {
        [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[1] + self.max[1]) * 0.5,
            (self.min[2] + self.max[2]) * 0.5,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpatialTransform {
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    /// Scale is optional because the current YMAP normalized model does not expose it.
    /// Callers must not assume identity scale when this is `None`.
    pub scale: Option<[f32; 3]>,
}

impl SpatialTransform {
    pub fn new(translation: [f32; 3], rotation: [f32; 4], scale: Option<[f32; 3]>) -> Option<Self> {
        if !translation
            .iter()
            .chain(rotation.iter())
            .all(|component| component.is_finite())
            || scale.is_some_and(|scale| !scale.iter().all(|component| component.is_finite()))
        {
            return None;
        }
        Some(Self {
            translation,
            rotation,
            scale,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpatialProvenance {
    AssetSemantics {
        kind: AssetKind,
    },
    YmapDefinition,
    YmapEntitiesExtents,
    YmapStreamingExtents,
    YmapEntityPositions,
    YmapEntityTransform {
        entity_index: usize,
        archetype_hash: u32,
    },
}

impl SpatialProvenance {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::AssetSemantics { .. } => "assetSemantics",
            Self::YmapDefinition => "ymapDefinition",
            Self::YmapEntitiesExtents => "ymapEntitiesExtents",
            Self::YmapStreamingExtents => "ymapStreamingExtents",
            Self::YmapEntityPositions => "ymapEntityPositions",
            Self::YmapEntityTransform { .. } => "ymapEntityTransform",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpatialContext {
    pub classification: SpatialClassification,
    pub world_transform: Option<SpatialTransform>,
    pub world_center: Option<[f32; 3]>,
    pub world_bounds: Option<SpatialAabb>,
    pub provenance: SpatialProvenance,
    pub context_asset: Option<PathBuf>,
    pub reason: Option<SpatialReason>,
}

impl SpatialContext {
    pub fn is_world_spatial(&self) -> bool {
        self.classification == SpatialClassification::WorldSpatial
    }

    pub fn unresolved(
        provenance: SpatialProvenance,
        context_asset: Option<PathBuf>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            classification: SpatialClassification::Unresolved,
            world_transform: None,
            world_center: None,
            world_bounds: None,
            provenance,
            context_asset,
            reason: Some(SpatialReason {
                code: SpatialReasonCode::NoProvenWorldPlacement,
                message: message.into(),
            }),
        }
    }

    pub fn ambiguous(
        provenance: SpatialProvenance,
        context_asset: Option<PathBuf>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            classification: SpatialClassification::Ambiguous,
            world_transform: None,
            world_center: None,
            world_bounds: None,
            provenance,
            context_asset,
            reason: Some(SpatialReason {
                code: SpatialReasonCode::AmbiguousWorldPlacement,
                message: message.into(),
            }),
        }
    }
}

pub fn isolated_asset_spatial_context(kind: AssetKind) -> SpatialContext {
    match kind {
        AssetKind::Ymap => SpatialContext {
            classification: SpatialClassification::WorldSpatial,
            world_transform: None,
            world_center: None,
            world_bounds: None,
            provenance: SpatialProvenance::YmapDefinition,
            context_asset: None,
            reason: None,
        },
        AssetKind::Ydr | AssetKind::Ydd | AssetKind::Ybn => SpatialContext {
            classification: SpatialClassification::LocalOnly,
            world_transform: None,
            world_center: None,
            world_bounds: None,
            provenance: SpatialProvenance::AssetSemantics { kind },
            context_asset: None,
            reason: Some(SpatialReason {
                code: SpatialReasonCode::LocalCoordinatesOnly,
                message: format!(
                    "{kind} coordinates and bounds are asset-local; no world placement is proven"
                ),
            }),
        },
        AssetKind::Ytd => SpatialContext {
            classification: SpatialClassification::NonSpatial,
            world_transform: None,
            world_center: None,
            world_bounds: None,
            provenance: SpatialProvenance::AssetSemantics { kind },
            context_asset: None,
            reason: Some(SpatialReason {
                code: SpatialReasonCode::NonSpatialAsset,
                message: "YTD texture dictionaries do not have world-space placement semantics"
                    .into(),
            }),
        },
        AssetKind::Ytyp | AssetKind::Yft | AssetKind::Ycd => SpatialContext {
            classification: SpatialClassification::Unsupported,
            world_transform: None,
            world_center: None,
            world_bounds: None,
            provenance: SpatialProvenance::AssetSemantics { kind },
            context_asset: None,
            reason: Some(SpatialReason {
                code: SpatialReasonCode::UnsupportedAssetKind,
                message: format!(
                    "spatial classification for isolated {kind} assets is not supported by the current contract"
                ),
            }),
        },
    }
}

pub fn ymap_spatial_context(ymap: &Ymap, context_asset: Option<PathBuf>) -> SpatialContext {
    let candidates = [
        (
            ymap.entities_extents_min,
            ymap.entities_extents_max,
            SpatialProvenance::YmapEntitiesExtents,
        ),
        (
            ymap.streaming_extents_min,
            ymap.streaming_extents_max,
            SpatialProvenance::YmapStreamingExtents,
        ),
    ];

    for (min, max, provenance) in candidates {
        if let (Some(min), Some(max)) = (min, max) {
            if let Some(bounds) = SpatialAabb::new([min.x, min.y, min.z], [max.x, max.y, max.z]) {
                return SpatialContext {
                    classification: SpatialClassification::WorldSpatial,
                    world_transform: None,
                    world_center: Some(bounds.center()),
                    world_bounds: Some(bounds),
                    provenance,
                    context_asset,
                    reason: None,
                };
            }
        }
    }

    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    let mut count = 0_usize;
    for entity in &ymap.entities {
        let position = [entity.position.x, entity.position.y, entity.position.z];
        if !position.iter().all(|component| component.is_finite()) {
            continue;
        }
        for axis in 0..3 {
            min[axis] = min[axis].min(position[axis]);
            max[axis] = max[axis].max(position[axis]);
        }
        count += 1;
    }

    if count > 0 {
        if let Some(bounds) = SpatialAabb::new(min, max) {
            return SpatialContext {
                classification: SpatialClassification::WorldSpatial,
                world_transform: None,
                world_center: Some(bounds.center()),
                world_bounds: Some(bounds),
                provenance: SpatialProvenance::YmapEntityPositions,
                context_asset,
                reason: None,
            };
        }
    }

    SpatialContext {
        classification: SpatialClassification::WorldSpatial,
        world_transform: None,
        world_center: None,
        world_bounds: None,
        provenance: SpatialProvenance::YmapDefinition,
        context_asset,
        reason: None,
    }
}

pub fn ymap_entity_spatial_context(
    context_asset: PathBuf,
    entity_index: usize,
    entity: &YmapEntity,
) -> SpatialContext {
    let provenance = SpatialProvenance::YmapEntityTransform {
        entity_index,
        archetype_hash: entity.archetype_name.0,
    };
    let translation = [entity.position.x, entity.position.y, entity.position.z];
    let rotation = [
        entity.rotation.x,
        entity.rotation.y,
        entity.rotation.z,
        entity.rotation.w,
    ];
    let scale = match (entity.scale_xy, entity.scale_z) {
        (Some(scale_xy), Some(scale_z)) => Some([scale_xy, scale_xy, scale_z]),
        (None, None) => None,
        _ => {
            return SpatialContext {
                classification: SpatialClassification::Unresolved,
                world_transform: None,
                world_center: None,
                world_bounds: None,
                provenance,
                context_asset: Some(context_asset),
                reason: Some(SpatialReason {
                    code: SpatialReasonCode::InvalidWorldTransform,
                    message: "YMAP entity placement exposes incomplete scaleXY/scaleZ data and cannot be located safely".into(),
                }),
            };
        }
    };
    let Some(world_transform) = SpatialTransform::new(translation, rotation, scale) else {
        return SpatialContext {
            classification: SpatialClassification::Unresolved,
            world_transform: None,
            world_center: None,
            world_bounds: None,
            provenance,
            context_asset: Some(context_asset),
            reason: Some(SpatialReason {
                code: SpatialReasonCode::InvalidWorldTransform,
                message:
                    "YMAP entity placement contains non-finite transform components and cannot be located safely"
                        .into(),
            }),
        };
    };

    SpatialContext {
        classification: SpatialClassification::WorldSpatial,
        world_transform: Some(world_transform),
        world_center: Some(world_transform.translation),
        world_bounds: None,
        provenance,
        context_asset: Some(context_asset),
        reason: None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ragelab_ymap::{Vec3, Ymap, YmapEntity};

    use super::*;

    const SIMPLE_YMAP: &[u8] = include_bytes!("../../../fixtures/synthetic/simple.ymap");

    fn entity(position: [f32; 3]) -> YmapEntity {
        let mut ymap = Ymap::from_bytes(SIMPLE_YMAP).expect("synthetic YMAP should parse");
        let mut entity = ymap.entities.remove(0);
        entity.position = Vec3 {
            x: position[0],
            y: position[1],
            z: position[2],
        };
        entity
    }

    #[test]
    fn isolated_asset_classifications_fail_closed() {
        assert_eq!(
            isolated_asset_spatial_context(AssetKind::Ymap).classification,
            SpatialClassification::WorldSpatial
        );
        assert_eq!(
            isolated_asset_spatial_context(AssetKind::Ydr).classification,
            SpatialClassification::LocalOnly
        );
        assert_eq!(
            isolated_asset_spatial_context(AssetKind::Ydd).classification,
            SpatialClassification::LocalOnly
        );
        let ybn = isolated_asset_spatial_context(AssetKind::Ybn);
        assert_eq!(ybn.classification, SpatialClassification::LocalOnly);
        assert!(ybn.world_transform.is_none());
        assert!(ybn.world_bounds.is_none());
        assert_eq!(
            isolated_asset_spatial_context(AssetKind::Ytd).classification,
            SpatialClassification::NonSpatial
        );
        let unsupported = isolated_asset_spatial_context(AssetKind::Ytyp);
        assert_eq!(
            unsupported.classification,
            SpatialClassification::Unsupported
        );
        assert_eq!(
            unsupported.reason.as_ref().map(|reason| reason.code),
            Some(SpatialReasonCode::UnsupportedAssetKind)
        );
    }

    #[test]
    fn ymap_uses_declared_world_extents_when_valid() {
        let ymap = Ymap {
            entities_extents_min: Some(Vec3 {
                x: -10.0,
                y: -20.0,
                z: -2.0,
            }),
            entities_extents_max: Some(Vec3 {
                x: 30.0,
                y: 40.0,
                z: 8.0,
            }),
            ..Ymap::default()
        };

        let context = ymap_spatial_context(&ymap, Some(PathBuf::from("stream/test.ymap")));
        assert_eq!(context.classification, SpatialClassification::WorldSpatial);
        assert_eq!(context.provenance, SpatialProvenance::YmapEntitiesExtents);
        assert_eq!(context.world_center, Some([10.0, 10.0, 3.0]));
        assert_eq!(
            context.world_bounds,
            Some(SpatialAabb {
                min: [-10.0, -20.0, -2.0],
                max: [30.0, 40.0, 8.0],
            })
        );
    }

    #[test]
    fn ymap_falls_back_to_entity_positions_without_inventing_bounds() {
        let ymap = Ymap {
            entities: vec![entity([4.0, 8.0, 12.0]), entity([10.0, 2.0, 6.0])],
            ..Ymap::default()
        };

        let context = ymap_spatial_context(&ymap, None);
        assert_eq!(context.provenance, SpatialProvenance::YmapEntityPositions);
        assert_eq!(context.world_center, Some([7.0, 5.0, 9.0]));
        assert_eq!(
            context.world_bounds,
            Some(SpatialAabb {
                min: [4.0, 2.0, 6.0],
                max: [10.0, 8.0, 12.0],
            })
        );
    }

    #[test]
    fn ymap_entity_has_proven_world_transform() {
        let entity = entity([100.0, 200.0, 30.0]);
        let context = ymap_entity_spatial_context(PathBuf::from("stream/map.ymap"), 7, &entity);

        assert!(context.is_world_spatial());
        assert_eq!(context.world_center, Some([100.0, 200.0, 30.0]));
        assert_eq!(
            context.world_transform,
            Some(SpatialTransform {
                translation: [100.0, 200.0, 30.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: Some([1.25, 1.25, 0.75]),
            })
        );
        assert_eq!(
            context.provenance,
            SpatialProvenance::YmapEntityTransform {
                entity_index: 7,
                archetype_hash: 0xEF3D_BDA5,
            }
        );
    }

    #[test]
    fn incomplete_entity_scale_is_explicitly_unresolved() {
        let mut entity = entity([1.0, 2.0, 3.0]);
        entity.scale_z = None;

        let context = ymap_entity_spatial_context(PathBuf::from("stream/map.ymap"), 0, &entity);
        assert_eq!(context.classification, SpatialClassification::Unresolved);
        assert!(context.world_transform.is_none());
        assert_eq!(
            context.reason.as_ref().map(|reason| reason.code),
            Some(SpatialReasonCode::InvalidWorldTransform)
        );
    }

    #[test]
    fn invalid_entity_transform_is_explicitly_unresolved() {
        let mut entity = entity([1.0, 2.0, 3.0]);
        entity.position.x = f32::NAN;

        let context = ymap_entity_spatial_context(PathBuf::from("stream/map.ymap"), 0, &entity);
        assert_eq!(context.classification, SpatialClassification::Unresolved);
        assert!(context.world_transform.is_none());
        assert_eq!(
            context.reason.as_ref().map(|reason| reason.code),
            Some(SpatialReasonCode::InvalidWorldTransform)
        );
    }

    #[test]
    fn ambiguous_context_never_carries_world_data() {
        let context = SpatialContext::ambiguous(
            SpatialProvenance::AssetSemantics {
                kind: AssetKind::Ybn,
            },
            None,
            "multiple structural placements exist and none is authoritative",
        );

        assert_eq!(context.classification, SpatialClassification::Ambiguous);
        assert!(context.world_transform.is_none());
        assert!(context.world_center.is_none());
        assert!(context.world_bounds.is_none());
        assert_eq!(
            context.reason.as_ref().map(|reason| reason.code),
            Some(SpatialReasonCode::AmbiguousWorldPlacement)
        );
    }
}

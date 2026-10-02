use ragelab_meta::MetaHash;
use ragelab_ytyp::{ArchetypeKind, AssetType, Ytyp};

#[test]
fn parses_synthetic_ytyp_end_to_end() {
    let bytes = include_bytes!("../../../fixtures/synthetic/simple.ytyp");
    let ytyp = Ytyp::from_bytes(bytes).expect("synthetic YTYP should parse");

    assert_eq!(ytyp.name, Some(MetaHash(0xAA2B_639B))); // simple
    assert!(ytyp.dependencies.is_empty());
    assert_eq!(ytyp.archetypes.len(), 1);

    let archetype = &ytyp.archetypes[0];
    assert_eq!(archetype.kind, ArchetypeKind::Base);
    assert_eq!(archetype.name, MetaHash(0xEF3D_BDA5)); // test_archetype
    assert_eq!(archetype.asset_type, AssetType::Drawable);
    assert_eq!(archetype.asset_name, Some(MetaHash(0x4431_E7D7))); // test_drawable
    assert_eq!(
        archetype.texture_dictionary,
        Some(MetaHash(0x94A8_96A3)) // test_textures
    );
    assert_eq!(
        archetype.physics_dictionary,
        Some(MetaHash(0xA18F_1BB9)) // test_collision
    );
    assert_eq!(archetype.clip_dictionary, Some(MetaHash(0x5D68_DAE9))); // test_clip
    assert_eq!(
        archetype.drawable_dictionary,
        Some(MetaHash(0x4583_EF51)) // test_drawable_dict
    );
    assert_eq!(archetype.mlo_entity_count, 0);
    assert!(archetype.mlo_entity_archetypes.is_empty());
    assert_eq!(archetype.mlo_room_count, 0);
    assert_eq!(archetype.mlo_portal_count, 0);
}

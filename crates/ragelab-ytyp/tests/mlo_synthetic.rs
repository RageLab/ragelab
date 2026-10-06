use ragelab_ytyp::{ArchetypeKind, AssetType, Vec3, Ytyp};

#[test]
fn parses_synthetic_mlo_topology_and_provenance() {
    let bytes = include_bytes!("../../../fixtures/synthetic/mlo.ytyp");
    let ytyp = Ytyp::from_bytes(bytes).expect("synthetic MLO YTYP should parse");

    assert_eq!(ytyp.archetypes.len(), 1);
    let archetype = &ytyp.archetypes[0];
    assert_eq!(archetype.kind, ArchetypeKind::Mlo);
    assert_eq!(archetype.asset_type, AssetType::Assetless);
    assert_eq!(archetype.mlo_entity_count, 1);
    assert_eq!(archetype.mlo_room_count, 2);
    assert_eq!(archetype.mlo_portal_count, 1);
    assert_eq!(archetype.mlo_entity_archetypes.len(), 1);

    assert_eq!(ytyp.mlos.len(), 1);
    let mlo = &ytyp.mlos[0];
    assert_eq!(mlo.archetype_name, archetype.name);
    assert_eq!(
        mlo.bounds_min,
        Vec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }
    );
    assert_eq!(
        mlo.bounds_max,
        Vec3 {
            x: 9.0,
            y: 3.0,
            z: 2.5,
        }
    );

    assert_eq!(mlo.entities.len(), 1);
    assert_eq!(mlo.entities[0].index, 0);
    assert_eq!(
        mlo.entities[0].archetype_name,
        archetype.mlo_entity_archetypes[0]
    );
    assert_eq!(
        mlo.entities[0].position,
        Vec3 {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        }
    );
    assert_eq!(mlo.entities[0].rotation.w, 1.0);
    assert_eq!(mlo.entities[0].scale_xy, Some(1.0));
    assert_eq!(mlo.entities[0].scale_z, Some(1.0));
    assert_eq!(mlo.entities[0].parent_index, None);

    assert_eq!(mlo.rooms.len(), 2);
    assert_eq!(mlo.rooms[0].index, 0);
    assert_eq!(mlo.rooms[0].name, "kitchen");
    assert_eq!(mlo.rooms[0].flags, 1);
    assert_eq!(mlo.rooms[0].floor_id, 0);
    assert!(mlo.rooms[0].attached_objects.is_empty());
    assert_eq!(mlo.rooms[1].index, 1);
    assert_eq!(mlo.rooms[1].name, "hall");
    assert_eq!(mlo.rooms[1].flags, 2);
    assert_eq!(mlo.rooms[1].floor_id, 1);
    assert_eq!(mlo.rooms[1].attached_objects, vec![0]);

    assert_eq!(mlo.portals.len(), 1);
    let portal = &mlo.portals[0];
    assert_eq!(portal.index, 0);
    assert_eq!((portal.room_from, portal.room_to), (1, 2));
    assert_eq!(portal.flags, 4);
    assert_eq!(portal.mirror_priority, 3);
    assert_eq!(portal.opacity, 128);
    assert_eq!(portal.audio_occlusion, 7);
    assert!(!portal.is_exterior());
    assert_eq!(portal.corners.len(), 4);
    assert_eq!(
        portal.corners[2],
        Vec3 {
            x: 4.0,
            y: 3.0,
            z: 2.5,
        }
    );

    assert_eq!(mlo.entity_sets.len(), 1);
    let set = &mlo.entity_sets[0];
    assert_eq!(set.index, 0);
    assert_eq!(set.name.0, 0xABCD);
    assert_eq!(set.locations, vec![1]);
    assert_eq!(set.entities.len(), 1);
    assert_eq!(set.entities[0].position, mlo.entities[0].position);
}

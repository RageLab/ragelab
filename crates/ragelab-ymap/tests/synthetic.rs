use ragelab_ymap::Ymap;

const SIMPLE_YMAP: &[u8] = include_bytes!("../../../fixtures/synthetic/simple.ymap");

#[test]
fn parses_synthetic_ymap_end_to_end() {
    let ymap = Ymap::from_bytes(SIMPLE_YMAP).expect("synthetic YMAP should parse");

    assert_eq!(ymap.name.expect("name").0, 0x6C3A_E62A); // simple_map
    assert_eq!(ymap.parent, None);
    assert_eq!(ymap.physics_dictionaries.len(), 1);
    assert_eq!(ymap.physics_dictionaries[0].0, 0xA18F_1BB9); // test_collision

    assert_eq!(ymap.entities.len(), 1);
    let entity = &ymap.entities[0];
    assert_eq!(entity.archetype_name.0, 0xEF3D_BDA5); // test_archetype
    assert_eq!(entity.flags, 0x55AA_55AA);
    assert_eq!(entity.parent_index, None);
    assert_eq!(
        (entity.position.x, entity.position.y, entity.position.z),
        (1.0, 2.0, 3.0)
    );
    assert_eq!(
        (
            entity.rotation.x,
            entity.rotation.y,
            entity.rotation.z,
            entity.rotation.w,
        ),
        (0.0, 0.0, 0.0, 1.0)
    );
}

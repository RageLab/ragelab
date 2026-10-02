use std::path::PathBuf;

use ragelab_assets::{AssetKind, DependencyReason, WorkspaceIndex};
use ragelab_hash::joaat;

#[test]
fn resolves_synthetic_map_dependencies() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic/stream");
    let index = WorkspaceIndex::scan(&root).expect("synthetic stream should index");
    let report = index
        .resolve_map("simple.ymap")
        .expect("synthetic map should resolve");

    assert!(report.unresolved.is_empty(), "{:#?}", report.unresolved);
    assert_eq!(report.files.len(), 9);
    assert_eq!(report.texture_parents.len(), 2);

    let kinds: Vec<AssetKind> = report.files.values().map(|file| file.kind).collect();
    for expected in [
        AssetKind::Ymap,
        AssetKind::Ytyp,
        AssetKind::Ydr,
        AssetKind::Ydd,
        AssetKind::Ytd,
        AssetKind::Ybn,
        AssetKind::Ycd,
    ] {
        assert!(kinds.contains(&expected), "missing {expected:?}");
    }

    let collision = report
        .files
        .values()
        .find(|file| file.kind == AssetKind::Ybn)
        .expect("collision should resolve once");
    assert!(collision
        .reasons
        .iter()
        .any(|reason| matches!(reason, DependencyReason::YmapPhysicsDictionary { .. })));
    assert!(collision
        .reasons
        .iter()
        .any(|reason| matches!(reason, DependencyReason::PhysicsDictionary { .. })));

    let parent_texture = report
        .files
        .values()
        .find(|file| {
            file.path.file_name().and_then(|name| name.to_str()) == Some("test_textures_parent.ytd")
        })
        .expect("parent texture dictionary should resolve");
    assert!(parent_texture
        .reasons
        .iter()
        .any(|reason| matches!(reason, DependencyReason::ParentTextureDictionary { .. })));
}

#[test]
fn selected_parent_closes_over_structural_child_ymaps() {
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic/ymap-relations");
    let index = WorkspaceIndex::scan(&root).expect("relation fixtures should index");
    let report = index
        .resolve_map("relation_parent.ymap")
        .expect("parent map should resolve");

    let ymap_names = report
        .files
        .values()
        .filter(|file| file.kind == AssetKind::Ymap)
        .filter_map(|file| file.path.file_name().and_then(|name| name.to_str()))
        .collect::<Vec<_>>();
    assert_eq!(ymap_names.len(), 4, "{ymap_names:#?}");
    for expected in [
        "relation_parent.ymap",
        "relation_child_a.ymap",
        "relation_child_b.ymap",
        "relation_grandchild.ymap",
    ] {
        assert!(ymap_names.contains(&expected), "missing {expected}");
    }

    let child_a = report
        .files
        .values()
        .find(|file| {
            file.path.file_name().and_then(|name| name.to_str()) == Some("relation_child_a.ymap")
        })
        .expect("child A should be in reverse-parent closure");
    assert!(child_a.reasons.contains(&DependencyReason::ChildYmap {
        parent_hash: joaat("relation_parent"),
    }));

    let grandchild = report
        .files
        .values()
        .find(|file| {
            file.path.file_name().and_then(|name| name.to_str()) == Some("relation_grandchild.ymap")
        })
        .expect("grandchild should be in recursive reverse-parent closure");
    assert!(grandchild.reasons.contains(&DependencyReason::ChildYmap {
        parent_hash: joaat("relation_child_a"),
    }));
}

#[test]
fn canonical_selected_path_deduplicates_against_indexed_ymap() {
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic/ymap-relations");
    let index = WorkspaceIndex::scan(&root).expect("relation fixtures should index");
    let selected = std::fs::canonicalize(root.join("relation_parent.ymap"))
        .expect("fixture path should canonicalize");
    let report = index
        .resolve_map(&selected)
        .expect("canonical parent path should resolve");

    let ymaps = report
        .files
        .values()
        .filter(|file| file.kind == AssetKind::Ymap)
        .collect::<Vec<_>>();
    assert_eq!(
        ymaps.len(),
        4,
        "canonical path must not duplicate the selected parent"
    );

    let parent = ymaps
        .into_iter()
        .find(|file| {
            file.path.file_name().and_then(|name| name.to_str()) == Some("relation_parent.ymap")
        })
        .expect("parent should appear exactly once");
    assert!(parent.reasons.contains(&DependencyReason::SelectedYmap));
    assert!(parent
        .reasons
        .iter()
        .any(|reason| matches!(reason, DependencyReason::ParentYmap { .. })));
}

#[test]
fn selected_child_keeps_ancestor_without_expanding_siblings() {
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic/ymap-relations");
    let index = WorkspaceIndex::scan(&root).expect("relation fixtures should index");
    let report = index
        .resolve_map("relation_child_a.ymap")
        .expect("child map should resolve");

    let ymap_names = report
        .files
        .values()
        .filter(|file| file.kind == AssetKind::Ymap)
        .filter_map(|file| file.path.file_name().and_then(|name| name.to_str()))
        .collect::<Vec<_>>();
    assert_eq!(ymap_names.len(), 3, "{ymap_names:#?}");
    assert!(ymap_names.contains(&"relation_parent.ymap"));
    assert!(ymap_names.contains(&"relation_child_a.ymap"));
    assert!(ymap_names.contains(&"relation_grandchild.ymap"));
    assert!(!ymap_names.contains(&"relation_child_b.ymap"));

    let parent = report
        .files
        .values()
        .find(|file| {
            file.path.file_name().and_then(|name| name.to_str()) == Some("relation_parent.ymap")
        })
        .expect("parent should still be included");
    assert!(parent.reasons.contains(&DependencyReason::ParentYmap {
        child_hash: joaat("relation_child_a"),
    }));
}

#[test]
fn exports_synthetic_resource_with_fresh_manifest() {
    use std::{
        fs, process,
        time::{SystemTime, UNIX_EPOCH},
    };

    use ragelab_assets::ExportOptions;
    use ragelab_ymf::Ymf;

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic/stream");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let output =
        std::env::temp_dir().join(format!("ragelab-export-test-{}-{nonce}", process::id()));

    let index = WorkspaceIndex::scan(&root).expect("synthetic stream should index");
    let result = index
        .export_map(
            "simple.ymap",
            &output,
            ExportOptions {
                allow_unresolved: false,
                overwrite: true,
            },
        )
        .expect("synthetic resource should export");

    assert_eq!(result.copied_files.len(), 9);
    assert!(output.join("fxmanifest.lua").is_file());
    assert!(output.join("stream/simple.ymap").is_file());
    assert!(output.join("stream/test_textures_parent.ytd").is_file());
    assert!(output
        .join("stream/test_textures_grandparent.ytd")
        .is_file());
    assert!(output.join("data/gtxd.meta").is_file());
    assert!(result.gtxd_path.is_some());
    assert!(result.manifest_path.is_file());

    let fxmanifest =
        fs::read_to_string(output.join("fxmanifest.lua")).expect("fxmanifest should be readable");
    assert!(fxmanifest.contains("GTXD_PARENTING_DATA"));

    let manifest_bytes = fs::read(&result.manifest_path).expect("manifest should be readable");
    let manifest = Ymf::from_bytes(&manifest_bytes).expect("generated manifest should parse");
    assert_eq!(manifest.maps.len(), 1);
    assert_eq!(manifest.maps[0].ymap.0, 0x6C3A_E62A);
    assert_eq!(manifest.maps[0].ytyps.len(), 1);
    assert_eq!(manifest.maps[0].ytyps[0].0, 0xAA2B_639B);

    fs::remove_dir_all(&output).expect("test output should clean up");
}

#[test]
fn exports_combined_resource_and_deduplicates_shared_dependencies() {
    use std::{
        fs, process,
        time::{SystemTime, UNIX_EPOCH},
    };

    use ragelab_assets::ExportOptions;
    use ragelab_ymf::Ymf;

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic/stream");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let output = std::env::temp_dir().join(format!(
        "ragelab-combined-export-test-{}-{nonce}",
        process::id()
    ));

    let index = WorkspaceIndex::scan(&root).expect("synthetic stream should index");
    let selected = vec![PathBuf::from("simple.ymap"), PathBuf::from("simple.ymap")];
    let result = index
        .export_maps(
            &selected,
            &output,
            ExportOptions {
                allow_unresolved: false,
                overwrite: true,
            },
        )
        .expect("combined synthetic resource should export");

    assert_eq!(
        result.copied_files.len(),
        9,
        "duplicate selections must not duplicate files"
    );
    assert_eq!(result.report.files.len(), 9);
    assert!(output.join("fxmanifest.lua").is_file());
    assert!(output.join("stream/simple.ymap").is_file());
    assert!(output.join("data/gtxd.meta").is_file());

    let manifest_bytes = fs::read(&result.manifest_path).expect("manifest should be readable");
    let manifest = Ymf::from_bytes(&manifest_bytes).expect("generated manifest should parse");
    assert_eq!(
        manifest.maps.len(),
        1,
        "duplicate map inputs must normalize in the YMF"
    );

    fs::remove_dir_all(&output).expect("test output should clean up");
}

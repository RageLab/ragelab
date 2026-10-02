use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use ragelab_assets::{TextureParentRelationship, WorkspaceIndex};

const CHILD: &str = "test_textures";
const PARENT: &str = "test_textures_parent";
const GRANDPARENT: &str = "test_textures_grandparent";

struct TempWorkspace {
    path: PathBuf,
}

impl TempWorkspace {
    fn synthetic(name: &str) -> Self {
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic/stream");
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("ragelab-gtxd-rbf-{name}-{}-{nonce}", process::id()));
        fs::create_dir_all(&path).expect("create temp fixture root");
        for entry in fs::read_dir(&source).expect("read synthetic stream") {
            let entry = entry.expect("fixture entry");
            if entry.file_type().expect("fixture type").is_file() {
                fs::copy(entry.path(), path.join(entry.file_name())).expect("copy fixture file");
            }
        }
        fs::remove_file(path.join("gtxd.meta")).expect("remove XML GTXD fixture");
        Self { path }
    }

    fn write_gtxd(&self, relationships: &[(&str, &str)]) {
        fs::write(self.path.join("gtxd.ymt"), build_gtxd_rbf(relationships))
            .expect("write synthetic RBF GTXD");
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Default)]
struct RbfBuilder {
    bytes: Vec<u8>,
    descriptors: BTreeMap<String, u8>,
}

impl RbfBuilder {
    fn new() -> Self {
        Self {
            bytes: b"RBF0".to_vec(),
            descriptors: BTreeMap::new(),
        }
    }

    fn record(&mut self, name: &str, data_type: u8) {
        if let Some(index) = self.descriptors.get(name).copied() {
            self.bytes.extend_from_slice(&[index, data_type]);
            return;
        }
        let index = u8::try_from(self.descriptors.len()).expect("small synthetic descriptor table");
        self.descriptors.insert(name.to_string(), index);
        self.bytes.extend_from_slice(&[index, data_type]);
        self.bytes.extend_from_slice(
            &i16::try_from(name.len())
                .expect("small synthetic descriptor name")
                .to_le_bytes(),
        );
        self.bytes.extend_from_slice(name.as_bytes());
    }

    fn open(&mut self, name: &str) {
        self.record(name, 0x00);
        self.bytes.extend_from_slice(&0_i16.to_le_bytes());
        self.bytes.extend_from_slice(&0_i16.to_le_bytes());
        self.bytes.extend_from_slice(&0_i16.to_le_bytes());
    }

    fn bytes(&mut self, value: &[u8]) {
        self.bytes.extend_from_slice(&[0xFD, 0xFF]);
        self.bytes.extend_from_slice(
            &i32::try_from(value.len())
                .expect("small synthetic byte value")
                .to_le_bytes(),
        );
        self.bytes.extend_from_slice(value);
    }

    fn close(&mut self) {
        self.bytes.extend_from_slice(&[0xFF, 0xFF]);
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

fn build_gtxd_rbf(relationships: &[(&str, &str)]) -> Vec<u8> {
    let mut builder = RbfBuilder::new();
    builder.open("CMapParentTxds");
    builder.open("txdRelationships");
    for (parent, child) in relationships {
        builder.open("item");
        builder.open("parent");
        let mut parent_bytes = parent.as_bytes().to_vec();
        parent_bytes.push(0);
        builder.bytes(&parent_bytes);
        builder.close();
        builder.open("child");
        let mut child_bytes = child.as_bytes().to_vec();
        child_bytes.push(0);
        builder.bytes(&child_bytes);
        builder.close();
        builder.close();
    }
    builder.close();
    builder.close();
    builder.finish()
}

fn relationship(parent: &str, child: &str) -> TextureParentRelationship {
    TextureParentRelationship {
        parent: parent.to_string(),
        child: child.to_string(),
    }
}

#[test]
fn resolves_two_hop_parent_chain_from_binary_gtxd() {
    let workspace = TempWorkspace::synthetic("two-hop");
    workspace.write_gtxd(&[(PARENT, CHILD), (GRANDPARENT, PARENT)]);

    let index = WorkspaceIndex::scan(workspace.path()).expect("index RBF workspace");
    assert!(
        index
            .warnings
            .iter()
            .all(|warning| !warning.contains("GTXD RBF parse failed")),
        "{:#?}",
        index.warnings
    );
    let report = index.resolve_map("simple.ymap").expect("resolve map");
    assert!(report.unresolved.is_empty(), "{:#?}", report.unresolved);
    assert_eq!(report.texture_parents.len(), 2);
    assert!(report
        .texture_parents
        .contains(&relationship(PARENT, CHILD)));
    assert!(report
        .texture_parents
        .contains(&relationship(GRANDPARENT, PARENT)));
    assert!(report.files.keys().any(|path| {
        path.file_name().and_then(|name| name.to_str()) == Some("test_textures_parent.ytd")
    }));
    assert!(report.files.keys().any(|path| {
        path.file_name().and_then(|name| name.to_str()) == Some("test_textures_grandparent.ytd")
    }));
}

#[test]
fn duplicate_binary_relationship_is_deduplicated() {
    let workspace = TempWorkspace::synthetic("duplicate");
    workspace.write_gtxd(&[(PARENT, CHILD), (PARENT, CHILD)]);

    let index = WorkspaceIndex::scan(workspace.path()).expect("index RBF workspace");
    assert!(
        index
            .warnings
            .iter()
            .all(|warning| !warning.contains("multiple parents")),
        "{:#?}",
        index.warnings
    );
    let report = index.resolve_map("simple.ymap").expect("resolve map");
    assert_eq!(report.texture_parents.len(), 1);
    assert!(report
        .texture_parents
        .contains(&relationship(PARENT, CHILD)));
}

#[test]
fn conflicting_binary_relationship_keeps_first_parent_and_warns() {
    let workspace = TempWorkspace::synthetic("conflict");
    workspace.write_gtxd(&[(PARENT, CHILD), (GRANDPARENT, CHILD)]);

    let index = WorkspaceIndex::scan(workspace.path()).expect("index RBF workspace");
    assert!(
        index
            .warnings
            .iter()
            .any(|warning| warning.contains("multiple parents")),
        "{:#?}",
        index.warnings
    );
    let report = index.resolve_map("simple.ymap").expect("resolve map");
    assert_eq!(report.texture_parents.len(), 1);
    assert!(report
        .texture_parents
        .contains(&relationship(PARENT, CHILD)));
    assert!(!report
        .texture_parents
        .contains(&relationship(GRANDPARENT, CHILD)));
}

#[test]
fn binary_parent_cycle_is_reported_without_recursing_forever() {
    let workspace = TempWorkspace::synthetic("cycle");
    workspace.write_gtxd(&[(PARENT, CHILD), (CHILD, PARENT)]);

    let index = WorkspaceIndex::scan(workspace.path()).expect("index RBF workspace");
    let report = index.resolve_map("simple.ymap").expect("resolve map");
    assert_eq!(report.texture_parents.len(), 2);
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("GTXD parent cycle detected")),
        "{:#?}",
        report.warnings
    );
}

#[test]
fn truncated_binary_gtxd_warns_and_is_not_invented_as_xml() {
    let workspace = TempWorkspace::synthetic("truncated");
    let mut bytes = build_gtxd_rbf(&[(PARENT, CHILD)]);
    bytes.truncate(bytes.len() - 3);
    fs::write(workspace.path().join("gtxd.ymt"), bytes).expect("write truncated fixture");

    let index = WorkspaceIndex::scan(workspace.path()).expect("index malformed workspace");
    assert!(
        index
            .warnings
            .iter()
            .any(|warning| warning.contains("GTXD RBF parse failed")),
        "{:#?}",
        index.warnings
    );
    let report = index.resolve_map("simple.ymap").expect("resolve map");
    assert!(report.texture_parents.is_empty());
}

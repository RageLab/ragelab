use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use ragelab_assets::AssetKind;
use serde::Serialize;

use crate::{
    discovery::clean_display_path, parse_vanilla_file_catalog, validate_gta_v_installation,
    GtaVInstallation,
};

pub const GTA_CATALOG_SCHEMA: &str = "ragelab.gta.catalog";
pub const GTA_CATALOG_SCHEMA_VERSION: u64 = 1;
const MAX_RPF_EXAMPLES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum VanillaCatalogSourceKind {
    FilesystemTree,
    GtaLegacyInstallation,
}

impl VanillaCatalogSourceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FilesystemTree => "filesystemTree",
            Self::GtaLegacyInstallation => "gtaLegacyInstallation",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum VanillaCatalogCoverage {
    FilesystemTreeComplete,
    PartialRpfBoundary,
}

impl VanillaCatalogCoverage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FilesystemTreeComplete => "filesystemTreeComplete",
            Self::PartialRpfBoundary => "partialRpfBoundary",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VanillaCatalogArchiveBoundary {
    pub rpf_archives_found: usize,
    pub enumeration_supported: bool,
    pub examples: Vec<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VanillaCatalogBuildReport {
    pub schema: &'static str,
    pub schema_version: u64,
    pub root: PathBuf,
    pub source_kind: VanillaCatalogSourceKind,
    pub coverage: VanillaCatalogCoverage,
    pub complete: bool,
    pub scanned_files: usize,
    pub supported_loose_files: usize,
    pub path_entries: usize,
    pub unique_catalog_entries: usize,
    pub ignored_files: usize,
    pub archive_boundary: VanillaCatalogArchiveBoundary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gta_installation: Option<GtaVInstallation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VanillaCatalogBuild {
    pub report: VanillaCatalogBuildReport,
    pub paths: Vec<String>,
    pub files: BTreeMap<(AssetKind, u32), String>,
}

pub fn build_vanilla_catalog(root: &Path) -> Result<VanillaCatalogBuild, io::Error> {
    if !root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a directory: {}", root.display()),
        ));
    }

    let root = fs::canonicalize(root)
        .map(clean_display_path)
        .unwrap_or_else(|_| clean_display_path(root.to_path_buf()));
    let gta = validate_gta_v_installation(&root);
    let source_kind = if gta.valid {
        VanillaCatalogSourceKind::GtaLegacyInstallation
    } else {
        VanillaCatalogSourceKind::FilesystemTree
    };

    let mut stack = vec![root.clone()];
    let mut paths = Vec::<String>::new();
    let mut scanned_files = 0_usize;
    let mut supported_loose_files = 0_usize;
    let mut rpf_archives_found = 0_usize;
    let mut rpf_examples = Vec::<PathBuf>::new();

    while let Some(directory) = stack.pop() {
        let mut entries = fs::read_dir(&directory)?.collect::<Result<Vec<_>, io::Error>>()?;
        entries.sort_by_key(|entry| entry.file_name());

        for entry in entries {
            let file_type = entry.file_type()?;
            let path = entry.path();

            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }

            scanned_files += 1;
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("");

            if extension.eq_ignore_ascii_case("rpf") {
                rpf_archives_found += 1;
                if rpf_examples.len() < MAX_RPF_EXAMPLES {
                    rpf_examples.push(relative_display_path(&root, &path));
                }
                continue;
            }

            if AssetKind::from_extension(extension).is_none() {
                continue;
            }

            supported_loose_files += 1;
            paths.push(
                path.strip_prefix(&root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }

    paths.sort();
    paths.dedup();

    let body = render_vanilla_catalog_paths(&paths);
    let files = parse_vanilla_file_catalog(&body);
    let coverage = if rpf_archives_found == 0 {
        VanillaCatalogCoverage::FilesystemTreeComplete
    } else {
        VanillaCatalogCoverage::PartialRpfBoundary
    };
    let complete = rpf_archives_found == 0;
    let ignored_files = scanned_files
        .saturating_sub(supported_loose_files)
        .saturating_sub(rpf_archives_found);

    Ok(VanillaCatalogBuild {
        report: VanillaCatalogBuildReport {
            schema: GTA_CATALOG_SCHEMA,
            schema_version: GTA_CATALOG_SCHEMA_VERSION,
            root,
            source_kind,
            coverage,
            complete,
            scanned_files,
            supported_loose_files,
            path_entries: paths.len(),
            unique_catalog_entries: files.len(),
            ignored_files,
            archive_boundary: VanillaCatalogArchiveBoundary {
                rpf_archives_found,
                enumeration_supported: false,
                examples: rpf_examples,
                reason: (rpf_archives_found > 0).then_some(
                    "RageLab does not enumerate raw/encrypted RPF archive contents in the current Legacy catalog scope",
                ),
            },
            gta_installation: gta.valid.then_some(gta),
        },
        paths,
        files,
    })
}

pub fn render_vanilla_catalog_paths(paths: &[String]) -> String {
    if paths.is_empty() {
        String::new()
    } else {
        let mut body = paths.join("\n");
        body.push('\n');
        body
    }
}

fn relative_display_path(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("ragelab-catalog-{label}-{}-{nonce}", process::id()))
    }

    fn touch(root: &Path, relative: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, []).unwrap();
    }

    fn create_legacy_fixture(root: &Path) {
        for marker in [
            "GTA5.exe",
            "PlayGTAV.exe",
            "common.rpf",
            "x64a.rpf",
            "update/update.rpf",
        ] {
            touch(root, marker);
        }
    }

    #[test]
    fn builds_deterministic_catalog_from_loose_filesystem_tree() {
        let root = temp_root("loose");
        touch(&root, "stream/z_asset.ytd");
        touch(&root, "stream/a_asset.ydr");
        touch(&root, "stream/readme.txt");

        let build = build_vanilla_catalog(&root).unwrap();

        assert_eq!(
            build.report.source_kind,
            VanillaCatalogSourceKind::FilesystemTree
        );
        assert_eq!(
            build.report.coverage,
            VanillaCatalogCoverage::FilesystemTreeComplete
        );
        assert!(build.report.complete);
        assert_eq!(build.report.scanned_files, 3);
        assert_eq!(build.report.supported_loose_files, 2);
        assert_eq!(build.report.path_entries, 2);
        assert_eq!(build.report.unique_catalog_entries, 2);
        assert_eq!(build.report.archive_boundary.rpf_archives_found, 0);
        assert_eq!(
            build.paths,
            vec![
                "stream/a_asset.ydr".to_string(),
                "stream/z_asset.ytd".to_string()
            ]
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reports_explicit_rpf_boundary_for_valid_legacy_installation() {
        let root = temp_root("legacy");
        create_legacy_fixture(&root);
        touch(&root, "mods/loose_prop.ydr");
        touch(&root, "packs/extra.rpf");

        let build = build_vanilla_catalog(&root).unwrap();

        assert_eq!(
            build.report.source_kind,
            VanillaCatalogSourceKind::GtaLegacyInstallation
        );
        assert_eq!(
            build.report.coverage,
            VanillaCatalogCoverage::PartialRpfBoundary
        );
        assert!(!build.report.complete);
        assert_eq!(build.report.supported_loose_files, 1);
        assert_eq!(build.report.path_entries, 1);
        assert_eq!(build.report.archive_boundary.rpf_archives_found, 4);
        assert!(!build.report.archive_boundary.enumeration_supported);
        assert!(build.report.archive_boundary.reason.is_some());
        assert!(build
            .report
            .gta_installation
            .as_ref()
            .is_some_and(|installation| installation.valid));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catalog_entry_count_distinguishes_paths_from_hash_identity() {
        let root = temp_root("duplicates");
        touch(&root, "a/same_name.ydr");
        touch(&root, "b/same_name.ydr");

        let build = build_vanilla_catalog(&root).unwrap();

        assert_eq!(build.report.path_entries, 2);
        assert_eq!(build.report.unique_catalog_entries, 1);

        fs::remove_dir_all(root).unwrap();
    }
}

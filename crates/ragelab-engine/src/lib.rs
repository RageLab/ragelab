//! Shared application-layer policy for RageLab adapters.
//!
//! This crate intentionally stays transport-neutral: no Axum, no browser DTOs,
//! and no process-global workspace state.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use ragelab_assets::{
    AssetKind, DependencyReport, ExportOptions, ExportResult, MloArchetypeAudit,
    UnresolvedDependency, UnresolvedKind, WorkspaceError, WorkspaceIndex,
};
use ragelab_hash::joaat;
use ragelab_ymf::{ManifestFlags, Ymf};
use serde::{Deserialize, Serialize};

mod asset;
mod catalog;
mod discovery;
mod operations;
mod preview;
mod rpf;
mod scene;
mod spatial;
pub use asset::*;
pub use catalog::*;
pub use discovery::*;
pub use operations::*;
pub use preview::*;
pub use rpf::*;
pub use scene::*;
pub use spatial::*;

pub const DURTYFREE_OBJECT_LIST_URL: &str =
    "https://raw.githubusercontent.com/DurtyFree/gta-v-data-dumps/master/ObjectList.ini";
pub const DURTYFREE_REPOSITORY_URL: &str = "https://github.com/DurtyFree/gta-v-data-dumps";

#[derive(Debug, Clone)]
pub struct DurtyFreeCatalog {
    pub objects: BTreeMap<u32, String>,
    pub synced_at: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct VanillaFileCatalog {
    pub files: BTreeMap<(AssetKind, u32), String>,
    pub synced_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CatalogRefs<'a> {
    pub durtyfree: Option<&'a DurtyFreeCatalog>,
    pub file_catalog: Option<&'a VanillaFileCatalog>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogMatch {
    pub name: String,
    pub source: &'static str,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UnresolvedSummary {
    pub raw: usize,
    pub vanilla: usize,
    pub unknown: usize,
    pub catalog_used: bool,
    pub durtyfree_used: bool,
    pub file_catalog_used: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownDependencyGroup {
    pub kind: String,
    pub hash: u32,
    pub uses: usize,
    pub reasons: Vec<String>,
    pub affected_maps: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MloRisk {
    Ok,
    Warning,
}

impl MloRisk {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "warning",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MloAuditSummary {
    pub archetype_hash: u32,
    pub ytyp_path: PathBuf,
    pub entities: usize,
    pub unique_entity_archetypes: usize,
    pub rooms: usize,
    pub portals: usize,
    pub local_files: usize,
    pub vanilla: usize,
    pub unknown: usize,
    pub risk: MloRisk,
}

pub fn parse_durtyfree_object_list(input: &str) -> BTreeMap<u32, String> {
    let mut objects = BTreeMap::new();
    for raw_line in input.lines() {
        let line = raw_line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let name = line
            .split_once('=')
            .map(|(name, _)| name)
            .unwrap_or(line)
            .trim();
        if name.is_empty() || !name.is_ascii() {
            continue;
        }
        objects
            .entry(joaat(name))
            .or_insert_with(|| name.to_string());
    }
    objects
}

pub fn parse_vanilla_file_catalog(input: &str) -> BTreeMap<(AssetKind, u32), String> {
    let mut files = BTreeMap::new();
    for raw_line in input.lines() {
        let line = raw_line
            .trim()
            .trim_matches('"')
            .trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let normalized = line.replace('\\', "/");
        let path = Path::new(&normalized);
        let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };
        let Some(kind) = AssetKind::from_extension(extension) else {
            continue;
        };
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        if stem.is_empty() {
            continue;
        }
        files.entry((kind, joaat(stem))).or_insert(normalized);
    }
    files
}

pub fn load_durtyfree_catalog(root: &Path) -> Result<Option<DurtyFreeCatalog>, std::io::Error> {
    let directory = root.join("durtyfree");
    let object_list = directory.join("ObjectList.ini");
    if !object_list.is_file() {
        return Ok(None);
    }
    let body = fs::read_to_string(object_list)?;
    let objects = parse_durtyfree_object_list(&body);
    if objects.len() < 1_000 {
        return Ok(None);
    }
    let synced_at = fs::read_to_string(directory.join("synced-at.txt"))
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok());
    Ok(Some(DurtyFreeCatalog { objects, synced_at }))
}

pub fn load_vanilla_file_catalog(
    root: &Path,
) -> Result<Option<VanillaFileCatalog>, std::io::Error> {
    let directory = root.join("files");
    let path_list = directory.join("paths.txt");
    if !path_list.is_file() {
        return Ok(None);
    }
    let body = fs::read_to_string(path_list)?;
    let files = parse_vanilla_file_catalog(&body);
    if files.is_empty() {
        return Ok(None);
    }
    let synced_at = fs::read_to_string(directory.join("synced-at.txt"))
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok());
    Ok(Some(VanillaFileCatalog { files, synced_at }))
}

pub fn catalog_match(
    unresolved: &UnresolvedDependency,
    durtyfree: Option<&DurtyFreeCatalog>,
    file_catalog: Option<&VanillaFileCatalog>,
) -> Option<CatalogMatch> {
    if matches!(unresolved.kind, UnresolvedKind::ArchetypeProvider) {
        if let Some(name) = durtyfree.and_then(|catalog| catalog.objects.get(&unresolved.hash)) {
            return Some(CatalogMatch {
                name: name.clone(),
                source: "DurtyFree ObjectList.ini",
            });
        }
    }

    if let Some(catalog) = file_catalog {
        let match_path = match unresolved.kind {
            UnresolvedKind::File(kind) => catalog.files.get(&(kind, unresolved.hash)),
            UnresolvedKind::PrimaryAssetUnknownType => {
                [AssetKind::Ydr, AssetKind::Ydd, AssetKind::Yft]
                    .into_iter()
                    .find_map(|kind| catalog.files.get(&(kind, unresolved.hash)))
            }
            UnresolvedKind::ArchetypeProvider => None,
        };
        if let Some(path) = match_path {
            return Some(CatalogMatch {
                name: path.clone(),
                source: "GTA vanilla file index",
            });
        }
    }
    None
}

pub fn classify_unresolved(
    report: &DependencyReport,
    durtyfree: Option<&DurtyFreeCatalog>,
    file_catalog: Option<&VanillaFileCatalog>,
) -> UnresolvedSummary {
    let raw = report.unresolved.len();
    let vanilla = report
        .unresolved
        .iter()
        .filter(|unresolved| catalog_match(unresolved, durtyfree, file_catalog).is_some())
        .count();
    UnresolvedSummary {
        raw,
        vanilla,
        unknown: raw.saturating_sub(vanilla),
        catalog_used: durtyfree.is_some() || file_catalog.is_some(),
        durtyfree_used: durtyfree.is_some(),
        file_catalog_used: file_catalog.is_some(),
    }
}

/// Returns the user-facing export gate error text, or `None` when export is allowed.
pub fn export_gate_error(unresolved: UnresolvedSummary, allow_unresolved: bool) -> Option<String> {
    if allow_unresolved || unresolved.unknown == 0 {
        return None;
    }

    if unresolved.catalog_used && unresolved.vanilla > 0 {
        return Some(format!(
            "{} unknown external dependencies remain after ignoring {} catalog-confirmed vanilla references; enable Allow unresolved to export",
            unresolved.unknown, unresolved.vanilla
        ));
    }

    Some(format!(
        "{} unresolved dependencies remain; enable Allow unresolved to export",
        unresolved.unknown
    ))
}

pub fn unresolved_kind_name(kind: &UnresolvedKind) -> &'static str {
    match kind {
        UnresolvedKind::File(kind) => kind.extension(),
        UnresolvedKind::ArchetypeProvider => "ytyp",
        // Preserve the public/API convention used before the engine extraction.
        UnresolvedKind::PrimaryAssetUnknownType => "ydr",
    }
}

pub fn group_unknown_dependencies(
    reports: &[(String, DependencyReport)],
    durtyfree: Option<&DurtyFreeCatalog>,
    file_catalog: Option<&VanillaFileCatalog>,
) -> Vec<UnknownDependencyGroup> {
    let mut grouped = BTreeMap::<(String, u32), (usize, BTreeSet<String>, BTreeSet<String>)>::new();
    for (map, report) in reports {
        for unresolved in &report.unresolved {
            if catalog_match(unresolved, durtyfree, file_catalog).is_some() {
                continue;
            }
            let kind = unresolved_kind_name(&unresolved.kind).to_string();
            let entry = grouped
                .entry((kind, unresolved.hash))
                .or_insert_with(|| (0, BTreeSet::new(), BTreeSet::new()));
            entry.0 += 1;
            entry.1.insert(unresolved.reason.to_string());
            entry.2.insert(map.clone());
        }
    }
    grouped
        .into_iter()
        .map(
            |((kind, hash), (uses, reasons, affected_maps))| UnknownDependencyGroup {
                kind,
                hash,
                uses,
                reasons: reasons.into_iter().collect(),
                affected_maps: affected_maps.into_iter().collect(),
            },
        )
        .collect()
}

pub fn summarize_mlo_audit(
    audit: MloArchetypeAudit,
    durtyfree: Option<&DurtyFreeCatalog>,
    file_catalog: Option<&VanillaFileCatalog>,
) -> MloAuditSummary {
    let classified = classify_unresolved(&audit.report, durtyfree, file_catalog);
    MloAuditSummary {
        archetype_hash: audit.archetype_hash,
        ytyp_path: audit.ytyp_path,
        entities: audit.entity_count,
        unique_entity_archetypes: audit.unique_entity_archetypes,
        rooms: audit.room_count,
        portals: audit.portal_count,
        local_files: audit.report.files.len(),
        vanilla: classified.vanilla,
        unknown: classified.unknown,
        risk: if classified.unknown == 0 {
            MloRisk::Ok
        } else {
            MloRisk::Warning
        },
    }
}

#[derive(Debug, Clone)]
pub struct CombinedPreflight {
    pub selected_roots: Vec<String>,
    pub closure_ymaps: Vec<String>,
    pub predicted_files: Vec<String>,
    pub unresolved: UnresolvedSummary,
    pub unknown_groups: Vec<UnknownDependencyGroup>,
    pub mlo_audits: Vec<MloAuditSummary>,
    pub warnings: Vec<String>,
}

pub fn preflight_maps_combined(
    index: &WorkspaceIndex,
    workspace: &Path,
    selected_paths: &[PathBuf],
    durtyfree: Option<&DurtyFreeCatalog>,
    file_catalog: Option<&VanillaFileCatalog>,
) -> Result<CombinedPreflight, WorkspaceError> {
    let report = index.resolve_maps(selected_paths)?;
    let unresolved = classify_unresolved(&report, durtyfree, file_catalog);

    let mut selected_roots = selected_paths
        .iter()
        .map(|path| relative_name(workspace, path))
        .collect::<Vec<_>>();
    selected_roots.sort();
    selected_roots.dedup();

    let mut closure_ymaps = report
        .files
        .values()
        .filter(|file| file.kind == AssetKind::Ymap)
        .map(|file| relative_name(workspace, &file.path))
        .collect::<Vec<_>>();
    closure_ymaps.sort();
    closure_ymaps.dedup();

    let mut predicted_files = report
        .files
        .values()
        .filter_map(|file| {
            file.path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    predicted_files.sort();
    predicted_files.dedup();

    let mut per_map_reports = Vec::new();
    let mut mlo_audits = Vec::new();
    let mut seen_mlos = BTreeSet::<(u32, PathBuf)>::new();
    for path in selected_paths {
        let map_name = relative_name(workspace, path);
        let map_report = index.resolve_map(path)?;
        per_map_reports.push((map_name, map_report));
        for audit in index.audit_map_mlos(path)? {
            let summary = summarize_mlo_audit(audit, durtyfree, file_catalog);
            if seen_mlos.insert((summary.archetype_hash, summary.ytyp_path.clone())) {
                mlo_audits.push(summary);
            }
        }
    }
    mlo_audits.sort_by(|left, right| {
        (left.archetype_hash, &left.ytyp_path).cmp(&(right.archetype_hash, &right.ytyp_path))
    });

    let unknown_groups = group_unknown_dependencies(&per_map_reports, durtyfree, file_catalog);

    Ok(CombinedPreflight {
        selected_roots,
        closure_ymaps,
        predicted_files,
        unresolved,
        unknown_groups,
        mlo_audits,
        warnings: report.warnings,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceExportPreflightReport {
    pub workspace: String,
    pub selected_roots: Vec<String>,
    pub closure_ymaps: Vec<String>,
    pub predicted_files: Vec<String>,
    pub unresolved: UnresolvedSummaryReport,
    pub export_gate: ExportGateReport,
    pub unknown_groups: Vec<UnknownDependencyGroupReport>,
    pub mlo_audits: Vec<MloAuditReport>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvedSummaryReport {
    pub raw: usize,
    pub vanilla: usize,
    pub unknown: usize,
    pub catalog_used: bool,
    pub durtyfree_used: bool,
    pub file_catalog_used: bool,
}

impl From<UnresolvedSummary> for UnresolvedSummaryReport {
    fn from(value: UnresolvedSummary) -> Self {
        Self {
            raw: value.raw,
            vanilla: value.vanilla,
            unknown: value.unknown,
            catalog_used: value.catalog_used,
            durtyfree_used: value.durtyfree_used,
            file_catalog_used: value.file_catalog_used,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportGateReport {
    pub allowed_without_override: bool,
    pub requires_allow_unresolved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnknownDependencyGroupReport {
    pub kind: String,
    pub hash: String,
    pub uses: usize,
    pub reasons: Vec<String>,
    pub affected_maps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MloAuditReport {
    pub archetype_hash: String,
    pub ytyp: String,
    pub entities: usize,
    pub unique_entity_archetypes: usize,
    pub rooms: usize,
    pub portals: usize,
    pub local_files: usize,
    pub vanilla: usize,
    pub unknown: usize,
    pub risk: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceExportReport {
    pub workspace: String,
    pub resource_name: String,
    pub selected_roots: Vec<String>,
    pub output: WorkspaceExportOutputReport,
    pub unresolved: ExportUnresolvedReport,
    pub validation: ExportValidationReport,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceExportOutputReport {
    pub resource: String,
    pub stream: String,
    pub manifest: String,
    pub metadata: String,
    pub gtxd: Option<String>,
    pub copied_files: Vec<String>,
    pub manifest_maps: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportUnresolvedReport {
    pub raw: usize,
    pub vanilla: usize,
    pub unknown: usize,
    pub catalog_used: bool,
    pub durtyfree_used: bool,
    pub file_catalog_used: bool,
    pub allow_unresolved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportValidationReport {
    pub status: String,
    pub valid: bool,
    pub metadata_present: bool,
    pub manifest_valid: bool,
    pub fxmanifest_present: bool,
    pub selected_roots: usize,
    pub closure_maps: usize,
    pub copied_files: usize,
    pub interior_maps: usize,
    pub interior_bounds: usize,
    pub local_missing: Vec<String>,
    pub post_export_raw_unresolved: usize,
    pub post_export_vanilla: usize,
    pub post_export_unknown: usize,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

impl From<&ExportValidation> for ExportValidationReport {
    fn from(value: &ExportValidation) -> Self {
        Self {
            status: value.status.clone(),
            valid: value.valid,
            metadata_present: value.metadata_present,
            manifest_valid: value.manifest_valid,
            fxmanifest_present: value.fxmanifest_present,
            selected_roots: value.selected_roots,
            closure_maps: value.closure_maps,
            copied_files: value.copied_files,
            interior_maps: value.interior_maps,
            interior_bounds: value.interior_bounds,
            local_missing: value.local_missing.clone(),
            post_export_raw_unresolved: value.post_export_raw_unresolved,
            post_export_vanilla: value.post_export_vanilla,
            post_export_unknown: value.post_export_unknown,
            warnings: value.warnings.clone(),
            errors: value.errors.clone(),
        }
    }
}

pub fn workspace_export_preflight_report(
    workspace: &Path,
    maps: &[PathBuf],
    catalogs: CatalogRefs<'_>,
) -> Result<WorkspaceExportPreflightReport, EngineError> {
    let canonical_workspace = canonical_workspace_root(workspace)?;
    let selected_paths = resolve_workspace_maps(&canonical_workspace, maps)?;
    let index = WorkspaceIndex::scan(&canonical_workspace)?;
    let combined = preflight_maps_combined(
        &index,
        &canonical_workspace,
        &selected_paths,
        catalogs.durtyfree,
        catalogs.file_catalog,
    )?;

    Ok(preflight_report_from_combined(
        workspace,
        &canonical_workspace,
        combined,
    ))
}

fn preflight_report_from_combined(
    workspace_display: &Path,
    canonical_workspace: &Path,
    combined: CombinedPreflight,
) -> WorkspaceExportPreflightReport {
    let unresolved = combined.unresolved;
    WorkspaceExportPreflightReport {
        workspace: workspace_display.display().to_string(),
        selected_roots: combined.selected_roots,
        closure_ymaps: combined.closure_ymaps,
        predicted_files: combined.predicted_files,
        unresolved: unresolved.into(),
        export_gate: ExportGateReport {
            allowed_without_override: unresolved.unknown == 0,
            requires_allow_unresolved: unresolved.unknown > 0,
        },
        unknown_groups: combined
            .unknown_groups
            .into_iter()
            .map(|group| UnknownDependencyGroupReport {
                kind: group.kind,
                hash: format!("0x{:08X}", group.hash),
                uses: group.uses,
                reasons: group.reasons,
                affected_maps: group.affected_maps,
            })
            .collect(),
        mlo_audits: combined
            .mlo_audits
            .into_iter()
            .map(|audit| MloAuditReport {
                archetype_hash: format!("0x{:08X}", audit.archetype_hash),
                ytyp: relative_name(canonical_workspace, &audit.ytyp_path),
                entities: audit.entities,
                unique_entity_archetypes: audit.unique_entity_archetypes,
                rooms: audit.rooms,
                portals: audit.portals,
                local_files: audit.local_files,
                vanilla: audit.vanilla,
                unknown: audit.unknown,
                risk: audit.risk.as_str().to_string(),
            })
            .collect(),
        warnings: combined.warnings,
    }
}

fn canonical_workspace_root(workspace: &Path) -> Result<PathBuf, EngineError> {
    let canonical = workspace.canonicalize()?;
    if !canonical.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("not a directory: {}", workspace.display()),
        )
        .into());
    }
    Ok(canonical)
}

fn resolve_workspace_maps(
    canonical_workspace: &Path,
    maps: &[PathBuf],
) -> Result<Vec<PathBuf>, EngineError> {
    if maps.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "at least one YMAP must be selected",
        )
        .into());
    }

    maps.iter()
        .map(|map| {
            let candidate = if map.is_absolute() {
                map.clone()
            } else {
                canonical_workspace.join(map)
            };
            let canonical = candidate.canonicalize()?;
            if !canonical.is_file() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("not a file: {}", candidate.display()),
                )
                .into());
            }
            if !canonical.starts_with(canonical_workspace) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "YMAP must be contained by workspace: {}",
                        candidate.display()
                    ),
                )
                .into());
            }
            if !canonical
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("ymap"))
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("selected map is not a .ymap file: {}", candidate.display()),
                )
                .into());
            }
            Ok(canonical)
        })
        .collect()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SharedExportOptions {
    pub allow_unresolved: bool,
    pub overwrite: bool,
}

#[derive(Debug)]
pub struct ExportOutcome {
    pub result: ExportResult,
    pub metadata_path: PathBuf,
    pub validation: ExportValidation,
    pub unresolved: UnresolvedSummary,
}

pub fn export_map_resource(
    index: &WorkspaceIndex,
    workspace: &Path,
    map_path: &Path,
    output: &Path,
    resource_name: &str,
    options: SharedExportOptions,
    catalogs: CatalogRefs<'_>,
) -> Result<ExportOutcome, EngineError> {
    let preflight = index.resolve_map(map_path)?;
    let unresolved = classify_unresolved(&preflight, catalogs.durtyfree, catalogs.file_catalog);
    enforce_export_gate(unresolved, options.allow_unresolved)?;

    let result = index.export_map(
        map_path,
        output,
        ExportOptions {
            // Catalog-aware gating is handled above; ragelab-assets should only gate
            // references that the shared engine has already classified.
            allow_unresolved: true,
            overwrite: options.overwrite,
        },
    )?;
    let (metadata_path, validation) = finalize_export(
        workspace,
        resource_name,
        &[map_path.to_path_buf()],
        options.allow_unresolved,
        unresolved,
        catalogs,
        &result,
    )?;

    Ok(ExportOutcome {
        result,
        metadata_path,
        validation,
        unresolved,
    })
}

pub fn export_maps_resource(
    index: &WorkspaceIndex,
    workspace: &Path,
    selected_paths: &[PathBuf],
    output: &Path,
    resource_name: &str,
    options: SharedExportOptions,
    catalogs: CatalogRefs<'_>,
) -> Result<ExportOutcome, EngineError> {
    let preflight = index.resolve_maps(selected_paths)?;
    let unresolved = classify_unresolved(&preflight, catalogs.durtyfree, catalogs.file_catalog);
    enforce_export_gate(unresolved, options.allow_unresolved)?;

    let result = index.export_maps(
        selected_paths,
        output,
        ExportOptions {
            allow_unresolved: true,
            overwrite: options.overwrite,
        },
    )?;
    let (metadata_path, validation) = finalize_export(
        workspace,
        resource_name,
        selected_paths,
        options.allow_unresolved,
        unresolved,
        catalogs,
        &result,
    )?;

    Ok(ExportOutcome {
        result,
        metadata_path,
        validation,
        unresolved,
    })
}

pub fn workspace_export_report(
    workspace: &Path,
    maps: &[PathBuf],
    output: &Path,
    resource_name: &str,
    options: SharedExportOptions,
    catalogs: CatalogRefs<'_>,
) -> Result<WorkspaceExportReport, EngineError> {
    let canonical_workspace = canonical_workspace_root(workspace)?;
    let selected_paths = resolve_workspace_maps(&canonical_workspace, maps)?;
    let index = WorkspaceIndex::scan(&canonical_workspace)?;

    let outcome = if selected_paths.len() == 1 {
        export_map_resource(
            &index,
            &canonical_workspace,
            &selected_paths[0],
            output,
            resource_name,
            options,
            catalogs,
        )?
    } else {
        export_maps_resource(
            &index,
            &canonical_workspace,
            &selected_paths,
            output,
            resource_name,
            options,
            catalogs,
        )?
    };

    Ok(export_report_from_outcome(
        workspace,
        &canonical_workspace,
        &selected_paths,
        resource_name,
        options.allow_unresolved,
        outcome,
    ))
}

fn export_report_from_outcome(
    workspace_display: &Path,
    canonical_workspace: &Path,
    selected_paths: &[PathBuf],
    resource_name: &str,
    allow_unresolved: bool,
    outcome: ExportOutcome,
) -> WorkspaceExportReport {
    let ExportOutcome {
        result,
        metadata_path,
        validation,
        unresolved,
    } = outcome;

    let copied_files = result
        .copied_files
        .iter()
        .filter_map(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    let warnings = result.report.warnings.clone();

    WorkspaceExportReport {
        workspace: workspace_display.display().to_string(),
        resource_name: resource_name.to_string(),
        selected_roots: selected_paths
            .iter()
            .map(|path| relative_name(canonical_workspace, path))
            .collect(),
        output: WorkspaceExportOutputReport {
            resource: result.output_dir.display().to_string(),
            stream: result.stream_dir.display().to_string(),
            manifest: result.manifest_path.display().to_string(),
            metadata: metadata_path.display().to_string(),
            gtxd: result
                .gtxd_path
                .as_ref()
                .map(|path| path.display().to_string()),
            copied_files,
            manifest_maps: result.manifest.maps.len(),
        },
        unresolved: ExportUnresolvedReport {
            raw: unresolved.raw,
            vanilla: unresolved.vanilla,
            unknown: unresolved.unknown,
            catalog_used: unresolved.catalog_used,
            durtyfree_used: unresolved.durtyfree_used,
            file_catalog_used: unresolved.file_catalog_used,
            allow_unresolved,
        },
        validation: ExportValidationReport::from(&validation),
        warnings,
    }
}

fn enforce_export_gate(
    unresolved: UnresolvedSummary,
    allow_unresolved: bool,
) -> Result<(), EngineError> {
    match export_gate_error(unresolved, allow_unresolved) {
        Some(message) => Err(EngineError::ExportGate(message)),
        None => Ok(()),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExportMetadata {
    pub schema_version: u32,
    pub resource_name: String,
    pub workspace: String,
    pub selected_roots: Vec<String>,
    pub closure_ymaps: Vec<String>,
    pub copied_files: Vec<String>,
    pub allow_unresolved: bool,
    pub vanilla_catalog_used: bool,
    #[serde(default)]
    pub file_catalog_used: bool,
    pub raw_unresolved: usize,
    pub vanilla: usize,
    pub unknown_external: usize,
    pub generated_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportValidation {
    pub status: String,
    pub valid: bool,
    pub metadata_present: bool,
    pub manifest_valid: bool,
    pub fxmanifest_present: bool,
    pub selected_roots: usize,
    pub closure_maps: usize,
    pub copied_files: usize,
    pub interior_maps: usize,
    pub interior_bounds: usize,
    pub local_missing: Vec<String>,
    pub post_export_raw_unresolved: usize,
    pub post_export_vanilla: usize,
    pub post_export_unknown: usize,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug)]
pub enum EngineError {
    Workspace(WorkspaceError),
    ExportGate(String),
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Workspace(error) => write!(f, "{error}"),
            Self::ExportGate(message) => f.write_str(message),
            Self::Io(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<WorkspaceError> for EngineError {
    fn from(error: WorkspaceError) -> Self {
        Self::Workspace(error)
    }
}

impl From<std::io::Error> for EngineError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for EngineError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub fn finalize_export(
    workspace: &Path,
    resource_name: &str,
    selected_paths: &[PathBuf],
    allow_unresolved: bool,
    unresolved: UnresolvedSummary,
    catalogs: CatalogRefs<'_>,
    result: &ExportResult,
) -> Result<(PathBuf, ExportValidation), EngineError> {
    let mut selected_roots = selected_paths
        .iter()
        .map(|path| relative_name(workspace, path))
        .collect::<Vec<_>>();
    selected_roots.sort();
    selected_roots.dedup();

    let mut closure_ymaps = result
        .report
        .files
        .values()
        .filter(|file| file.kind == AssetKind::Ymap)
        .map(|file| relative_name(workspace, &file.path))
        .collect::<Vec<_>>();
    closure_ymaps.sort();
    closure_ymaps.dedup();

    let mut copied_files = result
        .copied_files
        .iter()
        .filter_map(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    copied_files.sort();
    copied_files.dedup();

    let metadata = ExportMetadata {
        schema_version: 1,
        resource_name: resource_name.to_string(),
        workspace: workspace.to_string_lossy().into_owned(),
        selected_roots,
        closure_ymaps,
        copied_files,
        allow_unresolved,
        vanilla_catalog_used: unresolved.durtyfree_used,
        file_catalog_used: unresolved.file_catalog_used,
        raw_unresolved: unresolved.raw,
        vanilla: unresolved.vanilla,
        unknown_external: unresolved.unknown,
        generated_at: unix_timestamp(),
    };
    let metadata_path = result.output_dir.join(".ragelab-export.json");
    let encoded = serde_json::to_vec_pretty(&metadata)?;
    fs::write(&metadata_path, encoded)?;

    let validation =
        validate_export_output(result, &metadata, catalogs.durtyfree, catalogs.file_catalog);
    Ok((metadata_path, validation))
}

pub fn validate_export_output(
    result: &ExportResult,
    metadata: &ExportMetadata,
    vanilla_catalog: Option<&DurtyFreeCatalog>,
    file_catalog: Option<&VanillaFileCatalog>,
) -> ExportValidation {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let metadata_path = result.output_dir.join(".ragelab-export.json");
    let metadata_present = metadata_path.is_file();
    if !metadata_present {
        errors.push(".ragelab-export.json was not written".into());
    } else {
        match fs::read(&metadata_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<ExportMetadata>(&bytes).ok())
        {
            Some(saved) if saved.schema_version == 1 => {}
            Some(saved) => warnings.push(format!(
                "unknown export metadata schema version {}",
                saved.schema_version
            )),
            None => errors.push(".ragelab-export.json could not be parsed".into()),
        }
    }

    let fxmanifest_present = result.output_dir.join("fxmanifest.lua").is_file();
    if !fxmanifest_present {
        errors.push("fxmanifest.lua is missing".into());
    }

    let mut manifest_valid = false;
    let mut interior_maps = 0_usize;
    let mut interior_bounds = 0_usize;
    match fs::read(&result.manifest_path) {
        Ok(bytes) => match Ymf::from_bytes(&bytes) {
            Ok(manifest) => {
                manifest_valid = true;
                interior_maps = manifest
                    .maps
                    .iter()
                    .filter(|map| map.flags.contains(ManifestFlags::INTERIOR_DATA))
                    .count();
                interior_bounds = manifest.interiors.len();
                if manifest.maps.len() != metadata.closure_ymaps.len() {
                    warnings.push(format!(
                        "manifest contains {} YMAP entries but export closure records {} YMAPs",
                        manifest.maps.len(),
                        metadata.closure_ymaps.len()
                    ));
                }
                if interior_maps > 0 && interior_bounds == 0 {
                    warnings.push(format!(
                        "manifest marks {interior_maps} interior YMAP(s) but has no interior bounds"
                    ));
                }
            }
            Err(error) => errors.push(format!("_manifest.ymf parse failed: {error}")),
        },
        Err(error) => errors.push(format!("_manifest.ymf could not be read: {error}")),
    }

    let mut local_missing = Vec::new();
    for file in result.report.files.values() {
        let Some(filename) = file.path.file_name() else {
            continue;
        };
        if !result.stream_dir.join(filename).is_file() {
            local_missing.push(file.path.to_string_lossy().into_owned());
        }
    }
    local_missing.sort();
    local_missing.dedup();
    if !local_missing.is_empty() {
        errors.push(format!(
            "{} locally resolved dependency file(s) are missing from stream/",
            local_missing.len()
        ));
    }

    for root in &metadata.selected_roots {
        let Some(filename) = Path::new(root).file_name() else {
            errors.push(format!("selected root has no filename: {root}"));
            continue;
        };
        if !result.stream_dir.join(filename).is_file() {
            errors.push(format!("selected root is missing from stream/: {root}"));
        }
    }

    let mut post_export = UnresolvedSummary::default();
    match WorkspaceIndex::scan(&result.stream_dir) {
        Ok(index) => {
            let selected = metadata
                .selected_roots
                .iter()
                .filter_map(|name| {
                    Path::new(name)
                        .file_name()
                        .map(|filename| result.stream_dir.join(filename))
                })
                .filter(|path| path.is_file())
                .collect::<Vec<_>>();
            if !selected.is_empty() {
                match index.resolve_maps(&selected) {
                    Ok(report) => {
                        post_export = classify_unresolved(&report, vanilla_catalog, file_catalog);
                        for warning in report.warnings.into_iter().take(10) {
                            warnings.push(format!("post-export resolver: {warning}"));
                        }
                    }
                    Err(error) => {
                        errors.push(format!("post-export dependency scan failed: {error}"))
                    }
                }
            }
        }
        Err(error) => errors.push(format!("post-export stream index failed: {error}")),
    }

    if post_export.unknown > 0 {
        warnings.push(format!(
            "{} unknown external reference(s) remain after re-scanning the exported resource",
            post_export.unknown
        ));
    }

    let valid = errors.is_empty();
    let status = if !valid {
        "error"
    } else if warnings.is_empty() {
        "ok"
    } else {
        "warning"
    }
    .to_string();

    ExportValidation {
        status,
        valid,
        metadata_present,
        manifest_valid,
        fxmanifest_present,
        selected_roots: metadata.selected_roots.len(),
        closure_maps: metadata.closure_ymaps.len(),
        copied_files: metadata.copied_files.len(),
        interior_maps,
        interior_bounds,
        local_missing,
        post_export_raw_unresolved: post_export.raw,
        post_export_vanilla: post_export.vanilla,
        post_export_unknown: post_export.unknown,
        warnings,
        errors,
    }
}

fn relative_name(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ragelab_assets::{DependencyReason, UnresolvedDependency};

    #[test]
    fn combined_preflight_uses_shared_engine_domain() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/synthetic/stream")
            .canonicalize()
            .expect("synthetic workspace");
        let map = workspace.join("simple.ymap");
        let index = WorkspaceIndex::scan(&workspace).expect("scan synthetic workspace");
        let preflight =
            preflight_maps_combined(&index, &workspace, std::slice::from_ref(&map), None, None)
                .expect("combined preflight");

        assert_eq!(preflight.selected_roots, vec!["simple.ymap"]);
        assert!(preflight
            .closure_ymaps
            .iter()
            .any(|name| name == "simple.ymap"));
        assert!(preflight
            .predicted_files
            .iter()
            .any(|name| name == "simple.ymap"));
        assert_eq!(preflight.unresolved.raw, preflight.unresolved.unknown);
        assert!(!preflight.unresolved.catalog_used);
        assert!(preflight.mlo_audits.is_empty());
    }

    #[test]
    fn workspace_export_preflight_report_preserves_public_contract() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/synthetic/stream")
            .canonicalize()
            .expect("synthetic workspace");
        let report = workspace_export_preflight_report(
            &workspace,
            &[PathBuf::from("simple.ymap")],
            CatalogRefs::default(),
        )
        .expect("workspace preflight report");

        assert_eq!(report.selected_roots, vec!["simple.ymap"]);
        assert!(report
            .closure_ymaps
            .iter()
            .any(|name| name == "simple.ymap"));
        assert_eq!(
            report.export_gate.allowed_without_override,
            report.unresolved.unknown == 0
        );
        assert_eq!(
            report.export_gate.requires_allow_unresolved,
            report.unresolved.unknown > 0
        );

        let value = serde_json::to_value(report).expect("serialize preflight report");
        assert!(value["workspace"].is_string());
        assert_eq!(value["selectedRoots"][0], "simple.ymap");
        assert!(value["unresolved"]["catalogUsed"].is_boolean());
        assert!(value["exportGate"]["allowedWithoutOverride"].is_boolean());
        assert!(value["unknownGroups"].is_array());
        assert!(value["mloAudits"].is_array());
    }

    #[test]
    fn workspace_export_preflight_rejects_map_outside_workspace() {
        let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/synthetic")
            .canonicalize()
            .expect("synthetic fixture root");
        let workspace = fixture_root.join("stream");
        let outside_map = fixture_root.join("simple.ymap");

        let error =
            workspace_export_preflight_report(&workspace, &[outside_map], CatalogRefs::default())
                .unwrap_err();

        assert!(error.to_string().contains("contained by workspace"));
    }

    #[test]
    fn workspace_export_report_owns_path_resolution_and_serialization() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/synthetic/stream")
            .canonicalize()
            .expect("synthetic workspace");
        let output = std::env::temp_dir().join(format!(
            "ragelab-engine-workspace-export-{}-{}",
            std::process::id(),
            unix_timestamp()
        ));
        let _ = fs::remove_dir_all(&output);

        let report = workspace_export_report(
            &workspace,
            &[PathBuf::from("simple.ymap")],
            &output,
            "simple",
            SharedExportOptions {
                allow_unresolved: true,
                overwrite: false,
            },
            CatalogRefs::default(),
        )
        .expect("workspace export report");

        assert_eq!(report.resource_name, "simple");
        assert_eq!(report.selected_roots, vec!["simple.ymap"]);
        assert_eq!(report.output.resource, output.display().to_string());
        assert!(report.validation.valid, "{:?}", report.validation.errors);
        assert!(Path::new(&report.output.metadata).is_file());
        assert!(Path::new(&report.output.manifest).is_file());

        let value = serde_json::to_value(&report).expect("serialize export report");
        assert_eq!(value["resourceName"], "simple");
        assert_eq!(value["selectedRoots"][0], "simple.ymap");
        assert!(value["output"]["copiedFiles"].is_array());
        assert!(value["validation"]["metadataPresent"].as_bool().unwrap());

        fs::remove_dir_all(output).expect("clean synthetic workspace export");
    }

    #[test]
    fn finalizes_and_validates_synthetic_export() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/synthetic/stream")
            .canonicalize()
            .expect("synthetic workspace");
        let map = workspace.join("simple.ymap");
        let index = WorkspaceIndex::scan(&workspace).expect("scan synthetic workspace");
        let output = std::env::temp_dir().join(format!(
            "ragelab-engine-finalize-{}-{}",
            std::process::id(),
            unix_timestamp()
        ));
        let outcome = export_map_resource(
            &index,
            &workspace,
            &map,
            &output,
            "simple",
            SharedExportOptions {
                allow_unresolved: true,
                overwrite: true,
            },
            CatalogRefs::default(),
        )
        .expect("export/finalize synthetic map");

        assert!(outcome.metadata_path.is_file());
        assert!(outcome.validation.valid, "{:?}", outcome.validation.errors);
        assert!(outcome.validation.metadata_present);
        assert!(outcome.validation.manifest_valid);
        assert!(outcome.validation.fxmanifest_present);
        assert!(outcome.validation.local_missing.is_empty());
        let saved: ExportMetadata = serde_json::from_slice(
            &fs::read(&outcome.metadata_path).expect("read saved export metadata"),
        )
        .expect("parse saved export metadata");
        assert_eq!(saved.schema_version, 1);
        assert_eq!(saved.resource_name, "simple");
        assert_eq!(saved.selected_roots, vec!["simple.ymap"]);

        fs::remove_dir_all(output).expect("clean synthetic export");
    }

    #[test]
    fn export_metadata_v1_remains_backward_compatible_without_file_catalog_field() {
        let json = r#"{
          "schemaVersion":1,
          "resourceName":"test",
          "workspace":"/workspace",
          "selectedRoots":["simple.ymap"],
          "closureYmaps":["simple.ymap"],
          "copiedFiles":["simple.ymap"],
          "allowUnresolved":false,
          "vanillaCatalogUsed":true,
          "rawUnresolved":3,
          "vanilla":3,
          "unknownExternal":0,
          "generatedAt":1
        }"#;
        let metadata: ExportMetadata =
            serde_json::from_str(json).expect("old v1 metadata should parse");
        assert_eq!(metadata.schema_version, 1);
        assert!(!metadata.file_catalog_used);
        assert_eq!(metadata.selected_roots, vec!["simple.ymap"]);
    }

    #[test]
    fn parses_durtyfree_object_list_into_joaat_catalog() {
        let catalog = parse_durtyfree_object_list("prop_chair_01a\n# comment\nPROP_DOOR_01\n");
        assert_eq!(catalog.len(), 2);
        assert_eq!(
            catalog.get(&joaat("prop_chair_01a")).map(String::as_str),
            Some("prop_chair_01a")
        );
        assert_eq!(
            catalog.get(&joaat("prop_door_01")).map(String::as_str),
            Some("PROP_DOOR_01")
        );
    }

    #[test]
    fn vanilla_catalog_removes_only_confirmed_archetypes_from_export_gate() {
        let vanilla_hash = joaat("prop_tree_birch_02");
        let unknown_hash = 0xDEAD_BEEF;
        let report = DependencyReport {
            unresolved: vec![
                UnresolvedDependency {
                    kind: UnresolvedKind::ArchetypeProvider,
                    hash: vanilla_hash,
                    reason: DependencyReason::ArchetypeProvider {
                        archetype: vanilla_hash,
                    },
                },
                UnresolvedDependency {
                    kind: UnresolvedKind::File(AssetKind::Ytd),
                    hash: unknown_hash,
                    reason: DependencyReason::TextureDictionary {
                        archetype: 0x1234_5678,
                    },
                },
            ],
            ..DependencyReport::default()
        };
        let catalog = DurtyFreeCatalog {
            objects: BTreeMap::from([(vanilla_hash, "prop_tree_birch_02".into())]),
            synced_at: None,
        };

        let classified = classify_unresolved(&report, Some(&catalog), None);
        assert_eq!(classified.raw, 2);
        assert_eq!(classified.vanilla, 1);
        assert_eq!(classified.unknown, 1);
        assert!(classified.catalog_used);
        assert!(export_gate_error(classified, false).is_some());
        assert!(export_gate_error(classified, true).is_none());
    }

    #[test]
    fn export_gate_allows_confirmed_vanilla_without_allow_unresolved() {
        let vanilla_hash = joaat("prop_tree_birch_02");
        let report = DependencyReport {
            unresolved: vec![UnresolvedDependency {
                kind: UnresolvedKind::ArchetypeProvider,
                hash: vanilla_hash,
                reason: DependencyReason::ArchetypeProvider {
                    archetype: vanilla_hash,
                },
            }],
            ..DependencyReport::default()
        };
        let catalog = DurtyFreeCatalog {
            objects: BTreeMap::from([(vanilla_hash, "prop_tree_birch_02".into())]),
            synced_at: None,
        };

        let classified = classify_unresolved(&report, Some(&catalog), None);
        assert_eq!(classified.unknown, 0);
        assert_eq!(classified.vanilla, 1);
        assert!(export_gate_error(classified, false).is_none());

        let without_catalog = classify_unresolved(&report, None, None);
        assert_eq!(without_catalog.unknown, 1);
        assert!(export_gate_error(without_catalog, false).is_some());
    }

    #[test]
    fn file_catalog_classifies_streamed_file_hashes_as_vanilla() {
        let body =
            "x64/levels/gta5/props/shared_textures.ytd\nupdate/x64/collision/city_block.ybn\n";
        let files = parse_vanilla_file_catalog(body);
        assert_eq!(files.len(), 2);
        let texture_hash = joaat("shared_textures");
        assert!(files.contains_key(&(AssetKind::Ytd, texture_hash)));

        let report = DependencyReport {
            unresolved: vec![UnresolvedDependency {
                kind: UnresolvedKind::File(AssetKind::Ytd),
                hash: texture_hash,
                reason: DependencyReason::TextureDictionary {
                    archetype: 0x1234_5678,
                },
            }],
            ..DependencyReport::default()
        };
        let catalog = VanillaFileCatalog {
            files,
            synced_at: None,
        };
        let classified = classify_unresolved(&report, None, Some(&catalog));
        assert_eq!(classified.raw, 1);
        assert_eq!(classified.vanilla, 1);
        assert_eq!(classified.unknown, 0);
        assert!(classified.file_catalog_used);
    }

    #[test]
    fn groups_only_unknown_references() {
        let vanilla_hash = joaat("prop_tree_birch_02");
        let unknown_hash = 0xCAFE_BABE;
        let report = DependencyReport {
            unresolved: vec![
                UnresolvedDependency {
                    kind: UnresolvedKind::ArchetypeProvider,
                    hash: vanilla_hash,
                    reason: DependencyReason::ArchetypeProvider {
                        archetype: vanilla_hash,
                    },
                },
                UnresolvedDependency {
                    kind: UnresolvedKind::File(AssetKind::Ytd),
                    hash: unknown_hash,
                    reason: DependencyReason::TextureDictionary {
                        archetype: 0x1111_2222,
                    },
                },
            ],
            ..DependencyReport::default()
        };
        let catalog = DurtyFreeCatalog {
            objects: BTreeMap::from([(vanilla_hash, "prop_tree_birch_02".into())]),
            synced_at: None,
        };
        let groups =
            group_unknown_dependencies(&[("one.ymap".into(), report)], Some(&catalog), None);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].kind, "ytd");
        assert_eq!(groups[0].hash, unknown_hash);
        assert_eq!(groups[0].uses, 1);
        assert_eq!(groups[0].affected_maps, vec!["one.ymap"]);
    }
}

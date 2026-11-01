use std::{
    env, fs,
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
};

use ragelab_engine::{
    compiled_asset_formats, compiled_core_modules, core_build_metadata, discover_fivem_legacy,
    discover_gta_v_legacy, CompiledAssetFormat, CoreBuildMetadata, FiveMDiscoveryReport,
    GtaVDiscoveryReport, FIVEM_ENV, GTA_V_LEGACY_ENV,
};
use ragelab_render::OffscreenRenderer;
use serde::Serialize;

pub const DOCTOR_SCHEMA: &str = "ragelab.cli.doctor";
pub const DOCTOR_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Blocker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    Healthy,
    Warning,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorCheck {
    pub id: &'static str,
    pub category: &'static str,
    pub severity: DiagnosticSeverity,
    pub status: &'static str,
    pub summary: String,
    pub details: Option<String>,
    pub recommendation: Option<String>,
    pub affects_commands: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorSummary {
    pub checks: usize,
    pub info: usize,
    pub warnings: usize,
    pub blockers: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedInstallation {
    pub root: String,
    pub valid: bool,
    pub edition_or_relationship: String,
    pub discovered_by: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationProbe {
    pub id: &'static str,
    pub configured_environment_variable: &'static str,
    pub environment_variable_set: bool,
    pub status: &'static str,
    pub candidates: usize,
    pub valid_installations: usize,
    pub linked_legacy_installations: Option<usize>,
    pub detected_paths: Vec<DetectedInstallation>,
    pub globally_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesystemProbe {
    pub current_directory: Option<String>,
    pub current_directory_is_directory: bool,
    pub metadata_readable: bool,
    pub write_probe_performed: bool,
    pub implicit_output_directory_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorEnvironment {
    pub renderer: String,
    pub gta_v_legacy: InstallationProbe,
    pub fivem: InstallationProbe,
    pub filesystem: FilesystemProbe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalDependencyCapability {
    pub id: &'static str,
    pub required_globally: bool,
    pub applies_to: &'static [&'static str],
    pub configuration: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub schema: &'static str,
    pub schema_version: u32,
    pub status: DoctorStatus,
    pub read_only: bool,
    pub build: CoreBuildMetadata,
    pub compiled_modules: Vec<&'static str>,
    pub asset_formats: Vec<CompiledAssetFormat>,
    pub external_dependencies: Vec<ExternalDependencyCapability>,
    pub environment: DoctorEnvironment,
    pub checks: Vec<DoctorCheck>,
    pub summary: DoctorSummary,
    pub actions_taken: Vec<&'static str>,
}

#[derive(Debug, Clone)]
struct ProbeSnapshot {
    renderer_error: Option<String>,
    gta: InstallationProbe,
    fivem: InstallationProbe,
    current_directory: Result<PathBuf, String>,
}

const EXTERNAL_DEPENDENCIES: &[ExternalDependencyCapability] = &[
    ExternalDependencyCapability {
        id: "wgpu-render-device",
        required_globally: false,
        applies_to: &["render.asset", "render.scene"],
        configuration: "Install or enable a compatible graphics driver/adapter; no global path is read.",
        description: "The offscreen renderer initializes a wgpu adapter and device. Parser, validator, GLB/YDR and workspace operations do not require a GPU.",
    },
    ExternalDependencyCapability {
        id: "gta-v-legacy-installation",
        required_globally: false,
        applies_to: &["gta.discover", "gta.rpf-index", "gta.world-query", "gta.mlo-validate"],
        configuration: "Pass game roots explicitly to commands; optional discovery override: RAGELAB_GTA5_LEGACY.",
        description: "A local Legacy installation is input data for selected GTA/RPF workflows, not a prerequisite for the Core or standalone asset readers/writers.",
    },
    ExternalDependencyCapability {
        id: "fivem-installation",
        required_globally: false,
        applies_to: &["fivem.discover"],
        configuration: "Pass paths explicitly to workflows; optional discovery override: RAGELAB_FIVEM.",
        description: "FiveM discovery and cache/storage integration are optional. Their absence does not disable format parsers or the GLB/YDR pipeline.",
    },
];

pub fn external_dependency_capabilities() -> &'static [ExternalDependencyCapability] {
    EXTERNAL_DEPENDENCIES
}

pub fn collect_doctor_report() -> DoctorReport {
    let renderer_error = probe_renderer().err();
    let snapshot = ProbeSnapshot {
        renderer_error,
        gta: installation_probe_gta(discover_gta_v_legacy()),
        fivem: installation_probe_fivem(discover_fivem_legacy()),
        current_directory: env::current_dir().map_err(|error| error.to_string()),
    };
    build_doctor_report(
        core_build_metadata(),
        compiled_core_modules(),
        compiled_asset_formats(),
        snapshot,
    )
}

fn probe_renderer() -> Result<(), String> {
    match catch_unwind(AssertUnwindSafe(OffscreenRenderer::new)) {
        Ok(Ok(renderer)) => {
            drop(renderer);
            Ok(())
        }
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => {
            Err("renderer initialization panicked while probing the local wgpu device".into())
        }
    }
}

fn build_doctor_report(
    build: CoreBuildMetadata,
    modules: &[&'static str],
    formats: Vec<CompiledAssetFormat>,
    snapshot: ProbeSnapshot,
) -> DoctorReport {
    let gta = snapshot.gta;
    let fivem = snapshot.fivem;
    let filesystem = filesystem_probe(snapshot.current_directory);
    let renderer = if let Some(error) = snapshot.renderer_error {
        format!("unavailable: {error}")
    } else {
        "ready".to_owned()
    };

    let mut checks = Vec::new();
    if build.revision == "unknown" {
        checks.push(check(
            "build.revision",
            "build",
            DiagnosticSeverity::Warning,
            "unknown",
            "Core revision metadata is unavailable",
            Some("The binary was built without a readable Git revision.".into()),
            Some("Build from a Git checkout or set RAGELAB_CORE_REVISION in the release build environment.".into()),
            &["version", "capabilities", "doctor"],
        ));
    } else {
        checks.push(check(
            "build.revision",
            "build",
            DiagnosticSeverity::Info,
            "available",
            format!("Core revision: {}", build.revision),
            None,
            None,
            &["version", "capabilities", "doctor"],
        ));
    }

    if build.dirty {
        checks.push(check(
            "build.sourceState",
            "build",
            DiagnosticSeverity::Warning,
            "dirty",
            "Core was built from a dirty source tree",
            Some(
                "The build metadata records tracked or untracked source changes at compile time."
                    .into(),
            ),
            Some("Use a clean, committed worktree for reproducible release builds.".into()),
            &["version", "capabilities", "doctor"],
        ));
    } else {
        checks.push(check(
            "build.sourceState",
            "build",
            DiagnosticSeverity::Info,
            "clean",
            "Core build source tree was clean",
            None,
            None,
            &["version", "capabilities", "doctor"],
        ));
    }

    if build.target.triple == "unknown"
        || build.target.operating_system == "unknown"
        || build.target.architecture == "unknown"
    {
        checks.push(check(
            "build.target",
            "build",
            DiagnosticSeverity::Warning,
            "incomplete",
            "Build target metadata is incomplete",
            Some(format!(
                "target={} os={} arch={}",
                build.target.triple, build.target.operating_system, build.target.architecture
            )),
            Some("Rebuild with a Cargo target triple and target cfg metadata.".into()),
            &["version", "capabilities", "doctor"],
        ));
    } else {
        checks.push(check(
            "build.target",
            "build",
            DiagnosticSeverity::Info,
            "available",
            format!(
                "Built for {} ({}/{})",
                build.target.triple, build.target.operating_system, build.target.architecture
            ),
            Some(format!(
                "profile={} apiCompatibility={} features={}",
                build.profile,
                build.api_compatibility_version,
                if build.features.is_empty() {
                    "none".to_owned()
                } else {
                    build.features.join(",")
                }
            )),
            None,
            &["version", "capabilities", "doctor"],
        ));
    }

    let required_modules = ["asset", "diagnostics", "discovery", "glb", "ydr"];
    let missing_modules = required_modules
        .iter()
        .filter(|required| !modules.contains(required))
        .copied()
        .collect::<Vec<_>>();
    if !missing_modules.is_empty() {
        checks.push(check(
            "core.moduleRegistry",
            "core",
            DiagnosticSeverity::Blocker,
            "incomplete",
            "Required Core modules are missing from the compiled registry",
            Some(format!("Missing modules: {}", missing_modules.join(", "))),
            Some("Rebuild the RageLab Core crate with its required modules enabled; this binary is not a complete Core build.".into()),
            &["capabilities", "doctor", "glb.inspect", "glb.export", "ydr.inspect", "ydr.validate"],
        ));
    } else {
        checks.push(check(
            "core.moduleRegistry",
            "core",
            DiagnosticSeverity::Info,
            "complete",
            format!("{} Core modules are compiled and registered", modules.len()),
            Some(modules.join(", ")),
            None,
            &["capabilities", "doctor"],
        ));
    }

    let required_semantic_formats = ["YMAP", "YTYP", "YDR", "YDD", "YTD", "YBN", "YFT", "GLB 2.0"];
    let missing_formats = required_semantic_formats
        .iter()
        .filter(|required| {
            !formats
                .iter()
                .any(|format| format.id == **required && format.semantic_parser)
        })
        .copied()
        .collect::<Vec<_>>();
    if !missing_formats.is_empty() {
        checks.push(check(
            "core.semanticFormats",
            "formats",
            DiagnosticSeverity::Blocker,
            "incomplete",
            "Required semantic format parsers are missing from the registry",
            Some(format!(
                "Missing semantic formats: {}",
                missing_formats.join(", ")
            )),
            Some(
                "Rebuild with the corresponding format crates and Engine dispatch compiled in."
                    .into(),
            ),
            &["capabilities", "doctor", "inspect", "validate"],
        ));
    } else {
        checks.push(check(
            "core.semanticFormats",
            "formats",
            DiagnosticSeverity::Info,
            "available",
            format!("{} format readers/validators are registered", formats.iter().filter(|format| format.semantic_parser).count()),
            Some("YFT support remains limited to the Legacy v162 mainDrawable parser; container-only formats are reported separately.".into()),
            None,
            &["capabilities", "doctor", "inspect", "validate"],
        ));
    }

    let container_only = formats
        .iter()
        .filter(|format| !format.semantic_parser)
        .map(|format| format.id)
        .collect::<Vec<_>>();
    if !container_only.is_empty() {
        checks.push(check(
            "formats.containerOnly",
            "formats",
            DiagnosticSeverity::Info,
            "declared",
            format!("{} formats are intentionally container-only", container_only.len()),
            Some(container_only.join(", ")),
            Some("Use format-specific semantic readers only when a dedicated parser is registered; container parsing is not format validation.".into()),
            &["capabilities", "inspect", "validate"],
        ));
    }

    if renderer == "ready" {
        checks.push(check(
            "render.wgpuDevice",
            "render",
            DiagnosticSeverity::Info,
            "ready",
            "Offscreen wgpu renderer initialized successfully",
            Some("The same renderer constructor used by render.asset and render.scene was exercised and then dropped.".into()),
            None,
            &["render.asset", "render.scene"],
        ));
    } else {
        checks.push(check(
            "render.wgpuDevice",
            "render",
            DiagnosticSeverity::Warning,
            "unavailable",
            "Offscreen rendering is not ready on this machine",
            Some(renderer.clone()),
            Some("Install/update a compatible graphics driver or use a device with a supported wgpu adapter. Other parsing, validation and authoring commands remain available.".into()),
            &["render.asset", "render.scene"],
        ));
    }

    checks.push(installation_check_gta(&gta));
    checks.push(installation_check_fivem(&fivem));

    if filesystem.metadata_readable && filesystem.current_directory_is_directory {
        checks.push(check(
            "paths.currentDirectory",
            "filesystem",
            DiagnosticSeverity::Info,
            "available",
            "Current working directory is readable",
            filesystem.current_directory.clone(),
            None,
            &[],
        ));
    } else {
        checks.push(check(
            "paths.currentDirectory",
            "filesystem",
            DiagnosticSeverity::Warning,
            "unavailable",
            "Current working directory could not be verified",
            filesystem.current_directory.clone(),
            Some("Run the command from an accessible directory or pass absolute paths to operations that require files.".into()),
            &[],
        ));
    }

    checks.push(check(
        "paths.outputPolicy",
        "filesystem",
        DiagnosticSeverity::Info,
        "explicitPaths",
        "Doctor does not require or create an implicit output/cache directory",
        Some("Doctor inspects the current directory metadata only. Commands receive their input, output, workspace and cache paths explicitly; Doctor does not create directories or perform write-permission probes.".into()),
        None,
        &[],
    ));

    let info = checks
        .iter()
        .filter(|item| item.severity == DiagnosticSeverity::Info)
        .count();
    let warnings = checks
        .iter()
        .filter(|item| item.severity == DiagnosticSeverity::Warning)
        .count();
    let blockers = checks
        .iter()
        .filter(|item| item.severity == DiagnosticSeverity::Blocker)
        .count();
    let status = if blockers > 0 {
        DoctorStatus::Blocked
    } else if warnings > 0 {
        DoctorStatus::Warning
    } else {
        DoctorStatus::Healthy
    };

    DoctorReport {
        schema: DOCTOR_SCHEMA,
        schema_version: DOCTOR_SCHEMA_VERSION,
        status,
        read_only: true,
        build,
        compiled_modules: modules.to_vec(),
        asset_formats: formats,
        external_dependencies: EXTERNAL_DEPENDENCIES.to_vec(),
        environment: DoctorEnvironment {
            renderer,
            gta_v_legacy: gta,
            fivem,
            filesystem,
        },
        summary: DoctorSummary {
            checks: checks.len(),
            info,
            warnings,
            blockers,
        },
        checks,
        actions_taken: Vec::new(),
    }
}

fn installation_probe_gta(report: GtaVDiscoveryReport) -> InstallationProbe {
    InstallationProbe {
        id: "gtaVLegacy",
        configured_environment_variable: GTA_V_LEGACY_ENV,
        environment_variable_set: env::var_os(GTA_V_LEGACY_ENV)
            .is_some_and(|value| !value.is_empty()),
        status: if report.valid_legacy_installations > 0 {
            "available"
        } else if report.candidates.is_empty() {
            "notDetected"
        } else {
            "incomplete"
        },
        candidates: report.candidates.len(),
        valid_installations: report.valid_legacy_installations,
        linked_legacy_installations: None,
        detected_paths: report
            .candidates
            .into_iter()
            .map(|candidate| DetectedInstallation {
                root: candidate.root.to_string_lossy().into_owned(),
                valid: candidate.valid,
                edition_or_relationship: candidate.edition.as_str().to_owned(),
                discovered_by: candidate
                    .provenance
                    .iter()
                    .map(|source| source.source.as_str().to_owned())
                    .collect(),
            })
            .collect(),
        globally_required: false,
    }
}

fn installation_probe_fivem(report: FiveMDiscoveryReport) -> InstallationProbe {
    InstallationProbe {
        id: "fivem",
        configured_environment_variable: FIVEM_ENV,
        environment_variable_set: env::var_os(FIVEM_ENV).is_some_and(|value| !value.is_empty()),
        status: if report.valid_installations > 0 {
            "available"
        } else if report.candidates.is_empty() {
            "notDetected"
        } else {
            "incomplete"
        },
        candidates: report.candidates.len(),
        valid_installations: report.valid_installations,
        linked_legacy_installations: Some(report.legacy_linked_installations),
        detected_paths: report
            .candidates
            .into_iter()
            .map(|candidate| DetectedInstallation {
                root: candidate.root.to_string_lossy().into_owned(),
                valid: candidate.valid,
                edition_or_relationship: candidate.gta.status.as_str().to_owned(),
                discovered_by: candidate
                    .provenance
                    .iter()
                    .map(|source| source.source.as_str().to_owned())
                    .collect(),
            })
            .collect(),
        globally_required: false,
    }
}

fn filesystem_probe(current_directory: Result<PathBuf, String>) -> FilesystemProbe {
    match current_directory {
        Ok(path) => {
            let metadata = fs::metadata(&path);
            let is_directory = metadata.as_ref().is_ok_and(fs::Metadata::is_dir);
            FilesystemProbe {
                current_directory: Some(path.to_string_lossy().into_owned()),
                current_directory_is_directory: is_directory,
                metadata_readable: metadata.is_ok(),
                write_probe_performed: false,
                implicit_output_directory_required: false,
            }
        }
        Err(_) => FilesystemProbe {
            current_directory: None,
            current_directory_is_directory: false,
            metadata_readable: false,
            write_probe_performed: false,
            implicit_output_directory_required: false,
        },
    }
}

fn installation_check_gta(probe: &InstallationProbe) -> DoctorCheck {
    match probe.status {
        "available" => check(
            "environment.gtaVLegacy",
            "environment",
            DiagnosticSeverity::Info,
            "available",
            format!("Found {} valid GTA V Legacy installation(s)", probe.valid_installations),
            Some(format!("{} candidate path(s); optional override {} is {}.", probe.candidates, probe.configured_environment_variable, if probe.environment_variable_set { "set" } else { "not set" })),
            None,
            &["gta.discover", "gta.rpf-index", "gta.world-query", "gta.mlo-validate"],
        ),
        "incomplete" => check(
            "environment.gtaVLegacy",
            "environment",
            DiagnosticSeverity::Warning,
            "incomplete",
            "GTA V Legacy candidates were found but none passed required installation checks",
            Some(format!("Candidates: {}. The checks are read-only and do not modify the installation.", probe.candidates)),
            Some(format!("Verify GTA5.exe, common.rpf, x64a.rpf and update/update.rpf, or pass a valid game root explicitly. Optional discovery override: {}.", probe.configured_environment_variable)),
            &["gta.discover", "gta.rpf-index", "gta.world-query", "gta.mlo-validate"],
        ),
        _ => check(
            "environment.gtaVLegacy",
            "environment",
            DiagnosticSeverity::Info,
            "notDetected",
            "No GTA V Legacy installation was detected",
            Some("GTA V Legacy is optional target data, not a prerequisite for Core parsers, validators or GLB/YDR authoring.".into()),
            Some(format!("When a GTA/RPF workflow needs it, pass the game root explicitly or configure {} for discovery.", probe.configured_environment_variable)),
            &["gta.discover", "gta.rpf-index", "gta.world-query", "gta.mlo-validate"],
        ),
    }
}

fn installation_check_fivem(probe: &InstallationProbe) -> DoctorCheck {
    match probe.status {
        "available" if probe.linked_legacy_installations == Some(0) => check(
            "environment.fivem",
            "environment",
            DiagnosticSeverity::Warning,
            "legacyLinkMissing",
            format!("{} FiveM installation(s) are present but none links to a valid GTA V Legacy installation", probe.valid_installations),
            Some(format!("{} candidate path(s); CitizenFX.ini IVPath relationships were not validated as Legacy.", probe.candidates)),
            Some(format!("Verify the Game/IVPath value in CitizenFX.ini and the referenced GTA V Legacy markers. Optional discovery override: {}.", probe.configured_environment_variable)),
            &["fivem.discover"],
        ),
        "available" => check(
            "environment.fivem",
            "environment",
            DiagnosticSeverity::Info,
            "available",
            format!("{} valid FiveM installation(s) detected", probe.valid_installations),
            Some(format!("{} candidate path(s); {} linked Legacy installation(s).", probe.candidates, probe.linked_legacy_installations.unwrap_or(0))),
            None,
            &["fivem.discover"],
        ),
        "incomplete" => check(
            "environment.fivem",
            "environment",
            DiagnosticSeverity::Warning,
            "incomplete",
            "FiveM candidates were found but required files/configuration are incomplete",
            Some(format!("Candidates: {}. Discovery only reads the root, CitizenFX.ini and metadata.", probe.candidates)),
            Some(format!("Verify FiveM.app/CitizenFX.ini and its Game/IVPath configuration, or pass explicit paths. Optional discovery override: {}.", probe.configured_environment_variable)),
            &["fivem.discover"],
        ),
        _ => check(
            "environment.fivem",
            "environment",
            DiagnosticSeverity::Info,
            "notDetected",
            "No FiveM installation was detected",
            Some("FiveM is optional; this does not disable format parsing or standalone asset workflows.".into()),
            Some(format!("When using FiveM discovery/cache integration, set {} or pass paths explicitly.", probe.configured_environment_variable)),
            &["fivem.discover"],
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn check(
    id: &'static str,
    category: &'static str,
    severity: DiagnosticSeverity,
    status: &'static str,
    summary: impl Into<String>,
    details: Option<String>,
    recommendation: Option<String>,
    affects_commands: &[&'static str],
) -> DoctorCheck {
    DoctorCheck {
        id,
        category,
        severity,
        status,
        summary: summary.into(),
        details,
        recommendation,
        affects_commands: affects_commands.to_vec(),
    }
}

pub fn print_doctor_report(
    report: &DoctorReport,
    json_output: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }

    println!("RageLab doctor — {:?}", report.status);
    println!(
        "version {} | core {}{} | API {}",
        report.build.version,
        report.build.revision,
        if report.build.dirty {
            " (dirty build)"
        } else {
            ""
        },
        report.build.api_compatibility_version
    );
    println!(
        "target {} | profile {} | features {}",
        report.build.target.triple,
        report.build.profile,
        if report.build.features.is_empty() {
            "none".to_owned()
        } else {
            report.build.features.join(",")
        }
    );
    println!(
        "compiled modules: {} | semantic asset formats: {}",
        report.compiled_modules.len(),
        report
            .asset_formats
            .iter()
            .filter(|format| format.semantic_parser)
            .count()
    );
    for item in &report.checks {
        println!("[{:?}] {} — {}", item.severity, item.id, item.summary);
        if let Some(details) = &item.details {
            println!("  {details}");
        }
        if let Some(recommendation) = &item.recommendation {
            println!("  action: {recommendation}");
        }
    }
    println!(
        "summary: {} checks, {} info, {} warning{}, {} blocker{}",
        report.summary.checks,
        report.summary.info,
        report.summary.warnings,
        if report.summary.warnings == 1 {
            ""
        } else {
            "s"
        },
        report.summary.blockers,
        if report.summary.blockers == 1 {
            ""
        } else {
            "s"
        }
    );
    println!(
        "read-only: {}; actions taken: {}",
        report.read_only,
        report.actions_taken.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ragelab_engine::core_build_metadata;

    fn probe(
        id: &'static str,
        environment_variable: &'static str,
        status: &'static str,
        candidates: usize,
        valid: usize,
        linked_legacy: Option<usize>,
    ) -> InstallationProbe {
        InstallationProbe {
            id,
            configured_environment_variable: environment_variable,
            environment_variable_set: false,
            status,
            candidates,
            valid_installations: valid,
            linked_legacy_installations: linked_legacy,
            detected_paths: Vec::new(),
            globally_required: false,
        }
    }

    fn build_snapshot(
        renderer_error: Option<&str>,
        gta: InstallationProbe,
        fivem: InstallationProbe,
        cwd: Result<PathBuf, String>,
    ) -> DoctorReport {
        build_doctor_report(
            core_build_metadata(),
            compiled_core_modules(),
            compiled_asset_formats(),
            ProbeSnapshot {
                renderer_error: renderer_error.map(str::to_owned),
                gta,
                fivem,
                current_directory: cwd,
            },
        )
    }

    #[test]
    fn missing_optional_targets_gpu_and_cwd_are_not_core_blockers() {
        let report = build_snapshot(
            Some("no compatible wgpu adapter was found"),
            probe("gtaVLegacy", GTA_V_LEGACY_ENV, "notDetected", 0, 0, None),
            probe("fivem", FIVEM_ENV, "notDetected", 0, 0, Some(0)),
            Err("working directory unavailable".into()),
        );
        assert!(report.read_only);
        assert!(report.actions_taken.is_empty());
        assert_eq!(report.status, DoctorStatus::Warning);
        assert_eq!(report.summary.blockers, 0);
        assert!(report
            .checks
            .iter()
            .any(|item| item.id == "render.wgpuDevice"
                && item.severity == DiagnosticSeverity::Warning));
        assert!(report
            .checks
            .iter()
            .any(|item| item.id == "environment.gtaVLegacy"
                && item.severity == DiagnosticSeverity::Info));
        assert!(report.checks.iter().any(
            |item| item.id == "environment.fivem" && item.severity == DiagnosticSeverity::Info
        ));
        assert!(report
            .checks
            .iter()
            .any(|item| item.id == "paths.currentDirectory"
                && item.severity == DiagnosticSeverity::Warning));
        assert!(!report.environment.filesystem.write_probe_performed);
    }

    #[test]
    fn present_optional_targets_and_renderer_are_reported_as_available() {
        let report = build_snapshot(
            None,
            probe("gtaVLegacy", GTA_V_LEGACY_ENV, "available", 1, 1, None),
            probe("fivem", FIVEM_ENV, "available", 1, 1, Some(1)),
            Ok(PathBuf::from(".")),
        );
        assert_eq!(report.environment.renderer, "ready");
        assert_eq!(report.environment.gta_v_legacy.status, "available");
        assert_eq!(report.environment.fivem.status, "available");
        assert!(report.checks.iter().any(
            |item| item.id == "render.wgpuDevice" && item.severity == DiagnosticSeverity::Info
        ));
        assert!(report
            .checks
            .iter()
            .any(|item| item.id == "environment.gtaVLegacy" && item.status == "available"));
        assert!(report
            .checks
            .iter()
            .any(|item| item.id == "environment.fivem" && item.status == "available"));
        assert!(report.environment.filesystem.current_directory_is_directory);
        assert!(!report.environment.filesystem.write_probe_performed);
    }

    #[test]
    fn incomplete_optional_targets_produce_actionable_warnings_not_blockers() {
        let report = build_snapshot(
            None,
            probe("gtaVLegacy", GTA_V_LEGACY_ENV, "incomplete", 1, 0, None),
            probe("fivem", FIVEM_ENV, "incomplete", 1, 0, Some(0)),
            Ok(PathBuf::from(".")),
        );
        assert_eq!(report.summary.blockers, 0);
        let gta = report
            .checks
            .iter()
            .find(|item| item.id == "environment.gtaVLegacy")
            .unwrap();
        assert_eq!(gta.severity, DiagnosticSeverity::Warning);
        assert!(gta.recommendation.is_some());
        let fivem = report
            .checks
            .iter()
            .find(|item| item.id == "environment.fivem")
            .unwrap();
        assert_eq!(fivem.severity, DiagnosticSeverity::Warning);
        assert!(fivem.recommendation.is_some());
    }

    #[test]
    fn fivem_without_a_valid_legacy_link_is_warning_scoped_to_fivem() {
        let report = build_snapshot(
            None,
            probe("gtaVLegacy", GTA_V_LEGACY_ENV, "notDetected", 0, 0, None),
            probe("fivem", FIVEM_ENV, "available", 1, 1, Some(0)),
            Ok(PathBuf::from(".")),
        );
        let fivem = report
            .checks
            .iter()
            .find(|item| item.id == "environment.fivem")
            .unwrap();
        assert_eq!(fivem.status, "legacyLinkMissing");
        assert_eq!(fivem.severity, DiagnosticSeverity::Warning);
        assert_eq!(fivem.affects_commands, vec!["fivem.discover"]);
        assert!(fivem.recommendation.is_some());
        assert_eq!(report.summary.blockers, 0);
    }

    #[test]
    fn missing_required_core_modules_and_formats_are_blockers() {
        let report = build_doctor_report(
            core_build_metadata(),
            &[],
            Vec::new(),
            ProbeSnapshot {
                renderer_error: Some("no adapter".into()),
                gta: probe("gtaVLegacy", GTA_V_LEGACY_ENV, "notDetected", 0, 0, None),
                fivem: probe("fivem", FIVEM_ENV, "notDetected", 0, 0, Some(0)),
                current_directory: Err("unavailable".into()),
            },
        );
        assert_eq!(report.status, DoctorStatus::Blocked);
        assert_eq!(report.summary.blockers, 2);
        assert!(report
            .checks
            .iter()
            .any(|item| item.id == "core.moduleRegistry"
                && item.severity == DiagnosticSeverity::Blocker));
        assert!(report
            .checks
            .iter()
            .any(|item| item.id == "core.semanticFormats"
                && item.severity == DiagnosticSeverity::Blocker));
    }

    #[test]
    fn doctor_contract_is_stable_read_only_and_has_no_secret_environment_values() {
        let report = build_snapshot(
            Some("adapter unavailable"),
            probe("gtaVLegacy", GTA_V_LEGACY_ENV, "notDetected", 0, 0, None),
            probe("fivem", FIVEM_ENV, "notDetected", 0, 0, Some(0)),
            Ok(PathBuf::from(".")),
        );
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["schema"], DOCTOR_SCHEMA);
        assert_eq!(json["schemaVersion"], DOCTOR_SCHEMA_VERSION);
        assert_eq!(json["readOnly"], true);
        assert_eq!(json["actionsTaken"], serde_json::json!([]));
        assert_eq!(
            json["environment"]["filesystem"]["writeProbePerformed"],
            false
        );
        assert!(json["externalDependencies"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["requiredGlobally"] == false));
        let serialized = serde_json::to_string(&json).unwrap();
        assert!(!serialized.contains("RAGELAB_GTA5_LEGACY="));
        assert!(!serialized.contains("RAGELAB_FIVEM="));
    }
}

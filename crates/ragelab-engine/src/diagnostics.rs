use serde::Serialize;

use crate::{CORE_API_COMPATIBILITY_VERSION, CORE_DIRTY, CORE_REVISION, CORE_COMPILED_MODULES};

pub const CORE_BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");
const BUILD_TARGET: &str = env!("RAGELAB_BUILD_TARGET");
const BUILD_TARGET_OS: &str = env!("RAGELAB_BUILD_TARGET_OS");
const BUILD_TARGET_ARCH: &str = env!("RAGELAB_BUILD_TARGET_ARCH");
const BUILD_TARGET_ENV: &str = env!("RAGELAB_BUILD_TARGET_ENV");
const BUILD_TARGET_FAMILY: &str = env!("RAGELAB_BUILD_TARGET_FAMILY");
const BUILD_PROFILE: &str = env!("RAGELAB_BUILD_PROFILE");
const BUILD_FEATURES: &str = env!("RAGELAB_BUILD_FEATURES");

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreTargetMetadata {
    pub triple: &'static str,
    pub operating_system: &'static str,
    pub architecture: &'static str,
    pub environment: Option<&'static str>,
    pub families: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreBuildMetadata {
    pub version: &'static str,
    pub revision: &'static str,
    pub dirty: bool,
    pub api_compatibility_version: u32,
    pub profile: &'static str,
    pub target: CoreTargetMetadata,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompiledAssetFormat {
    pub id: &'static str,
    pub extension: &'static str,
    pub semantic_parser: bool,
    pub support: &'static str,
    pub limitation: Option<&'static str>,
}

pub fn core_build_metadata() -> CoreBuildMetadata {
    CoreBuildMetadata {
        version: CORE_BUILD_VERSION,
        revision: CORE_REVISION,
        dirty: CORE_DIRTY,
        api_compatibility_version: CORE_API_COMPATIBILITY_VERSION,
        profile: BUILD_PROFILE,
        target: CoreTargetMetadata {
            triple: BUILD_TARGET,
            operating_system: BUILD_TARGET_OS,
            architecture: BUILD_TARGET_ARCH,
            environment: (BUILD_TARGET_ENV != "none").then_some(BUILD_TARGET_ENV),
            families: BUILD_TARGET_FAMILY
                .split(',')
                .filter(|family| !family.is_empty())
                .collect(),
        },
        features: BUILD_FEATURES
            .split(',')
            .filter(|feature| !feature.is_empty())
            .map(str::to_owned)
            .collect(),
    }
}

/// Module identifiers generated from the same macro invocation that declares
/// and re-exports the modules in the Engine crate root.
pub fn compiled_core_modules() -> &'static [&'static str] {
    CORE_COMPILED_MODULES
}

pub fn compiled_asset_formats() -> Vec<CompiledAssetFormat> {
    let mut formats = crate::asset::registered_asset_formats();
    if CORE_COMPILED_MODULES.contains(&"glb") {
        formats.push(CompiledAssetFormat {
            id: "GLB 2.0",
            extension: "glb",
            semantic_parser: true,
            support: "semantic",
            limitation: Some("input parser; YDR material export requires an explicit authoring profile and optional verified shader template"),
        });
    }
    formats
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_metadata_contains_the_actual_compilation_target() {
        let metadata = core_build_metadata();
        assert!(!metadata.version.is_empty());
        assert!(!metadata.revision.is_empty());
        assert!(!metadata.profile.is_empty());
        assert!(!metadata.target.triple.is_empty());
        assert!(!metadata.target.operating_system.is_empty());
        assert!(!metadata.target.architecture.is_empty());
        assert!(!metadata.target.families.is_empty());
    }

    #[test]
    fn compiled_module_registry_is_derived_from_module_declarations() {
        let modules = compiled_core_modules();
        assert!(modules.contains(&"diagnostics"));
        assert!(modules.contains(&"discovery"));
        assert!(modules.contains(&"glb"));
        assert!(modules.contains(&"ydr"));
        assert_eq!(
            modules.iter().copied().collect::<std::collections::BTreeSet<_>>().len(),
            modules.len(),
            "compiled module identifiers must be unique"
        );
    }

    #[test]
    fn asset_format_registry_distinguishes_semantic_and_container_only_formats() {
        let formats = compiled_asset_formats();
        let ydr = formats.iter().find(|format| format.id == "YDR").unwrap();
        assert!(ydr.semantic_parser);
        let glb = formats.iter().find(|format| format.id == "GLB 2.0").unwrap();
        assert!(glb.semantic_parser);
        assert_eq!(glb.extension, "glb");
        assert_eq!(ydr.support, "semantic");

        let ycd = formats.iter().find(|format| format.id == "YCD").unwrap();
        assert!(!ycd.semantic_parser);
        assert_eq!(ycd.support, "containerOnly");
        assert!(ycd.limitation.is_some());
    }
}

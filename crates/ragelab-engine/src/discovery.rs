use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

pub const GTA_V_DISCOVERY_SCHEMA: &str = "ragelab.gta.discovery";
pub const GTA_V_DISCOVERY_SCHEMA_VERSION: u64 = 1;
pub const GTA_V_LEGACY_STEAM_APP_ID: u32 = 271_590;
pub const GTA_V_ENHANCED_STEAM_APP_ID: u32 = 3_240_220;
pub const GTA_V_LEGACY_ENV: &str = "RAGELAB_GTA5_LEGACY";
pub const FIVEM_DISCOVERY_SCHEMA: &str = "ragelab.fivem.discovery";
pub const FIVEM_DISCOVERY_SCHEMA_VERSION: u64 = 1;
pub const FIVEM_ENV: &str = "RAGELAB_FIVEM";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FiveMDiscoverySource {
    ExplicitPath,
    Environment,
    LocalAppData,
}

impl FiveMDiscoverySource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitPath => "explicitPath",
            Self::Environment => "environment",
            Self::LocalAppData => "localAppData",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FiveMDiscoveryProvenance {
    pub source: FiveMDiscoverySource,
    pub reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FiveMInstallCheck {
    pub id: &'static str,
    pub required: bool,
    pub passed: bool,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FiveMStoragePath {
    pub id: &'static str,
    pub path: PathBuf,
    pub exists: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FiveMGtaRelationshipStatus {
    MissingConfiguration,
    InvalidConfiguredPath,
    ValidLegacy,
    InvalidLegacy,
    Enhanced,
    Ambiguous,
    Unknown,
}

impl FiveMGtaRelationshipStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingConfiguration => "missingConfiguration",
            Self::InvalidConfiguredPath => "invalidConfiguredPath",
            Self::ValidLegacy => "validLegacy",
            Self::InvalidLegacy => "invalidLegacy",
            Self::Enhanced => "enhanced",
            Self::Ambiguous => "ambiguous",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FiveMGtaRelationship {
    pub status: FiveMGtaRelationshipStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configured_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installation: Option<GtaVInstallation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FiveMInstallation {
    pub root: PathBuf,
    pub app_root: PathBuf,
    pub valid: bool,
    pub checks: Vec<FiveMInstallCheck>,
    pub storage_paths: Vec<FiveMStoragePath>,
    pub provenance: Vec<FiveMDiscoveryProvenance>,
    pub citizen_fx_ini: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_build_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_channel: Option<String>,
    pub gta: FiveMGtaRelationship,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FiveMDiscoveryReport {
    pub schema: &'static str,
    pub schema_version: u64,
    pub platform: String,
    pub valid_installations: usize,
    pub legacy_linked_installations: usize,
    pub candidates: Vec<FiveMInstallation>,
}

#[derive(Debug, Clone)]
struct FiveMCandidateSeed {
    root: PathBuf,
    provenance: Vec<FiveMDiscoveryProvenance>,
}

impl FiveMCandidateSeed {
    fn new(root: PathBuf, source: FiveMDiscoverySource, reference: impl Into<String>) -> Self {
        Self {
            root,
            provenance: vec![FiveMDiscoveryProvenance {
                source,
                reference: reference.into(),
            }],
        }
    }

    fn merge(&mut self, other: FiveMCandidateSeed) {
        for provenance in other.provenance {
            if !self.provenance.contains(&provenance) {
                self.provenance.push(provenance);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GtaVEdition {
    Legacy,
    Enhanced,
    Ambiguous,
    Unknown,
}

impl GtaVEdition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Enhanced => "enhanced",
            Self::Ambiguous => "ambiguous",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GtaVDiscoverySource {
    ExplicitPath,
    Environment,
    RockstarRegistry,
    SteamManifest,
    CommonPath,
}

impl GtaVDiscoverySource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitPath => "explicitPath",
            Self::Environment => "environment",
            Self::RockstarRegistry => "rockstarRegistry",
            Self::SteamManifest => "steamManifest",
            Self::CommonPath => "commonPath",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaVDiscoveryProvenance {
    pub source: GtaVDiscoverySource,
    pub reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaVInstallCheck {
    pub id: &'static str,
    pub required: bool,
    pub passed: bool,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaVInstallation {
    pub root: PathBuf,
    pub edition: GtaVEdition,
    pub valid: bool,
    pub checks: Vec<GtaVInstallCheck>,
    pub provenance: Vec<GtaVDiscoveryProvenance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steam_app_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steam_build_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steam_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaVDiscoveryTarget {
    pub product: &'static str,
    pub edition: &'static str,
    pub steam_app_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaVDiscoveryReport {
    pub schema: &'static str,
    pub schema_version: u64,
    pub platform: String,
    pub target: GtaVDiscoveryTarget,
    pub valid_legacy_installations: usize,
    pub candidates: Vec<GtaVInstallation>,
}

#[derive(Debug, Clone)]
struct CandidateSeed {
    root: PathBuf,
    provenance: Vec<GtaVDiscoveryProvenance>,
    steam_app_id: Option<u32>,
    steam_build_id: Option<String>,
    steam_name: Option<String>,
}

impl CandidateSeed {
    fn new(root: PathBuf, source: GtaVDiscoverySource, reference: impl Into<String>) -> Self {
        Self {
            root,
            provenance: vec![GtaVDiscoveryProvenance {
                source,
                reference: reference.into(),
            }],
            steam_app_id: None,
            steam_build_id: None,
            steam_name: None,
        }
    }

    fn merge(&mut self, other: CandidateSeed) {
        for provenance in other.provenance {
            if !self.provenance.contains(&provenance) {
                self.provenance.push(provenance);
            }
        }
        if self.steam_app_id.is_none() {
            self.steam_app_id = other.steam_app_id;
        }
        if self.steam_build_id.is_none() {
            self.steam_build_id = other.steam_build_id;
        }
        if self.steam_name.is_none() {
            self.steam_name = other.steam_name;
        }
    }
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct SteamManifest {
    app_id: u32,
    install_dir: String,
    build_id: Option<String>,
    name: Option<String>,
}

pub fn discover_gta_v_legacy() -> GtaVDiscoveryReport {
    let mut seeds = Vec::new();

    if let Some(root) = env::var_os(GTA_V_LEGACY_ENV).filter(|value| !value.is_empty()) {
        seeds.push(CandidateSeed::new(
            PathBuf::from(root),
            GtaVDiscoverySource::Environment,
            GTA_V_LEGACY_ENV,
        ));
    }

    #[cfg(windows)]
    {
        seeds.extend(discover_windows_registry_candidates());
        seeds.extend(discover_windows_steam_candidates());
        seeds.extend(discover_windows_common_path_candidates());
    }

    let mut merged = BTreeMap::<String, CandidateSeed>::new();
    for seed in seeds {
        let key = candidate_key(&seed.root);
        if let Some(existing) = merged.get_mut(&key) {
            existing.merge(seed);
        } else {
            merged.insert(key, seed);
        }
    }

    let mut candidates = merged
        .into_values()
        .map(validate_candidate)
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right.valid.cmp(&left.valid).then_with(|| {
            left.root
                .to_string_lossy()
                .to_ascii_lowercase()
                .cmp(&right.root.to_string_lossy().to_ascii_lowercase())
        })
    });

    let valid_legacy_installations = candidates
        .iter()
        .filter(|candidate| candidate.valid)
        .count();

    GtaVDiscoveryReport {
        schema: GTA_V_DISCOVERY_SCHEMA,
        schema_version: GTA_V_DISCOVERY_SCHEMA_VERSION,
        platform: env::consts::OS.to_string(),
        target: GtaVDiscoveryTarget {
            product: "gtaV",
            edition: "legacy",
            steam_app_id: GTA_V_LEGACY_STEAM_APP_ID,
        },
        valid_legacy_installations,
        candidates,
    }
}

pub fn validate_gta_v_installation(root: &Path) -> GtaVInstallation {
    validate_candidate(CandidateSeed::new(
        root.to_path_buf(),
        GtaVDiscoverySource::ExplicitPath,
        "explicit path",
    ))
}

fn validate_candidate(seed: CandidateSeed) -> GtaVInstallation {
    let root = fs::canonicalize(&seed.root)
        .map(clean_display_path)
        .unwrap_or_else(|_| clean_display_path(seed.root.clone()));
    let legacy_executable = root.join("GTA5.exe");
    let enhanced_executable = root.join("GTA5_Enhanced.exe");
    let launcher = root.join("PlayGTAV.exe");
    let common_archive = root.join("common.rpf");
    let x64_archive = root.join("x64a.rpf");
    let update_archive = root.join("update").join("update.rpf");

    let root_exists = root.is_dir();
    let legacy_exists = legacy_executable.is_file();
    let enhanced_exists = enhanced_executable.is_file();

    let edition = match (legacy_exists, enhanced_exists) {
        (true, false) => GtaVEdition::Legacy,
        (false, true) => GtaVEdition::Enhanced,
        (true, true) => GtaVEdition::Ambiguous,
        (false, false) => GtaVEdition::Unknown,
    };

    let checks = vec![
        GtaVInstallCheck {
            id: "rootDirectory",
            required: true,
            passed: root_exists,
            path: root.clone(),
        },
        GtaVInstallCheck {
            id: "legacyExecutable",
            required: true,
            passed: legacy_exists,
            path: legacy_executable,
        },
        GtaVInstallCheck {
            id: "enhancedExecutableAbsent",
            required: true,
            passed: !enhanced_exists,
            path: enhanced_executable,
        },
        GtaVInstallCheck {
            id: "commonArchive",
            required: true,
            passed: common_archive.is_file(),
            path: common_archive,
        },
        GtaVInstallCheck {
            id: "x64Archive",
            required: true,
            passed: x64_archive.is_file(),
            path: x64_archive,
        },
        GtaVInstallCheck {
            id: "updateArchive",
            required: true,
            passed: update_archive.is_file(),
            path: update_archive,
        },
        GtaVInstallCheck {
            id: "launcher",
            required: false,
            passed: launcher.is_file(),
            path: launcher,
        },
    ];

    let valid = edition == GtaVEdition::Legacy
        && checks
            .iter()
            .filter(|check| check.required)
            .all(|check| check.passed);

    GtaVInstallation {
        root,
        edition,
        valid,
        checks,
        provenance: seed.provenance,
        steam_app_id: seed.steam_app_id,
        steam_build_id: seed.steam_build_id,
        steam_name: seed.steam_name,
    }
}

fn candidate_key(root: &Path) -> String {
    let normalized = fs::canonicalize(root)
        .map(clean_display_path)
        .unwrap_or_else(|_| clean_display_path(root.to_path_buf()))
        .to_string_lossy()
        .replace('/', "\\");

    #[cfg(windows)]
    {
        normalized.to_ascii_lowercase()
    }

    #[cfg(not(windows))]
    {
        normalized
    }
}

pub(crate) fn clean_display_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(stripped) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{stripped}"));
        }
        if let Some(stripped) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(stripped);
        }
    }

    path
}

#[cfg(any(windows, test))]
fn parse_vdf_field(input: &str, key: &str) -> Option<String> {
    input.lines().find_map(|line| {
        let mut quoted = line.split('"').skip(1);
        let field = quoted.next()?;
        let _between = quoted.next()?;
        let value = quoted.next()?;
        if field.eq_ignore_ascii_case(key) {
            Some(value.replace(r"\\", r"\"))
        } else {
            None
        }
    })
}

#[cfg(any(windows, test))]
fn parse_steam_manifest(input: &str) -> Option<SteamManifest> {
    let app_id = parse_vdf_field(input, "appid")?.parse::<u32>().ok()?;
    let install_dir = parse_vdf_field(input, "installdir")?;
    if install_dir.trim().is_empty() {
        return None;
    }

    Some(SteamManifest {
        app_id,
        install_dir,
        build_id: parse_vdf_field(input, "buildid"),
        name: parse_vdf_field(input, "name"),
    })
}

#[cfg(any(windows, test))]
fn steam_library_paths(input: &str) -> Vec<PathBuf> {
    let mut libraries = Vec::new();
    for line in input.lines() {
        let mut quoted = line.split('"').skip(1);
        let Some(field) = quoted.next() else {
            continue;
        };
        let _between = quoted.next();
        let Some(value) = quoted.next() else {
            continue;
        };
        if !field.eq_ignore_ascii_case("path") {
            continue;
        }
        let path = PathBuf::from(value.replace(r"\\", r"\"));
        if !libraries.contains(&path) {
            libraries.push(path);
        }
    }
    libraries
}

#[cfg(windows)]
fn discover_windows_registry_candidates() -> Vec<CandidateSeed> {
    use winreg::{
        enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY},
        RegKey,
    };

    const KEY_PATH: &str = r"SOFTWARE\Rockstar Games\Grand Theft Auto V";
    const VALUE_NAMES: &[&str] = &["InstallFolderSteam", "InstallFolder", "InstallFolderEpic"];

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let mut candidates = Vec::new();

    for (view_name, view_flag) in [("32-bit", KEY_WOW64_32KEY), ("64-bit", KEY_WOW64_64KEY)] {
        let Ok(key) = hklm.open_subkey_with_flags(KEY_PATH, KEY_READ | view_flag) else {
            continue;
        };
        for value_name in VALUE_NAMES {
            let Ok(value) = key.get_value::<String, _>(*value_name) else {
                continue;
            };
            let value = value.trim().trim_matches('"');
            if value.is_empty() {
                continue;
            }
            candidates.push(CandidateSeed::new(
                PathBuf::from(value),
                GtaVDiscoverySource::RockstarRegistry,
                format!(r"HKLM[{view_name}]\{KEY_PATH}::{value_name}"),
            ));
        }
    }

    candidates
}

#[cfg(windows)]
fn discover_windows_steam_candidates() -> Vec<CandidateSeed> {
    let mut steam_roots = windows_steam_roots();
    dedupe_paths(&mut steam_roots);

    let mut candidates = Vec::new();
    for steam_root in steam_roots {
        let mut libraries = vec![steam_root.clone()];
        let library_file = steam_root.join("steamapps").join("libraryfolders.vdf");
        if let Ok(body) = fs::read_to_string(&library_file) {
            libraries.extend(steam_library_paths(&body));
        }
        dedupe_paths(&mut libraries);

        for library in libraries {
            let manifest_path = library
                .join("steamapps")
                .join(format!("appmanifest_{GTA_V_LEGACY_STEAM_APP_ID}.acf"));
            let Ok(body) = fs::read_to_string(&manifest_path) else {
                continue;
            };
            let Some(manifest) = parse_steam_manifest(&body) else {
                continue;
            };
            if manifest.app_id != GTA_V_LEGACY_STEAM_APP_ID {
                continue;
            }

            let root = library
                .join("steamapps")
                .join("common")
                .join(&manifest.install_dir);
            let mut seed = CandidateSeed::new(
                root,
                GtaVDiscoverySource::SteamManifest,
                manifest_path.display().to_string(),
            );
            seed.steam_app_id = Some(manifest.app_id);
            seed.steam_build_id = manifest.build_id;
            seed.steam_name = manifest.name;
            candidates.push(seed);
        }
    }

    candidates
}

#[cfg(windows)]
fn windows_steam_roots() -> Vec<PathBuf> {
    use winreg::{
        enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY},
        RegKey,
    };

    let mut roots = Vec::new();

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(key) = hkcu.open_subkey_with_flags(r"Software\Valve\Steam", KEY_READ) {
        if let Ok(path) = key.get_value::<String, _>("SteamPath") {
            roots.push(PathBuf::from(path.replace('/', r"")));
        }
    }

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    if let Ok(key) =
        hklm.open_subkey_with_flags(r"SOFTWARE\Valve\Steam", KEY_READ | KEY_WOW64_32KEY)
    {
        if let Ok(path) = key.get_value::<String, _>("InstallPath") {
            roots.push(PathBuf::from(path.replace('/', r"")));
        }
    }

    if let Some(program_files_x86) = env::var_os("ProgramFiles(x86)") {
        roots.push(PathBuf::from(program_files_x86).join("Steam"));
    }

    roots
}

#[cfg(windows)]
fn discover_windows_common_path_candidates() -> Vec<CandidateSeed> {
    let mut candidates = Vec::new();

    if let Some(program_files_x86) = env::var_os("ProgramFiles(x86)") {
        let steam = PathBuf::from(program_files_x86)
            .join("Steam")
            .join("steamapps")
            .join("common")
            .join("Grand Theft Auto V");
        if steam.is_dir() {
            candidates.push(CandidateSeed::new(
                steam,
                GtaVDiscoverySource::CommonPath,
                "ProgramFiles(x86) Steam default library",
            ));
        }
    }

    if let Some(program_files) = env::var_os("ProgramFiles") {
        let program_files = PathBuf::from(program_files);
        for (root, reference) in [
            (
                program_files
                    .join("Rockstar Games")
                    .join("Grand Theft Auto V"),
                "ProgramFiles Rockstar Games default",
            ),
            (
                program_files.join("Epic Games").join("GTAV"),
                "ProgramFiles Epic Games GTAV default",
            ),
            (
                program_files.join("Epic Games").join("Grand Theft Auto V"),
                "ProgramFiles Epic Games Grand Theft Auto V default",
            ),
        ] {
            if root.is_dir() {
                candidates.push(CandidateSeed::new(
                    root,
                    GtaVDiscoverySource::CommonPath,
                    reference,
                ));
            }
        }
    }

    candidates
}

#[cfg(windows)]
fn dedupe_paths(paths: &mut Vec<PathBuf>) {
    let mut seen = BTreeMap::<String, ()>::new();
    paths.retain(|path| seen.insert(candidate_key(path), ()).is_none());
}

pub fn discover_fivem_legacy() -> FiveMDiscoveryReport {
    let mut seeds = Vec::new();

    if let Some(root) = env::var_os(FIVEM_ENV).filter(|value| !value.is_empty()) {
        seeds.push(FiveMCandidateSeed::new(
            PathBuf::from(root),
            FiveMDiscoverySource::Environment,
            FIVEM_ENV,
        ));
    }

    #[cfg(windows)]
    if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
        let root = PathBuf::from(local_app_data).join("FiveM");
        if root.is_dir() || root.join("FiveM.app").is_dir() {
            seeds.push(FiveMCandidateSeed::new(
                root,
                FiveMDiscoverySource::LocalAppData,
                r"%LOCALAPPDATA%\FiveM",
            ));
        }
    }

    let mut merged = BTreeMap::<String, FiveMCandidateSeed>::new();
    for mut seed in seeds {
        let (root, _) = normalize_fivem_roots(&seed.root);
        seed.root = root;
        let key = candidate_key(&seed.root);
        if let Some(existing) = merged.get_mut(&key) {
            existing.merge(seed);
        } else {
            merged.insert(key, seed);
        }
    }

    let mut candidates = merged
        .into_values()
        .map(validate_fivem_candidate)
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right.valid.cmp(&left.valid).then_with(|| {
            left.root
                .to_string_lossy()
                .to_ascii_lowercase()
                .cmp(&right.root.to_string_lossy().to_ascii_lowercase())
        })
    });

    let valid_installations = candidates
        .iter()
        .filter(|candidate| candidate.valid)
        .count();
    let legacy_linked_installations = candidates
        .iter()
        .filter(|candidate| candidate.gta.status == FiveMGtaRelationshipStatus::ValidLegacy)
        .count();

    FiveMDiscoveryReport {
        schema: FIVEM_DISCOVERY_SCHEMA,
        schema_version: FIVEM_DISCOVERY_SCHEMA_VERSION,
        platform: env::consts::OS.to_string(),
        valid_installations,
        legacy_linked_installations,
        candidates,
    }
}

pub fn validate_fivem_installation(root: &Path) -> FiveMInstallation {
    validate_fivem_candidate(FiveMCandidateSeed::new(
        root.to_path_buf(),
        FiveMDiscoverySource::ExplicitPath,
        "explicit path",
    ))
}

fn validate_fivem_candidate(seed: FiveMCandidateSeed) -> FiveMInstallation {
    let (root, app_root) = normalize_fivem_roots(&seed.root);
    let root = fs::canonicalize(&root)
        .map(clean_display_path)
        .unwrap_or_else(|_| clean_display_path(root));
    let app_root = fs::canonicalize(&app_root)
        .map(clean_display_path)
        .unwrap_or_else(|_| clean_display_path(app_root));

    let citizen_fx_ini = app_root.join("CitizenFX.ini");
    let executable = root.join("FiveM.exe");
    let data_root = app_root.join("data");
    let game_storage = data_root.join("game-storage");
    let cache_root = data_root.join("cache");
    let citizen_root = app_root.join("citizen");

    let ini_body = fs::read_to_string(&citizen_fx_ini).ok();
    let iv_path = ini_body
        .as_deref()
        .and_then(|body| parse_citizen_fx_game_value(body, "IVPath"))
        .map(PathBuf::from);
    let saved_build_number = ini_body
        .as_deref()
        .and_then(|body| parse_citizen_fx_game_value(body, "SavedBuildNumber"));
    let update_channel = ini_body
        .as_deref()
        .and_then(|body| parse_citizen_fx_game_value(body, "UpdateChannel"));

    let checks = vec![
        FiveMInstallCheck {
            id: "rootDirectory",
            required: true,
            passed: root.is_dir(),
            path: root.clone(),
        },
        FiveMInstallCheck {
            id: "appDirectory",
            required: true,
            passed: app_root.is_dir(),
            path: app_root.clone(),
        },
        FiveMInstallCheck {
            id: "citizenFxIni",
            required: true,
            passed: citizen_fx_ini.is_file(),
            path: citizen_fx_ini.clone(),
        },
        FiveMInstallCheck {
            id: "citizenFxIniReadable",
            required: true,
            passed: ini_body.is_some(),
            path: citizen_fx_ini.clone(),
        },
        FiveMInstallCheck {
            id: "executable",
            required: false,
            passed: executable.is_file(),
            path: executable,
        },
    ];

    let valid = checks
        .iter()
        .filter(|check| check.required)
        .all(|check| check.passed);

    let storage_paths = vec![
        FiveMStoragePath {
            id: "data",
            exists: data_root.is_dir(),
            path: data_root,
        },
        FiveMStoragePath {
            id: "gameStorage",
            exists: game_storage.is_dir(),
            path: game_storage,
        },
        FiveMStoragePath {
            id: "cache",
            exists: cache_root.is_dir(),
            path: cache_root,
        },
        FiveMStoragePath {
            id: "citizen",
            exists: citizen_root.is_dir(),
            path: citizen_root,
        },
    ];

    let gta = fivem_gta_relationship(iv_path, &citizen_fx_ini);

    FiveMInstallation {
        root,
        app_root,
        valid,
        checks,
        storage_paths,
        provenance: seed.provenance,
        citizen_fx_ini,
        saved_build_number,
        update_channel,
        gta,
    }
}

fn normalize_fivem_roots(path: &Path) -> (PathBuf, PathBuf) {
    let path = clean_display_path(path.to_path_buf());
    let is_app_root = path
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("FiveM.app"));

    if is_app_root {
        let root = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.clone());
        (root, path)
    } else {
        let app_root = path.join("FiveM.app");
        (path, app_root)
    }
}

fn parse_citizen_fx_game_value(input: &str, key: &str) -> Option<String> {
    let mut in_game_section = false;

    for raw_line in input.lines() {
        let line = raw_line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_game_section = line[1..line.len() - 1].trim().eq_ignore_ascii_case("Game");
            continue;
        }
        if !in_game_section {
            continue;
        }

        let Some((field, value)) = line.split_once('=') else {
            continue;
        };
        if !field.trim().eq_ignore_ascii_case(key) {
            continue;
        }

        let value = value.trim().trim_matches('"').trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }

    None
}

fn fivem_gta_relationship(
    configured_path: Option<PathBuf>,
    citizen_fx_ini: &Path,
) -> FiveMGtaRelationship {
    let evidence = citizen_fx_ini
        .is_file()
        .then(|| citizen_fx_ini.to_path_buf());

    let Some(configured_path) = configured_path else {
        return FiveMGtaRelationship {
            status: FiveMGtaRelationshipStatus::MissingConfiguration,
            configured_path: None,
            evidence,
            installation: None,
        };
    };

    if !configured_path.is_absolute() {
        return FiveMGtaRelationship {
            status: FiveMGtaRelationshipStatus::InvalidConfiguredPath,
            configured_path: Some(configured_path),
            evidence,
            installation: None,
        };
    }

    let installation = validate_gta_v_installation(&configured_path);
    let status = match installation.edition {
        GtaVEdition::Legacy if installation.valid => FiveMGtaRelationshipStatus::ValidLegacy,
        GtaVEdition::Legacy => FiveMGtaRelationshipStatus::InvalidLegacy,
        GtaVEdition::Enhanced => FiveMGtaRelationshipStatus::Enhanced,
        GtaVEdition::Ambiguous => FiveMGtaRelationshipStatus::Ambiguous,
        GtaVEdition::Unknown => FiveMGtaRelationshipStatus::Unknown,
    };

    FiveMGtaRelationship {
        status,
        configured_path: Some(configured_path),
        evidence,
        installation: Some(installation),
    }
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
        env::temp_dir().join(format!(
            "ragelab-discovery-{label}-{}-{nonce}",
            process::id()
        ))
    }

    fn touch(root: &Path, relative: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, []).unwrap();
    }

    fn create_legacy_fixture(root: &Path) {
        fs::create_dir_all(root).unwrap();
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

    fn create_fivem_fixture(root: &Path, gta_root: &Path) {
        let app_root = root.join("FiveM.app");
        fs::create_dir_all(app_root.join("data").join("game-storage")).unwrap();
        fs::create_dir_all(app_root.join("citizen")).unwrap();
        touch(root, "FiveM.exe");
        fs::write(
            app_root.join("CitizenFX.ini"),
            format!(
                "[Game]\nIVPath={}\nSavedBuildNumber=3095\nUpdateChannel=production\n",
                gta_root.display()
            ),
        )
        .unwrap();
    }

    #[test]
    fn validates_legacy_installation_markers() {
        let root = temp_root("legacy");
        create_legacy_fixture(&root);

        let installation = validate_gta_v_installation(&root);
        assert_eq!(installation.edition, GtaVEdition::Legacy);
        assert!(installation.valid);
        assert!(installation
            .checks
            .iter()
            .all(|check| !check.required || check.passed));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_ambiguous_legacy_and_enhanced_executables() {
        let root = temp_root("ambiguous");
        create_legacy_fixture(&root);
        touch(&root, "GTA5_Enhanced.exe");

        let installation = validate_gta_v_installation(&root);
        assert_eq!(installation.edition, GtaVEdition::Ambiguous);
        assert!(!installation.valid);
        assert!(
            !installation
                .checks
                .iter()
                .find(|check| check.id == "enhancedExecutableAbsent")
                .unwrap()
                .passed
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_incomplete_legacy_installation() {
        let root = temp_root("incomplete");
        fs::create_dir_all(&root).unwrap();
        touch(&root, "GTA5.exe");
        touch(&root, "common.rpf");

        let installation = validate_gta_v_installation(&root);
        assert_eq!(installation.edition, GtaVEdition::Legacy);
        assert!(!installation.valid);
        assert!(
            !installation
                .checks
                .iter()
                .find(|check| check.id == "updateArchive")
                .unwrap()
                .passed
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_legacy_steam_manifest_fields() {
        let manifest = parse_steam_manifest(
            r#"
"AppState"
{
    "appid" "271590"
    "name" "Grand Theft Auto V Legacy"
    "installdir" "Grand Theft Auto V"
    "buildid" "24129523"
}
"#,
        )
        .unwrap();

        assert_eq!(manifest.app_id, GTA_V_LEGACY_STEAM_APP_ID);
        assert_eq!(manifest.install_dir, "Grand Theft Auto V");
        assert_eq!(manifest.build_id.as_deref(), Some("24129523"));
        assert_eq!(manifest.name.as_deref(), Some("Grand Theft Auto V Legacy"));
    }

    #[test]
    fn parses_modern_steam_library_paths() {
        let libraries = steam_library_paths(
            r#"
"libraryfolders"
{
    "0"
    {
        "path" "C:\\Program Files (x86)\\Steam"
    }
    "1"
    {
        "path" "D:\\SteamLibrary"
    }
}
"#,
        );

        assert_eq!(
            libraries,
            vec![
                PathBuf::from(r"C:\Program Files (x86)\Steam"),
                PathBuf::from(r"D:\SteamLibrary"),
            ]
        );
    }

    #[test]
    fn merges_candidate_provenance_without_duplicates() {
        let root = PathBuf::from(r"C:\Games\Grand Theft Auto V");
        let mut left = CandidateSeed::new(
            root.clone(),
            GtaVDiscoverySource::Environment,
            GTA_V_LEGACY_ENV,
        );
        left.merge(CandidateSeed::new(
            root,
            GtaVDiscoverySource::SteamManifest,
            "appmanifest_271590.acf",
        ));
        left.merge(CandidateSeed::new(
            PathBuf::from(r"C:\Games\Grand Theft Auto V"),
            GtaVDiscoverySource::SteamManifest,
            "appmanifest_271590.acf",
        ));

        assert_eq!(left.provenance.len(), 2);
    }

    #[test]
    fn parses_citizenfx_game_fields() {
        let body = "\u{feff}[Game]\nIVPath=\"C:\\Games\\Grand Theft Auto V\"\nSavedBuildNumber=3095\nUpdateChannel=production\n";

        assert_eq!(
            parse_citizen_fx_game_value(body, "IVPath").as_deref(),
            Some(r"C:\Games\Grand Theft Auto V")
        );
        assert_eq!(
            parse_citizen_fx_game_value(body, "savedbuildnumber").as_deref(),
            Some("3095")
        );
        assert_eq!(
            parse_citizen_fx_game_value(body, "UpdateChannel").as_deref(),
            Some("production")
        );
    }

    #[test]
    fn validates_fivem_and_legacy_ivpath_relationship() {
        let base = temp_root("fivem-legacy");
        let gta_root = base.join("gta");
        let fivem_root = base.join("FiveM");
        create_legacy_fixture(&gta_root);
        create_fivem_fixture(&fivem_root, &gta_root);

        let installation = validate_fivem_installation(&fivem_root);
        assert!(installation.valid);
        assert_eq!(
            installation.gta.status,
            FiveMGtaRelationshipStatus::ValidLegacy
        );
        assert!(installation
            .gta
            .installation
            .as_ref()
            .is_some_and(|gta| gta.valid && gta.edition == GtaVEdition::Legacy));
        assert_eq!(installation.saved_build_number.as_deref(), Some("3095"));
        assert_eq!(installation.update_channel.as_deref(), Some("production"));
        assert!(installation
            .storage_paths
            .iter()
            .find(|path| path.id == "gameStorage")
            .is_some_and(|path| path.exists));

        let from_app_root = validate_fivem_installation(&fivem_root.join("FiveM.app"));
        assert!(from_app_root.valid);
        assert_eq!(
            from_app_root.gta.status,
            FiveMGtaRelationshipStatus::ValidLegacy
        );

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn rejects_incomplete_fivem_installation_and_relative_ivpath() {
        let base = temp_root("fivem-invalid");
        let fivem_root = base.join("FiveM");
        let app_root = fivem_root.join("FiveM.app");
        fs::create_dir_all(&app_root).unwrap();
        fs::write(
            app_root.join("CitizenFX.ini"),
            "[Game]\nIVPath=relative\\gta\n",
        )
        .unwrap();

        let installation = validate_fivem_installation(&fivem_root);
        assert!(installation.valid);
        assert_eq!(
            installation.gta.status,
            FiveMGtaRelationshipStatus::InvalidConfiguredPath
        );
        assert!(
            !installation
                .checks
                .iter()
                .find(|check| check.id == "executable")
                .unwrap()
                .passed
        );

        fs::remove_file(app_root.join("CitizenFX.ini")).unwrap();
        let incomplete = validate_fivem_installation(&fivem_root);
        assert!(!incomplete.valid);
        assert_eq!(
            incomplete.gta.status,
            FiveMGtaRelationshipStatus::MissingConfiguration
        );

        fs::remove_dir_all(base).unwrap();
    }
}

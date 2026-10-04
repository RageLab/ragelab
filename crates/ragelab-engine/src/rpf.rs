use std::{
    io,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use ragelab_rpf::{GtaKeyStore, Rpf7Archive};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaRpfKeyCacheReport {
    pub schema: &'static str,
    pub schema_version: u32,
    pub executable: String,
    pub cache: String,
    pub cache_hit: bool,
}

pub fn prepare_gta_rpf_keys(
    exe_path: &Path,
    cache_root: &Path,
) -> Result<GtaRpfKeyCacheReport, io::Error> {
    if !exe_path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("GTA executable not found: {}", exe_path.display()),
        ));
    }

    let (_, cache_hit, cache_dir) = GtaKeyStore::load_or_extract_cached(exe_path, cache_root)
        .map_err(|error| io::Error::other(error.to_string()))?;

    Ok(GtaRpfKeyCacheReport {
        schema: "ragelab.gta.rpf-keys",
        schema_version: 1,
        executable: exe_path.display().to_string(),
        cache: cache_dir.display().to_string(),
        cache_hit,
    })
}

pub fn gta_rpf_key_cache_path(exe_path: &Path, cache_root: &Path) -> Result<PathBuf, io::Error> {
    let metadata = std::fs::metadata(exe_path)?;
    let modified = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?
        .as_secs();
    Ok(cache_root.join(format!("{}-{modified}", metadata.len())))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaRpfArchiveSpec {
    pub path: String,
    pub relative_path: String,
    pub tier: &'static str,
    pub dlc_pack: Option<String>,
    pub load_rank: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GtaRpfArchiveOrderReport {
    pub schema: &'static str,
    pub schema_version: u32,
    pub game_root: String,
    pub archives: Vec<GtaRpfArchiveSpec>,
    pub platform_packs: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn gta_rpf_archive_order(
    game_root: &Path,
    keys_path: &Path,
) -> Result<GtaRpfArchiveOrderReport, io::Error> {
    if !game_root.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("GTA Legacy root not found: {}", game_root.display()),
        ));
    }

    let keys = GtaKeyStore::load(keys_path).map_err(|error| io::Error::other(error.to_string()))?;
    let mut archives = Vec::new();
    let mut warnings = Vec::new();
    let mut platform_packs = Vec::new();
    let mut load_rank = 0_u32;

    let mut base_archives = std::fs::read_dir(game_root)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("rpf"))
        })
        .collect::<Vec<_>>();
    base_archives.sort_by_key(|path| {
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
    });
    for path in base_archives {
        push_archive(
            game_root,
            &path,
            "base",
            None,
            &mut load_rank,
            &mut archives,
        )?;
    }

    let update_root = game_root.join("update");
    for name in ["update.rpf", "update2.rpf"] {
        let path = update_root.join(name);
        if path.is_file() {
            push_archive(
                game_root,
                &path,
                "update",
                None,
                &mut load_rank,
                &mut archives,
            )?;
        }
    }

    let update_rpf = update_root.join("update.rpf");
    let dlclist = if update_rpf.is_file() {
        let archive = Rpf7Archive::open(&update_rpf, Some(&keys))
            .map_err(|error| io::Error::other(error.to_string()))?;
        let bytes = archive
            .read_file("common/data/dlclist.xml", Some(&keys))
            .map_err(|error| io::Error::other(error.to_string()))?;
        String::from_utf8(bytes).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("dlclist.xml is not UTF-8: {error}"),
            )
        })?
    } else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("update.rpf not found: {}", update_rpf.display()),
        ));
    };

    let pack_paths = parse_dlclist_paths(&dlclist);
    let dlcpacks_root = update_root.join("x64").join("dlcpacks");
    let mut pack_dirs = std::collections::BTreeMap::<String, PathBuf>::new();
    if dlcpacks_root.is_dir() {
        for entry in std::fs::read_dir(&dlcpacks_root)?.filter_map(Result::ok) {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            pack_dirs.insert(name.to_ascii_lowercase(), path);
        }
    }

    let mut referenced_filesystem_packs = std::collections::BTreeSet::new();
    for raw in pack_paths {
        let lower = raw.to_ascii_lowercase();
        if let Some(pack) = dlc_pack_name(&lower, "platform:/dlcpacks/") {
            platform_packs.push(pack);
            continue;
        }

        let Some(pack) = dlc_pack_name(&lower, "dlcpacks:/") else {
            warnings.push(format!("unsupported dlclist path: {raw}"));
            continue;
        };
        referenced_filesystem_packs.insert(pack.clone());

        let Some(dir) = pack_dirs.get(&pack) else {
            warnings.push(format!("dlclist pack directory not found: {pack}"));
            continue;
        };

        let mut pack_archives = std::fs::read_dir(dir)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .extension()
                        .and_then(|value| value.to_str())
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("rpf"))
            })
            .collect::<Vec<_>>();
        pack_archives.sort_by_key(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase()
        });

        if pack_archives.is_empty() {
            warnings.push(format!("dlclist pack has no outer RPF archive: {pack}"));
            continue;
        }

        for path in pack_archives {
            push_archive(
                game_root,
                &path,
                "dlc",
                Some(pack.clone()),
                &mut load_rank,
                &mut archives,
            )?;
        }
    }

    for pack in pack_dirs.keys() {
        if !referenced_filesystem_packs.contains(pack) {
            warnings.push(format!(
                "filesystem DLC pack is not present in dlclist.xml: {pack}"
            ));
        }
    }

    Ok(GtaRpfArchiveOrderReport {
        schema: "ragelab.gta.rpf-order",
        schema_version: 1,
        game_root: game_root.display().to_string(),
        archives,
        platform_packs,
        warnings,
    })
}

fn push_archive(
    game_root: &Path,
    path: &Path,
    tier: &'static str,
    dlc_pack: Option<String>,
    load_rank: &mut u32,
    archives: &mut Vec<GtaRpfArchiveSpec>,
) -> Result<(), io::Error> {
    let relative_path = path.strip_prefix(game_root).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "RPF archive {} is outside GTA root {}",
                path.display(),
                game_root.display()
            ),
        )
    })?;

    archives.push(GtaRpfArchiveSpec {
        path: path.display().to_string(),
        relative_path: relative_path.to_string_lossy().replace('\\', "/"),
        tier,
        dlc_pack,
        load_rank: *load_rank,
    });
    *load_rank = load_rank.saturating_add(1);
    Ok(())
}

fn parse_dlclist_paths(xml: &str) -> Vec<String> {
    let lower = xml.to_ascii_lowercase();
    let mut values = Vec::new();
    let mut cursor = 0_usize;

    while let Some(relative_start) = lower[cursor..].find("<item>") {
        let start = cursor + relative_start + "<item>".len();
        let Some(relative_end) = lower[start..].find("</item>") else {
            break;
        };
        let end = start + relative_end;
        let value = xml[start..end].trim();
        if !value.is_empty() {
            values.push(value.to_string());
        }
        cursor = end + "</item>".len();
    }

    values
}

fn dlc_pack_name(path: &str, prefix: &str) -> Option<String> {
    let rest = path.strip_prefix(prefix)?;
    let name = rest.trim_matches('/');
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dlclist_paths_case_insensitively_and_in_order() {
        let xml = r#"<Paths>
            <Item>platform:/dlcPacks/mpBeach/</Item>
            <item>dlcpacks:/patchday27ng/</item>
            <ITEM>dlcpacks:/mp2026_01/</ITEM>
        </Paths>"#;
        assert_eq!(
            parse_dlclist_paths(xml),
            vec![
                "platform:/dlcPacks/mpBeach/",
                "dlcpacks:/patchday27ng/",
                "dlcpacks:/mp2026_01/",
            ]
        );
    }

    #[test]
    fn normalizes_dlclist_pack_names() {
        assert_eq!(
            dlc_pack_name("dlcpacks:/patchday27ng/", "dlcpacks:/"),
            Some("patchday27ng".into())
        );
        assert_eq!(
            dlc_pack_name("platform:/dlcpacks/mpbeach/", "platform:/dlcpacks/"),
            Some("mpbeach".into())
        );
        assert_eq!(dlc_pack_name("mods:/custom/", "dlcpacks:/"), None);
    }

    #[test]
    fn prepare_keys_reuses_valid_cache_without_executable_scanning() {
        let root =
            std::env::temp_dir().join(format!("ragelab-engine-rpf-keys-{}", std::process::id()));
        let exe = root.join("GTA5.exe");
        let cache_root = root.join("cache");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create fixture root");
        std::fs::write(&exe, b"synthetic-executable").expect("write synthetic exe");

        let cache_dir = gta_rpf_key_cache_path(&exe, &cache_root).expect("derive cache dir");
        std::fs::create_dir_all(&cache_dir).expect("create key cache");
        std::fs::write(cache_dir.join("gtav_aes_key.dat"), vec![0x11_u8; 32])
            .expect("write aes key");
        std::fs::write(cache_dir.join("gtav_ng_key.dat"), vec![0x22_u8; 101 * 272])
            .expect("write ng keys");
        std::fs::write(
            cache_dir.join("gtav_ng_decrypt_tables.dat"),
            vec![0x33_u8; 17 * 16 * 256 * 4],
        )
        .expect("write ng tables");

        let report = prepare_gta_rpf_keys(&exe, &cache_root).expect("reuse valid key cache");
        assert_eq!(report.schema, "ragelab.gta.rpf-keys");
        assert_eq!(report.schema_version, 1);
        assert!(report.cache_hit);
        assert_eq!(PathBuf::from(report.cache), cache_dir);

        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn prepare_keys_rejects_missing_executable() {
        let missing = std::env::temp_dir().join("ragelab-missing-gta5.exe");
        let error = prepare_gta_rpf_keys(&missing, Path::new(".")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }
}

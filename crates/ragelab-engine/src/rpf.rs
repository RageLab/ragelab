use std::{
    io,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use ragelab_rpf::GtaKeyStore;
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

#[cfg(test)]
mod tests {
    use super::*;

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

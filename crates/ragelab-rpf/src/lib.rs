use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    error::Error,
    fmt,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

use flate2::read::{DeflateDecoder, ZlibDecoder};
pub use rage_rpf::{GtaKeys, RpfEncryption};
use sha1::{Digest, Sha1};

use rage_rpf::{
    crypto::{decrypt_aes, decrypt_ng},
    resource_size_from_flags, resource_version_from_flags, FileRef, RpfEntry, RpfEntryKind,
    RPF7_MAGIC, RSC7_MAGIC,
};

const RPF7_HEADER_SIZE: usize = 16;
const RPF7_ENTRY_SIZE: usize = 16;
const RPF7_BLOCK_SIZE: u64 = 512;
const MAX_TOC_BYTES: usize = 256 * 1024 * 1024;
const NG_KEY_SIZE: usize = 272;
const NG_KEY_COUNT: usize = 101;
const NG_KEY_BYTES: usize = NG_KEY_SIZE * NG_KEY_COUNT;
const NG_TABLE_GROUPS: usize = 17;
const NG_TABLES_PER_GROUP: usize = 16;
const NG_TABLE_VALUES: usize = 256;
const NG_TABLE_BYTES: usize =
    NG_TABLE_GROUPS * NG_TABLES_PER_GROUP * NG_TABLE_VALUES * std::mem::size_of::<u32>();
// Encrypted/masked NG bootstrap data sourced from rpf-archive-rs v0.10
// (Unlicense). No decrypted user key material is embedded in RageLab.
const EMBEDDED_MAGIC: &[u8] = include_bytes!("../resources/magic.dat");
const PC_AES_KEY_HASH: [u8; 20] = [
    0xA0, 0x79, 0x61, 0x28, 0xA7, 0x75, 0x72, 0x0A, 0xC2, 0x04, 0xD9, 0x81, 0x9F, 0x68, 0xC1, 0x72,
    0xE3, 0x95, 0x2C, 0x6D,
];

#[derive(Debug)]
pub enum RpfError {
    Io(std::io::Error),
    Invalid(String),
    KeysRequired {
        archive: String,
        encryption: RpfEncryption,
    },
    Crypto(String),
    EntryNotFound(String),
    Unsupported(String),
}

impl fmt::Display for RpfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Invalid(message) => f.write_str(message),
            Self::KeysRequired {
                archive,
                encryption,
            } => write!(
                f,
                "{archive} uses {encryption:?} encryption and requires GTA V Legacy keys"
            ),
            Self::Crypto(message) => f.write_str(message),
            Self::EntryNotFound(path) => write!(f, "RPF entry not found: {path}"),
            Self::Unsupported(message) => f.write_str(message),
        }
    }
}

impl Error for RpfError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for RpfError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Clone, Debug)]
enum RpfBacking {
    File {
        path: Arc<PathBuf>,
        start: u64,
        len: u64,
    },
    Owned {
        bytes: Arc<[u8]>,
        start: usize,
        len: usize,
    },
}

impl RpfBacking {
    fn file(path: PathBuf) -> Result<Self, RpfError> {
        let len = std::fs::metadata(&path)?.len();
        Ok(Self::File {
            path: Arc::new(path),
            start: 0,
            len,
        })
    }

    fn owned(bytes: Vec<u8>) -> Self {
        let len = bytes.len();
        Self::Owned {
            bytes: Arc::from(bytes),
            start: 0,
            len,
        }
    }

    fn len(&self) -> u64 {
        match self {
            Self::File { len, .. } => *len,
            Self::Owned { len, .. } => *len as u64,
        }
    }

    fn read_range(&self, offset: u64, len: usize) -> Result<Vec<u8>, RpfError> {
        let end = offset
            .checked_add(len as u64)
            .ok_or_else(|| RpfError::Invalid("RPF read range overflow".into()))?;
        if end > self.len() {
            return Err(RpfError::Invalid(format!(
                "RPF read range {offset}..{end} exceeds backing length {}",
                self.len()
            )));
        }

        match self {
            Self::File { path, start, .. } => {
                let mut file = File::open(path.as_ref())?;
                file.seek(SeekFrom::Start(start + offset))?;
                let mut bytes = vec![0_u8; len];
                file.read_exact(&mut bytes)?;
                Ok(bytes)
            }
            Self::Owned { bytes, start, .. } => {
                let begin = *start + offset as usize;
                let end = begin + len;
                Ok(bytes[begin..end].to_vec())
            }
        }
    }

    fn slice(&self, offset: u64, len: u64) -> Result<Self, RpfError> {
        let end = offset
            .checked_add(len)
            .ok_or_else(|| RpfError::Invalid("RPF slice overflow".into()))?;
        if end > self.len() {
            return Err(RpfError::Invalid(format!(
                "RPF slice {offset}..{end} exceeds backing length {}",
                self.len()
            )));
        }

        match self {
            Self::File { path, start, .. } => Ok(Self::File {
                path: Arc::clone(path),
                start: start + offset,
                len,
            }),
            Self::Owned { bytes, start, .. } => Ok(Self::Owned {
                bytes: Arc::clone(bytes),
                start: *start + offset as usize,
                len: len as usize,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpfEntryInfo {
    pub path: String,
    pub name: String,
    pub size: u32,
    pub memory_size: u32,
    pub resource: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RpfEntryLocator {
    pub archive: PathBuf,
    pub nested: Vec<String>,
    pub entry: String,
    pub keys: PathBuf,
}

impl RpfEntryLocator {
    pub fn new(
        archive: impl Into<PathBuf>,
        nested: Vec<String>,
        entry: impl Into<String>,
        keys: impl Into<PathBuf>,
    ) -> Self {
        Self {
            archive: archive.into(),
            nested,
            entry: entry.into(),
            keys: keys.into(),
        }
    }

    pub fn read(&self) -> Result<Vec<u8>, RpfError> {
        let keys = GtaKeyStore::load(&self.keys)?;
        let mut archive = Rpf7Archive::open(&self.archive, Some(&keys))?;
        for nested in &self.nested {
            archive = archive.open_nested(nested, Some(&keys))?;
        }
        archive.read_file(&self.entry, Some(&keys))
    }

    pub fn provenance(&self) -> String {
        let mut value = format!("rpf://{}", self.archive.display());
        for nested in &self.nested {
            value.push_str("!/");
            value.push_str(nested);
        }
        value.push_str("!/");
        value.push_str(&self.entry);
        value
    }
}

pub struct RpfMount {
    archive_path: PathBuf,
    nested: Vec<String>,
    keys_path: PathBuf,
    keys: GtaKeys,
    archive: Rpf7Archive,
}

impl RpfMount {
    pub fn open(
        archive_path: impl AsRef<Path>,
        nested: Vec<String>,
        keys_path: impl AsRef<Path>,
    ) -> Result<Self, RpfError> {
        let archive_path = archive_path.as_ref().to_path_buf();
        let keys_path = keys_path.as_ref().to_path_buf();
        let keys = GtaKeyStore::load(&keys_path)?;
        let mut archive = Rpf7Archive::open(&archive_path, Some(&keys))?;
        for nested_path in &nested {
            archive = archive.open_nested(nested_path, Some(&keys))?;
        }

        Ok(Self {
            archive_path,
            nested,
            keys_path,
            keys,
            archive,
        })
    }

    pub fn files(&self) -> impl Iterator<Item = RpfEntryInfo> + '_ {
        self.archive.files()
    }

    pub fn locator(&self, entry: impl Into<String>) -> RpfEntryLocator {
        RpfEntryLocator::new(
            self.archive_path.clone(),
            self.nested.clone(),
            entry,
            self.keys_path.clone(),
        )
    }

    pub fn read(&self, entry: &str) -> Result<Vec<u8>, RpfError> {
        self.archive.read_file(entry, Some(&self.keys))
    }

    pub fn archive_name(&self) -> &str {
        self.archive.name()
    }
}

#[derive(Debug)]
pub struct Rpf7Archive {
    name: String,
    backing: RpfBacking,
    encryption: RpfEncryption,
    entries: Vec<RpfEntry>,
    files: BTreeMap<String, FileRef>,
}

impl Rpf7Archive {
    pub fn open(path: impl AsRef<Path>, keys: Option<&GtaKeys>) -> Result<Self, RpfError> {
        let path = path.as_ref();
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| RpfError::Invalid(format!("invalid RPF path: {}", path.display())))?
            .to_string();
        Self::from_backing(RpfBacking::file(path.to_path_buf())?, name, keys)
    }

    pub fn from_bytes(
        bytes: Vec<u8>,
        name: impl Into<String>,
        keys: Option<&GtaKeys>,
    ) -> Result<Self, RpfError> {
        Self::from_backing(RpfBacking::owned(bytes), name.into(), keys)
    }

    fn from_backing(
        backing: RpfBacking,
        name: String,
        keys: Option<&GtaKeys>,
    ) -> Result<Self, RpfError> {
        let header = backing.read_range(0, RPF7_HEADER_SIZE)?;
        let magic = u32::from_le_bytes(header[0..4].try_into().expect("fixed RPF header"));
        if magic != RPF7_MAGIC {
            return Err(RpfError::Invalid(format!(
                "{name} is not an RPF7 archive (magic 0x{magic:08X})"
            )));
        }

        let entry_count =
            u32::from_le_bytes(header[4..8].try_into().expect("fixed RPF header")) as usize;
        let names_length =
            u32::from_le_bytes(header[8..12].try_into().expect("fixed RPF header")) as usize;
        let encryption = RpfEncryption::from_u32(u32::from_le_bytes(
            header[12..16].try_into().expect("fixed RPF header"),
        ));
        let entries_size = entry_count
            .checked_mul(RPF7_ENTRY_SIZE)
            .ok_or_else(|| RpfError::Invalid("RPF7 entry table size overflow".into()))?;
        let toc_size = entries_size
            .checked_add(names_length)
            .ok_or_else(|| RpfError::Invalid("RPF7 TOC size overflow".into()))?;
        if toc_size > MAX_TOC_BYTES {
            return Err(RpfError::Invalid(format!(
                "RPF7 TOC is unexpectedly large: {toc_size} bytes"
            )));
        }
        if RPF7_HEADER_SIZE as u64 + toc_size as u64 > backing.len() {
            return Err(RpfError::Invalid(
                "RPF7 TOC extends past archive length".into(),
            ));
        }

        let mut entries_data = backing.read_range(RPF7_HEADER_SIZE as u64, entries_size)?;
        let mut names_data =
            backing.read_range((RPF7_HEADER_SIZE + entries_size) as u64, names_length)?;

        match encryption {
            RpfEncryption::Aes => {
                let keys = keys.ok_or_else(|| RpfError::KeysRequired {
                    archive: name.clone(),
                    encryption,
                })?;
                entries_data = decrypt_aes(&entries_data, &keys.aes_key);
                names_data = decrypt_aes(&names_data, &keys.aes_key);
            }
            RpfEncryption::Ng => {
                let keys = keys.ok_or_else(|| RpfError::KeysRequired {
                    archive: name.clone(),
                    encryption,
                })?;
                let archive_len = u32::try_from(backing.len()).map_err(|_| {
                    RpfError::Unsupported(format!(
                        "{name} exceeds the RPF7 NG size range supported by the key schedule"
                    ))
                })?;
                entries_data = decrypt_ng(&entries_data, keys, &name, archive_len);
                names_data = decrypt_ng(&names_data, keys, &name, archive_len);
            }
            RpfEncryption::None | RpfEncryption::Open => {}
        }

        let mut entries = parse_entries(&entries_data, &names_data, entry_count)?;
        resolve_resource_sizes(&backing, &mut entries)?;
        let files = index_files(&entries)?;

        Ok(Self {
            name,
            backing,
            encryption,
            entries,
            files,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn encryption(&self) -> RpfEncryption {
        self.encryption
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn files(&self) -> impl Iterator<Item = RpfEntryInfo> + '_ {
        self.files.values().map(|file| RpfEntryInfo {
            path: file.path.clone(),
            name: file.name.clone(),
            size: file.size,
            memory_size: file.mem_size,
            resource: file.is_resource,
        })
    }

    pub fn find_file(&self, path: &str) -> Option<RpfEntryInfo> {
        let key = normalize_path(path);
        self.files.get(&key).map(|file| RpfEntryInfo {
            path: file.path.clone(),
            name: file.name.clone(),
            size: file.size,
            memory_size: file.mem_size,
            resource: file.is_resource,
        })
    }

    pub fn read_file(&self, path: &str, keys: Option<&GtaKeys>) -> Result<Vec<u8>, RpfError> {
        let key = normalize_path(path);
        let file = self
            .files
            .get(&key)
            .ok_or_else(|| RpfError::EntryNotFound(path.to_string()))?;
        self.read_ref(file, keys)
    }

    pub fn open_nested(&self, path: &str, keys: Option<&GtaKeys>) -> Result<Rpf7Archive, RpfError> {
        let key = normalize_path(path);
        let file = self
            .files
            .get(&key)
            .ok_or_else(|| RpfError::EntryNotFound(path.to_string()))?;
        if !file.name.to_ascii_lowercase().ends_with(".rpf") {
            return Err(RpfError::Invalid(format!(
                "{} is not a nested RPF entry",
                file.path
            )));
        }

        if let Some(backing) = self.stored_backing(file)? {
            return Self::from_backing(backing, file.name.clone(), keys);
        }

        Self::from_bytes(self.read_ref(file, keys)?, file.name.clone(), keys)
    }

    fn stored_backing(&self, file: &FileRef) -> Result<Option<RpfBacking>, RpfError> {
        let entry = &self.entries[file.entry_index];
        let RpfEntryKind::BinaryFile {
            file_offset,
            file_size,
            uncompressed_size,
            is_encrypted,
        } = entry.kind
        else {
            return Ok(None);
        };

        let compressed = file_size != 0 && file_size != uncompressed_size;
        if is_encrypted || compressed || uncompressed_size == 0 {
            return Ok(None);
        }

        let offset = u64::from(file_offset) * RPF7_BLOCK_SIZE;
        Ok(Some(
            self.backing.slice(offset, u64::from(uncompressed_size))?,
        ))
    }

    fn read_ref(&self, file: &FileRef, keys: Option<&GtaKeys>) -> Result<Vec<u8>, RpfError> {
        let entry = &self.entries[file.entry_index];
        match &entry.kind {
            RpfEntryKind::Directory { .. } => {
                Err(RpfError::Invalid(format!("{} is a directory", file.path)))
            }
            RpfEntryKind::BinaryFile {
                file_offset,
                file_size,
                uncompressed_size,
                is_encrypted,
            } => {
                let disk_size = if *file_size == 0 {
                    *uncompressed_size
                } else {
                    *file_size
                };
                if disk_size == 0 {
                    return Err(RpfError::Invalid(format!("{} has zero length", file.path)));
                }
                let offset = u64::from(*file_offset) * RPF7_BLOCK_SIZE;
                let mut bytes = self.backing.read_range(offset, disk_size as usize)?;
                if *is_encrypted {
                    bytes = self.decrypt_entry(bytes, &file.name, *uncompressed_size, keys)?;
                }
                if *file_size > 0 && *file_size < *uncompressed_size {
                    bytes = inflate(&bytes).ok_or_else(|| {
                        RpfError::Invalid(format!("failed to inflate {}", file.path))
                    })?;
                    if bytes.len() != *uncompressed_size as usize {
                        return Err(RpfError::Invalid(format!(
                            "{} inflated to {} bytes, expected {}",
                            file.path,
                            bytes.len(),
                            uncompressed_size
                        )));
                    }
                }
                Ok(bytes)
            }
            RpfEntryKind::ResourceFile {
                file_offset,
                file_size,
                system_flags,
                graphics_flags,
                is_encrypted,
            } => {
                if *file_size < 16 {
                    return Err(RpfError::Invalid(format!(
                        "{} is an invalid RSC7 resource",
                        file.path
                    )));
                }
                let offset = u64::from(*file_offset) * RPF7_BLOCK_SIZE;
                let body_size = *file_size as usize - 16;
                let mut body = self.backing.read_range(offset + 16, body_size)?;
                if *is_encrypted {
                    body = self.decrypt_entry(body, &file.name, *file_size, keys)?;
                }

                let version = resource_version_from_flags(*system_flags, *graphics_flags);
                let mut output = Vec::with_capacity(body.len() + 16);
                output.extend_from_slice(&RSC7_MAGIC.to_le_bytes());
                output.extend_from_slice(&version.to_le_bytes());
                output.extend_from_slice(&system_flags.to_le_bytes());
                output.extend_from_slice(&graphics_flags.to_le_bytes());
                output.extend_from_slice(&body);
                Ok(output)
            }
        }
    }

    fn decrypt_entry(
        &self,
        bytes: Vec<u8>,
        name: &str,
        length: u32,
        keys: Option<&GtaKeys>,
    ) -> Result<Vec<u8>, RpfError> {
        match self.encryption {
            RpfEncryption::Aes => {
                let keys = keys.ok_or_else(|| RpfError::KeysRequired {
                    archive: self.name.clone(),
                    encryption: self.encryption,
                })?;
                Ok(decrypt_aes(&bytes, &keys.aes_key))
            }
            RpfEncryption::Ng => {
                let keys = keys.ok_or_else(|| RpfError::KeysRequired {
                    archive: self.name.clone(),
                    encryption: self.encryption,
                })?;
                Ok(decrypt_ng(&bytes, keys, name, length))
            }
            RpfEncryption::None | RpfEncryption::Open => Ok(bytes),
        }
    }
}

pub struct GtaKeyStore;

impl GtaKeyStore {
    pub fn load(path: impl AsRef<Path>) -> Result<GtaKeys, RpfError> {
        load_gta_keys(path.as_ref())
    }

    pub fn extract_from_exe(exe_path: impl AsRef<Path>) -> Result<GtaKeys, RpfError> {
        let exe_path = exe_path.as_ref();
        let exe_data = std::fs::read(exe_path)?;
        let aes_bytes = search_hash(&exe_data, &PC_AES_KEY_HASH, 32).ok_or_else(|| {
            RpfError::Crypto(format!(
                "AES key not found in {}; expected a GTA V Legacy PC executable",
                exe_path.display()
            ))
        })?;
        let aes_key: [u8; 32] = aes_bytes
            .try_into()
            .map_err(|_| RpfError::Crypto("invalid AES key length".into()))?;

        keys_from_aes_key(aes_key)
    }

    pub fn load_or_extract_cached(
        exe_path: impl AsRef<Path>,
        cache_root: impl AsRef<Path>,
    ) -> Result<(GtaKeys, bool, PathBuf), RpfError> {
        let exe_path = exe_path.as_ref();
        let cache_root = cache_root.as_ref();
        let metadata = std::fs::metadata(exe_path)?;
        let modified = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map_err(|error| RpfError::Invalid(error.to_string()))?
            .as_secs();
        let cache_dir = cache_root.join(format!("{}-{modified}", metadata.len()));

        if let Ok(keys) = load_gta_keys(&cache_dir) {
            return Ok((keys, true, cache_dir));
        }

        let keys = Self::extract_from_exe(exe_path)?;
        save_gta_keys(&cache_dir, &keys)?;
        Ok((keys, false, cache_dir))
    }
}

fn load_gta_keys(path: &Path) -> Result<GtaKeys, RpfError> {
    let aes_bytes = std::fs::read(path.join("gtav_aes_key.dat"))?;
    let aes_key: [u8; 32] = aes_bytes.try_into().map_err(|bytes: Vec<u8>| {
        RpfError::Invalid(format!(
            "gtav_aes_key.dat has {} bytes, expected 32",
            bytes.len()
        ))
    })?;

    let ng_key_bytes = std::fs::read(path.join("gtav_ng_key.dat"))?;
    let ng_keys = read_ng_keys(&ng_key_bytes)?;

    let table_bytes = std::fs::read(path.join("gtav_ng_decrypt_tables.dat"))?;
    let ng_decrypt_tables = read_ng_tables(&table_bytes)?;

    Ok(GtaKeys {
        aes_key,
        ng_keys,
        ng_decrypt_tables,
    })
}

fn save_gta_keys(path: &Path, keys: &GtaKeys) -> Result<(), RpfError> {
    std::fs::create_dir_all(path)?;
    std::fs::write(path.join("gtav_aes_key.dat"), keys.aes_key)?;
    std::fs::write(
        path.join("gtav_ng_key.dat"),
        keys.ng_keys.iter().flatten().copied().collect::<Vec<_>>(),
    )?;
    std::fs::write(
        path.join("gtav_ng_decrypt_tables.dat"),
        write_ng_tables(&keys.ng_decrypt_tables),
    )?;
    Ok(())
}

fn keys_from_aes_key(aes_key: [u8; 32]) -> Result<GtaKeys, RpfError> {
    let magic = unwrap_magic(EMBEDDED_MAGIC, &aes_key)?;
    if magic.len() < NG_KEY_BYTES + NG_TABLE_BYTES {
        return Err(RpfError::Crypto(format!(
            "decrypted GTA key bootstrap has {} bytes, expected at least {}",
            magic.len(),
            NG_KEY_BYTES + NG_TABLE_BYTES
        )));
    }

    let ng_keys = read_ng_keys(&magic[..NG_KEY_BYTES])?;
    let ng_decrypt_tables = read_ng_tables(&magic[NG_KEY_BYTES..NG_KEY_BYTES + NG_TABLE_BYTES])?;

    Ok(GtaKeys {
        aes_key,
        ng_keys,
        ng_decrypt_tables,
    })
}

fn read_ng_keys(data: &[u8]) -> Result<Vec<Vec<u8>>, RpfError> {
    if data.len() < NG_KEY_BYTES {
        return Err(RpfError::Invalid(format!(
            "NG key data has {} bytes, expected at least {NG_KEY_BYTES}",
            data.len()
        )));
    }
    Ok((0..NG_KEY_COUNT)
        .map(|index| {
            let start = index * NG_KEY_SIZE;
            data[start..start + NG_KEY_SIZE].to_vec()
        })
        .collect())
}

fn read_ng_tables(
    data: &[u8],
) -> Result<Box<[[[u32; NG_TABLE_VALUES]; NG_TABLES_PER_GROUP]; NG_TABLE_GROUPS]>, RpfError> {
    if data.len() < NG_TABLE_BYTES {
        return Err(RpfError::Invalid(format!(
            "NG table data has {} bytes, expected at least {NG_TABLE_BYTES}",
            data.len()
        )));
    }

    let mut tables = vec![[[0_u32; NG_TABLE_VALUES]; NG_TABLES_PER_GROUP]; NG_TABLE_GROUPS];
    let mut offset = 0_usize;
    for group in &mut tables {
        for table in group {
            for value in table {
                *value = u32::from_le_bytes(
                    data[offset..offset + 4]
                        .try_into()
                        .expect("four-byte NG table value"),
                );
                offset += 4;
            }
        }
    }

    tables
        .into_boxed_slice()
        .try_into()
        .map_err(|_| RpfError::Invalid("failed to materialize GTA NG decrypt tables".into()))
}

fn write_ng_tables(
    tables: &[[[u32; NG_TABLE_VALUES]; NG_TABLES_PER_GROUP]; NG_TABLE_GROUPS],
) -> Vec<u8> {
    let mut output = Vec::with_capacity(NG_TABLE_BYTES);
    for group in tables {
        for table in group {
            for value in table {
                output.extend_from_slice(&value.to_le_bytes());
            }
        }
    }
    output
}

fn search_hash(data: &[u8], expected_sha1: &[u8; 20], length: usize) -> Option<Vec<u8>> {
    if data.len() < length {
        return None;
    }

    for offset in 0..=data.len() - length {
        let candidate = &data[offset..offset + length];
        let mut hasher = Sha1::new();
        hasher.update(candidate);
        let hash: [u8; 20] = hasher.finalize().into();
        if &hash == expected_sha1 {
            return Some(candidate.to_vec());
        }
    }
    None
}

fn unwrap_magic(magic: &[u8], aes_key: &[u8; 32]) -> Result<Vec<u8>, RpfError> {
    let len = magic.len();
    let mut mask = vec![0_u8; len * 4];
    DotNetRandom::new(jenkins_hash_bytes(aes_key) as i32).next_bytes(&mut mask);

    let mut data = magic.to_vec();
    for (index, byte) in data.iter_mut().enumerate() {
        for pass in 0..4 {
            *byte = byte.wrapping_sub(mask[pass * len + index]);
        }
    }

    let decrypted = decrypt_aes(&data, aes_key);
    let mut output = Vec::new();
    DeflateDecoder::new(decrypted.as_slice())
        .read_to_end(&mut output)
        .map_err(|error| {
            RpfError::Crypto(format!(
                "failed to inflate GTA key bootstrap; AES key does not match: {error}"
            ))
        })?;

    let expected = NG_KEY_BYTES + NG_TABLE_BYTES;
    if output.len() < expected {
        return Err(RpfError::Crypto(format!(
            "GTA key bootstrap has {} bytes, expected at least {expected}",
            output.len()
        )));
    }
    output.truncate(expected);
    Ok(output)
}

fn jenkins_hash_bytes(data: &[u8]) -> u32 {
    let mut hash = 0_u32;
    for byte in data {
        hash = hash.wrapping_add(u32::from(*byte));
        hash = hash.wrapping_add(hash << 10);
        hash ^= hash >> 6;
    }
    hash = hash.wrapping_add(hash << 3);
    hash ^= hash >> 11;
    hash.wrapping_add(hash << 15)
}

struct DotNetRandom {
    seed_array: [i32; 56],
    inext: usize,
    inextp: usize,
}

impl DotNetRandom {
    const MBIG: i32 = i32::MAX;
    const MSEED: i32 = 161_803_398;

    fn new(seed: i32) -> Self {
        let subtraction = if seed == i32::MIN {
            i32::MAX
        } else {
            seed.abs()
        };
        let mut seed_array = [0_i32; 56];

        let mut mj = Self::MSEED.wrapping_sub(subtraction);
        seed_array[55] = mj;
        let mut mk = 1_i32;

        for index in 1..55 {
            let slot = (21 * index) % 55;
            seed_array[slot] = mk;
            mk = mj.wrapping_sub(mk);
            if mk < 0 {
                mk = mk.wrapping_add(Self::MBIG);
            }
            mj = seed_array[slot];
        }

        for _ in 1..5 {
            for index in 1..56 {
                seed_array[index] =
                    seed_array[index].wrapping_sub(seed_array[1 + (index + 30) % 55]);
                if seed_array[index] < 0 {
                    seed_array[index] = seed_array[index].wrapping_add(Self::MBIG);
                }
            }
        }

        Self {
            seed_array,
            inext: 0,
            inextp: 21,
        }
    }

    fn internal_sample(&mut self) -> i32 {
        let mut inext = self.inext + 1;
        if inext >= 56 {
            inext = 1;
        }

        let mut inextp = self.inextp + 1;
        if inextp >= 56 {
            inextp = 1;
        }

        let mut value = self.seed_array[inext].wrapping_sub(self.seed_array[inextp]);
        if value == Self::MBIG {
            value -= 1;
        }
        if value < 0 {
            value = value.wrapping_add(Self::MBIG);
        }

        self.seed_array[inext] = value;
        self.inext = inext;
        self.inextp = inextp;
        value
    }

    fn next_bytes(&mut self, buffer: &mut [u8]) {
        for byte in buffer {
            *byte = (self.internal_sample() % 256) as u8;
        }
    }
}

fn parse_entries(
    entries_data: &[u8],
    names_data: &[u8],
    entry_count: usize,
) -> Result<Vec<RpfEntry>, RpfError> {
    if entries_data.len() != entry_count * RPF7_ENTRY_SIZE {
        return Err(RpfError::Invalid("RPF7 entry table length mismatch".into()));
    }

    let mut entries = Vec::with_capacity(entry_count);
    for (index, chunk) in entries_data.chunks_exact(RPF7_ENTRY_SIZE).enumerate() {
        let discriminator = u32::from_le_bytes(chunk[4..8].try_into().expect("RPF7 entry"));
        let entry = if discriminator == 0x7FFF_FF00 {
            let name_offset =
                u32::from_le_bytes(chunk[0..4].try_into().expect("RPF7 directory")) as usize;
            let entries_index =
                u32::from_le_bytes(chunk[8..12].try_into().expect("RPF7 directory"));
            let entries_count =
                u32::from_le_bytes(chunk[12..16].try_into().expect("RPF7 directory"));
            let name =
                read_cstring(names_data, name_offset).unwrap_or_else(|| format!("dir_{index}"));
            RpfEntry {
                name_lower: name.to_ascii_lowercase(),
                name,
                kind: RpfEntryKind::Directory {
                    entries_index,
                    entries_count,
                },
            }
        } else if discriminator & 0x8000_0000 == 0 {
            let name_offset =
                u16::from_le_bytes(chunk[0..2].try_into().expect("RPF7 binary")) as usize;
            let file_size =
                u32::from(chunk[2]) | (u32::from(chunk[3]) << 8) | (u32::from(chunk[4]) << 16);
            let file_offset =
                u32::from(chunk[5]) | (u32::from(chunk[6]) << 8) | (u32::from(chunk[7]) << 16);
            let uncompressed_size =
                u32::from_le_bytes(chunk[8..12].try_into().expect("RPF7 binary"));
            let is_encrypted =
                u32::from_le_bytes(chunk[12..16].try_into().expect("RPF7 binary")) == 1;
            let name =
                read_cstring(names_data, name_offset).unwrap_or_else(|| format!("binary_{index}"));
            RpfEntry {
                name_lower: name.to_ascii_lowercase(),
                name,
                kind: RpfEntryKind::BinaryFile {
                    file_offset,
                    file_size,
                    uncompressed_size,
                    is_encrypted,
                },
            }
        } else {
            let name_offset =
                u16::from_le_bytes(chunk[0..2].try_into().expect("RPF7 resource")) as usize;
            let file_size =
                u32::from(chunk[2]) | (u32::from(chunk[3]) << 8) | (u32::from(chunk[4]) << 16);
            let file_offset =
                (u32::from(chunk[5]) | (u32::from(chunk[6]) << 8) | (u32::from(chunk[7]) << 16))
                    & 0x7F_FFFF;
            let system_flags = u32::from_le_bytes(chunk[8..12].try_into().expect("RPF7 resource"));
            let graphics_flags =
                u32::from_le_bytes(chunk[12..16].try_into().expect("RPF7 resource"));
            let name = read_cstring(names_data, name_offset)
                .unwrap_or_else(|| format!("resource_{index}"));
            let name_lower = name.to_ascii_lowercase();
            let is_encrypted = name_lower.ends_with(".ysc");
            RpfEntry {
                name,
                name_lower,
                kind: RpfEntryKind::ResourceFile {
                    file_offset,
                    file_size,
                    system_flags,
                    graphics_flags,
                    is_encrypted,
                },
            }
        };
        entries.push(entry);
    }
    Ok(entries)
}

fn resolve_resource_sizes(backing: &RpfBacking, entries: &mut [RpfEntry]) -> Result<(), RpfError> {
    for entry in entries {
        let RpfEntryKind::ResourceFile {
            file_offset,
            file_size,
            ..
        } = &mut entry.kind
        else {
            continue;
        };
        if *file_size != 0xFF_FFFF {
            continue;
        }

        let offset = u64::from(*file_offset) * RPF7_BLOCK_SIZE;
        let header = backing.read_range(offset, 16)?;
        *file_size = (u32::from(header[7]))
            | (u32::from(header[14]) << 8)
            | (u32::from(header[5]) << 16)
            | (u32::from(header[2]) << 24);
    }
    Ok(())
}

fn index_files(entries: &[RpfEntry]) -> Result<BTreeMap<String, FileRef>, RpfError> {
    let mut files = BTreeMap::new();
    if entries.is_empty() {
        return Ok(files);
    }

    let mut queue = VecDeque::new();
    let mut visited_directories = BTreeSet::new();

    if let RpfEntryKind::Directory {
        entries_index,
        entries_count,
    } = entries[0].kind
    {
        queue.push_back((0_usize, String::new(), entries_index, entries_count));
    } else {
        queue.push_back((usize::MAX, String::new(), 0, entries.len() as u32));
    }

    while let Some((directory_index, parent_path, start, count)) = queue.pop_front() {
        if directory_index != usize::MAX && !visited_directories.insert(directory_index) {
            return Err(RpfError::Invalid(format!(
                "RPF7 directory graph contains a cycle or duplicate reference at entry {directory_index}"
            )));
        }

        let start = start as usize;
        let end = start
            .checked_add(count as usize)
            .ok_or_else(|| RpfError::Invalid("RPF7 directory range overflow".into()))?;
        if end > entries.len() {
            return Err(RpfError::Invalid(format!(
                "RPF7 directory range {start}..{end} exceeds {} entries",
                entries.len()
            )));
        }

        for (entry_index, entry) in entries.iter().enumerate().take(end).skip(start) {
            match &entry.kind {
                RpfEntryKind::Directory {
                    entries_index,
                    entries_count,
                } => {
                    let path = child_path(&parent_path, &entry.name_lower);
                    queue.push_back((entry_index, path, *entries_index, *entries_count));
                }
                RpfEntryKind::BinaryFile {
                    file_size,
                    uncompressed_size,
                    ..
                } => {
                    let path = child_path(&parent_path, &entry.name_lower);
                    let file = FileRef {
                        name: entry.name.clone(),
                        path: path.clone(),
                        entry_index,
                        size: *file_size,
                        mem_size: *uncompressed_size,
                        is_resource: false,
                    };
                    if files.insert(normalize_path(&path), file).is_some() {
                        return Err(RpfError::Invalid(format!(
                            "duplicate RPF7 file path: {path}"
                        )));
                    }
                }
                RpfEntryKind::ResourceFile {
                    file_size,
                    system_flags,
                    ..
                } => {
                    let path = child_path(&parent_path, &entry.name_lower);
                    let file = FileRef {
                        name: entry.name.clone(),
                        path: path.clone(),
                        entry_index,
                        size: *file_size,
                        mem_size: resource_size_from_flags(*system_flags) as u32,
                        is_resource: true,
                    };
                    if files.insert(normalize_path(&path), file).is_some() {
                        return Err(RpfError::Invalid(format!(
                            "duplicate RPF7 file path: {path}"
                        )));
                    }
                }
            }
        }
    }

    Ok(files)
}

fn child_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches('/')
        .to_ascii_lowercase()
}

fn read_cstring(data: &[u8], offset: usize) -> Option<String> {
    if offset >= data.len() {
        return None;
    }
    let end = data[offset..]
        .iter()
        .position(|byte| *byte == 0)
        .map(|length| offset + length)
        .unwrap_or(data.len());
    Some(String::from_utf8_lossy(&data[offset..end]).into_owned())
}

fn inflate(data: &[u8]) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    if DeflateDecoder::new(data).read_to_end(&mut output).is_ok() && !output.is_empty() {
        return Some(output);
    }

    output.clear();
    if ZlibDecoder::new(data).read_to_end(&mut output).is_ok() && !output.is_empty() {
        return Some(output);
    }
    None
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::{write::DeflateEncoder, Compression};

    use super::*;

    fn write_u24(target: &mut [u8], value: u32) {
        target[0] = (value & 0xFF) as u8;
        target[1] = ((value >> 8) & 0xFF) as u8;
        target[2] = ((value >> 16) & 0xFF) as u8;
    }

    fn synthetic_binary_rpf(name: &str, payload: &[u8], compress: bool) -> Vec<u8> {
        let stored = if compress {
            let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
            encoder
                .write_all(payload)
                .expect("compress synthetic payload");
            encoder.finish().expect("finish synthetic deflate")
        } else {
            payload.to_vec()
        };
        assert!(
            !compress || stored.len() < payload.len(),
            "compressed test payload must actually shrink"
        );

        let names = format!("root\0{name}\0").into_bytes();
        let entry_count = 2_u32;
        let entries_size = entry_count as usize * RPF7_ENTRY_SIZE;
        let data_offset = RPF7_BLOCK_SIZE as usize;
        let mut bytes = vec![0_u8; data_offset + stored.len()];

        bytes[0..4].copy_from_slice(&RPF7_MAGIC.to_le_bytes());
        bytes[4..8].copy_from_slice(&entry_count.to_le_bytes());
        bytes[8..12].copy_from_slice(&(names.len() as u32).to_le_bytes());
        bytes[12..16].copy_from_slice(&RpfEncryption::None.as_u32().to_le_bytes());

        let root = &mut bytes[16..32];
        root[0..4].copy_from_slice(&0_u32.to_le_bytes());
        root[4..8].copy_from_slice(&0x7FFF_FF00_u32.to_le_bytes());
        root[8..12].copy_from_slice(&1_u32.to_le_bytes());
        root[12..16].copy_from_slice(&1_u32.to_le_bytes());

        let file = &mut bytes[32..48];
        file[0..2].copy_from_slice(&5_u16.to_le_bytes());
        let file_size = if compress { stored.len() as u32 } else { 0 };
        write_u24(&mut file[2..5], file_size);
        write_u24(&mut file[5..8], 1);
        file[8..12].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        file[12..16].copy_from_slice(&0_u32.to_le_bytes());

        let names_offset = RPF7_HEADER_SIZE + entries_size;
        bytes[names_offset..names_offset + names.len()].copy_from_slice(&names);
        bytes[data_offset..data_offset + stored.len()].copy_from_slice(&stored);
        bytes
    }

    #[test]
    fn normalizes_archive_paths() {
        assert_eq!(
            normalize_path(r"\X64\Models\Test.YDR"),
            "x64/models/test.ydr"
        );
    }

    #[test]
    fn rejects_non_rpf7_bytes() {
        let error = Rpf7Archive::from_bytes(vec![0_u8; 32], "bad.rpf", None).unwrap_err();
        assert!(error.to_string().contains("not an RPF7 archive"));
    }

    #[test]
    fn reads_stored_binary_entry_from_synthetic_rpf() {
        let payload = b"rage-lab-rpf";
        let archive = Rpf7Archive::from_bytes(
            synthetic_binary_rpf("hello.bin", payload, false),
            "test.rpf",
            None,
        )
        .expect("parse synthetic RPF");
        assert_eq!(archive.file_count(), 1);
        assert_eq!(
            archive
                .read_file("hello.bin", None)
                .expect("read stored entry"),
            payload
        );
    }

    #[test]
    fn inflates_compressed_binary_entry_from_synthetic_rpf() {
        let payload = vec![0x41_u8; 8192];
        let archive = Rpf7Archive::from_bytes(
            synthetic_binary_rpf("compressed.bin", &payload, true),
            "compressed.rpf",
            None,
        )
        .expect("parse compressed synthetic RPF");
        assert_eq!(
            archive
                .read_file("compressed.bin", None)
                .expect("inflate compressed entry"),
            payload
        );
    }

    #[test]
    fn opens_nested_rpf_without_materializing_to_disk() {
        let inner = synthetic_binary_rpf("inside.bin", b"nested-value", false);
        let outer = Rpf7Archive::from_bytes(
            synthetic_binary_rpf("nested.rpf", &inner, false),
            "outer.rpf",
            None,
        )
        .expect("parse outer RPF");

        let nested = outer
            .open_nested("nested.rpf", None)
            .expect("open nested RPF");
        assert_eq!(
            nested
                .read_file("inside.bin", None)
                .expect("read nested entry"),
            b"nested-value"
        );
    }

    #[test]
    fn sha1_key_search_finds_unaligned_candidate() {
        let candidate = (0_u8..32).collect::<Vec<_>>();
        let mut data = vec![0xEE_u8; 17];
        data.extend_from_slice(&candidate);
        data.extend_from_slice(&[0xAA_u8; 19]);

        let mut hasher = Sha1::new();
        hasher.update(&candidate);
        let expected: [u8; 20] = hasher.finalize().into();

        assert_eq!(search_hash(&data, &expected, 32), Some(candidate));
    }

    #[test]
    fn key_store_round_trip_preserves_heap_backed_ng_material() {
        let root = std::env::temp_dir().join(format!("ragelab-rpf-keys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        let table_bytes = vec![0x2A_u8; NG_TABLE_BYTES];
        let keys = GtaKeys {
            aes_key: [0x11_u8; 32],
            ng_keys: (0..NG_KEY_COUNT)
                .map(|index| vec![index as u8; NG_KEY_SIZE])
                .collect(),
            ng_decrypt_tables: read_ng_tables(&table_bytes).expect("build synthetic tables"),
        };

        save_gta_keys(&root, &keys).expect("save synthetic keys");
        let loaded = load_gta_keys(&root).expect("reload synthetic keys");

        assert_eq!(loaded.aes_key, keys.aes_key);
        assert_eq!(loaded.ng_keys, keys.ng_keys);
        assert_eq!(loaded.ng_decrypt_tables, keys.ng_decrypt_tables);

        std::fs::remove_dir_all(root).expect("remove synthetic key store");
    }

    #[test]
    fn cached_key_store_is_reused_without_scanning_executable() {
        let root =
            std::env::temp_dir().join(format!("ragelab-rpf-key-cache-{}", std::process::id()));
        let exe = root.join("GTA5.exe");
        let cache_root = root.join("cache");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create cache fixture");
        std::fs::write(&exe, b"not-a-real-executable").expect("write fake executable");

        let metadata = std::fs::metadata(&exe).expect("fake exe metadata");
        let modified = metadata
            .modified()
            .expect("fake exe modified")
            .duration_since(UNIX_EPOCH)
            .expect("fake exe timestamp")
            .as_secs();
        let cache_dir = cache_root.join(format!("{}-{modified}", metadata.len()));

        let table_bytes = vec![0x35_u8; NG_TABLE_BYTES];
        let expected = GtaKeys {
            aes_key: [0x22_u8; 32],
            ng_keys: (0..NG_KEY_COUNT)
                .map(|index| vec![(index as u8).wrapping_add(1); NG_KEY_SIZE])
                .collect(),
            ng_decrypt_tables: read_ng_tables(&table_bytes).expect("build cached tables"),
        };
        save_gta_keys(&cache_dir, &expected).expect("seed key cache");

        let (loaded, cache_hit, resolved_dir) =
            GtaKeyStore::load_or_extract_cached(&exe, &cache_root).expect("load cached keys");

        assert!(cache_hit);
        assert_eq!(resolved_dir, cache_dir);
        assert_eq!(loaded.aes_key, expected.aes_key);
        assert_eq!(loaded.ng_keys, expected.ng_keys);
        assert_eq!(loaded.ng_decrypt_tables, expected.ng_decrypt_tables);

        std::fs::remove_dir_all(root).expect("remove key cache fixture");
    }
}

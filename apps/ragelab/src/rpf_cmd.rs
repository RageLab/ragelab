use std::{
    error::Error,
    fs, io,
    path::{Path, PathBuf},
};

use ragelab_engine::prepare_gta_rpf_keys;
use ragelab_rpf::{GtaKeyStore, Rpf7Archive};
use serde_json::json;

#[derive(Debug, Default)]
pub struct RpfCommandOptions {
    pub keys: Option<PathBuf>,
    pub nested: Vec<String>,
    pub contains: Option<String>,
    pub entry: Option<String>,
    pub output: Option<PathBuf>,
    pub overwrite: bool,
    pub json: bool,
}

pub fn parse_key_options(
    args: impl Iterator<Item = String>,
    usage: &str,
) -> Result<(PathBuf, PathBuf, bool), io::Error> {
    let mut exe = None;
    let mut cache_root = None;
    let mut json_output = false;
    let mut args = args.peekable();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cache-root" if cache_root.is_none() => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                cache_root = Some(PathBuf::from(value));
            }
            "--json" if !json_output => json_output = true,
            _ if arg.starts_with('-') => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{usage}; unknown option: {arg}"),
                ))
            }
            _ if exe.is_none() => exe = Some(PathBuf::from(arg)),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    usage.to_string(),
                ))
            }
        }
    }

    let exe = exe.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
    let cache_root =
        cache_root.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
    Ok((exe, cache_root, json_output))
}

pub fn keys(exe_path: &Path, cache_root: &Path, json_output: bool) -> Result<(), Box<dyn Error>> {
    let report = prepare_gta_rpf_keys(exe_path, cache_root)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("executable: {}", report.executable);
        println!("cache: {}", report.cache);
        println!("cache hit: {}", report.cache_hit);
    }
    Ok(())
}

pub fn parse_options(
    args: impl Iterator<Item = String>,
    usage: &str,
) -> Result<(PathBuf, RpfCommandOptions), io::Error> {
    let mut archive = None;
    let mut options = RpfCommandOptions::default();
    let mut args = args.peekable();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--keys" => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                options.keys = Some(PathBuf::from(value));
            }
            "--nested" => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                options.nested.push(value);
            }
            "--contains" => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                options.contains = Some(value.to_ascii_lowercase());
            }
            "--entry" => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                options.entry = Some(value);
            }
            "--output" => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
                options.output = Some(PathBuf::from(value));
            }
            "--overwrite" if !options.overwrite => options.overwrite = true,
            "--json" if !options.json => options.json = true,
            _ if arg.starts_with('-') => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{usage}; unknown option: {arg}"),
                ))
            }
            _ if archive.is_none() => archive = Some(PathBuf::from(arg)),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    usage.to_string(),
                ))
            }
        }
    }

    let archive = archive.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
    Ok((archive, options))
}

pub fn info(archive_path: &Path, options: &RpfCommandOptions) -> Result<(), Box<dyn Error>> {
    let (archive, chain) = open_chain(archive_path, options)?;
    let data = json!({
        "archive": archive.name(),
        "source": archive_path,
        "nested": chain,
        "encryption": encryption_name(archive.encryption()),
        "entries": archive.entry_count(),
        "files": archive.file_count(),
    });

    if options.json {
        println!("{}", serde_json::to_string_pretty(&data)?);
    } else {
        println!("archive: {}", archive.name());
        println!("source: {}", archive_path.display());
        if !chain.is_empty() {
            println!("nested: {}", chain.join(" -> "));
        }
        println!("encryption: {}", encryption_name(archive.encryption()));
        println!("entries: {}", archive.entry_count());
        println!("files: {}", archive.file_count());
    }
    Ok(())
}

pub fn list(archive_path: &Path, options: &RpfCommandOptions) -> Result<(), Box<dyn Error>> {
    let (archive, chain) = open_chain(archive_path, options)?;
    let files = archive
        .files()
        .filter(|file| {
            options.contains.as_ref().map_or(true, |needle| {
                file.path.to_ascii_lowercase().contains(needle)
            })
        })
        .collect::<Vec<_>>();

    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "archive": archive.name(),
                "source": archive_path,
                "nested": chain,
                "count": files.len(),
                "files": files.iter().map(|file| json!({
                    "path": file.path,
                    "name": file.name,
                    "size": file.size,
                    "memorySize": file.memory_size,
                    "resource": file.resource,
                })).collect::<Vec<_>>(),
            }))?
        );
    } else {
        for file in files {
            println!("{}", file.path);
        }
    }
    Ok(())
}

pub fn extract(archive_path: &Path, options: &RpfCommandOptions) -> Result<(), Box<dyn Error>> {
    let entry = options.entry.as_deref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "rpf extract requires --entry <path>",
        )
    })?;
    let output = options.output.as_ref().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "rpf extract requires --output <file>",
        )
    })?;
    if output.exists() && !options.overwrite {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "output already exists: {}; pass --overwrite to replace it",
                output.display()
            ),
        )
        .into());
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }

    let keys = load_keys(options)?;
    let mut archive = Rpf7Archive::open(archive_path, keys.as_ref())?;
    for nested in &options.nested {
        archive = archive.open_nested(nested, keys.as_ref())?;
    }
    let bytes = archive.read_file(entry, keys.as_ref())?;
    fs::write(output, &bytes)?;

    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "archive": archive.name(),
                "entry": entry,
                "output": output,
                "bytes": bytes.len(),
            }))?
        );
    } else {
        println!("wrote {} bytes to {}", bytes.len(), output.display());
    }
    Ok(())
}

fn open_chain(
    archive_path: &Path,
    options: &RpfCommandOptions,
) -> Result<(Rpf7Archive, Vec<String>), Box<dyn Error>> {
    let keys = load_keys(options)?;
    let mut archive = Rpf7Archive::open(archive_path, keys.as_ref())?;
    let mut chain = Vec::with_capacity(options.nested.len());
    for nested in &options.nested {
        archive = archive.open_nested(nested, keys.as_ref())?;
        chain.push(nested.clone());
    }
    Ok((archive, chain))
}

fn load_keys(options: &RpfCommandOptions) -> Result<Option<ragelab_rpf::GtaKeys>, Box<dyn Error>> {
    options
        .keys
        .as_ref()
        .map(GtaKeyStore::load)
        .transpose()
        .map_err(Into::into)
}

fn encryption_name(encryption: ragelab_rpf::RpfEncryption) -> &'static str {
    match encryption {
        ragelab_rpf::RpfEncryption::None => "none",
        ragelab_rpf::RpfEncryption::Open => "open",
        ragelab_rpf::RpfEncryption::Aes => "aes",
        ragelab_rpf::RpfEncryption::Ng => "ng",
    }
}

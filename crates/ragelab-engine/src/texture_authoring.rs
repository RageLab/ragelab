use std::{io, path::Path};

pub use ragelab_authoring::*;
use ragelab_ytd::Ytd;

pub fn write_png_replacement_to_explicit_output(
    source: &Path,
    output: &Path,
    texture_index: usize,
    png_bytes: &[u8],
) -> io::Result<usize> {
    if !source.is_absolute() || !output.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "YTD source and output must be absolute paths",
        ));
    }
    let source = source.canonicalize()?;
    if output == source {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "YTD authoring output must not overwrite the source",
        ));
    }
    if output.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("YTD authoring output already exists: {}", output.display()),
        ));
    }

    let source_bytes = std::fs::read(&source)?;
    let output_bytes = replace_ytd_texture_from_png(&source_bytes, texture_index, png_bytes)?;
    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    use std::io::Write;
    let mut file = options.open(output)?;
    file.write_all(&output_bytes)?;
    file.flush()?;

    let written = std::fs::read(output)?;
    Ytd::from_bytes(&written).map_err(core_error)?;
    if std::fs::read(&source)? != source_bytes {
        return Err(io::Error::other(format!(
            "source changed while writing YTD output: {}",
            source.display()
        )));
    }
    Ok(written.len())
}

fn core_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

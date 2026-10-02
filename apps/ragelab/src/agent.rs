use std::{
    collections::BTreeSet,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
};

use ragelab_resource::{Rsc7Probe, Rsc7Resource};
use ragelab_ybn::YbnCollision;
use ragelab_ydd::YddDictionary;
use ragelab_ydr::YdrDocument;
use ragelab_ymap::Ymap;
use ragelab_ymf::Ymf;
use ragelab_ytd::Ytd;
use ragelab_ytyp::{ArchetypeKind, Ytyp};
use serde_json::{json, Value};

pub const RESPONSE_SCHEMA: &str = "ragelab.cli.response";
pub const RESPONSE_SCHEMA_VERSION: u64 = 1;

pub fn is_structured_command(command: &str) -> bool {
    matches!(command, "version" | "capabilities" | "inspect" | "validate")
}

pub fn classify_exit(error: &(dyn Error + 'static)) -> (i32, &'static str) {
    if let Some(error) = error.downcast_ref::<io::Error>() {
        return match error.kind() {
            io::ErrorKind::InvalidInput => (2, "invalid_input"),
            io::ErrorKind::Unsupported => (3, "unsupported"),
            io::ErrorKind::InvalidData => (4, "validation_failed"),
            _ => (1, "operation_failed"),
        };
    }

    (1, "operation_failed")
}

pub fn print_error(command: &str, error: &(dyn Error + 'static)) {
    let (_, code) = classify_exit(error);
    let payload = json!({
        "schema": RESPONSE_SCHEMA,
        "schemaVersion": RESPONSE_SCHEMA_VERSION,
        "ok": false,
        "command": command,
        "error": {
            "code": code,
            "message": error.to_string(),
        }
    });

    match serde_json::to_string_pretty(&payload) {
        Ok(body) => println!("{body}"),
        Err(_) => eprintln!("error: {error}"),
    }
}

pub fn inspect(path: &Path, json_output: bool) -> Result<(), Box<dyn Error>> {
    let bytes = fs::read(path)?;
    let asset_type = asset_type(path);
    let container = inspect_container(&bytes);
    let details = inspect_format(path, &bytes, asset_type)?;

    if json_output {
        print_success(
            "inspect",
            json!({
                "path": path.display().to_string(),
                "type": asset_type,
                "bytes": bytes.len(),
                "container": container,
                "details": details,
            }),
        )?;
    } else {
        println!("file: {}", path.display());
        println!("type: {asset_type}");
        println!("bytes: {}", bytes.len());
        if let Some(container) = container {
            println!(
                "container: {}",
                container
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            );
        }
        println!(
            "status: {}",
            if details.is_null() {
                "container inspected"
            } else {
                "format parsed"
            }
        );
    }

    Ok(())
}

pub fn validate(path: &Path, json_output: bool) -> Result<(), Box<dyn Error>> {
    let bytes = fs::read(path)?;
    let asset_type = asset_type(path);
    let mut checks = Vec::new();

    if Rsc7Probe::parse(&bytes).is_ok() {
        Rsc7Resource::parse(&bytes).map_err(validation_error)?;
        checks.push("rsc7.container");
    }

    match asset_type {
        "YMAP" => {
            Ymap::from_bytes(&bytes).map_err(validation_error)?;
            checks.push("ymap.parse");
        }
        "YTYP" => {
            Ytyp::from_bytes(&bytes).map_err(validation_error)?;
            checks.push("ytyp.parse");
        }
        "YMF" => {
            Ymf::from_bytes(&bytes).map_err(validation_error)?;
            checks.push("ymf.parse");
        }
        "YTD" => {
            Ytd::from_bytes(&bytes).map_err(validation_error)?;
            checks.push("ytd.parse");
        }
        "YBN" => {
            YbnCollision::from_bytes(&bytes).map_err(validation_error)?;
            checks.push("ybn.parse");
        }
        "YDR" => {
            YdrDocument::from_bytes(&bytes).map_err(validation_error)?;
            checks.push("ydr.parse");
        }
        "YDD" => {
            YddDictionary::from_bytes(&bytes).map_err(validation_error)?;
            checks.push("ydd.parse");
        }
        _ if checks.is_empty() => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "no validator is available for file type {} ({})",
                    asset_type,
                    path.display()
                ),
            )
            .into());
        }
        _ => {}
    }

    if json_output {
        print_success(
            "validate",
            json!({
                "path": path.display().to_string(),
                "type": asset_type,
                "valid": true,
                "checks": checks,
            }),
        )?;
    } else {
        println!("file: {}", path.display());
        println!("type: {asset_type}");
        println!("valid: yes");
        for check in checks {
            println!("check: {check}");
        }
    }

    Ok(())
}

pub fn capabilities(path: &Path, json_output: bool) -> Result<(), Box<dyn Error>> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a file: {}", path.display()),
        )
        .into());
    }

    let asset_type = asset_type(path);
    let operations = operations_for(asset_type);

    if json_output {
        print_success(
            "capabilities",
            json!({
                "path": path.display().to_string(),
                "type": asset_type,
                "operations": operations,
            }),
        )?;
    } else {
        println!("file: {}", path.display());
        println!("type: {asset_type}");
        println!("operations:");
        for operation in operations {
            let id = operation
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let mode = if operation
                .get("writesAsset")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "write"
            } else {
                "read"
            };
            println!("  {id} [{mode}]");
        }
    }

    Ok(())
}

pub fn parse_path_json_args(
    args: impl Iterator<Item = String>,
    usage: &str,
) -> Result<(PathBuf, bool), io::Error> {
    let mut path = None;
    let mut json_output = false;

    for arg in args {
        match arg.as_str() {
            "--json" if !json_output => json_output = true,
            _ if arg.starts_with('-') => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{usage}; unknown option: {arg}"),
                ))
            }
            _ if path.is_none() => path = Some(PathBuf::from(arg)),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    usage.to_string(),
                ))
            }
        }
    }

    let path = path.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, usage))?;
    Ok((path, json_output))
}

pub fn parse_capabilities_args(
    args: impl Iterator<Item = String>,
    usage: &str,
) -> Result<(Option<PathBuf>, bool), io::Error> {
    let mut path = None;
    let mut json_output = false;

    for arg in args {
        match arg.as_str() {
            "--json" if !json_output => json_output = true,
            _ if arg.starts_with('-') => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{usage}; unknown option: {arg}"),
                ))
            }
            _ if path.is_none() => path = Some(PathBuf::from(arg)),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    usage.to_string(),
                ))
            }
        }
    }

    Ok((path, json_output))
}

pub fn normalize_command_args(args: Vec<String>) -> Vec<String> {
    if args.len() < 2 {
        return args;
    }

    let mapped = match (args[0].as_str(), args[1].as_str()) {
        ("ydr", "info") => Some("ydr-info"),
        ("ydr", "translate") => Some("ydr-translate"),
        ("ydr", "rebind-texture") => Some("ydr-rebind-texture"),
        ("ydr", "rebind-shader") => Some("ydr-rebind-shader"),
        ("ydd", "info") => Some("ydd-info"),
        ("ydd", "translate") => Some("ydd-translate"),
        ("ydd", "rebind-texture") => Some("ydd-rebind-texture"),
        ("ydd", "rebind-shader") => Some("ydd-rebind-shader"),
        ("ytd", "info") => Some("ytd-info"),
        ("ytd", "extract-dds") => Some("ytd-dds"),
        ("ytd", "replace-dds") => Some("ytd-replace-dds"),
        ("ytd", "repack-dds") => Some("ytd-repack-dds"),
        ("ytd", "repack-rgba") => Some("ytd-repack-rgba"),
        ("ytd", "rebuild-compact") => Some("ytd-rebuild-compact"),
        ("ybn", "info") => Some("ybn-info"),
        ("ymap", "info") => Some("ymap-info"),
        ("ytyp", "info") => Some("ytyp-info"),
        ("ymf", "info") => Some("ymf-info"),
        ("workspace", "scan") => Some("scan"),
        ("workspace", "deps") => Some("deps"),
        ("workspace", "providers") => Some("providers"),
        ("workspace", "preflight") => Some("preflight"),
        ("workspace", "mlo-audit") => Some("mlo-audit"),
        ("workspace", "extract") => Some("extract"),
        ("gta", "vanilla-index") => Some("vanilla-index"),
        _ => None,
    };

    let Some(mapped) = mapped else {
        return args;
    };

    let mut normalized = Vec::with_capacity(args.len() - 1);
    normalized.push(mapped.to_string());
    normalized.extend(args.into_iter().skip(2));
    normalized
}

fn print_success(command: &str, data: Value) -> Result<(), serde_json::Error> {
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": RESPONSE_SCHEMA,
            "schemaVersion": RESPONSE_SCHEMA_VERSION,
            "ok": true,
            "command": command,
            "data": data,
        }))?
    );
    Ok(())
}

fn inspect_container(bytes: &[u8]) -> Option<Value> {
    let probe = Rsc7Probe::parse(bytes).ok()?;
    let resource = Rsc7Resource::parse(bytes).ok();

    Some(json!({
        "kind": "RSC7",
        "version": probe.header.version,
        "systemFlags": format!("0x{:08X}", probe.header.system_flags),
        "graphicsFlags": format!("0x{:08X}", probe.header.graphics_flags),
        "systemSize": probe.header.system_size(),
        "graphicsSize": probe.header.graphics_size(),
        "decompression": resource.is_some(),
    }))
}

fn inspect_format(path: &Path, bytes: &[u8], asset_type: &str) -> Result<Value, Box<dyn Error>> {
    match asset_type {
        "YMAP" => {
            let ymap = Ymap::from_bytes(bytes).map_err(validation_error)?;
            let unique_archetypes = ymap
                .entities
                .iter()
                .map(|entity| entity.archetype_name.0)
                .collect::<BTreeSet<_>>()
                .len();
            Ok(json!({
                "name": optional_hash(ymap.name.map(|value| value.0)),
                "parent": optional_hash(ymap.parent.map(|value| value.0)),
                "entities": ymap.entities.len(),
                "uniqueArchetypes": unique_archetypes,
                "physicsDictionaries": ymap.physics_dictionaries.len(),
            }))
        }
        "YTYP" => {
            let ytyp = Ytyp::from_bytes(bytes).map_err(validation_error)?;
            let mlos = ytyp
                .archetypes
                .iter()
                .filter(|archetype| archetype.kind == ArchetypeKind::Mlo)
                .count();
            Ok(json!({
                "name": optional_hash(ytyp.name.map(|value| value.0)),
                "archetypes": ytyp.archetypes.len(),
                "mloArchetypes": mlos,
                "dependencies": ytyp.dependencies.len(),
            }))
        }
        "YMF" => {
            let ymf = Ymf::from_bytes(bytes).map_err(validation_error)?;
            Ok(json!({
                "format": "PSO",
                "ymaps": ymf.maps.len(),
                "ytypsWithDependencies": ymf.ytyps.len(),
                "interiors": ymf.interiors.len(),
            }))
        }
        "YTD" => {
            let ytd = Ytd::from_bytes(bytes).map_err(validation_error)?;
            let formats = ytd
                .textures
                .iter()
                .map(|texture| texture.format.normalized_name().to_string())
                .collect::<BTreeSet<_>>();
            Ok(json!({
                "resourceVersion": ytd.resource_version,
                "textures": ytd.textures.len(),
                "formats": formats,
            }))
        }
        "YBN" => {
            let collision = YbnCollision::from_bytes(bytes).map_err(validation_error)?;
            Ok(json!({
                "coordinateConvention": collision.coordinate_convention,
                "children": collision.children.len(),
                "vertices": collision.vertex_count(),
                "indices": collision.index_count(),
                "triangles": collision.triangle_count(),
                "materials": collision.materials.len(),
                "primitives": collision.primitives.len(),
            }))
        }
        "YDR" => {
            let document = YdrDocument::from_bytes(bytes).map_err(validation_error)?;
            let model = &document.model;
            Ok(json!({
                "name": model.name,
                "lod": model.lod.as_str(),
                "coordinateConvention": model.coordinate_convention.as_str(),
                "primitives": model.primitives.len(),
                "vertices": model.vertex_count(),
                "indices": model.index_count(),
                "triangles": model.triangle_count(),
                "shaders": model.shaders.len(),
                "embeddedTextures": document
                    .embedded_textures
                    .as_ref()
                    .map_or(0, |dictionary| dictionary.textures().len()),
            }))
        }
        "YDD" => {
            let dictionary = YddDictionary::from_bytes(bytes).map_err(validation_error)?;
            Ok(json!({
                "drawables": dictionary.entries().len(),
            }))
        }
        _ => {
            let _ = path;
            Ok(Value::Null)
        }
    }
}

fn operations_for(asset_type: &str) -> Vec<Value> {
    let mut operations = vec![
        operation("inspect", false, true, false),
        operation("validate", false, true, false),
        operation("capabilities", false, true, false),
    ];

    match asset_type {
        "YDR" => {
            operations.extend([
                operation("ydr.info", false, false, false),
                operation("ydr.translate", true, false, false),
                operation("ydr.rebind-texture", true, false, false),
                operation("ydr.rebind-shader", true, false, false),
            ]);
        }
        "YDD" => {
            operations.extend([
                operation("ydd.info", false, false, false),
                operation("ydd.translate", true, false, false),
                operation("ydd.rebind-texture", true, false, false),
                operation("ydd.rebind-shader", true, false, false),
            ]);
        }
        "YTD" => {
            operations.extend([
                operation("ytd.info", false, false, false),
                operation("ytd.extract-dds", true, false, false),
                operation("ytd.replace-dds", true, false, false),
                operation("ytd.repack-dds", true, false, false),
                operation("ytd.repack-rgba", true, false, false),
                operation("ytd.rebuild-compact", true, false, false),
            ]);
        }
        "YBN" => operations.push(operation("ybn.info", false, false, false)),
        "YMAP" => {
            operations.extend([
                operation("ymap.info", false, false, false),
                operation("workspace.deps", false, false, true),
                operation("workspace.providers", false, false, true),
                operation("workspace.preflight", false, false, true),
                operation("workspace.extract", true, false, true),
            ]);
        }
        "YTYP" => operations.push(operation("ytyp.info", false, false, false)),
        "YMF" => operations.push(operation("ymf.info", false, false, false)),
        _ => {}
    }

    operations
}

fn operation(
    id: &str,
    writes_asset: bool,
    structured_output: bool,
    requires_workspace: bool,
) -> Value {
    json!({
        "id": id,
        "writesAsset": writes_asset,
        "structuredOutput": structured_output,
        "requiresWorkspace": requires_workspace,
    })
}

fn optional_hash(value: Option<u32>) -> Value {
    value
        .map(|value| Value::String(format!("0x{value:08X}")))
        .unwrap_or(Value::Null)
}

fn asset_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "ymap" => "YMAP",
        "ytyp" => "YTYP",
        "ymf" => "YMF",
        "ydr" => "YDR",
        "ydd" => "YDD",
        "ytd" => "YTD",
        "ybn" => "YBN",
        "yft" => "YFT",
        "ycd" => "YCD",
        "ymt" => "YMT",
        "yld" => "YLD",
        "ynv" => "YNV",
        _ => "UNKNOWN",
    }
}

fn validation_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_namespaced_commands_without_touching_arguments() {
        assert_eq!(
            normalize_command_args(vec![
                "ydr".into(),
                "translate".into(),
                "a.ydr".into(),
                "1".into(),
                "2".into(),
                "3".into(),
                "b.ydr".into(),
            ]),
            vec!["ydr-translate", "a.ydr", "1", "2", "3", "b.ydr",]
        );
    }

    #[test]
    fn leaves_unknown_namespaces_unchanged() {
        assert_eq!(
            normalize_command_args(vec!["unknown".into(), "thing".into()]),
            vec!["unknown", "thing"]
        );
    }

    #[test]
    fn classifies_contract_errors() {
        let invalid = io::Error::new(io::ErrorKind::InvalidInput, "bad input");
        let unsupported = io::Error::new(io::ErrorKind::Unsupported, "unsupported");
        let validation = io::Error::new(io::ErrorKind::InvalidData, "invalid");

        assert_eq!(classify_exit(&invalid), (2, "invalid_input"));
        assert_eq!(classify_exit(&unsupported), (3, "unsupported"));
        assert_eq!(classify_exit(&validation), (4, "validation_failed"));
    }
}

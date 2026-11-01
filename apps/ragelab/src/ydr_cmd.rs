use std::{
    error::Error,
    fs, io,
    path::{Path, PathBuf},
};

use ragelab_engine::{
    inspect_ydr_bytes, validate_ydr_bytes, YdrDiagnosticSeverity, YdrInspectionReport,
    YdrValidationReport,
};
use serde_json::Value;

#[derive(Debug)]
pub struct ValidationCommandError {
    pub exit_code: i32,
    pub error_code: &'static str,
    pub message: String,
    pub report: Value,
}

impl std::fmt::Display for ValidationCommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ValidationCommandError {}

pub fn inspect(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    const USAGE: &str = "usage: ragelab ydr inspect <file.ydr> [--json]";
    let (source, json_output) = parse_source_args(args, USAGE)?;
    let bytes = fs::read(&source)?;
    let source_name = source_name(&source);
    let report = inspect_ydr_bytes(&bytes, &source_name)?;

    if json_output {
        super::agent::print_success("ydr.inspect", serde_json::to_value(&report)?)?;
    } else {
        print_inspection(&source, &report);
    }
    Ok(())
}

pub fn validate(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    const USAGE: &str = "usage: ragelab ydr validate <file.ydr> [--strict] [--json]";
    let (source, json_output, strict) = parse_validate_args(args, USAGE)?;
    let bytes = fs::read(&source)?;
    let source_name = source_name(&source);
    let report = validate_ydr_bytes(&bytes, &source_name, strict);

    if report.valid {
        if json_output {
            super::agent::print_success("ydr.validate", serde_json::to_value(&report)?)?;
        } else {
            print_validation(&source, &report);
        }
        return Ok(());
    }

    let has_error = report
        .diagnostics
        .iter()
        .any(|item| item.severity == YdrDiagnosticSeverity::Error);
    let has_unsupported = report
        .diagnostics
        .iter()
        .any(|item| item.severity == YdrDiagnosticSeverity::Unsupported);
    let (exit_code, error_code, message) = if has_error {
        (
            4,
            "validation_failed",
            format!(
                "YDR validation failed: {} error diagnostic(s)",
                report
                    .diagnostics
                    .iter()
                    .filter(|item| item.severity == YdrDiagnosticSeverity::Error)
                    .count()
            ),
        )
    } else if has_unsupported {
        (
            3,
            "unsupported",
            format!(
                "YDR contains {} unsupported feature(s)",
                report
                    .diagnostics
                    .iter()
                    .filter(|item| item.severity == YdrDiagnosticSeverity::Unsupported)
                    .count()
            ),
        )
    } else {
        (
            4,
            "validation_failed",
            "YDR validation did not pass".to_owned(),
        )
    };

    if !json_output {
        print_validation(&source, &report);
    }

    Err(Box::new(ValidationCommandError {
        exit_code,
        error_code,
        message,
        report: serde_json::to_value(&report)?,
    }))
}

fn parse_source_args(args: Vec<String>, usage: &str) -> Result<(PathBuf, bool), io::Error> {
    let mut source = None;
    let mut json_output = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json_output = true,
            value if value.starts_with('-') => {
                return Err(invalid_input(format!("unknown option {value}\\n{usage}")));
            }
            value => {
                if source.replace(PathBuf::from(value)).is_some() {
                    return Err(invalid_input(usage));
                }
            }
        }
    }
    Ok((source.ok_or_else(|| invalid_input(usage))?, json_output))
}

fn parse_validate_args(args: Vec<String>, usage: &str) -> Result<(PathBuf, bool, bool), io::Error> {
    let mut source = None;
    let mut json_output = false;
    let mut strict = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json_output = true,
            "--strict" => strict = true,
            value if value.starts_with('-') => {
                return Err(invalid_input(format!("unknown option {value}\\n{usage}")));
            }
            value => {
                if source.replace(PathBuf::from(value)).is_some() {
                    return Err(invalid_input(usage));
                }
            }
        }
    }
    Ok((
        source.ok_or_else(|| invalid_input(usage))?,
        json_output,
        strict,
    ))
}

fn source_name(path: &Path) -> String {
    path.file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("drawable.ydr")
        .to_owned()
}

fn print_inspection(path: &Path, report: &YdrInspectionReport) {
    println!("file: {}", path.display());
    println!("format: YDR / RSC7");
    println!("sha256: {}", report.source_sha256);
    println!("bytes: {}", report.source_byte_length);
    println!(
        "container: {} v{} | system={} bytes | graphics={} bytes | compressed payload={} bytes",
        report.container.magic,
        report.container.resource_version,
        report.container.system_segment_bytes,
        report.container.graphics_segment_bytes,
        report.container.compressed_payload_bytes
    );
    println!(
        "flags: system={} graphics={}",
        report.container.system_flags, report.container.graphics_flags
    );
    println!(
        "drawable: {} | selected LOD={} | coordinates={}",
        report.drawable.name.as_deref().unwrap_or("<unnamed>"),
        report.drawable.selected_lod,
        report.drawable.coordinate_convention
    );
    println!(
        "bounds: min={:?} max={:?} center={:?} radius={}",
        report.drawable.bounds.min,
        report.drawable.bounds.max,
        report.drawable.bounds.center,
        report.drawable.bounds.radius
    );
    println!(
        "totals: {} LODs, {} models, {} geometries, {} vertices, {} indices, {} triangles",
        report.drawable.lod_count,
        report.drawable.model_count,
        report.drawable.geometry_count,
        report.drawable.vertex_count,
        report.drawable.index_count,
        report.drawable.triangle_count
    );

    for lod in &report.lods {
        println!(
            "LOD {}: {} models, {} geometries, {} vertices, {} indices, {} triangles",
            lod.level,
            lod.model_count,
            lod.geometry_count,
            lod.vertex_count,
            lod.index_count,
            lod.triangle_count
        );
        for geometry in &lod.geometries {
            println!(
                "  model[{}]/geometry[{}]: {} vertices, {} indices, {} triangles, shader={}",
                geometry.model_index,
                geometry.geometry_index,
                geometry.vertex_count,
                geometry.index_count,
                geometry.triangle_count,
                geometry
                    .shader_index
                    .map_or_else(|| "none".into(), |value| value.to_string())
            );
            println!(
                "    layout: stride={} components={} flags={} types={} attributes={}",
                geometry.declaration_stride,
                geometry.declaration_component_count,
                geometry.declaration_flags,
                geometry.declaration_types,
                if geometry.attributes.is_empty() {
                    "<none>".into()
                } else {
                    geometry.attributes.join(", ")
                }
            );
            println!(
                "    geometry bounds: min={:?} max={:?}",
                geometry.geometry_bounds_min, geometry.geometry_bounds_max
            );
        }
    }

    println!("shaders: {}", report.shaders.len());
    for shader in &report.shaders {
        println!(
            "  [{}] name={} file={} bucket={} params={} texture-params={}",
            shader.index,
            shader.name_hash,
            shader.file_hash,
            shader.render_bucket,
            shader.parameters.len(),
            shader.texture_parameters_count
        );
        for reference in &shader.texture_references {
            println!(
                "    texture reference {} -> {}",
                reference.parameter_hash,
                reference.texture_name.as_deref().unwrap_or("<unresolved>")
            );
        }
    }

    match &report.embedded_textures {
        Some(dictionary) => {
            println!("embedded textures: {}", dictionary.texture_count);
            for texture in &dictionary.textures {
                println!(
                    "  [{}] {} {}x{}x{} format={} mips={}",
                    texture.index,
                    texture.name.as_deref().unwrap_or("<unnamed>"),
                    texture.width,
                    texture.height,
                    texture.depth,
                    texture.format,
                    texture.mip_levels
                );
            }
        }
        None => println!("embedded textures: none"),
    }
    println!("structural audit: {}", report.structural_audit.valid);
    if let Some(error) = &report.structural_audit.error {
        println!("structural audit error: {error}");
    }
    println!("semantic reopen: {}", report.semantic_reopen);
    println!("runtimeRenderable: {}", report.runtime_renderable);
}

fn print_validation(path: &Path, report: &YdrValidationReport) {
    println!("file: {}", path.display());
    println!("valid: {}", report.valid);
    println!("strict: {}", report.strict);
    println!("container valid: {}", report.container_valid);
    println!("format valid: {}", report.format_valid);
    println!("semantic reopen: {}", report.semantic_reopen);
    println!(
        "CodeWalker GUI accepted: {}",
        report.codewalker_gui_accepted
    );
    println!("runtimeRenderable: {}", report.runtime_renderable);
    if let Some(inspection) = &report.inspection {
        println!(
            "counts: {} LODs, {} models, {} geometries, {} shaders, {} textures",
            inspection.drawable.lod_count,
            inspection.drawable.model_count,
            inspection.drawable.geometry_count,
            inspection.shaders.len(),
            inspection
                .embedded_textures
                .as_ref()
                .map_or(0, |value| value.texture_count)
        );
    }
    for diagnostic in &report.diagnostics {
        println!(
            "{:?} {}{}: {}",
            diagnostic.severity,
            diagnostic.code,
            diagnostic
                .path
                .as_ref()
                .map_or_else(String::new, |path| format!(" [{path}]")),
            diagnostic.message
        );
    }
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

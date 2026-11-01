use std::{
    collections::HashSet,
    error::Error,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use ragelab_engine::{
    decode_glb_geometry_bytes, default_glb_authoring_profile, export_glb_to_ydr,
    export_glb_to_ydr_with_nodes, inspect_glb_bytes, validate_glb_authoring_profile_bytes,
    GlbShaderTemplateFormat, GlbShaderTemplateInput, GlbYdrExportRequest,
};
use ragelab_gltf::{
    DecodedGeometryReport, GlbInspectionReport, ProfileValidationReport, TextureBindingInfo,
};
use serde_json::json;

pub fn inspect(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    let (source, json_output) = parse_inspect_args(args)?;
    let bytes = fs::read(&source)?;
    let file_name = source
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("model.glb");
    let report = inspect_glb_bytes(&bytes, file_name)?;
    if json_output {
        super::agent::print_success("glb.inspect", serde_json::to_value(&report)?)?;
    } else {
        print_inspection(&source, &report);
    }
    Ok(())
}

pub fn geometry(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    let (source, json_output) = parse_inspect_args(args)?;
    let bytes = fs::read(&source)?;
    let file_name = source
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("model.glb");
    let report = decode_glb_geometry_bytes(&bytes, file_name)?;

    if json_output {
        super::agent::print_success("glb.geometry", serde_json::to_value(&report)?)?;
    } else {
        print_geometry(&source, &report);
    }
    Ok(())
}

pub fn tree(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    let (source, json_output) = parse_inspect_args(args)?;
    let bytes = fs::read(&source)?;
    let file_name = source
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("model.glb");
    let inspection = inspect_glb_bytes(&bytes, file_name)?;
    let scene_index = inspection
        .default_scene
        .or_else(|| (!inspection.scenes.is_empty()).then_some(0));
    let roots = if let Some(index) = scene_index {
        inspection
            .scenes
            .get(index)
            .ok_or_else(|| invalid_data(format!("active scene index {index} is out of range")))?
            .nodes
            .clone()
    } else {
        let child_nodes = inspection
            .nodes
            .iter()
            .flat_map(|node| node.children.iter().copied())
            .collect::<HashSet<_>>();
        inspection
            .nodes
            .iter()
            .filter(|node| !child_nodes.contains(&node.index))
            .map(|node| node.index)
            .collect()
    };

    let mut visited = HashSet::new();
    let mut tree_roots = Vec::with_capacity(roots.len());
    let mut mesh_node_count = 0usize;
    let mut primitive_count = 0usize;
    for root in roots {
        let (entry, meshes, primitives) =
            build_node_tree(root, None, 0, &inspection, &mut visited)?;
        tree_roots.push(entry);
        mesh_node_count += meshes;
        primitive_count += primitives;
    }

    let data = json!({
        "schema": "ragelab.glb.node-tree",
        "schemaVersion": 1,
        "source": {
            "fileName": inspection.source.file_name,
            "sha256": inspection.source.sha256,
            "byteLength": inspection.source.byte_length,
        },
        "sceneIndex": scene_index,
        "nodeCount": inspection.nodes.len(),
        "meshNodeCount": mesh_node_count,
        "primitiveCount": primitive_count,
        "roots": tree_roots,
    });

    if json_output {
        super::agent::print_success("glb.tree", data)?;
    } else {
        println!("file: {}", source.display());
        println!(
            "scene: {}",
            scene_index.map_or_else(|| "none".into(), |v| v.to_string())
        );
        println!(
            "hierarchy: {} nodes, {} mesh nodes, {} primitives",
            inspection.nodes.len(),
            mesh_node_count,
            primitive_count
        );
        for root in &tree_roots {
            print_node_tree(root);
        }
        println!("select a group by its node index with --node <index> on glb export");
    }
    Ok(())
}

fn build_node_tree(
    node_index: usize,
    parent_index: Option<usize>,
    depth: usize,
    inspection: &GlbInspectionReport,
    visited: &mut HashSet<usize>,
) -> Result<(serde_json::Value, usize, usize), io::Error> {
    if !visited.insert(node_index) {
        return Err(invalid_data(format!(
            "active scene contains a cycle or repeated node reference at {node_index}"
        )));
    }
    let node = inspection
        .nodes
        .get(node_index)
        .filter(|node| node.index == node_index)
        .ok_or_else(|| invalid_data(format!("scene references missing node {node_index}")))?;
    let own_primitive_count = if let Some(mesh_index) = node.mesh_index {
        inspection
            .meshes
            .get(mesh_index)
            .ok_or_else(|| {
                invalid_data(format!(
                    "node {node_index} references missing mesh {mesh_index}"
                ))
            })?
            .primitives
            .len()
    } else {
        0
    };
    let mut subtree_mesh_count = usize::from(node.mesh_index.is_some());
    let mut subtree_primitive_count = own_primitive_count;
    let mut children = Vec::with_capacity(node.children.len());
    for child_index in &node.children {
        let (child, child_meshes, child_primitives) = build_node_tree(
            *child_index,
            Some(node_index),
            depth + 1,
            inspection,
            visited,
        )?;
        children.push(child);
        subtree_mesh_count += child_meshes;
        subtree_primitive_count += child_primitives;
    }
    let value = json!({
        "nodeIndex": node.index,
        "parentNodeIndex": parent_index,
        "depth": depth,
        "name": node.name,
        "meshIndex": node.mesh_index,
        "primitiveCount": own_primitive_count,
        "subtreeMeshCount": subtree_mesh_count,
        "subtreePrimitiveCount": subtree_primitive_count,
        "children": children,
    });
    Ok((value, subtree_mesh_count, subtree_primitive_count))
}

fn print_node_tree(node: &serde_json::Value) {
    let depth = node["depth"].as_u64().unwrap_or(0) as usize;
    let node_index = node["nodeIndex"].as_u64().unwrap_or(0);
    let name = node["name"].as_str().unwrap_or("<unnamed>");
    let mesh_index = node["meshIndex"].as_u64();
    let own_primitives = node["primitiveCount"].as_u64().unwrap_or(0);
    let subtree_meshes = node["subtreeMeshCount"].as_u64().unwrap_or(0);
    let subtree_primitives = node["subtreePrimitiveCount"].as_u64().unwrap_or(0);
    let kind = mesh_index.map_or_else(
        || "group".to_owned(),
        |index| format!("mesh {index}, {own_primitives} primitives"),
    );
    println!(
        "{}[{}] {} — {}; subtree: {} meshes / {} primitives",
        "  ".repeat(depth),
        node_index,
        name,
        kind,
        subtree_meshes,
        subtree_primitives
    );
    if let Some(children) = node["children"].as_array() {
        for child in children {
            print_node_tree(child);
        }
    }
}

pub fn export(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    const USAGE: &str = "usage: ragelab glb export <source.glb> --output <output.ydr> [--node <index> ...] [--profile <profile.json>] [--shader-template <source.yft|source.ydr>] [--shader-index <n>] [--overwrite] [--json]";
    let mut source = None;
    let mut output = None;
    let mut profile_path = None;
    let mut shader_template_path = None;
    let mut shader_index = 0usize;
    let mut selected_node_indices = Vec::new();
    let mut overwrite = false;
    let mut json_output = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output" => {
                output = Some(PathBuf::from(
                    args.next().ok_or_else(|| invalid_input(USAGE))?,
                ))
            }
            "--profile" => {
                profile_path = Some(PathBuf::from(
                    args.next().ok_or_else(|| invalid_input(USAGE))?,
                ))
            }
            "--shader-template" => {
                shader_template_path = Some(PathBuf::from(
                    args.next().ok_or_else(|| invalid_input(USAGE))?,
                ))
            }
            "--shader-index" => {
                shader_index = args
                    .next()
                    .ok_or_else(|| invalid_input(USAGE))?
                    .parse::<usize>()
                    .map_err(|_| invalid_input("--shader-index must be a non-negative integer"))?;
            }
            "--node" => {
                let index = args
                    .next()
                    .ok_or_else(|| invalid_input(USAGE))?
                    .parse::<usize>()
                    .map_err(|_| invalid_input("--node must be a non-negative integer"))?;
                selected_node_indices.push(index);
            }
            "--overwrite" => overwrite = true,
            "--json" => json_output = true,
            value if value.starts_with('-') => {
                return Err(invalid_input(format!("unknown option {value}\\n{USAGE}")).into())
            }
            value => {
                if source.replace(PathBuf::from(value)).is_some() {
                    return Err(invalid_input(USAGE).into());
                }
            }
        }
    }
    let source = source.ok_or_else(|| invalid_input(USAGE))?;
    let output = output.ok_or_else(|| invalid_input(USAGE))?;
    ensure_distinct_paths(&source, &output)?;
    if let Some(path) = &profile_path {
        ensure_distinct_paths(path, &output)?;
    }
    if let Some(path) = &shader_template_path {
        ensure_distinct_paths(path, &output)?;
    }

    let source_bytes = fs::read(&source)?;
    let source_name = source
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("model.glb")
        .to_owned();
    let profile_bytes = profile_path.as_ref().map(fs::read).transpose()?;
    let shader_template = if let Some(path) = shader_template_path.as_ref() {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let format = match extension.as_str() {
            "yft" => GlbShaderTemplateFormat::LegacyYft,
            "ydr" => GlbShaderTemplateFormat::LegacyYdr,
            _ => {
                return Err(invalid_input(
                    "--shader-template must reference a Legacy .yft or .ydr resource",
                )
                .into())
            }
        };
        Some(GlbShaderTemplateInput {
            format,
            source_name: path.display().to_string(),
            bytes: fs::read(path)?,
        })
    } else {
        None
    };

    let request = GlbYdrExportRequest {
        source_name,
        source_bytes,
        profile_bytes,
        shader_template,
        shader_index,
    };
    let result = if selected_node_indices.is_empty() {
        export_glb_to_ydr(request)?
    } else {
        export_glb_to_ydr_with_nodes(request, &selected_node_indices)?
    };

    write_output(&output, &result.output_bytes, overwrite)?;
    if json_output {
        let mut data = serde_json::to_value(&result.report)?;
        let object = data
            .as_object_mut()
            .ok_or_else(|| invalid_data("engine export report must serialize as an object"))?;
        object.insert("source".into(), json!(source));
        object.insert("output".into(), json!(output));
        super::agent::print_success("glb.export", data)?;
    } else {
        print_export(&source, &output, &result.report);
    }
    Ok(())
}

pub fn profile(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    let mut args = args.into_iter();
    let action = args
        .next()
        .ok_or_else(|| invalid_input("usage: ragelab glb profile <init|validate> ..."))?;
    match action.as_str() {
        "init" => profile_init(args.collect()),
        "validate" => profile_validate(args.collect()),
        other => Err(invalid_input(format!("unknown glb profile action: {other}")).into()),
    }
}

fn profile_init(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    const USAGE: &str =
        "usage: ragelab glb profile init <source.glb> --output <profile.json> [--overwrite] [--json]";
    let mut source = None;
    let mut output = None;
    let mut overwrite = false;
    let mut json_output = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output" => {
                output = Some(PathBuf::from(
                    args.next().ok_or_else(|| invalid_input(USAGE))?,
                ))
            }
            "--overwrite" => overwrite = true,
            "--json" => json_output = true,
            value if value.starts_with('-') => {
                return Err(invalid_input(format!("unknown option {value}\\n{USAGE}")).into())
            }
            value => {
                if source.replace(PathBuf::from(value)).is_some() {
                    return Err(invalid_input(USAGE).into());
                }
            }
        }
    }
    let source = source.ok_or_else(|| invalid_input(USAGE))?;
    let output = output.ok_or_else(|| invalid_input(USAGE))?;
    ensure_distinct_paths(&source, &output)?;
    let source_bytes = fs::read(&source)?;
    let source_name = source
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("model.glb");
    let profile = default_glb_authoring_profile(&source_bytes, source_name)?;
    let body = serde_json::to_vec_pretty(&profile)?;
    write_profile(&output, &body, overwrite)?;

    if json_output {
        super::agent::print_success(
            "glb.profile.init",
            json!({
                "source": source,
                "profile": output,
                "materialCount": profile.materials.len(),
                "textureCount": profile.source.texture_count,
                "embedTextures": profile.settings.embed_textures,
                "sourceSha256": profile.source.sha256,
                "nextStep": "edit the profile JSON, then run glb profile validate"
            }),
        )?;
    } else {
        println!("profile: {}", output.display());
        println!("source: {}", source.display());
        println!("materials: {}", profile.materials.len());
        println!("textures: {}", profile.source.texture_count);
        println!("embedded textures: {}", profile.settings.embed_textures);
        println!("next: edit the profile JSON and run glb profile validate");
    }
    Ok(())
}

fn profile_validate(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    const USAGE: &str = "usage: ragelab glb profile validate <source.glb> <profile.json> [--json]";
    let mut positional = Vec::new();
    let mut json_output = false;
    for arg in args {
        if arg == "--json" {
            json_output = true;
        } else if arg.starts_with('-') {
            return Err(invalid_input(format!("unknown option {arg}\\n{USAGE}")).into());
        } else {
            positional.push(PathBuf::from(arg));
        }
    }
    if positional.len() != 2 {
        return Err(invalid_input(USAGE).into());
    }
    let source_bytes = fs::read(&positional[0])?;
    let profile_bytes = fs::read(&positional[1])?;
    let source_name = positional[0]
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("model.glb");
    let validation =
        validate_glb_authoring_profile_bytes(&source_bytes, source_name, &profile_bytes)?;
    if json_output {
        super::agent::print_success("glb.profile.validate", serde_json::to_value(&validation)?)?;
    } else {
        print_profile_validation(&validation);
    }
    Ok(())
}

fn parse_inspect_args(args: Vec<String>) -> Result<(PathBuf, bool), io::Error> {
    const USAGE: &str = "usage: ragelab glb inspect <file.glb> [--json]";
    let mut source = None;
    let mut json_output = false;
    for arg in args {
        if arg == "--json" {
            json_output = true;
        } else if arg.starts_with('-') {
            return Err(invalid_input(format!("unknown option {arg}\\n{USAGE}")));
        } else if source.replace(PathBuf::from(arg)).is_some() {
            return Err(invalid_input(USAGE));
        }
    }
    Ok((source.ok_or_else(|| invalid_input(USAGE))?, json_output))
}

fn print_geometry(path: &Path, report: &DecodedGeometryReport) {
    println!("file: {}", path.display());
    println!("sha256: {}", report.source_sha256);
    println!(
        "scene: {}",
        report
            .selected_scene
            .map_or_else(|| "none".into(), |value| value.to_string())
    );
    println!("decoded primitives: {}", report.primitives.len());
    for primitive in &report.primitives {
        println!(
            "  node {} / mesh {} / primitive {}: {} vertices, {} triangle indices, material {}",
            primitive.node_name.as_deref().unwrap_or("<unnamed>"),
            primitive.mesh_name.as_deref().unwrap_or("<unnamed>"),
            primitive.primitive_index,
            primitive.vertex_count,
            primitive.index_count,
            primitive
                .material_index
                .map_or_else(|| "<default>".into(), |value| value.to_string())
        );
        println!(
            "    normals={} tangents={} uv0={} uv1={} colors0={}",
            primitive.normals.is_some(),
            primitive.tangents.is_some(),
            primitive.texcoord_0.is_some(),
            primitive.texcoord_1.is_some(),
            primitive.colors_0.is_some()
        );
    }
    for warning in &report.warnings {
        println!("warning: {warning}");
    }
}

fn print_profile_validation(report: &ProfileValidationReport) {
    println!("valid: {}", report.valid);
    println!("materials: {}", report.material_count);
    println!("mapped texture slots: {}", report.mapped_texture_slots);
    println!("embedded textures requested: {}", report.embed_textures);
    for note in &report.notes {
        println!("note: {note}");
    }
}

fn print_export(source: &Path, output: &Path, report: &ragelab_engine::GlbYdrExportReport) {
    println!("source: {}", source.display());
    println!("output: {}", output.display());
    println!("YDR version: {}", report.ydr_version);
    println!("geometries: {}", report.geometry_count);
    if let Some(selection) = &report.selection {
        println!(
            "selection schema: {} v{}",
            selection.schema, selection.schema_version
        );
        println!("selected roots:");
        for selected in &selection.selected_nodes {
            println!(
                "  [{}] {} — {} meshes / {} primitives",
                selected.node_index,
                selected.node_name.as_deref().unwrap_or("<unnamed>"),
                selected.mesh_indices.len(),
                selected.primitive_count
            );
        }
        println!(
            "included nodes: {}; meshes: {}; primitives: {}",
            selection.included_node_indices.len(),
            selection.included_mesh_indices.len(),
            selection.primitive_count
        );
    }
    println!("vertices: {}", report.vertex_count);
    println!("triangles: {}", report.triangle_count);
    println!("embedded textures: {}", report.embedded_texture_count);
    println!("texture formats: {:?}", report.texture_formats);
    println!("material profile: validated");
    println!("ShaderFX templates: {}", report.shader_count);
    if report.shader_count == 0 {
        eprintln!("WARNING: this structural YDR has no ShaderFX. Do not open it in CodeWalker Model Viewer; its BasicShader path expects a resolved DrawableGeometry.Shader. Re-export with --shader-template and --shader-index using a verified Legacy template.");
    }
    println!(
        "material shader bindings: {}",
        report.material_shader_bindings
    );
    println!(
        "geometry shader mappings: {}",
        report.geometry_shader_mappings
    );
    println!(
        "sampler texture bindings: {}",
        report.shader_texture_bindings
    );
    println!("semantic reopen: {}", report.semantic_reopen);
    println!("runtimeRenderable: {}", report.runtime_renderable);
    println!("status: structural export completed; GTA runtime rendering is not established");
}

fn print_inspection(path: &Path, report: &GlbInspectionReport) {
    println!("file: {}", path.display());
    println!("format: GLB / glTF {}", report.asset.version);
    println!("bytes: {}", report.source.byte_length);
    println!("sha256: {}", report.source.sha256);
    println!(
        "scene: {}",
        report
            .default_scene
            .map_or_else(|| "none".into(), |v| v.to_string())
    );
    println!(
        "counts: {} nodes, {} meshes, {} primitives, {} materials, {} textures, {} images",
        report.counts.nodes,
        report.counts.meshes,
        report.counts.primitives,
        report.counts.materials,
        report.counts.textures,
        report.counts.images
    );

    println!("\nMeshes:");
    for mesh in &report.meshes {
        println!(
            "  [{}] {} ({} primitives)",
            mesh.index,
            mesh.name.as_deref().unwrap_or("<unnamed>"),
            mesh.primitives.len()
        );
        for primitive in &mesh.primitives {
            let positions = primitive
                .attributes
                .get("POSITION")
                .map_or_else(|| "no POSITION".into(), |a| format!("{} vertices", a.count));
            println!(
                "    primitive {}: {}, {}, material {}",
                primitive.index,
                primitive.mode_name,
                positions,
                primitive
                    .material_index
                    .map_or_else(|| "<default>".into(), |v| v.to_string())
            );
            println!(
                "      attributes: {}",
                primitive
                    .attributes
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }

    println!("\nMaterials:");
    for material in &report.materials {
        println!(
            "  [{}] {} | alpha={} | roughness={:.3} | metallic={:.3} | doubleSided={}",
            material.index,
            material.name.as_deref().unwrap_or("<unnamed>"),
            material.alpha_mode,
            material.roughness_factor,
            material.metallic_factor,
            material.double_sided
        );
        print_binding("albedo", material.base_color_texture.as_ref());
        print_binding("normal", material.normal_texture.as_ref());
        print_binding(
            "metallic/roughness",
            material.metallic_roughness_texture.as_ref(),
        );
        print_binding("specular", material.specular_texture.as_ref());
        print_binding("specular color", material.specular_color_texture.as_ref());
        print_binding("occlusion", material.occlusion_texture.as_ref());
        print_binding("emissive", material.emissive_texture.as_ref());
        if material.transmission_factor.unwrap_or(0.0) > 0.0 {
            println!(
                "      transmission: {:.3}",
                material.transmission_factor.unwrap_or(0.0)
            );
        }
        if let Some(ior) = material.ior {
            println!("      IOR: {:.3}", ior);
        }
    }

    println!("\nTextures:");
    for texture in &report.textures {
        let image = texture
            .source_image_index
            .and_then(|index| report.images.get(index));
        println!(
            "  [{}] {} -> image {}",
            texture.index,
            texture.name.as_deref().unwrap_or("<unnamed>"),
            texture
                .source_image_index
                .map_or_else(|| "<extension/unknown>".into(), |v| v.to_string())
        );
        if let Some(image) = image {
            println!(
                "      {} | {} | {}",
                image.mime_type.as_deref().unwrap_or("unknown MIME"),
                image.storage,
                image.dimensions.map_or_else(
                    || "dimensions unknown".into(),
                    |d| format!("{}x{}", d.width, d.height)
                )
            );
        }
    }
    println!("\nImages:");
    for image in &report.images {
        println!(
            "  [{}] {} | {} | {} bytes | embedded={}",
            image.index,
            image.name.as_deref().unwrap_or("<unnamed>"),
            image.mime_type.as_deref().unwrap_or("unknown MIME"),
            image
                .byte_length
                .map_or_else(|| "unknown".into(), |v| v.to_string()),
            image.embedded
        );
    }
    for extension in &report.extensions_required {
        println!("required extension: {extension}");
    }
    for warning in &report.warnings {
        println!("warning: {warning}");
    }
}

fn print_binding(label: &str, binding: Option<&TextureBindingInfo>) {
    if let Some(binding) = binding {
        println!(
            "      {label}: texture {} (UV set {})",
            binding.texture_index, binding.tex_coord
        );
    }
}

fn ensure_distinct_paths(source: &Path, output: &Path) -> Result<(), io::Error> {
    let source = source.canonicalize()?;
    let output_abs = if output.exists() {
        output.canonicalize()?
    } else {
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let name = output
            .file_name()
            .ok_or_else(|| invalid_input("output must name a file"))?;
        parent.canonicalize()?.join(name)
    };
    if source == output_abs {
        return Err(invalid_input("output must not overwrite the input file"));
    }
    Ok(())
}

fn write_output(path: &Path, body: &[u8], overwrite: bool) -> Result<(), io::Error> {
    let mut options = OpenOptions::new();
    options.write(true);
    if overwrite {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    let mut file = options.open(path)?;
    file.write_all(body)?;
    file.flush()
}

fn write_profile(path: &Path, body: &[u8], overwrite: bool) -> Result<(), io::Error> {
    let mut options = OpenOptions::new();
    options.write(true);
    if overwrite {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    let mut file = options.open(path)?;
    file.write_all(body)?;
    file.write_all(b"\n")?;
    file.flush()
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

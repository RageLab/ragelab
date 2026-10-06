mod agent;
mod bridge_cmd;
mod rpf_cmd;

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
};

use ragelab_assets::WorkspaceIndex;
use ragelab_engine::{
    build_vanilla_catalog, export_map_resource, export_ytd_texture_png,
    parse_durtyfree_object_list, parse_vanilla_file_catalog, render_vanilla_catalog_paths,
    replace_ytd_texture_from_png, summarize_mlo_audit, workspace_export_preflight_report,
    workspace_export_report, CatalogRefs, DurtyFreeCatalog, SharedExportOptions,
    VanillaFileCatalog,
};
use ragelab_hash::{jenkins, joaat};
use ragelab_meta::MetaDocument;
use ragelab_resource::{Rsc7Probe, Rsc7Resource};
use ragelab_ybn::YbnCollision;
use ragelab_ydd::{YddDictionary, YddEditSession};
use ragelab_ydr::{ShaderBindingKey, TextureBindingKey, YdrDocument, YdrEditSession};
use ragelab_ymap::Ymap;
use ragelab_ymf::{ManifestFlags, Ymf};
use ragelab_ytd::Ytd;
use ragelab_ytyp::{ArchetypeKind, AssetType, Ytyp};
use serde_json::json;

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let normalized_for_error = agent::normalize_command_args(args.clone());
    let command = normalized_for_error
        .first()
        .cloned()
        .unwrap_or_else(|| "help".to_string());
    let structured_error = normalized_for_error.iter().any(|arg| arg == "--json")
        && agent::is_structured_command(&command);

    if let Err(error) = run(args) {
        let (exit_code, _) = agent::classify_exit(error.as_ref());
        if structured_error {
            agent::print_error(&command, error.as_ref());
        } else {
            eprintln!("error: {error}");
        }
        std::process::exit(exit_code);
    }
}

fn run(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    let normalized = agent::normalize_command_args(args);
    let mut args = normalized.into_iter();
    let Some(command) = args.next() else {
        print_help();
        return Ok(());
    };

    match command.as_str() {
        "version" => {
            let json = parse_json_flag(args, "usage: ragelab version [--json]")?;
            print_version(json)?;
        }
        "capabilities" => {
            const USAGE: &str = "usage: ragelab capabilities [file] [--json]";
            let (path, json) = agent::parse_capabilities_args(args, USAGE)?;
            if let Some(path) = path {
                agent::capabilities(&path, json)?;
            } else {
                print_capabilities(json)?;
            }
        }
        "inspect" => {
            let (path, json) =
                agent::parse_path_json_args(args, "usage: ragelab inspect <file> [--json]")?;
            agent::inspect(&path, json)?;
        }
        "validate" => {
            let (path, json) =
                agent::parse_path_json_args(args, "usage: ragelab validate <file> [--json]")?;
            agent::validate(&path, json)?;
        }
        "spatial" => {
            let (path, json) =
                agent::parse_path_json_args(args, "usage: ragelab spatial <file> [--json]")?;
            agent::spatial(&path, json)?;
        }
        "preview" => {
            const USAGE: &str = "usage: ragelab preview <file> [--drawable-index <n>] [--max-primitives <n>] [--max-vertices <n>] [--max-indices <n>] [--max-shaders <n>] [--max-texture-references <n>] [--max-children <n>] [--max-materials <n>] [--json]";
            let (path, options, json) = agent::parse_preview_args(args, USAGE)?;
            agent::preview(&path, options, json)?;
        }
        "scene" => {
            const USAGE: &str =
                "usage: ragelab workspace scene <directory> <file.ymap> [--fallback-root <directory>]... [--rpf-mount <archive.rpf> [--rpf-nested <entry.rpf>]...]... [--game-root <directory> --game-index <file>] [--rpf-keys <directory>] [--max-nodes <n>] [--json]";
            let scene = agent::parse_scene_args(args, USAGE)?;
            agent::scene(
                &scene.workspace,
                &scene.ymap,
                &scene.fallback_roots,
                &scene.rpf_mounts,
                scene.game_index.as_ref(),
                scene.options,
                scene.json_output,
            )?;
        }
        "render-package" => {
            const USAGE: &str =
                "usage: ragelab workspace render-package <directory> <file.ymap> --output <file> [--fallback-root <directory>]... [--rpf-mount <archive.rpf> [--rpf-nested <entry.rpf>]...]... [--game-root <directory> --game-index <file>] [--rpf-keys <directory>] [--max-nodes <n>] [--max-assets <n>] [--max-blob-bytes <n>] [--overwrite] [--json]";
            let command = agent::parse_render_package_args(args, USAGE)?;
            agent::render_package(
                &command.scene,
                &command.output,
                command.options,
                command.overwrite,
            )?;
        }
        "render.asset" => {
            const USAGE: &str =
                "usage: ragelab render asset <file.ydr|file.ydd|file.yft> --output <file.png> [--drawable-index <n>] [--width <n>] [--height <n>] [--view auto|front|back|left|right|top|isometric] [--projection perspective|orthographic] [--transparent] [--grid] [--wireframe] [--bounds] [--metadata <file.json>] [--overwrite] [--json]";
            let command = agent::parse_render_asset_args(args, USAGE)?;
            agent::render_asset(&command)?;
        }
        "render.scene" => {
            const USAGE: &str =
                "usage: ragelab render scene <directory> <file.ymap> --output <file.png> [scene source options] [--max-nodes <n>] [--max-assets <n>] [--max-blob-bytes <n>] [--width <n>] [--height <n>] [--view auto|front|back|left|right|top|isometric] [--projection perspective|orthographic] [--transparent] [--grid] [--wireframe] [--bounds] [--metadata <file.json>] [--overwrite] [--json]";
            let command = agent::parse_render_scene_args(args, USAGE)?;
            agent::render_scene(&command)?;
        }
        "render.compare" => {
            const USAGE: &str =
                "usage: ragelab render compare <expected.png> <actual.png> [--tolerance <0..255>] [--json]";
            let command = agent::parse_render_compare_args(args, USAGE)?;
            agent::render_compare(&command)?;
        }
        "plan" => {
            let (path, json) =
                agent::parse_path_json_args(args, "usage: ragelab plan <operation.json> [--json]")?;
            agent::plan_operation_file(&path, json)?;
        }
        "apply" => {
            let (path, json) = agent::parse_path_json_args(
                args,
                "usage: ragelab apply <operation.json> [--json]",
            )?;
            agent::apply_operation_file(&path, json)?;
        }
        "hash" => {
            let name = required_arg(args.next(), "usage: ragelab hash <name>")?;
            let hash = joaat(&name);
            println!("{name}\t0x{hash:08X}\t{hash}");
        }
        "meta-hash" => {
            let name = required_arg(args.next(), "usage: ragelab meta-hash <name>")?;
            let hash = jenkins(&name);
            println!("{name}\t0x{hash:08X}\t{hash}");
        }
        "probe" => {
            let path = required_arg(args.next(), "usage: ragelab probe <file>")?;
            probe(Path::new(&path))?;
        }
        "meta-info" => {
            let path = required_arg(args.next(), "usage: ragelab meta-info <file>")?;
            meta_info(Path::new(&path))?;
        }
        "ymap-info" => {
            let path = required_arg(args.next(), "usage: ragelab ymap-info <file.ymap>")?;
            ymap_info(Path::new(&path))?;
        }
        "ytyp-info" => {
            let path = required_arg(args.next(), "usage: ragelab ytyp-info <file.ytyp>")?;
            ytyp_info(Path::new(&path))?;
        }
        "ydr-info" => {
            let path = required_arg(args.next(), "usage: ragelab ydr-info <file.ydr>")?;
            ydr_info(Path::new(&path))?;
        }
        "ydd-info" => {
            let path = required_arg(
                args.next(),
                "usage: ragelab ydd-info <file.ydd> [drawable-index]",
            )?;
            let selector = args.next();
            ydd_info(Path::new(&path), selector.as_deref())?;
        }
        "ydr-rebind-texture" => {
            const USAGE: &str = "usage: ragelab ydr-rebind-texture <source.ydr> <source-shader> <source-param> <target-shader> <target-param> <output.ydr>";
            let source = required_arg(args.next(), USAGE)?;
            let source_shader = parse_non_negative_index(args.next(), "source-shader", USAGE)?;
            let source_param = parse_non_negative_index(args.next(), "source-param", USAGE)?;
            let target_shader = parse_non_negative_index(args.next(), "target-shader", USAGE)?;
            let target_param = parse_non_negative_index(args.next(), "target-param", USAGE)?;
            let output = required_arg(args.next(), USAGE)?;
            ydr_rebind_texture(
                Path::new(&source),
                TextureBindingKey {
                    shader_index: source_shader,
                    parameter_index: source_param,
                },
                TextureBindingKey {
                    shader_index: target_shader,
                    parameter_index: target_param,
                },
                Path::new(&output),
            )?;
        }
        "ydd-rebind-texture" => {
            const USAGE: &str = "usage: ragelab ydd-rebind-texture <source.ydd> <drawable-index> <source-shader> <source-param> <target-shader> <target-param> <output.ydd>";
            let source = required_arg(args.next(), USAGE)?;
            let drawable_index = parse_non_negative_index(args.next(), "drawable-index", USAGE)?;
            let source_shader = parse_non_negative_index(args.next(), "source-shader", USAGE)?;
            let source_param = parse_non_negative_index(args.next(), "source-param", USAGE)?;
            let target_shader = parse_non_negative_index(args.next(), "target-shader", USAGE)?;
            let target_param = parse_non_negative_index(args.next(), "target-param", USAGE)?;
            let output = required_arg(args.next(), USAGE)?;
            ydd_rebind_texture(
                Path::new(&source),
                drawable_index,
                TextureBindingKey {
                    shader_index: source_shader,
                    parameter_index: source_param,
                },
                TextureBindingKey {
                    shader_index: target_shader,
                    parameter_index: target_param,
                },
                Path::new(&output),
            )?;
        }
        "ydr-rebind-shader" => {
            const USAGE: &str = "usage: ragelab ydr-rebind-shader <source.ydr> <model-index> <geometry-index> <target-shader-index> <output.ydr>";
            let source = required_arg(args.next(), USAGE)?;
            let model_index = parse_non_negative_index(args.next(), "model-index", USAGE)?;
            let geometry_index = parse_non_negative_index(args.next(), "geometry-index", USAGE)?;
            let target_shader_index = u16::try_from(parse_non_negative_index(
                args.next(),
                "target-shader-index",
                USAGE,
            )?)
            .map_err(|_| invalid_input("target-shader-index must fit in u16"))?;
            let output = required_arg(args.next(), USAGE)?;
            ydr_rebind_shader(
                Path::new(&source),
                ShaderBindingKey {
                    model_index,
                    geometry_index,
                },
                target_shader_index,
                Path::new(&output),
            )?;
        }
        "ydd-rebind-shader" => {
            const USAGE: &str = "usage: ragelab ydd-rebind-shader <source.ydd> <drawable-index> <model-index> <geometry-index> <target-shader-index> <output.ydd>";
            let source = required_arg(args.next(), USAGE)?;
            let drawable_index = parse_non_negative_index(args.next(), "drawable-index", USAGE)?;
            let model_index = parse_non_negative_index(args.next(), "model-index", USAGE)?;
            let geometry_index = parse_non_negative_index(args.next(), "geometry-index", USAGE)?;
            let target_shader_index = u16::try_from(parse_non_negative_index(
                args.next(),
                "target-shader-index",
                USAGE,
            )?)
            .map_err(|_| invalid_input("target-shader-index must fit in u16"))?;
            let output = required_arg(args.next(), USAGE)?;
            ydd_rebind_shader(
                Path::new(&source),
                drawable_index,
                ShaderBindingKey {
                    model_index,
                    geometry_index,
                },
                target_shader_index,
                Path::new(&output),
            )?;
        }
        "ydr-translate" => {
            const USAGE: &str =
                "usage: ragelab ydr-translate <source.ydr> <dx> <dy> <dz> <output.ydr>";
            let source = required_arg(args.next(), USAGE)?;
            let delta = [
                parse_finite_f32(args.next(), "dx", USAGE)?,
                parse_finite_f32(args.next(), "dy", USAGE)?,
                parse_finite_f32(args.next(), "dz", USAGE)?,
            ];
            if delta == [0.0, 0.0, 0.0] {
                return Err(invalid_input("translation delta must not be zero").into());
            }
            let output = required_arg(args.next(), USAGE)?;
            ydr_translate(Path::new(&source), delta, Path::new(&output))?;
        }
        "ydd-translate" => {
            const USAGE: &str = "usage: ragelab ydd-translate <source.ydd> <drawable-index> <dx> <dy> <dz> <output.ydd>";
            let source = required_arg(args.next(), USAGE)?;
            let drawable_index = parse_non_negative_index(args.next(), "drawable-index", USAGE)?;
            let delta = [
                parse_finite_f32(args.next(), "dx", USAGE)?,
                parse_finite_f32(args.next(), "dy", USAGE)?,
                parse_finite_f32(args.next(), "dz", USAGE)?,
            ];
            if delta == [0.0, 0.0, 0.0] {
                return Err(invalid_input("translation delta must not be zero").into());
            }
            let output = required_arg(args.next(), USAGE)?;
            ydd_translate(
                Path::new(&source),
                drawable_index,
                delta,
                Path::new(&output),
            )?;
        }
        "ybn-info" => {
            let path = required_arg(args.next(), "usage: ragelab ybn-info <file.ybn>")?;
            ybn_info(Path::new(&path))?;
        }
        "ymf-info" => {
            let path = required_arg(args.next(), "usage: ragelab ymf-info <_manifest.ymf>")?;
            ymf_info(Path::new(&path))?;
        }
        "ytd-info" => {
            let path = required_arg(args.next(), "usage: ragelab ytd-info <file.ytd>")?;
            ytd_info(Path::new(&path))?;
        }
        "ytd-dds" => {
            const USAGE: &str = "usage: ragelab ytd-dds <file.ytd> <texture-index> <output.dds>";
            let path = required_arg(args.next(), USAGE)?;
            let index = required_arg(args.next(), USAGE)?
                .parse::<usize>()
                .map_err(|_| invalid_input("texture-index must be a non-negative integer"))?;
            let output = required_arg(args.next(), USAGE)?;
            ytd_dds(Path::new(&path), index, Path::new(&output))?;
        }
        "ytd-png" => {
            const USAGE: &str = "usage: ragelab ytd-png <file.ytd> <texture-index> <output.png>";
            let path = required_arg(args.next(), USAGE)?;
            let index = required_arg(args.next(), USAGE)?
                .parse::<usize>()
                .map_err(|_| invalid_input("texture-index must be a non-negative integer"))?;
            let output = required_arg(args.next(), USAGE)?;
            ytd_png(Path::new(&path), index, Path::new(&output))?;
        }
        "ytd-replace-dds" => {
            const USAGE: &str = "usage: ragelab ytd-replace-dds <source.ytd> <texture-index> <replacement.dds> <output.ytd>";
            let source = required_arg(args.next(), USAGE)?;
            let index = required_arg(args.next(), USAGE)?
                .parse::<usize>()
                .map_err(|_| invalid_input("texture-index must be a non-negative integer"))?;
            let replacement = required_arg(args.next(), USAGE)?;
            let output = required_arg(args.next(), USAGE)?;
            ytd_replace_dds(
                Path::new(&source),
                index,
                Path::new(&replacement),
                Path::new(&output),
            )?;
        }
        "ytd-repack-dds" => {
            const USAGE: &str = "usage: ragelab ytd-repack-dds <source.ytd> <texture-index> <replacement.dds> <output.ytd>";
            let source = required_arg(args.next(), USAGE)?;
            let index = required_arg(args.next(), USAGE)?
                .parse::<usize>()
                .map_err(|_| invalid_input("texture-index must be a non-negative integer"))?;
            let replacement = required_arg(args.next(), USAGE)?;
            let output = required_arg(args.next(), USAGE)?;
            ytd_repack_dds(
                Path::new(&source),
                index,
                Path::new(&replacement),
                Path::new(&output),
            )?;
        }
        "ytd-repack-png" => {
            const USAGE: &str = "usage: ragelab ytd-repack-png <source.ytd> <texture-index> <replacement.png> <output.ytd>";
            let source = required_arg(args.next(), USAGE)?;
            let index = required_arg(args.next(), USAGE)?
                .parse::<usize>()
                .map_err(|_| invalid_input("texture-index must be a non-negative integer"))?;
            let replacement = required_arg(args.next(), USAGE)?;
            let output = required_arg(args.next(), USAGE)?;
            ytd_repack_png(
                Path::new(&source),
                index,
                Path::new(&replacement),
                Path::new(&output),
            )?;
        }
        "ytd-repack-rgba" => {
            const USAGE: &str = "usage: ragelab ytd-repack-rgba <source.ytd> <texture-index> <width> <height> <replacement.rgba> <output.ytd>";
            let source = required_arg(args.next(), USAGE)?;
            let index = required_arg(args.next(), USAGE)?
                .parse::<usize>()
                .map_err(|_| invalid_input("texture-index must be a non-negative integer"))?;
            let width = required_arg(args.next(), USAGE)?
                .parse::<u16>()
                .map_err(|_| invalid_input("width must be a positive integer that fits u16"))?;
            let height = required_arg(args.next(), USAGE)?
                .parse::<u16>()
                .map_err(|_| invalid_input("height must be a positive integer that fits u16"))?;
            let replacement = required_arg(args.next(), USAGE)?;
            let output = required_arg(args.next(), USAGE)?;
            ytd_repack_rgba(
                Path::new(&source),
                index,
                width,
                height,
                Path::new(&replacement),
                Path::new(&output),
            )?;
        }
        "ytd-rebuild-compact" => {
            const USAGE: &str = "usage: ragelab ytd-rebuild-compact <source.ytd> <output.ytd>";
            let source = required_arg(args.next(), USAGE)?;
            let output = required_arg(args.next(), USAGE)?;
            ytd_rebuild_compact(Path::new(&source), Path::new(&output))?;
        }
        "extract" => {
            const USAGE: &str = "usage: ragelab extract <directory> <file.ymap> <output> \
                [--allow-unresolved] [--overwrite]";
            let root = required_arg(args.next(), USAGE)?;
            let ymap = required_arg(args.next(), USAGE)?;
            let output = required_arg(args.next(), USAGE)?;
            let mut options = SharedExportOptions::default();
            for flag in args {
                match flag.as_str() {
                    "--allow-unresolved" => options.allow_unresolved = true,
                    "--overwrite" => options.overwrite = true,
                    other => {
                        return Err(invalid_input(format!("unknown extract option: {other}")).into())
                    }
                }
            }
            extract(
                Path::new(&root),
                Path::new(&ymap),
                Path::new(&output),
                options,
            )?;
        }
        "preflight" => {
            const USAGE: &str = "usage: ragelab workspace preflight <directory> <file.ymap> [more.ymap ...] [--durtyfree-object-list <ObjectList.ini>] [--vanilla-file-index <paths.txt>] [--json]";
            let root = required_arg(args.next(), USAGE)?;
            let (maps, catalogs, json) = parse_preflight_args(args, USAGE)?;
            preflight(Path::new(&root), &maps, catalogs, json)?;
        }
        "export" => {
            const USAGE: &str = "usage: ragelab workspace export <directory> <file.ymap> [more.ymap ...] --output <directory> [--resource-name <name>] [--allow-unresolved] [--overwrite] [--durtyfree-object-list <ObjectList.ini>] [--vanilla-file-index <paths.txt>] [--json]";
            let root = required_arg(args.next(), USAGE)?;
            let request = parse_export_args(args, USAGE)?;
            workspace_export(Path::new(&root), request)?;
        }
        "mlo-audit" => {
            const USAGE: &str = "usage: ragelab mlo-audit <directory> <file.ymap> [--durtyfree-object-list <ObjectList.ini>] [--vanilla-file-index <paths.txt>]";
            let root = required_arg(args.next(), USAGE)?;
            let ymap = required_arg(args.next(), USAGE)?;
            let catalogs = parse_catalog_args(args, USAGE)?;
            mlo_audit(Path::new(&root), Path::new(&ymap), catalogs)?;
        }
        "deps" => {
            let root = required_arg(args.next(), "usage: ragelab deps <directory> <file.ymap>")?;
            let ymap = required_arg(args.next(), "usage: ragelab deps <directory> <file.ymap>")?;
            deps(Path::new(&root), Path::new(&ymap))?;
        }
        "providers" => {
            let root = required_arg(
                args.next(),
                "usage: ragelab providers <directory> <file.ymap>",
            )?;
            let ymap = required_arg(
                args.next(),
                "usage: ragelab providers <directory> <file.ymap>",
            )?;
            providers(Path::new(&root), Path::new(&ymap))?;
        }
        "scan" => {
            let path = required_arg(args.next(), "usage: ragelab scan <directory>")?;
            scan(Path::new(&path))?;
        }
        "gta.discover" => {
            const USAGE: &str = "usage: ragelab gta discover [--json]";
            let json = parse_json_flag(args, USAGE)?;
            agent::gta_discover(json)?;
        }
        "fivem.discover" => {
            const USAGE: &str = "usage: ragelab fivem discover [--json]";
            let json = parse_json_flag(args, USAGE)?;
            agent::fivem_discover(json)?;
        }
        "gta.catalog" => {
            const USAGE: &str =
                "usage: ragelab gta catalog <directory> --output <paths.txt> [--overwrite] [--json]";
            let (root, output, overwrite, json) = agent::parse_gta_catalog_args(args, USAGE)?;
            agent::gta_catalog(&root, &output, overwrite, json)?;
        }
        "gta.rpf-order" => {
            const USAGE: &str =
                "usage: ragelab gta rpf-order <game-root> --keys <directory> [--json]";
            let (root, keys, json) = agent::parse_gta_rpf_order_args(args, USAGE)?;
            agent::gta_rpf_order(&root, &keys, json)?;
        }
        "gta.rpf-index" => {
            const USAGE: &str =
                "usage: ragelab gta rpf-index <game-root> --keys <directory> --output <file> [--overwrite] [--json]";
            let request = agent::parse_gta_rpf_index_args(args, USAGE)?;
            agent::gta_rpf_index(request)?;
        }
        "gta.world-query" => {
            const USAGE: &str =
                "usage: ragelab gta world-query <index> (--point <x> <y> <z> --radius <r> | --box <minx> <miny> <minz> <maxx> <maxy> <maxz>) [--entities] [--repeat <n>] [--json]";
            let request = agent::parse_gta_world_query_args(args, USAGE)?;
            agent::gta_world_query(request)?;
        }
        "gta.mlo-validate" => {
            const USAGE: &str =
                "usage: ragelab gta mlo-validate <index> --game-root <directory> --keys <directory> [--hash <u32|0xHEX>] [--json]";
            let request = agent::parse_gta_mlo_validate_args(args, USAGE)?;
            agent::gta_mlo_validate(request)?;
        }
        "bridge.serve" => {
            bridge_cmd::serve(args)?;
        }
        "rpf.keys" => {
            const USAGE: &str =
                "usage: ragelab rpf keys <GTA5.exe> --cache-root <directory> [--json]";
            let (exe, cache_root, json) = rpf_cmd::parse_key_options(args, USAGE)?;
            rpf_cmd::keys(&exe, &cache_root, json)?;
        }
        "rpf.info" => {
            const USAGE: &str =
                "usage: ragelab rpf info <archive.rpf> [--keys <directory>] [--nested <entry.rpf>]... [--json]";
            let (archive, options) = rpf_cmd::parse_options(args, USAGE)?;
            rpf_cmd::info(&archive, &options)?;
        }
        "rpf.list" => {
            const USAGE: &str =
                "usage: ragelab rpf list <archive.rpf> [--keys <directory>] [--nested <entry.rpf>]... [--contains <text>] [--json]";
            let (archive, options) = rpf_cmd::parse_options(args, USAGE)?;
            rpf_cmd::list(&archive, &options)?;
        }
        "rpf.extract" => {
            const USAGE: &str =
                "usage: ragelab rpf extract <archive.rpf> [--keys <directory>] [--nested <entry.rpf>]... --entry <path> --output <file> [--overwrite] [--json]";
            let (archive, options) = rpf_cmd::parse_options(args, USAGE)?;
            rpf_cmd::extract(&archive, &options)?;
        }
        "vanilla-index" => {
            const USAGE: &str =
                "usage: ragelab vanilla-index <extracted-gta-directory> <output.txt>";
            let root = required_arg(args.next(), USAGE)?;
            let output = required_arg(args.next(), USAGE)?;
            vanilla_index(Path::new(&root), Path::new(&output))?;
        }
        "help" | "--help" | "-h" => print_help(),
        other => return Err(invalid_input(format!("unknown command: {other}")).into()),
    }

    Ok(())
}

fn required_arg(value: Option<String>, usage: &str) -> Result<String, io::Error> {
    value.ok_or_else(|| invalid_input(usage))
}

fn parse_non_negative_index(
    value: Option<String>,
    label: &str,
    usage: &str,
) -> Result<usize, io::Error> {
    required_arg(value, usage)?
        .parse::<usize>()
        .map_err(|_| invalid_input(format!("{label} must be a non-negative integer")))
}

fn parse_finite_f32(value: Option<String>, label: &str, usage: &str) -> Result<f32, io::Error> {
    let parsed = required_arg(value, usage)?
        .parse::<f32>()
        .map_err(|_| invalid_input(format!("{label} must be a finite number")))?;
    if !parsed.is_finite() {
        return Err(invalid_input(format!("{label} must be a finite number")));
    }
    Ok(parsed)
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn parse_json_flag(args: impl Iterator<Item = String>, usage: &str) -> Result<bool, io::Error> {
    let mut json_output = false;
    for arg in args {
        match arg.as_str() {
            "--json" if !json_output => json_output = true,
            _ => return Err(invalid_input(format!("{usage}; unknown option: {arg}"))),
        }
    }
    Ok(json_output)
}

fn print_version(json_output: bool) -> Result<(), Box<dyn Error>> {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "schema": "ragelab.cli.version",
                "schemaVersion": 1,
                "product": "RageLab",
                "version": env!("CARGO_PKG_VERSION"),
            }))?
        );
    } else {
        println!("ragelab {}", env!("CARGO_PKG_VERSION"));
    }
    Ok(())
}

fn print_capabilities(json_output: bool) -> Result<(), Box<dyn Error>> {
    const CANONICAL_COMMANDS: &[&str] = &[
        "version",
        "capabilities",
        "inspect",
        "validate",
        "spatial",
        "preview",
        "plan",
        "apply",
        "hash",
        "meta-hash",
        "probe",
        "ydr.info",
        "ydr.translate",
        "ydr.rebind-texture",
        "ydr.rebind-shader",
        "ydd.info",
        "ydd.translate",
        "ydd.rebind-texture",
        "ydd.rebind-shader",
        "ytd.info",
        "ytd.extract-dds",
        "ytd.extract-png",
        "ytd.replace-dds",
        "ytd.repack-dds",
        "ytd.repack-png",
        "ytd.repack-rgba",
        "ytd.rebuild-compact",
        "ybn.info",
        "ymap.info",
        "ytyp.info",
        "ymf.info",
        "workspace.scan",
        "workspace.deps",
        "workspace.providers",
        "workspace.preflight",
        "workspace.mlo-audit",
        "workspace.extract",
        "workspace.export",
        "workspace.scene",
        "workspace.render-package",
        "render.asset",
        "render.scene",
        "render.compare",
        "gta.discover",
        "gta.catalog",
        "gta.rpf-order",
        "gta.rpf-index",
        "gta.world-query",
        "gta.mlo-validate",
        "gta.vanilla-index",
        "bridge.serve",
        "rpf.keys",
        "rpf.info",
        "rpf.list",
        "rpf.extract",
        "fivem.discover",
    ];
    const LEGACY_ALIASES: &[&str] = &[
        "meta-info",
        "ymap-info",
        "ytyp-info",
        "ydr-info",
        "ydd-info",
        "ydr-rebind-texture",
        "ydd-rebind-texture",
        "ydr-rebind-shader",
        "ydd-rebind-shader",
        "ydr-translate",
        "ydd-translate",
        "ybn-info",
        "ymf-info",
        "ytd-info",
        "ytd-dds",
        "ytd-png",
        "ytd-replace-dds",
        "ytd-repack-dds",
        "ytd-repack-png",
        "ytd-repack-rgba",
        "ytd-rebuild-compact",
        "extract",
        "preflight",
        "mlo-audit",
        "deps",
        "providers",
        "scan",
        "vanilla-index",
    ];
    const DISCOVERY_COMMANDS: &[&str] = &[
        "version",
        "capabilities",
        "inspect",
        "validate",
        "spatial",
        "preview",
        "plan",
        "apply",
        "hash",
        "meta-hash",
        "probe",
        "meta-info",
        "ymap-info",
        "ytyp-info",
        "ydr-info",
        "ydd-info",
        "ydr-rebind-texture",
        "ydd-rebind-texture",
        "ydr-rebind-shader",
        "ydd-rebind-shader",
        "ydr-translate",
        "ydd-translate",
        "ybn-info",
        "ymf-info",
        "ytd-info",
        "ytd-dds",
        "ytd-png",
        "ytd-replace-dds",
        "ytd-repack-dds",
        "ytd-repack-png",
        "ytd-repack-rgba",
        "ytd-rebuild-compact",
        "extract",
        "preflight",
        "mlo-audit",
        "deps",
        "providers",
        "scan",
        "vanilla-index",
        "ydr.info",
        "ydr.translate",
        "ydr.rebind-texture",
        "ydr.rebind-shader",
        "ydd.info",
        "ydd.translate",
        "ydd.rebind-texture",
        "ydd.rebind-shader",
        "ytd.info",
        "ytd.extract-dds",
        "ytd.extract-png",
        "ytd.replace-dds",
        "ytd.repack-dds",
        "ytd.repack-png",
        "ytd.repack-rgba",
        "ytd.rebuild-compact",
        "ybn.info",
        "ymap.info",
        "ytyp.info",
        "ymf.info",
        "workspace.scan",
        "workspace.deps",
        "workspace.providers",
        "workspace.preflight",
        "workspace.mlo-audit",
        "workspace.extract",
        "workspace.export",
        "workspace.scene",
        "workspace.render-package",
        "render.asset",
        "render.scene",
        "render.compare",
        "gta.discover",
        "gta.catalog",
        "gta.rpf-order",
        "gta.rpf-index",
        "gta.world-query",
        "gta.mlo-validate",
        "gta.vanilla-index",
        "bridge.serve",
        "rpf.keys",
        "rpf.info",
        "rpf.list",
        "rpf.extract",
        "fivem.discover",
    ];

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "schema": "ragelab.cli.capabilities",
                "schemaVersion": 1,
                "product": "RageLab",
                "version": env!("CARGO_PKG_VERSION"),
                "commands": DISCOVERY_COMMANDS,
                "canonicalCommands": CANONICAL_COMMANDS,
                "structuredOutput": ["version", "capabilities", "inspect", "validate", "spatial", "preview", "plan", "apply", "workspace.preflight", "workspace.export", "workspace.scene", "workspace.render-package", "render.asset", "render.scene", "render.compare", "gta.discover", "gta.catalog", "gta.rpf-order", "gta.rpf-index", "gta.world-query", "gta.mlo-validate", "rpf.keys", "rpf.info", "rpf.list", "rpf.extract", "fivem.discover"],
                "legacyAliases": LEGACY_ALIASES,
                "responseEnvelope": {
                    "schema": agent::RESPONSE_SCHEMA,
                    "schemaVersion": agent::RESPONSE_SCHEMA_VERSION,
                }
            }))?
        );
    } else {
        println!("RageLab {} capabilities:", env!("CARGO_PKG_VERSION"));
        for command in CANONICAL_COMMANDS {
            println!("{command}");
        }
        println!("\nLegacy flat command aliases remain available during the 0.x series.");
    }
    Ok(())
}

fn print_help() {
    println!(
        "ragelab {}\n\n\
Agent-first commands:\n  \
ragelab inspect <file> [--json]\n  \
ragelab capabilities [file] [--json]\n  \
ragelab validate <file> [--json]\n  \
ragelab spatial <file> [--json]\n  \
ragelab preview <file> [--drawable-index <n>] [--max-primitives <n>] [--max-vertices <n>] [--max-indices <n>] [--max-shaders <n>] [--max-texture-references <n>] [--max-children <n>] [--max-materials <n>] [--json]\n  \
ragelab plan <operation.json> [--json]\n  \
ragelab apply <operation.json> [--json]\n  \
ragelab version [--json]\n\n\
Format commands:\n  \
ragelab ydr info <file.ydr>\n  \
ragelab ydr translate <source.ydr> <dx> <dy> <dz> <output.ydr>\n  \
ragelab ydr rebind-texture <source.ydr> <source-shader> <source-param> <target-shader> <target-param> <output.ydr>\n  \
ragelab ydr rebind-shader <source.ydr> <model-index> <geometry-index> <target-shader-index> <output.ydr>\n  \
ragelab ydd info <file.ydd> [drawable-index]\n  \
ragelab ydd translate <source.ydd> <drawable-index> <dx> <dy> <dz> <output.ydd>\n  \
ragelab ydd rebind-texture <source.ydd> <drawable-index> <source-shader> <source-param> <target-shader> <target-param> <output.ydd>\n  \
ragelab ydd rebind-shader <source.ydd> <drawable-index> <model-index> <geometry-index> <target-shader-index> <output.ydd>\n  \
ragelab ytd info <file.ytd>\n  \
ragelab ytd extract-dds <file.ytd> <texture-index> <output.dds>\n  \
ragelab ytd extract-png <file.ytd> <texture-index> <output.png>\n  \
ragelab ytd replace-dds <source.ytd> <texture-index> <replacement.dds> <output.ytd>\n  \
ragelab ytd repack-dds <source.ytd> <texture-index> <replacement.dds> <output.ytd>\n  \
ragelab ytd repack-png <source.ytd> <texture-index> <replacement.png> <output.ytd>\n  \
ragelab ytd repack-rgba <source.ytd> <texture-index> <width> <height> <replacement.rgba> <output.ytd>\n  \
ragelab ytd rebuild-compact <source.ytd> <output.ytd>\n  \
ragelab ybn info <file.ybn>\n  \
ragelab ymap info <file.ymap>\n  \
ragelab ytyp info <file.ytyp>\n  \
ragelab ymf info <_manifest.ymf>\n\n\
Workspace commands:\n  \
ragelab workspace scan <directory>\n  \
ragelab workspace deps <directory> <file.ymap>\n  \
ragelab workspace providers <directory> <file.ymap>\n  \
ragelab workspace preflight <directory> <file.ymap> [more.ymap ...] [catalog options] [--json]\n  \
ragelab workspace mlo-audit <directory> <file.ymap> [catalog options]\n  \
ragelab workspace extract <directory> <file.ymap> <output> [--allow-unresolved] [--overwrite]\n  \
ragelab workspace export <directory> <file.ymap> [more.ymap ...] --output <directory> [--resource-name <name>] [--allow-unresolved] [--overwrite] [catalog options] [--json]\n  \
ragelab workspace scene <directory> <file.ymap> [--fallback-root <directory>]... [--rpf-mount <archive.rpf> [--rpf-nested <entry.rpf>]...]... [--game-root <directory> --game-index <file>] [--rpf-keys <directory>] [--max-nodes <n>] [--json]\n  \
ragelab workspace render-package <directory> <file.ymap> --output <file> [scene source options] [--max-nodes <n>] [--max-assets <n>] [--max-blob-bytes <n>] [--overwrite] [--json]\n\n\
Render commands:\n  \
ragelab render asset <file.ydr|file.ydd|file.yft> --output <file.png> [--drawable-index <n>] [--width <n>] [--height <n>] [--view auto|front|back|left|right|top|isometric] [--projection perspective|orthographic] [--transparent] [--grid] [--wireframe] [--bounds] [--metadata <file.json>] [--overwrite] [--json]\n  \
ragelab render scene <directory> <file.ymap> --output <file.png> [scene source options] [--max-nodes <n>] [--max-assets <n>] [--max-blob-bytes <n>] [render options] [--overwrite] [--json]\n  \
ragelab render compare <expected.png> <actual.png> [--tolerance <0..255>] [--json]\n\n\
ragelab gta discover [--json]\n  \
ragelab fivem discover [--json]\n  \
ragelab gta catalog <directory> --output <paths.txt> [--overwrite] [--json]\n  \
ragelab gta rpf-order <game-root> --keys <directory> [--json]\n  \
ragelab gta rpf-index <game-root> --keys <directory> --output <file> [--overwrite] [--json]\n  \
ragelab gta world-query <index> (--point <x> <y> <z> --radius <r> | --box <minx> <miny> <minz> <maxx> <maxy> <maxz>) [--entities] [--repeat <n>] [--json]\n  \
ragelab gta mlo-validate <index> --game-root <directory> --keys <directory> [--hash <u32|0xHEX>] [--json]\n  \
ragelab gta vanilla-index <extracted-gta-directory> <output.txt>\n\n\
Optional local Web bridge:\n  \
ragelab bridge serve --game-root <directory> --index <legacy-v5.bin> --keys <directory> --origin <http(s)://host[:port]> [--origin <...>]... [--port <n>] [--max-asset-bytes <n>] [--max-bundle-assets <n>] [--max-bundle-bytes <n>] [--audit-log <file>]\n\n\
RPF commands:\n  \
ragelab rpf keys <GTA5.exe> --cache-root <directory> [--json]\n  \
ragelab rpf info <archive.rpf> [--keys <directory>] [--nested <entry.rpf>]... [--json]\n  \
ragelab rpf list <archive.rpf> [--keys <directory>] [--nested <entry.rpf>]... [--contains <text>] [--json]\n  \
ragelab rpf extract <archive.rpf> [--keys <directory>] [--nested <entry.rpf>]... --entry <path> --output <file> [--overwrite] [--json]\n\n\
Utility commands:\n  \
ragelab hash <asset-name>\n  \
ragelab meta-hash <case-sensitive-name>\n  \
ragelab probe <file>\n  \
ragelab meta-info <file>\n\n\
Legacy flat aliases remain available during the 0.x series.",
        env!("CARGO_PKG_VERSION")
    );
}

fn probe(path: &Path) -> Result<(), Box<dyn Error>> {
    let data = fs::read(path)?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    println!("file: {}", path.display());
    println!("size: {} bytes", data.len());
    println!(
        "type: {}",
        classify_extension(&extension).unwrap_or("unknown")
    );

    match Rsc7Probe::parse(&data) {
        Ok(probe) => {
            println!("container: RSC7");
            println!("version: {}", probe.header.version);
            println!("system-flags: 0x{:08X}", probe.header.system_flags);
            println!("graphics-flags: 0x{:08X}", probe.header.graphics_flags);
            println!("system-size: {} bytes", probe.header.system_size());
            println!("graphics-size: {} bytes", probe.header.graphics_size());

            match Rsc7Resource::parse(&data) {
                Ok(resource) => println!(
                    "decompression: ok ({} system + {} graphics)",
                    resource.system().len(),
                    resource.graphics().len()
                ),
                Err(error) => println!("decompression: failed ({error})"),
            }
        }
        Err(error) => println!("container: not recognized as RSC7 ({error})"),
    }

    Ok(())
}

fn meta_info(path: &Path) -> Result<(), Box<dyn Error>> {
    let data = fs::read(path)?;
    let meta = MetaDocument::from_rsc7(&data)?;
    let root = meta.root_block()?;

    println!("file: {}", path.display());
    println!("resource-version: {}", meta.resource_header.version);
    println!("meta-name: {}", meta.name.as_deref().unwrap_or("<none>"));
    println!("root-block: {}", meta.header.root_block_index);
    println!("root-type: 0x{:08X}", root.structure_name.0);
    println!("structures: {}", meta.structures.len());
    println!("enums: {}", meta.enums.len());
    println!("data-blocks: {}", meta.blocks.len());
    println!("root-bytes: {}", root.data.len());

    Ok(())
}

fn ymap_info(path: &Path) -> Result<(), Box<dyn Error>> {
    let data = fs::read(path)?;
    let ymap = Ymap::from_bytes(&data)?;

    println!("file: {}", path.display());
    println!("name: {}", format_hash(ymap.name.map(|hash| hash.0)));
    println!("parent: {}", format_hash(ymap.parent.map(|hash| hash.0)));
    println!("entities: {}", ymap.entities.len());
    println!("physics-dictionaries: {}", ymap.physics_dictionaries.len());

    let archetypes: BTreeSet<u32> = ymap
        .entities
        .iter()
        .map(|entity| entity.archetype_name.0)
        .collect();
    println!("unique-archetypes: {}", archetypes.len());

    if !ymap.physics_dictionaries.is_empty() {
        println!("\nphysics dictionaries:");
        for hash in &ymap.physics_dictionaries {
            println!("  0x{:08X}", hash.0);
        }
    }

    if !archetypes.is_empty() {
        println!("\narchetypes:");
        for hash in archetypes {
            println!("  0x{hash:08X}");
        }
    }

    Ok(())
}

fn ybn_info(path: &Path) -> Result<(), Box<dyn Error>> {
    let data = fs::read(path)?;
    let collision = YbnCollision::from_bytes(&data)?;

    println!("file: {}", path.display());
    println!("coordinates: {}", collision.coordinate_convention);
    println!("bounds-center: {:?}", collision.bounds.center);
    println!("bounds-radius: {}", collision.bounds.sphere_radius);
    println!("bounds-min: {:?}", collision.bounds.min);
    println!("bounds-max: {:?}", collision.bounds.max);
    println!("children: {}", collision.children.len());
    println!("vertices: {}", collision.vertex_count());
    println!("indices: {}", collision.index_count());
    println!("triangles: {}", collision.triangle_count());
    println!("materials: {}", collision.materials.len());
    println!("primitives: {}", collision.primitives.len());
    for child in &collision.children {
        println!(
            "  child {}: type={} vertices={} triangles={}",
            child.index,
            child.bounds_type.as_str(),
            child.vertices,
            child.triangles
        );
    }
    for material in collision.materials.iter().take(32) {
        println!(
            "  material {}: child={} local={} type={} procedural={} flags=0x{:04X} color={}",
            material.index,
            material.child_index,
            material.local_index,
            material.material_type,
            material.procedural_id,
            material.flags,
            material.color_index
        );
    }
    if collision.materials.len() > 32 {
        println!("  ... {} more materials", collision.materials.len() - 32);
    }
    Ok(())
}

fn ydd_info(path: &Path, selector: Option<&str>) -> Result<(), Box<dyn Error>> {
    let data = fs::read(path)?;
    let dictionary = YddDictionary::from_bytes(&data)?;

    println!("file: {}", path.display());
    println!("drawables: {}", dictionary.entries().len());
    for entry in dictionary.entries().iter().take(64) {
        println!(
            "  [{:3}] 0x{:08X} {}",
            entry.index,
            entry.name_hash,
            entry.name.as_deref().unwrap_or("<unnamed>")
        );
    }
    if dictionary.entries().len() > 64 {
        println!("  ... {} more drawables", dictionary.entries().len() - 64);
    }

    if let Some(selector) = selector {
        let index = selector.parse::<usize>().map_err(|_| {
            invalid_input(format!(
                "YDD drawable selector must be an integer index: {selector}"
            ))
        })?;
        let document = dictionary.document(index)?;
        let model = &document.model;
        println!("selected-index: {index}");
        println!(
            "selected-name: {}",
            model.name.as_deref().unwrap_or("<none>")
        );
        println!("selected-lod: {}", model.lod.as_str());
        println!("selected-primitives: {}", model.primitives.len());
        println!("selected-vertices: {}", model.vertex_count());
        println!("selected-indices: {}", model.index_count());
        println!("selected-triangles: {}", model.triangle_count());
        println!("selected-shaders: {}", model.shaders.len());
        println!(
            "selected-embedded-textures: {}",
            document
                .embedded_textures
                .as_ref()
                .map_or(0, |textures| textures.textures().len())
        );
    }

    Ok(())
}

fn ydr_info(path: &Path) -> Result<(), Box<dyn Error>> {
    let data = fs::read(path)?;
    let document = YdrDocument::from_bytes(&data)?;
    let model = &document.model;

    println!("file: {}", path.display());
    println!("name: {}", model.name.as_deref().unwrap_or("<none>"));
    println!("lod: {}", model.lod.as_str());
    println!("coordinates: {}", model.coordinate_convention.as_str());
    println!("bounds-center: {:?}", model.bounds.center);
    println!("bounds-radius: {}", model.bounds.radius);
    println!("bounds-min: {:?}", model.bounds.min);
    println!("bounds-max: {:?}", model.bounds.max);
    println!("primitives: {}", model.primitives.len());
    println!("vertices: {}", model.vertex_count());
    println!("indices: {}", model.index_count());
    println!("triangles: {}", model.triangle_count());
    println!("shaders: {}", model.shaders.len());
    println!(
        "embedded-textures: {}",
        document
            .embedded_textures
            .as_ref()
            .map_or(0, |dictionary| dictionary.textures().len())
    );
    if let Some(dictionary) = &document.embedded_textures {
        for texture in dictionary.textures().iter().take(16) {
            println!(
                "  embedded [{}] {}x{} {} {}",
                texture.index,
                texture.width,
                texture.height,
                texture.format.as_str(),
                texture.name.as_deref().unwrap_or("<none>")
            );
        }
        if dictionary.textures().len() > 16 {
            println!(
                "  ... {} more embedded textures",
                dictionary.textures().len() - 16
            );
        }
    }

    for (shader_index, shader) in model.shaders.iter().enumerate() {
        println!(
            "  shader {shader_index}: name=0x{:08X} file=0x{:08X} textures={}",
            shader.name_hash,
            shader.file_hash,
            shader.texture_references.len()
        );
        for texture in &shader.texture_references {
            println!(
                "    param=0x{:08X} texture={}",
                texture.parameter_hash,
                texture.texture_name.as_deref().unwrap_or("<none>")
            );
        }
    }

    for primitive in &model.primitives {
        println!(
            "  model {} geometry {}: shader={:?} topology={} vertices={} indices={} triangles={} stride={} flags=0x{:08X} types=0x{:016X} normals={} uv0={}",
            primitive.model_index,
            primitive.geometry_index,
            primitive.shader_index,
            primitive.topology.as_str(),
            primitive.positions.len(),
            primitive.indices.len(),
            primitive.triangle_count(),
            primitive.declaration.stride,
            primitive.declaration.flags,
            primitive.declaration.types,
            primitive.normals.is_some(),
            primitive.uv0.is_some(),
        );
        if let Some(winding) = primitive.winding_summary() {
            println!(
                "    winding: aligned={} opposed={} degenerate={}",
                winding.aligned, winding.opposed, winding.degenerate
            );
        }
    }

    Ok(())
}

fn validate_new_drawable_output(
    source: &Path,
    output: &Path,
    expected_extension: &str,
) -> Result<(), io::Error> {
    let source_extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if !source_extension.eq_ignore_ascii_case(expected_extension) {
        return Err(invalid_input(format!(
            "source must be a .{expected_extension} file"
        )));
    }
    let output_extension = output
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if !output_extension.eq_ignore_ascii_case(expected_extension) {
        return Err(invalid_input(format!(
            "output must be a .{expected_extension} file"
        )));
    }

    let source_canonical = fs::canonicalize(source)?;
    if output.exists() {
        let output_canonical = fs::canonicalize(output)?;
        if output_canonical == source_canonical {
            return Err(invalid_input(format!(
                "refusing in-place .{expected_extension} edit; output must differ from source"
            )));
        }
        return Err(invalid_input(format!(
            "output already exists: {}; remove it or choose a new path",
            output.display()
        )));
    }

    let output_parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let output_name = output
        .file_name()
        .ok_or_else(|| invalid_input("output path must name a file"))?;
    if output_parent.exists()
        && fs::canonicalize(output_parent)?.join(output_name) == source_canonical
    {
        return Err(invalid_input(format!(
            "refusing in-place .{expected_extension} edit; output must differ from source"
        )));
    }
    Ok(())
}

fn ydr_rebind_texture(
    source: &Path,
    source_binding: TextureBindingKey,
    target_binding: TextureBindingKey,
    output: &Path,
) -> Result<(), Box<dyn Error>> {
    validate_new_drawable_output(source, output, "ydr")?;
    let bytes = fs::read(source)?;
    let mut session = YdrEditSession::from_bytes(&bytes)?;
    let target = session
        .texture_bindings()
        .iter()
        .find(|binding| binding.key == target_binding)
        .cloned()
        .ok_or_else(|| invalid_input("target texture binding was not found"))?;
    session.rebind_texture(source_binding, target_binding)?;
    let rewritten = session.to_bytes()?;
    let reopened = YdrEditSession::from_bytes(&rewritten)?;
    let rebound = reopened
        .texture_bindings()
        .iter()
        .find(|binding| binding.key == source_binding)
        .ok_or_else(|| io::Error::other("edited YDR lost the source texture binding"))?;
    if rebound.parameter_hash != target.parameter_hash
        || rebound.texture_name != target.texture_name
    {
        return Err(
            io::Error::other("edited YDR failed semantic verification after re-open").into(),
        );
    }

    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, &rewritten)?;
    println!("source: {}", source.display());
    println!(
        "source-binding: shader={} parameter={}",
        source_binding.shader_index, source_binding.parameter_index
    );
    println!(
        "target-binding: shader={} parameter={}",
        target_binding.shader_index, target_binding.parameter_index
    );
    println!("parameter-hash: 0x{:08X}", target.parameter_hash);
    println!(
        "texture: {}",
        target.texture_name.as_deref().unwrap_or("<none>")
    );
    println!("output: {}", output.display());
    println!("semantic-reopen: ok");
    Ok(())
}

fn ydd_rebind_texture(
    source: &Path,
    drawable_index: usize,
    source_binding: TextureBindingKey,
    target_binding: TextureBindingKey,
    output: &Path,
) -> Result<(), Box<dyn Error>> {
    validate_new_drawable_output(source, output, "ydd")?;
    let bytes = fs::read(source)?;
    let original_dictionary = YddDictionary::from_bytes(&bytes)?;
    let mut session = YddEditSession::from_bytes(&bytes, drawable_index)?;
    let target = session
        .texture_bindings()
        .iter()
        .find(|binding| binding.key == target_binding)
        .cloned()
        .ok_or_else(|| invalid_input("target texture binding was not found"))?;
    session.rebind_texture(source_binding, target_binding)?;
    let rewritten = session.to_bytes()?;
    let reopened_dictionary = YddDictionary::from_bytes(&rewritten)?;
    if reopened_dictionary.entries() != original_dictionary.entries() {
        return Err(io::Error::other(
            "edited YDD changed dictionary identity metadata during re-open",
        )
        .into());
    }
    let reopened = YddEditSession::from_bytes(&rewritten, drawable_index)?;
    let rebound = reopened
        .texture_bindings()
        .iter()
        .find(|binding| binding.key == source_binding)
        .ok_or_else(|| io::Error::other("edited YDD lost the source texture binding"))?;
    if rebound.parameter_hash != target.parameter_hash
        || rebound.texture_name != target.texture_name
    {
        return Err(
            io::Error::other("edited YDD failed semantic verification after re-open").into(),
        );
    }

    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, &rewritten)?;
    println!("source: {}", source.display());
    println!("drawable-index: {drawable_index}");
    println!(
        "source-binding: shader={} parameter={}",
        source_binding.shader_index, source_binding.parameter_index
    );
    println!(
        "target-binding: shader={} parameter={}",
        target_binding.shader_index, target_binding.parameter_index
    );
    println!("parameter-hash: 0x{:08X}", target.parameter_hash);
    println!(
        "texture: {}",
        target.texture_name.as_deref().unwrap_or("<none>")
    );
    println!("output: {}", output.display());
    println!("semantic-reopen: ok");
    Ok(())
}

fn ydr_rebind_shader(
    source: &Path,
    binding: ShaderBindingKey,
    target_shader_index: u16,
    output: &Path,
) -> Result<(), Box<dyn Error>> {
    validate_new_drawable_output(source, output, "ydr")?;
    let bytes = fs::read(source)?;
    let mut session = YdrEditSession::from_bytes(&bytes)?;
    let original = session
        .shader_bindings()
        .iter()
        .find(|candidate| candidate.key == binding)
        .copied()
        .ok_or_else(|| invalid_input("source shader binding was not found"))?;
    if original.shader_index == target_shader_index {
        return Err(invalid_input("source geometry already uses target shader").into());
    }
    session.rebind_shader(binding, target_shader_index)?;
    let rewritten = session.to_bytes()?;
    let reopened = YdrEditSession::from_bytes(&rewritten)?;
    let rebound = reopened
        .shader_bindings()
        .iter()
        .find(|candidate| candidate.key == binding)
        .ok_or_else(|| io::Error::other("edited YDR lost the source shader binding"))?;
    if rebound.shader_index != target_shader_index {
        return Err(io::Error::other(
            "edited YDR failed shader verification after semantic re-open",
        )
        .into());
    }

    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, &rewritten)?;
    println!("source: {}", source.display());
    println!(
        "shader-binding: model={} geometry={}",
        binding.model_index, binding.geometry_index
    );
    println!(
        "shader-index: {} -> {}",
        original.shader_index, target_shader_index
    );
    println!("shader-count: {}", reopened.shader_count());
    println!("output: {}", output.display());
    println!("semantic-reopen: ok");
    Ok(())
}

fn ydd_rebind_shader(
    source: &Path,
    drawable_index: usize,
    binding: ShaderBindingKey,
    target_shader_index: u16,
    output: &Path,
) -> Result<(), Box<dyn Error>> {
    validate_new_drawable_output(source, output, "ydd")?;
    let bytes = fs::read(source)?;
    let original_dictionary = YddDictionary::from_bytes(&bytes)?;
    let mut session = YddEditSession::from_bytes(&bytes, drawable_index)?;
    let original = session
        .shader_bindings()
        .iter()
        .find(|candidate| candidate.key == binding)
        .copied()
        .ok_or_else(|| invalid_input("source shader binding was not found"))?;
    if original.shader_index == target_shader_index {
        return Err(invalid_input("source geometry already uses target shader").into());
    }
    session.rebind_shader(binding, target_shader_index)?;
    let rewritten = session.to_bytes()?;
    let reopened_dictionary = YddDictionary::from_bytes(&rewritten)?;
    if reopened_dictionary.entries() != original_dictionary.entries() {
        return Err(io::Error::other(
            "edited YDD changed dictionary identity metadata during re-open",
        )
        .into());
    }
    let reopened = YddEditSession::from_bytes(&rewritten, drawable_index)?;
    let rebound = reopened
        .shader_bindings()
        .iter()
        .find(|candidate| candidate.key == binding)
        .ok_or_else(|| io::Error::other("edited YDD lost the source shader binding"))?;
    if rebound.shader_index != target_shader_index {
        return Err(io::Error::other(
            "edited YDD failed shader verification after semantic re-open",
        )
        .into());
    }

    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, &rewritten)?;
    println!("source: {}", source.display());
    println!("drawable-index: {drawable_index}");
    println!(
        "shader-binding: model={} geometry={}",
        binding.model_index, binding.geometry_index
    );
    println!(
        "shader-index: {} -> {}",
        original.shader_index, target_shader_index
    );
    println!("shader-count: {}", reopened.shader_count());
    println!("output: {}", output.display());
    println!("semantic-reopen: ok");
    Ok(())
}

fn ydr_translate(source: &Path, delta: [f32; 3], output: &Path) -> Result<(), Box<dyn Error>> {
    validate_new_drawable_output(source, output, "ydr")?;
    let bytes = fs::read(source)?;
    let mut session = YdrEditSession::from_bytes(&bytes)?;
    let capability = session.rigid_translation_capability();
    if !capability.writable {
        return Err(invalid_input(
            capability
                .reason
                .unwrap_or_else(|| "rigid translation is unavailable for this YDR".into()),
        )
        .into());
    }
    let before = session.document()?;
    session.translate_rigid_model(delta)?;
    let rewritten = session.to_bytes()?;
    let reopened = YdrEditSession::from_bytes(&rewritten)?;
    let after = reopened.document()?;
    verify_rigid_translation(&before, &after, delta)?;

    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, &rewritten)?;
    println!("source: {}", source.display());
    println!("delta: [{}, {}, {}]", delta[0], delta[1], delta[2]);
    println!("vertices: {}", after.model.vertex_count());
    println!(
        "bounds-before: {:?} -> {:?}",
        before.model.bounds.min, before.model.bounds.max
    );
    println!(
        "bounds-after: {:?} -> {:?}",
        after.model.bounds.min, after.model.bounds.max
    );
    println!("output: {}", output.display());
    println!("semantic-reopen: ok");
    Ok(())
}

fn ydd_translate(
    source: &Path,
    drawable_index: usize,
    delta: [f32; 3],
    output: &Path,
) -> Result<(), Box<dyn Error>> {
    validate_new_drawable_output(source, output, "ydd")?;
    let bytes = fs::read(source)?;
    let original_dictionary = YddDictionary::from_bytes(&bytes)?;
    let mut session = YddEditSession::from_bytes(&bytes, drawable_index)?;
    let capability = session.rigid_translation_capability();
    if !capability.writable {
        return Err(invalid_input(
            capability
                .reason
                .unwrap_or_else(|| "rigid translation is unavailable for this YDD drawable".into()),
        )
        .into());
    }
    let before = session.document()?;
    session.translate_rigid_model(delta)?;
    let rewritten = session.to_bytes()?;
    let reopened_dictionary = YddDictionary::from_bytes(&rewritten)?;
    if reopened_dictionary.entries() != original_dictionary.entries() {
        return Err(io::Error::other(
            "translated YDD changed dictionary identity metadata during semantic re-open",
        )
        .into());
    }
    let reopened = YddEditSession::from_bytes(&rewritten, drawable_index)?;
    let after = reopened.document()?;
    verify_rigid_translation(&before, &after, delta)?;

    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, &rewritten)?;
    println!("source: {}", source.display());
    println!("drawable-index: {drawable_index}");
    println!("delta: [{}, {}, {}]", delta[0], delta[1], delta[2]);
    println!("vertices: {}", after.model.vertex_count());
    println!(
        "bounds-before: {:?} -> {:?}",
        before.model.bounds.min, before.model.bounds.max
    );
    println!(
        "bounds-after: {:?} -> {:?}",
        after.model.bounds.min, after.model.bounds.max
    );
    println!("output: {}", output.display());
    println!("semantic-reopen: ok");
    Ok(())
}

fn verify_rigid_translation(
    before: &YdrDocument,
    after: &YdrDocument,
    delta: [f32; 3],
) -> Result<(), io::Error> {
    let before_model = &before.model;
    let after_model = &after.model;
    if before_model.name != after_model.name
        || before_model.lod != after_model.lod
        || before_model.coordinate_convention != after_model.coordinate_convention
        || before_model.shaders != after_model.shaders
        || before_model.primitives.len() != after_model.primitives.len()
        || before_model.bounds.radius != after_model.bounds.radius
        || translated_vec3(before_model.bounds.center, delta) != after_model.bounds.center
        || translated_vec3(before_model.bounds.min, delta) != after_model.bounds.min
        || translated_vec3(before_model.bounds.max, delta) != after_model.bounds.max
    {
        return Err(io::Error::other(
            "rigid translation changed drawable metadata or bounds unexpectedly",
        ));
    }

    let embedded_textures_match = match (
        before.embedded_textures.as_ref(),
        after.embedded_textures.as_ref(),
    ) {
        (None, None) => true,
        (Some(left), Some(right)) => left.textures() == right.textures(),
        _ => false,
    };
    if !embedded_textures_match {
        return Err(io::Error::other(
            "rigid translation changed embedded texture metadata",
        ));
    }

    for (before_primitive, after_primitive) in before_model
        .primitives
        .iter()
        .zip(after_model.primitives.iter())
    {
        if before_primitive.model_index != after_primitive.model_index
            || before_primitive.geometry_index != after_primitive.geometry_index
            || before_primitive.shader_index != after_primitive.shader_index
            || before_primitive.topology != after_primitive.topology
            || before_primitive.normals != after_primitive.normals
            || before_primitive.uv0 != after_primitive.uv0
            || before_primitive.indices != after_primitive.indices
            || before_primitive.declaration != after_primitive.declaration
            || before_primitive.positions.len() != after_primitive.positions.len()
        {
            return Err(io::Error::other(
                "rigid translation changed primitive topology or non-position attributes",
            ));
        }
        for (before_position, after_position) in before_primitive
            .positions
            .iter()
            .zip(after_primitive.positions.iter())
        {
            if translated_vec3(*before_position, delta) != *after_position {
                return Err(io::Error::other(
                    "rigid translation produced an unexpected vertex position",
                ));
            }
        }
    }
    Ok(())
}

fn translated_vec3(value: [f32; 3], delta: [f32; 3]) -> [f32; 3] {
    [
        value[0] + delta[0],
        value[1] + delta[1],
        value[2] + delta[2],
    ]
}

fn ytyp_info(path: &Path) -> Result<(), Box<dyn Error>> {
    let data = fs::read(path)?;
    let ytyp = Ytyp::from_bytes(&data)?;

    println!("file: {}", path.display());
    println!("name: {}", format_hash(ytyp.name.map(|hash| hash.0)));
    println!("archetypes: {}", ytyp.archetypes.len());
    println!("dependencies: {}", ytyp.dependencies.len());

    for archetype in &ytyp.archetypes {
        println!("\narchetype: 0x{:08X}", archetype.name.0);
        println!("  kind: {}", format_archetype_kind(archetype.kind));
        println!("  asset-type: {}", format_asset_type(archetype.asset_type));
        println!(
            "  asset-name: {}",
            format_hash(archetype.asset_name.map(|hash| hash.0))
        );
        println!(
            "  texture-dictionary: {}",
            format_hash(archetype.texture_dictionary.map(|hash| hash.0))
        );
        println!(
            "  physics-dictionary: {}",
            format_hash(archetype.physics_dictionary.map(|hash| hash.0))
        );
        println!(
            "  clip-dictionary: {}",
            format_hash(archetype.clip_dictionary.map(|hash| hash.0))
        );
        println!(
            "  drawable-dictionary: {}",
            format_hash(archetype.drawable_dictionary.map(|hash| hash.0))
        );
        if archetype.kind == ArchetypeKind::Mlo {
            println!("  mlo-entities: {}", archetype.mlo_entity_count);
            println!(
                "  mlo-entity-archetypes: {}",
                archetype.mlo_entity_archetypes.len()
            );
            println!("  mlo-rooms: {}", archetype.mlo_room_count);
            println!("  mlo-portals: {}", archetype.mlo_portal_count);
            for entity_archetype in &archetype.mlo_entity_archetypes {
                println!("    -> 0x{:08X}", entity_archetype.0);
            }
        }
    }

    Ok(())
}

fn format_archetype_kind(kind: ArchetypeKind) -> String {
    match kind {
        ArchetypeKind::Base => "base".into(),
        ArchetypeKind::Time => "time".into(),
        ArchetypeKind::Mlo => "mlo".into(),
        ArchetypeKind::Unknown(hash) => format!("unknown(0x{:08X})", hash.0),
    }
}

fn format_asset_type(asset_type: AssetType) -> String {
    match asset_type {
        AssetType::Uninitialized => "uninitialized".into(),
        AssetType::Fragment => "fragment (.yft)".into(),
        AssetType::Drawable => "drawable (.ydr)".into(),
        AssetType::DrawableDictionary => "drawable-dictionary (.ydd)".into(),
        AssetType::Assetless => "assetless".into(),
        AssetType::Unknown(value) => format!("unknown({value})"),
    }
}

fn ymf_info(path: &Path) -> Result<(), Box<dyn Error>> {
    let bytes = fs::read(path)?;
    let ymf = Ymf::from_bytes(&bytes)?;
    println!("file: {}", path.display());
    println!("format: PSO");
    println!("ymaps: {}", ymf.maps.len());
    println!("ytyps-with-dependencies: {}", ymf.ytyps.len());
    println!("interiors: {}", ymf.interiors.len());

    if !ymf.maps.is_empty() {
        println!("\nymap dependencies:");
        for map in &ymf.maps {
            let interior = if map.flags.contains(ManifestFlags::INTERIOR_DATA) {
                " [INTERIOR_DATA]"
            } else {
                ""
            };
            println!("  0x{:08X}{interior}", map.ymap.0);
            for ytyp in &map.ytyps {
                println!("    -> YTYP 0x{:08X}", ytyp.0);
            }
        }
    }
    if !ymf.ytyps.is_empty() {
        println!("\nytyp dependencies:");
        for entry in &ymf.ytyps {
            println!("  0x{:08X}", entry.ytyp.0);
            for dependency in &entry.ytyps {
                println!("    -> YTYP 0x{:08X}", dependency.0);
            }
        }
    }
    if !ymf.interiors.is_empty() {
        println!("\ninterior bounds:");
        for interior in &ymf.interiors {
            println!("  0x{:08X}", interior.name.0);
            for bound in &interior.bounds {
                println!("    -> YBN 0x{:08X}", bound.0);
            }
        }
    }
    Ok(())
}

fn ytd_info(path: &Path) -> Result<(), Box<dyn Error>> {
    let bytes = fs::read(path)?;
    let ytd = Ytd::from_bytes(&bytes)?;

    println!("file: {}", path.display());
    println!("resource-version: {}", ytd.resource_version);
    println!("file-vft: 0x{:08X}", ytd.file_vft);
    println!("file-unknown: 0x{:08X}", ytd.file_unknown);
    println!("textures: {}", ytd.textures.len());

    for texture in &ytd.textures {
        let hash_state = if texture.dictionary_hash_matches_name() {
            "match"
        } else {
            "MISMATCH"
        };
        println!("\ntexture: {}", texture.name);
        println!(
            "  dictionary-hash: 0x{:08X} ({hash_state})",
            texture.dictionary_hash
        );
        println!("  name-hash: 0x{:08X}", texture.name_hash);
        println!(
            "  size: {}x{}x{}",
            texture.width, texture.height, texture.depth
        );
        println!("  stride: {}", texture.stride);
        println!(
            "  format: {} / {} (0x{:08X})",
            texture.format,
            texture.format.normalized_name(),
            texture.format.raw()
        );
        println!("  mip-levels: {}", texture.levels);
        println!("  usage: {}", texture.usage);
        println!("  usage-flags: 0x{:08X}", texture.usage_flags);
        println!("  extra-flags: 0x{:08X}", texture.extra_flags);
        println!("  data-pointer: 0x{:016X}", texture.data_pointer);
        println!("  data-length: {}", texture.data_length);
    }

    Ok(())
}

fn ytd_dds(path: &Path, index: usize, output: &Path) -> Result<(), Box<dyn Error>> {
    let bytes = fs::read(path)?;
    let ytd = Ytd::from_bytes(&bytes)?;
    let texture = ytd.textures.get(index).ok_or_else(|| {
        invalid_input(format!(
            "texture index {index} is out of bounds for YTD with {} textures",
            ytd.textures.len()
        ))
    })?;
    let dds = Ytd::texture_dds(&bytes, index)?;

    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, &dds)?;

    println!("file: {}", path.display());
    println!("texture-index: {index}");
    println!("texture: {}", texture.name);
    println!("hash: 0x{:08X}", texture.dictionary_hash);
    println!("size: {}x{}", texture.width, texture.height);
    println!("format: {}", texture.format.normalized_name());
    println!("mip-levels: {}", texture.levels);
    println!("encoded-bytes: {}", texture.data_length);
    println!("dds-bytes: {}", dds.len());
    println!("output: {}", output.display());
    Ok(())
}

fn ytd_png(path: &Path, index: usize, output: &Path) -> Result<(), Box<dyn Error>> {
    let bytes = fs::read(path)?;
    let ytd = Ytd::from_bytes(&bytes)?;
    let texture = ytd.textures.get(index).ok_or_else(|| {
        invalid_input(format!(
            "texture index {index} is out of bounds for YTD with {} textures",
            ytd.textures.len()
        ))
    })?;
    let png = export_ytd_texture_png(&bytes, index)?;

    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, &png)?;

    println!("file: {}", path.display());
    println!("texture-index: {index}");
    println!("texture: {}", texture.name);
    println!("hash: 0x{:08X}", texture.dictionary_hash);
    println!("size: {}x{}", texture.width, texture.height);
    println!("format: {}", texture.format.normalized_name());
    println!("png-bytes: {}", png.len());
    println!("output: {}", output.display());
    println!("exported-mips: top");
    Ok(())
}

fn validate_new_ytd_output(source: &Path, output: &Path) -> Result<(), io::Error> {
    let source_canonical = fs::canonicalize(source)?;
    if output.exists() {
        let output_canonical = fs::canonicalize(output)?;
        if output_canonical == source_canonical {
            return Err(invalid_input(
                "refusing in-place YTD replacement; output must differ from source",
            ));
        }
        return Err(invalid_input(format!(
            "output already exists: {}; remove it or choose a new path",
            output.display()
        )));
    }

    let output_parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let output_name = output
        .file_name()
        .ok_or_else(|| invalid_input("output path must name a file"))?;
    if output_parent.exists()
        && fs::canonicalize(output_parent)?.join(output_name) == source_canonical
    {
        return Err(invalid_input(
            "refusing in-place YTD replacement; output must differ from source",
        ));
    }
    Ok(())
}

fn ytd_replace_dds(
    source: &Path,
    index: usize,
    replacement: &Path,
    output: &Path,
) -> Result<(), Box<dyn Error>> {
    validate_new_ytd_output(source, output)?;

    let source_bytes = fs::read(source)?;
    let replacement_bytes = fs::read(replacement)?;
    let before = Ytd::from_bytes(&source_bytes)?;
    let texture = before.textures.get(index).cloned().ok_or_else(|| {
        invalid_input(format!(
            "texture index {index} is out of bounds for YTD with {} textures",
            before.textures.len()
        ))
    })?;
    let rewritten = Ytd::replace_texture_from_dds(&source_bytes, index, &replacement_bytes)?;
    let after = Ytd::from_bytes(&rewritten)?;
    if after != before {
        return Err(io::Error::other(
            "layout-preserving replacement unexpectedly changed YTD metadata",
        )
        .into());
    }

    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, &rewritten)?;

    println!("source: {}", source.display());
    println!("texture-index: {index}");
    println!("texture: {}", texture.name);
    println!("hash: 0x{:08X}", texture.dictionary_hash);
    println!("size: {}x{}", texture.width, texture.height);
    println!("format: {}", texture.format.normalized_name());
    println!("mip-levels: {}", texture.levels);
    println!("encoded-bytes: {}", texture.data_length);
    println!("replacement-dds: {}", replacement.display());
    println!("output: {}", output.display());
    println!("layout-preserved: yes");
    Ok(())
}

fn ytd_repack_dds(
    source: &Path,
    index: usize,
    replacement: &Path,
    output: &Path,
) -> Result<(), Box<dyn Error>> {
    validate_new_ytd_output(source, output)?;

    let source_bytes = fs::read(source)?;
    let replacement_bytes = fs::read(replacement)?;
    let before = Ytd::from_bytes(&source_bytes)?;
    let before_texture = before.textures.get(index).cloned().ok_or_else(|| {
        invalid_input(format!(
            "texture index {index} is out of bounds for YTD with {} textures",
            before.textures.len()
        ))
    })?;
    let rewritten =
        Ytd::replace_texture_from_dds_relocated(&source_bytes, index, &replacement_bytes)?;
    let after = Ytd::from_bytes(&rewritten)?;
    let after_texture = after.textures.get(index).ok_or_else(|| {
        io::Error::other("repacked YTD lost the replaced texture during semantic reparse")
    })?;

    if before.resource_version != after.resource_version
        || before.file_vft != after.file_vft
        || before.file_unknown != after.file_unknown
        || before.pages_info_pointer != after.pages_info_pointer
        || before.textures.len() != after.textures.len()
    {
        return Err(io::Error::other(
            "repacked YTD unexpectedly changed dictionary-level metadata",
        )
        .into());
    }
    for (other_index, (left, right)) in before
        .textures
        .iter()
        .zip(after.textures.iter())
        .enumerate()
    {
        if other_index != index && left != right {
            return Err(io::Error::other(format!(
                "repacked YTD unexpectedly changed texture metadata at index {other_index}"
            ))
            .into());
        }
    }
    if before_texture.dictionary_hash != after_texture.dictionary_hash
        || before_texture.name != after_texture.name
        || before_texture.name_hash != after_texture.name_hash
        || before_texture.depth != after_texture.depth
        || before_texture.format != after_texture.format
        || before_texture.usage != after_texture.usage
        || before_texture.usage_flags != after_texture.usage_flags
        || before_texture.extra_flags != after_texture.extra_flags
    {
        return Err(io::Error::other(
            "repacked YTD unexpectedly changed identity/usage metadata for the target texture",
        )
        .into());
    }

    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, &rewritten)?;

    println!("source: {}", source.display());
    println!("texture-index: {index}");
    println!("texture: {}", after_texture.name);
    println!("hash: 0x{:08X}", after_texture.dictionary_hash);
    println!(
        "size: {}x{} -> {}x{}",
        before_texture.width, before_texture.height, after_texture.width, after_texture.height
    );
    println!("format: {}", after_texture.format.normalized_name());
    println!(
        "mip-levels: {} -> {}",
        before_texture.levels, after_texture.levels
    );
    println!(
        "encoded-bytes: {} -> {}",
        before_texture.data_length, after_texture.data_length
    );
    println!("replacement-dds: {}", replacement.display());
    println!("output: {}", output.display());
    println!("payload-relocated: yes");
    println!("format-preserved: yes");
    Ok(())
}

fn ytd_repack_png(
    source: &Path,
    index: usize,
    replacement: &Path,
    output: &Path,
) -> Result<(), Box<dyn Error>> {
    validate_new_ytd_output(source, output)?;

    let source_bytes = fs::read(source)?;
    let replacement_bytes = fs::read(replacement)?;
    let before = Ytd::from_bytes(&source_bytes)?;
    let before_texture = before.textures.get(index).cloned().ok_or_else(|| {
        invalid_input(format!(
            "texture index {index} is out of bounds for YTD with {} textures",
            before.textures.len()
        ))
    })?;
    let rewritten = replace_ytd_texture_from_png(&source_bytes, index, &replacement_bytes)?;
    let after = Ytd::from_bytes(&rewritten)?;
    let after_texture = after.textures.get(index).ok_or_else(|| {
        io::Error::other("PNG-repacked YTD lost the replaced texture during semantic reparse")
    })?;

    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    use std::io::Write as _;
    file.write_all(&rewritten)?;
    file.flush()?;
    if fs::read(source)? != source_bytes {
        return Err(io::Error::other("source changed during PNG YTD repack").into());
    }
    Ytd::from_bytes(&fs::read(output)?)?;

    println!("source: {}", source.display());
    println!("texture-index: {index}");
    println!("texture: {}", after_texture.name);
    println!("hash: 0x{:08X}", after_texture.dictionary_hash);
    println!(
        "size: {}x{} -> {}x{}",
        before_texture.width, before_texture.height, after_texture.width, after_texture.height
    );
    println!("format: {}", after_texture.format.normalized_name());
    println!(
        "mip-levels: {} -> {}",
        before_texture.levels, after_texture.levels
    );
    println!("replacement-png: {}", replacement.display());
    println!("output: {}", output.display());
    println!("semantic-reopen: yes");
    println!("source-unchanged: yes");
    println!("mips-generated: yes");
    println!("format-preserved: yes");
    Ok(())
}

fn ytd_repack_rgba(
    source: &Path,
    index: usize,
    width: u16,
    height: u16,
    replacement: &Path,
    output: &Path,
) -> Result<(), Box<dyn Error>> {
    validate_new_ytd_output(source, output)?;

    let source_bytes = fs::read(source)?;
    let rgba = fs::read(replacement)?;
    let before = Ytd::from_bytes(&source_bytes)?;
    let before_texture = before.textures.get(index).cloned().ok_or_else(|| {
        invalid_input(format!(
            "texture index {index} is out of bounds for YTD with {} textures",
            before.textures.len()
        ))
    })?;
    let rewritten =
        Ytd::replace_texture_from_rgba_relocated(&source_bytes, index, width, height, &rgba)?;
    let after = Ytd::from_bytes(&rewritten)?;
    let after_texture = after.textures.get(index).ok_or_else(|| {
        io::Error::other("RGBA-repacked YTD lost the replaced texture during semantic reparse")
    })?;

    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, &rewritten)?;

    println!("source: {}", source.display());
    println!("texture-index: {index}");
    println!("texture: {}", after_texture.name);
    println!("hash: 0x{:08X}", after_texture.dictionary_hash);
    println!(
        "size: {}x{} -> {}x{}",
        before_texture.width, before_texture.height, after_texture.width, after_texture.height
    );
    println!("format: {}", after_texture.format.normalized_name());
    println!(
        "mip-levels: {} -> {}",
        before_texture.levels, after_texture.levels
    );
    println!(
        "encoded-bytes: {} -> {}",
        before_texture.data_length, after_texture.data_length
    );
    println!("replacement-rgba: {}", replacement.display());
    println!("rgba-bytes: {}", rgba.len());
    println!("output: {}", output.display());
    println!("mips-generated: yes");
    println!("format-preserved: yes");
    println!("payload-relocated: yes");
    Ok(())
}

fn ytd_rebuild_compact(source: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    validate_new_ytd_output(source, output)?;

    let source_bytes = fs::read(source)?;
    let before_resource = Rsc7Resource::parse(&source_bytes)?;
    let before = Ytd::from_bytes(&source_bytes)?;
    let rebuilt = Ytd::rebuild_legacy_compact(&source_bytes)?;
    let after_resource = Rsc7Resource::parse(&rebuilt)?;
    let after = Ytd::from_bytes(&rebuilt)?;

    if before.resource_version != after.resource_version
        || before.file_vft != after.file_vft
        || before.file_unknown != after.file_unknown
        || before.textures.len() != after.textures.len()
    {
        return Err(io::Error::other(
            "compact YTD rebuild failed dictionary-level semantic verification",
        )
        .into());
    }
    for (index, (left, right)) in before
        .textures
        .iter()
        .zip(after.textures.iter())
        .enumerate()
    {
        if left.dictionary_hash != right.dictionary_hash
            || left.name != right.name
            || left.name_hash != right.name_hash
            || left.width != right.width
            || left.height != right.height
            || left.depth != right.depth
            || left.stride != right.stride
            || left.format != right.format
            || left.levels != right.levels
            || left.usage != right.usage
            || left.usage_flags != right.usage_flags
            || left.extra_flags != right.extra_flags
            || left.data_length != right.data_length
        {
            return Err(io::Error::other(format!(
                "compact YTD rebuild changed semantic metadata at texture {index}"
            ))
            .into());
        }
    }

    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, &rebuilt)?;

    println!("source: {}", source.display());
    println!("textures: {}", after.textures.len());
    println!("source-bytes: {}", source_bytes.len());
    println!("output-bytes: {}", rebuilt.len());
    println!(
        "system-segment: {} -> {}",
        before_resource.system().len(),
        after_resource.system().len()
    );
    println!(
        "graphics-segment: {} -> {}",
        before_resource.graphics().len(),
        after_resource.graphics().len()
    );
    println!("output: {}", output.display());
    println!("semantic-reopen: ok");
    println!("layout-rebuilt: yes");
    Ok(())
}

#[derive(Debug, Default)]
struct CliCatalogArgs {
    durtyfree_object_list: Option<PathBuf>,
    vanilla_file_index: Option<PathBuf>,
}

struct LoadedCliCatalogs {
    durtyfree: Option<DurtyFreeCatalog>,
    file_catalog: Option<VanillaFileCatalog>,
}

fn parse_catalog_args(
    args: impl Iterator<Item = String>,
    usage: &str,
) -> Result<CliCatalogArgs, io::Error> {
    let mut args = args;
    let mut catalogs = CliCatalogArgs::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--durtyfree-object-list" => {
                catalogs.durtyfree_object_list =
                    Some(PathBuf::from(required_arg(args.next(), usage)?));
            }
            "--vanilla-file-index" => {
                catalogs.vanilla_file_index =
                    Some(PathBuf::from(required_arg(args.next(), usage)?));
            }
            other => {
                return Err(invalid_input(format!("unknown catalog option: {other}")));
            }
        }
    }
    Ok(catalogs)
}

fn parse_preflight_args(
    args: impl Iterator<Item = String>,
    usage: &str,
) -> Result<(Vec<PathBuf>, CliCatalogArgs, bool), io::Error> {
    let mut args = args;
    let mut maps = Vec::new();
    let mut catalogs = CliCatalogArgs::default();
    let mut json_output = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--durtyfree-object-list" => {
                catalogs.durtyfree_object_list =
                    Some(PathBuf::from(required_arg(args.next(), usage)?));
            }
            "--vanilla-file-index" => {
                catalogs.vanilla_file_index =
                    Some(PathBuf::from(required_arg(args.next(), usage)?));
            }
            "--json" if !json_output => json_output = true,
            _ if arg.starts_with('-') => {
                return Err(invalid_input(format!("unknown preflight option: {arg}")));
            }
            _ => maps.push(PathBuf::from(arg)),
        }
    }
    if maps.is_empty() {
        return Err(invalid_input(usage));
    }
    Ok((maps, catalogs, json_output))
}

struct CliExportArgs {
    maps: Vec<PathBuf>,
    output: PathBuf,
    resource_name: Option<String>,
    options: SharedExportOptions,
    catalogs: CliCatalogArgs,
    json_output: bool,
}

fn parse_export_args(
    args: impl Iterator<Item = String>,
    usage: &str,
) -> Result<CliExportArgs, io::Error> {
    let mut args = args;
    let mut maps = Vec::new();
    let mut output = None;
    let mut resource_name = None;
    let mut options = SharedExportOptions::default();
    let mut catalogs = CliCatalogArgs::default();
    let mut json_output = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output" if output.is_none() => {
                output = Some(PathBuf::from(required_arg(args.next(), usage)?));
            }
            "--resource-name" if resource_name.is_none() => {
                resource_name = Some(required_arg(args.next(), usage)?);
            }
            "--allow-unresolved" => options.allow_unresolved = true,
            "--overwrite" => options.overwrite = true,
            "--durtyfree-object-list" => {
                catalogs.durtyfree_object_list =
                    Some(PathBuf::from(required_arg(args.next(), usage)?));
            }
            "--vanilla-file-index" => {
                catalogs.vanilla_file_index =
                    Some(PathBuf::from(required_arg(args.next(), usage)?));
            }
            "--json" if !json_output => json_output = true,
            _ if arg.starts_with('-') => {
                return Err(invalid_input(format!("unknown export option: {arg}")));
            }
            _ => maps.push(PathBuf::from(arg)),
        }
    }

    if maps.is_empty() || output.is_none() {
        return Err(invalid_input(usage));
    }

    Ok(CliExportArgs {
        maps,
        output: output.expect("checked above"),
        resource_name,
        options,
        catalogs,
        json_output,
    })
}

fn load_cli_catalogs(args: CliCatalogArgs) -> Result<LoadedCliCatalogs, Box<dyn Error>> {
    let durtyfree = match args.durtyfree_object_list {
        Some(path) => {
            let body = fs::read_to_string(&path)?;
            let objects = parse_durtyfree_object_list(&body);
            if objects.len() < 1_000 {
                return Err(invalid_input(format!(
                    "DurtyFree ObjectList looked incomplete: only {} valid objects in {}",
                    objects.len(),
                    path.display()
                ))
                .into());
            }
            Some(DurtyFreeCatalog {
                objects,
                synced_at: None,
            })
        }
        None => None,
    };
    let file_catalog = match args.vanilla_file_index {
        Some(path) => {
            let body = fs::read_to_string(&path)?;
            let files = parse_vanilla_file_catalog(&body);
            if files.is_empty() {
                return Err(invalid_input(format!(
                    "vanilla file index contains no supported GTA asset paths: {}",
                    path.display()
                ))
                .into());
            }
            Some(VanillaFileCatalog {
                files,
                synced_at: None,
            })
        }
        None => None,
    };
    Ok(LoadedCliCatalogs {
        durtyfree,
        file_catalog,
    })
}

fn workspace_map_path(workspace: &Path, map: &Path) -> PathBuf {
    if map.is_absolute() {
        map.to_path_buf()
    } else {
        workspace.join(map)
    }
}

fn preflight(
    root: &Path,
    maps: &[PathBuf],
    catalog_args: CliCatalogArgs,
    json_output: bool,
) -> Result<(), Box<dyn Error>> {
    let catalogs = load_cli_catalogs(catalog_args)?;
    let report = workspace_export_preflight_report(
        root,
        maps,
        CatalogRefs {
            durtyfree: catalogs.durtyfree.as_ref(),
            file_catalog: catalogs.file_catalog.as_ref(),
        },
    )?;

    if json_output {
        agent::print_success("preflight", serde_json::to_value(&report)?)?;
        return Ok(());
    }

    println!("workspace: {}", report.workspace);
    println!("selected-roots: {}", report.selected_roots.len());
    println!("closure-ymaps: {}", report.closure_ymaps.len());
    println!("predicted-files: {}", report.predicted_files.len());
    println!("raw-unresolved: {}", report.unresolved.raw);
    println!("vanilla: {}", report.unresolved.vanilla);
    println!("unknown: {}", report.unresolved.unknown);
    println!("unknown-groups: {}", report.unknown_groups.len());
    println!("mlo-audits: {}", report.mlo_audits.len());
    println!("warnings: {}", report.warnings.len());

    if !report.selected_roots.is_empty() {
        println!("\nselected roots:");
        for root in &report.selected_roots {
            println!("  {root}");
        }
    }
    if !report.mlo_audits.is_empty() {
        println!("\nMLO audits:");
        for audit in &report.mlo_audits {
            println!(
                "  {} {} - entities={} unique-archetypes={} rooms={} portals={} local={} vanilla={} unknown={} risk={}",
                audit.archetype_hash,
                audit.ytyp,
                audit.entities,
                audit.unique_entity_archetypes,
                audit.rooms,
                audit.portals,
                audit.local_files,
                audit.vanilla,
                audit.unknown,
                audit.risk
            );
        }
    }
    if !report.unknown_groups.is_empty() {
        println!("\nUNKNOWN groups (first 25):");
        for group in report.unknown_groups.iter().take(25) {
            println!(
                "  {} {} uses={} maps={} - {}",
                group.kind,
                group.hash,
                group.uses,
                group.affected_maps.len(),
                group
                    .reasons
                    .first()
                    .map(String::as_str)
                    .unwrap_or("<no reason>")
            );
        }
        if report.unknown_groups.len() > 25 {
            println!("  ... {} more", report.unknown_groups.len() - 25);
        }
    }
    if !report.warnings.is_empty() {
        println!("\nwarnings:");
        for warning in &report.warnings {
            println!("  {warning}");
        }
    }
    Ok(())
}
fn mlo_audit(
    root: &Path,
    ymap_arg: &Path,
    catalog_args: CliCatalogArgs,
) -> Result<(), Box<dyn Error>> {
    let workspace = root.canonicalize()?;
    let index = WorkspaceIndex::scan(&workspace)?;
    let catalogs = load_cli_catalogs(catalog_args)?;
    let map = workspace_map_path(&workspace, ymap_arg);
    let audits = index.audit_map_mlos(&map)?;

    println!("workspace: {}", workspace.display());
    println!("ymap: {}", map.display());
    println!("mlo-archetypes: {}", audits.len());
    for audit in audits {
        let summary = summarize_mlo_audit(
            audit,
            catalogs.durtyfree.as_ref(),
            catalogs.file_catalog.as_ref(),
        );
        println!("\n0x{:08X}", summary.archetype_hash);
        println!("  ytyp: {}", summary.ytyp_path.display());
        println!("  entities: {}", summary.entities);
        println!(
            "  unique-entity-archetypes: {}",
            summary.unique_entity_archetypes
        );
        println!("  rooms: {}", summary.rooms);
        println!("  portals: {}", summary.portals);
        println!("  local-files: {}", summary.local_files);
        println!("  vanilla: {}", summary.vanilla);
        println!("  unknown: {}", summary.unknown);
        println!("  risk: {}", summary.risk.as_str());
    }
    Ok(())
}

fn workspace_export(root: &Path, request: CliExportArgs) -> Result<(), Box<dyn Error>> {
    let catalogs = load_cli_catalogs(request.catalogs)?;
    let resource_name = request.resource_name.unwrap_or_else(|| {
        request
            .output
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .or_else(|| {
                request
                    .maps
                    .first()
                    .and_then(|path| path.file_stem())
                    .and_then(|value| value.to_str())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "ragelab-export".to_string())
    });

    let report = workspace_export_report(
        root,
        &request.maps,
        &request.output,
        &resource_name,
        request.options,
        CatalogRefs {
            durtyfree: catalogs.durtyfree.as_ref(),
            file_catalog: catalogs.file_catalog.as_ref(),
        },
    )?;

    if request.json_output {
        agent::print_success("export", serde_json::to_value(&report)?)?;
        return Ok(());
    }

    println!("resource: {}", report.output.resource);
    println!("stream: {}", report.output.stream);
    println!("selected-roots: {}", report.selected_roots.len());
    println!("copied-files: {}", report.output.copied_files.len());
    println!("manifest: {}", report.output.manifest);
    println!("metadata: {}", report.output.metadata);
    if let Some(gtxd) = &report.output.gtxd {
        println!("gtxd: {gtxd}");
    }
    println!("ymaps-in-manifest: {}", report.output.manifest_maps);
    println!("unresolved: {}", report.unresolved.unknown);
    println!("raw-unresolved: {}", report.unresolved.raw);
    println!("vanilla: {}", report.unresolved.vanilla);
    println!("validation: {}", report.validation.status);
    println!("warnings: {}", report.warnings.len());
    for warning in &report.warnings {
        println!("  warning: {warning}");
    }
    for warning in &report.validation.warnings {
        println!("  validation-warning: {warning}");
    }
    Ok(())
}
fn extract(
    root: &Path,
    ymap_arg: &Path,
    output: &Path,
    options: SharedExportOptions,
) -> Result<(), Box<dyn Error>> {
    let index = WorkspaceIndex::scan(root)?;
    let resource_name = ymap_arg
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("ragelab-export");
    let outcome = export_map_resource(
        &index,
        root,
        ymap_arg,
        output,
        resource_name,
        options,
        CatalogRefs::default(),
    )?;
    let unresolved = outcome.unresolved;
    let metadata_path = outcome.metadata_path;
    let validation = outcome.validation;
    let result = outcome.result;

    println!("resource: {}", result.output_dir.display());
    println!("stream: {}", result.stream_dir.display());
    println!("copied-files: {}", result.copied_files.len());
    println!("manifest: {}", result.manifest_path.display());
    println!("metadata: {}", metadata_path.display());
    if let Some(gtxd) = &result.gtxd_path {
        println!("gtxd: {}", gtxd.display());
    }
    println!("ymaps-in-manifest: {}", result.manifest.maps.len());
    println!("unresolved: {}", unresolved.unknown);
    println!("raw-unresolved: {}", unresolved.raw);
    println!("vanilla: {}", unresolved.vanilla);
    println!("validation: {}", validation.status);
    println!("warnings: {}", result.report.warnings.len());
    for warning in &result.report.warnings {
        println!("  warning: {warning}");
    }
    for warning in &validation.warnings {
        println!("  validation-warning: {warning}");
    }
    Ok(())
}

fn deps(root: &Path, ymap_arg: &Path) -> Result<(), Box<dyn Error>> {
    let index = WorkspaceIndex::scan(root)?;
    let report = index.resolve_map(ymap_arg)?;

    println!("workspace: {}", index.root().display());
    println!("resolved-files: {}", report.files.len());
    println!("unresolved: {}", report.unresolved.len());

    println!("\nfiles:");
    for file in report.files.values() {
        println!("  [{:4}] {}", file.kind, file.path.display());
        for reason in &file.reasons {
            println!("         - {reason}");
        }
    }

    if !report.unresolved.is_empty() {
        println!("\nunresolved:");
        for dependency in &report.unresolved {
            println!(
                "  {} 0x{:08X} - {}",
                dependency.kind, dependency.hash, dependency.reason
            );
        }
    }

    if !report.warnings.is_empty() {
        println!("\nwarnings:");
        for warning in &report.warnings {
            println!("  {warning}");
        }
    }

    Ok(())
}

fn providers(root: &Path, ymap_arg: &Path) -> Result<(), Box<dyn Error>> {
    if !root.is_dir() {
        return Err(invalid_input(format!("not a directory: {}", root.display())).into());
    }

    let ymap_path = if ymap_arg.is_absolute() || ymap_arg.exists() {
        ymap_arg.to_path_buf()
    } else {
        root.join(ymap_arg)
    };
    let ymap = Ymap::from_bytes(&fs::read(&ymap_path)?)?;
    let wanted: BTreeSet<u32> = ymap
        .entities
        .iter()
        .map(|entity| entity.archetype_name.0)
        .collect();

    let mut provider_index = BTreeMap::<u32, Vec<PathBuf>>::new();
    let mut indexed_ytyps = 0_usize;
    let mut parse_errors = Vec::<(PathBuf, String)>::new();

    walk(root, &mut |path| {
        let is_ytyp = path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("ytyp"));
        if !is_ytyp {
            return;
        }

        match fs::read(path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| Ytyp::from_bytes(&bytes).map_err(|error| error.to_string()))
        {
            Ok(ytyp) => {
                indexed_ytyps += 1;
                for archetype in ytyp.archetypes {
                    provider_index
                        .entry(archetype.name.0)
                        .or_default()
                        .push(path.to_path_buf());
                }
            }
            Err(error) => parse_errors.push((path.to_path_buf(), error)),
        }
    })?;

    println!("workspace: {}", root.display());
    println!("ymap: {}", ymap_path.display());
    println!("unique-archetypes: {}", wanted.len());
    println!("indexed-ytyps: {indexed_ytyps}");

    println!("\nproviders:");
    for hash in wanted {
        match provider_index.get(&hash) {
            Some(paths) => {
                for path in paths {
                    println!("  0x{hash:08X} -> {}", path.display());
                }
            }
            None => println!("  0x{hash:08X} -> <unresolved>"),
        }
    }

    if !parse_errors.is_empty() {
        println!("\nunreadable YTYP files:");
        for (path, error) in parse_errors {
            println!("  {}: {error}", path.display());
        }
    }

    Ok(())
}

fn format_hash(hash: Option<u32>) -> String {
    hash.map(|value| format!("0x{value:08X}"))
        .unwrap_or_else(|| "<none>".to_string())
}

fn scan(root: &Path) -> Result<(), Box<dyn Error>> {
    if !root.is_dir() {
        return Err(invalid_input(format!("not a directory: {}", root.display())).into());
    }

    let mut counts = BTreeMap::<String, usize>::new();
    let mut files = Vec::<(String, PathBuf)>::new();
    walk(root, &mut |path| {
        let ext = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        if let Some(kind) = classify_extension(&ext) {
            *counts.entry(kind.to_string()).or_default() += 1;
            files.push((kind.to_string(), path.to_path_buf()));
        }
    })?;

    println!("workspace: {}", root.display());
    println!("assets: {}", files.len());
    println!();
    for (kind, count) in &counts {
        println!("{kind:5} {count:>6}");
    }

    if !files.is_empty() {
        println!("\nYMAP files:");
        for (_, path) in files.iter().filter(|(kind, _)| kind == "YMAP") {
            println!("  {}", path.display());
        }
    }

    Ok(())
}

fn vanilla_index(root: &Path, output: &Path) -> Result<(), Box<dyn Error>> {
    let build = build_vanilla_catalog(root)?;
    let body = render_vanilla_catalog_paths(&build.paths);

    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, &body)?;

    println!("root: {}", build.report.root.display());
    println!("vanilla-file-paths: {}", build.report.path_entries);
    println!(
        "vanilla-file-entries: {}",
        build.report.unique_catalog_entries
    );
    println!("coverage: {}", build.report.coverage.as_str());
    println!(
        "rpf-archives: {}",
        build.report.archive_boundary.rpf_archives_found
    );
    println!("output: {}", output.display());
    Ok(())
}

fn walk(root: &Path, callback: &mut impl FnMut(&Path)) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            walk(&path, callback)?;
        } else if file_type.is_file() {
            callback(&path);
        }
    }
    Ok(())
}

fn classify_extension(extension: &str) -> Option<&'static str> {
    match extension {
        "ymap" => Some("YMAP"),
        "ytyp" => Some("YTYP"),
        "ymf" => Some("YMF"),
        "ydr" => Some("YDR"),
        "ydd" => Some("YDD"),
        "ytd" => Some("YTD"),
        "ybn" => Some("YBN"),
        "yft" => Some("YFT"),
        "ycd" => Some("YCD"),
        "ymt" => Some("YMT"),
        "yld" => Some("YLD"),
        "ynv" => Some("YNV"),
        _ => None,
    }
}

#![cfg(target_arch = "wasm32")]

use js_sys::{Array, Object, Reflect, Uint8Array};
use ragelab_resource::Rsc7Resource;
use ragelab_wasm::{
    capabilities, inspect_ydd, inspect_ydr_materials, inspect_yft, inspect_ymap, inspect_ytd,
    inspect_ytyp, resolve_supplied_ymap_scene, validate_asset, wasm_export_ytd_texture_png,
    wasm_replace_ytd_texture_png, ydd_model, ydd_rebind_texture, ydr_embedded_texture,
    ydr_embedded_texture_by_name, ydr_model, ydr_rebind_texture, yft_model, ymap_set_flags,
    ytd_texture, ytd_texture_by_name, ytd_texture_mip,
};
use ragelab_yft::YFT_LEGACY_VERSION;
use wasm_bindgen::JsValue;
use wasm_bindgen_test::*;

const YMAP: &[u8] = include_bytes!("../../../fixtures/synthetic/stream/simple.ymap");
const SIMPLE_YTYP: &[u8] = include_bytes!("../../../fixtures/synthetic/simple.ytyp");
const YTYP: &[u8] = include_bytes!("../../../fixtures/synthetic/mlo.ytyp");
const YDR: &[u8] = include_bytes!("../../../fixtures/synthetic/ydr/simple.ydr");
const EDITABLE_YDR: &[u8] = include_bytes!("../../../fixtures/synthetic/ydr/editable.ydr");
const EMBEDDED_YDR: &[u8] = include_bytes!("../../../fixtures/synthetic/ydr/embedded.ydr");
const YDD: &[u8] = include_bytes!("../../../fixtures/synthetic/ydd/editable.ydd");
const YTD: &[u8] = include_bytes!("../../../fixtures/synthetic/ytd/simple.ytd");

fn synthetic_yft_without_drawable() -> Vec<u8> {
    let system = vec![0_u8; 0x60];
    Rsc7Resource::from_segments(YFT_LEGACY_VERSION, 0xA000_0000, 0, &system, &[])
        .expect("build synthetic YFT")
        .to_bytes()
        .expect("encode synthetic YFT")
}

fn field(value: &JsValue, name: &str) -> JsValue {
    Reflect::get(value, &JsValue::from_str(name)).expect("JS field")
}

fn supplied_file(name: &str, bytes: &[u8]) -> JsValue {
    let object = Object::new();
    Reflect::set(
        &object,
        &JsValue::from_str("name"),
        &JsValue::from_str(name),
    )
    .expect("set supplied name");
    let payload = Uint8Array::from(bytes);
    Reflect::set(&object, &JsValue::from_str("bytes"), &payload).expect("set supplied bytes");
    object.into()
}

#[wasm_bindgen_test]
fn typed_model_and_texture_packets_are_available_to_js() {
    let model = ydr_model(YDR).expect("YDR model packet");
    assert!(model.positions().length() > 0);
    assert!(model.indices().length() > 0);

    let dictionary_model = ydd_model(YDD, 0).expect("YDD model packet");
    assert!(dictionary_model.positions().length() > 0);

    let texture = ytd_texture(YTD, 0).expect("YTD texture packet");
    assert!(texture.rgba().length() > 0);
}

#[wasm_bindgen_test]
fn all_supported_formats_validate_and_inspect_from_supplied_bytes() {
    let yft = synthetic_yft_without_drawable();
    for (format, bytes) in [
        ("ymap", YMAP),
        ("ytyp", YTYP),
        ("ydr", YDR),
        ("ydd", YDD),
        ("ytd", YTD),
        ("yft", yft.as_slice()),
    ] {
        validate_asset(format, bytes).unwrap_or_else(|_| panic!("{format} should validate"));
    }

    inspect_ymap(YMAP).expect("inspect YMAP");
    inspect_ytyp(YTYP).expect("inspect YTYP");
    inspect_ydr_materials(YDR).expect("inspect YDR materials");
    inspect_ydd(YDD).expect("inspect YDD");
    inspect_ytd(YTD).expect("inspect YTD");
    inspect_yft(&yft).expect("inspect YFT");

    match yft_model(&yft) {
        Ok(_) => panic!("synthetic YFT intentionally has no pristine main drawable"),
        Err(error) => {
            assert_eq!(
                field(&error, "code").as_string().as_deref(),
                Some("unsupportedAsset")
            );
        }
    }
}

#[wasm_bindgen_test]
fn ydr_embedded_texture_packet_matches_proven_diffuse_binding() {
    let model = ydr_model(EMBEDDED_YDR).expect("embedded YDR model");
    let metadata = model.metadata().expect("model metadata");
    let shaders = Array::from(&field(&metadata, "shaders"));
    assert_eq!(shaders.length(), 1);
    let shader = shaders.get(0);
    assert_eq!(
        field(&shader, "diffuseTextureName").as_string().as_deref(),
        Some("embedded_diff")
    );
    assert_eq!(field(&metadata, "embeddedTextureCount").as_f64(), Some(1.0));

    let texture = ydr_embedded_texture(EMBEDDED_YDR, 0).expect("embedded texture packet");
    let texture_metadata = texture.metadata().expect("texture metadata");
    assert_eq!(
        field(&texture_metadata, "name").as_string().as_deref(),
        Some("embedded_diff")
    );
    assert_eq!(field(&texture_metadata, "width").as_f64(), Some(4.0));
    assert_eq!(field(&texture_metadata, "height").as_f64(), Some(4.0));
    let rgba = texture.rgba().to_vec();
    assert_eq!(rgba.len(), 64);
    assert!(rgba.chunks_exact(4).all(|pixel| pixel == [255, 0, 0, 255]));

    let error = match ydr_embedded_texture(EMBEDDED_YDR, 1) {
        Ok(_) => panic!("embedded texture index must fail closed"),
        Err(error) => error,
    };
    assert_eq!(
        field(&error, "code").as_string().as_deref(),
        Some("indexOutOfBounds")
    );

    let by_name = ydr_embedded_texture_by_name(EMBEDDED_YDR, "embedded_diff").expect("name lookup");
    assert_eq!(
        field(&by_name.metadata().expect("metadata"), "name")
            .as_string()
            .as_deref(),
        Some("embedded_diff")
    );

    let missing = match ydr_embedded_texture_by_name(EMBEDDED_YDR, "not_present") {
        Ok(_) => panic!("unknown embedded texture name must fail closed"),
        Err(error) => error,
    };
    assert_eq!(
        field(&missing, "code").as_string().as_deref(),
        Some("textureNotFound")
    );
}

#[wasm_bindgen_test]
fn ytd_name_lookup_uses_core_name_or_hash_semantics() {
    let indexed = ytd_texture(YTD, 0).expect("indexed YTD texture");
    let metadata = indexed.metadata().expect("YTD metadata");
    let name = field(&metadata, "name")
        .as_string()
        .expect("fixture texture name");
    let named = ytd_texture_by_name(YTD, &name).expect("lookup YTD by Core name/hash semantics");
    assert_eq!(named.rgba().to_vec(), indexed.rgba().to_vec());
}

#[wasm_bindgen_test]
fn shared_ytd_png_operation_roundtrips_in_wasm() {
    let png = wasm_export_ytd_texture_png(YTD, 0).expect("export PNG");
    let png_bytes = png.to_vec();
    let rewritten =
        wasm_replace_ytd_texture_png(YTD, 0, &png_bytes).expect("replace PNG through shared core");
    assert!(rewritten.length() > 0);
    validate_asset("ytd", &rewritten.to_vec()).expect("semantic reopen through WASM");
}

#[wasm_bindgen_test]
fn ytd_mip_binding_decodes_declared_level_and_fails_closed_out_of_bounds() {
    let png = wasm_export_ytd_texture_png(YTD, 0).expect("export PNG");
    let rewritten = wasm_replace_ytd_texture_png(YTD, 0, &png.to_vec())
        .expect("regenerate deterministic mip chain");
    let rewritten = rewritten.to_vec();

    let mip = ytd_texture_mip(&rewritten, 0, 1).expect("decode mip 1");
    let metadata = mip.metadata().expect("mip metadata");
    assert_eq!(field(&metadata, "mipIndex").as_f64(), Some(1.0));
    assert_eq!(field(&metadata, "width").as_f64(), Some(2.0));
    assert_eq!(field(&metadata, "height").as_f64(), Some(2.0));
    assert_eq!(mip.rgba().length(), 16);

    let error = match ytd_texture_mip(&rewritten, 0, 99) {
        Ok(_) => panic!("mip beyond declared count must fail closed"),
        Err(error) => error,
    };
    assert_eq!(
        field(&error, "code").as_string().as_deref(),
        Some("indexOutOfBounds")
    );
}

#[wasm_bindgen_test]
fn supplied_ymap_scene_resolves_only_explicit_dependencies_and_fails_closed() {
    let supplied = Array::new();
    supplied.push(&supplied_file("simple.ytyp", SIMPLE_YTYP));
    supplied.push(&supplied_file("test_drawable.ydr", YDR));

    let report =
        resolve_supplied_ymap_scene(YMAP, supplied.into()).expect("resolve supplied scene");
    assert_eq!(
        field(&report, "schema").as_string().as_deref(),
        Some("ragelab.wasm.supplied-ymap-scene")
    );
    let entities = Array::from(&field(&report, "entities"));
    assert_eq!(entities.length(), 1);
    let entity = entities.get(0);
    let resolution = field(&entity, "resolution");
    assert!(!resolution.is_null() && !resolution.is_undefined());
    assert_eq!(
        field(&resolution, "modelFormat").as_string().as_deref(),
        Some("ydr")
    );
    assert_eq!(
        field(&resolution, "modelDependencyIndex").as_f64(),
        Some(1.0)
    );
    assert_eq!(Array::from(&field(&report, "diagnostics")).length(), 0);

    let missing_provider =
        resolve_supplied_ymap_scene(YMAP, Array::new().into()).expect("missing provider report");
    let diagnostics = Array::from(&field(&missing_provider, "diagnostics"));
    assert_eq!(diagnostics.length(), 1);
    assert_eq!(
        field(&diagnostics.get(0), "code").as_string().as_deref(),
        Some("providerMissing")
    );

    let missing_model = Array::new();
    missing_model.push(&supplied_file("simple.ytyp", SIMPLE_YTYP));
    let missing_model =
        resolve_supplied_ymap_scene(YMAP, missing_model.into()).expect("missing model report");
    let diagnostics = Array::from(&field(&missing_model, "diagnostics"));
    assert_eq!(diagnostics.length(), 1);
    assert_eq!(
        field(&diagnostics.get(0), "code").as_string().as_deref(),
        Some("assetMissing")
    );

    let ambiguous = Array::new();
    ambiguous.push(&supplied_file("simple.ytyp", SIMPLE_YTYP));
    ambiguous.push(&supplied_file("duplicate.ytyp", SIMPLE_YTYP));
    ambiguous.push(&supplied_file("test_drawable.ydr", YDR));
    let ambiguous =
        resolve_supplied_ymap_scene(YMAP, ambiguous.into()).expect("ambiguous provider report");
    let diagnostics = Array::from(&field(&ambiguous, "diagnostics"));
    assert_eq!(diagnostics.length(), 1);
    assert_eq!(
        field(&diagnostics.get(0), "code").as_string().as_deref(),
        Some("providerAmbiguous")
    );
}

#[wasm_bindgen_test]
fn ymap_safe_write_returns_semantically_valid_bytes() {
    let rewritten = ymap_set_flags(YMAP, 0, 17).expect("set YMAP flags");
    assert!(rewritten.length() > 0);
    validate_asset("ymap", &rewritten.to_vec()).expect("reopen edited YMAP");
}

#[wasm_bindgen_test]
fn material_safe_writes_reopen_for_ydr_and_ydd() {
    let ydr = ydr_rebind_texture(EDITABLE_YDR, 0, 0, 0, 1).expect("rebind existing YDR texture");
    validate_asset("ydr", &ydr.to_vec()).expect("reopen edited YDR");

    let ydd = ydd_rebind_texture(YDD, 0, 0, 0, 0, 1).expect("rebind existing YDD texture");
    validate_asset("ydd", &ydd.to_vec()).expect("reopen edited YDD");
}

#[wasm_bindgen_test]
fn capability_and_error_schema_fail_closed_in_js() {
    let report = capabilities().expect("capabilities");
    let desktop_only = Array::from(&field(&report, "desktopOnly"));
    assert!(desktop_only
        .iter()
        .filter_map(|value| value.as_string())
        .any(|value| value == "rpf"));
    assert!(desktop_only
        .iter()
        .filter_map(|value| value.as_string())
        .any(|value| value == "gtaKeys"));

    let error = validate_asset("rpf", &[]).expect_err("RPF must remain desktop-only");
    assert_eq!(
        field(&error, "schema").as_string().as_deref(),
        Some("ragelab.wasm.error")
    );
    assert_eq!(
        field(&error, "code").as_string().as_deref(),
        Some("unsupportedFormat")
    );
}

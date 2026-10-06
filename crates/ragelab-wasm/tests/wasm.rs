#![cfg(target_arch = "wasm32")]

use js_sys::{Array, Reflect};
use ragelab_resource::Rsc7Resource;
use ragelab_wasm::{
    capabilities, inspect_ydd, inspect_ydr_materials, inspect_yft, inspect_ymap, inspect_ytd,
    inspect_ytyp, validate_asset, wasm_export_ytd_texture_png, wasm_replace_ytd_texture_png,
    ydd_model, ydd_rebind_texture, ydr_model, ydr_rebind_texture, yft_model, ymap_set_flags,
    ytd_texture,
};
use ragelab_yft::YFT_LEGACY_VERSION;
use wasm_bindgen::JsValue;
use wasm_bindgen_test::*;

const YMAP: &[u8] = include_bytes!("../../../fixtures/synthetic/stream/simple.ymap");
const YTYP: &[u8] = include_bytes!("../../../fixtures/synthetic/mlo.ytyp");
const YDR: &[u8] = include_bytes!("../../../fixtures/synthetic/ydr/simple.ydr");
const EDITABLE_YDR: &[u8] = include_bytes!("../../../fixtures/synthetic/ydr/editable.ydr");
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
fn shared_ytd_png_operation_roundtrips_in_wasm() {
    let png = wasm_export_ytd_texture_png(YTD, 0).expect("export PNG");
    let png_bytes = png.to_vec();
    let rewritten =
        wasm_replace_ytd_texture_png(YTD, 0, &png_bytes).expect("replace PNG through shared core");
    assert!(rewritten.length() > 0);
    validate_asset("ytd", &rewritten.to_vec()).expect("semantic reopen through WASM");
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

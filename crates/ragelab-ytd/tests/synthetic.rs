use ragelab_hash::joaat;
use ragelab_resource::{decode_page_count, Rsc7Resource, GRAPHICS_BASE, SYSTEM_BASE};
use ragelab_ytd::{TextureFormat, Ytd, YtdError};

const SIMPLE_YTD: &[u8] = include_bytes!("../../../fixtures/synthetic/ytd/simple.ytd");
const SYNTHETIC_TEXTURE_POINTER: u64 = SYSTEM_BASE + 0x80;

fn synthetic_bc1_ytd() -> Vec<u8> {
    let mut resource = Rsc7Resource::parse(SIMPLE_YTD).expect("parse synthetic resource");
    resource
        .bytes_at_mut(SYNTHETIC_TEXTURE_POINTER + 0x56, 2)
        .expect("texture stride")
        .copy_from_slice(&2_u16.to_le_bytes());
    resource
        .bytes_at_mut(SYNTHETIC_TEXTURE_POINTER + 0x58, 4)
        .expect("texture format")
        .copy_from_slice(&0x3154_5844_u32.to_le_bytes());

    let mut red_block = [0_u8; 8];
    red_block[0..2].copy_from_slice(&0xF800_u16.to_le_bytes());
    red_block[2..4].copy_from_slice(&0x07E0_u16.to_le_bytes());
    resource
        .bytes_at_mut(GRAPHICS_BASE, red_block.len())
        .expect("graphics block")
        .copy_from_slice(&red_block);
    resource.to_bytes().expect("encode synthetic BC1 YTD")
}

fn synthetic_bc3_ytd() -> Vec<u8> {
    let mut resource = Rsc7Resource::parse(SIMPLE_YTD).expect("parse synthetic resource");
    resource
        .bytes_at_mut(SYNTHETIC_TEXTURE_POINTER + 0x56, 2)
        .expect("texture stride")
        .copy_from_slice(&4_u16.to_le_bytes());
    resource
        .bytes_at_mut(SYNTHETIC_TEXTURE_POINTER + 0x58, 4)
        .expect("texture format")
        .copy_from_slice(&0x3554_5844_u32.to_le_bytes());

    let mut blue_block = [0_u8; 16];
    blue_block[0] = 255;
    blue_block[1] = 0;
    blue_block[8..10].copy_from_slice(&0x001F_u16.to_le_bytes());
    resource
        .bytes_at_mut(GRAPHICS_BASE, blue_block.len())
        .expect("graphics block")
        .copy_from_slice(&blue_block);
    resource.to_bytes().expect("encode synthetic BC3 YTD")
}

fn synthetic_two_texture_ytd() -> Vec<u8> {
    const SECOND_TEXTURE_POINTER: u64 = SYSTEM_BASE + 0x160;
    const SECOND_NAME_POINTER: u64 = SYSTEM_BASE + 0x154;
    const SECOND_DATA_POINTER: u64 = GRAPHICS_BASE + 0x40;
    const SECOND_NAME: &str = "second";

    let mut resource = Rsc7Resource::parse(SIMPLE_YTD).expect("parse synthetic resource");
    let template = resource
        .bytes_at(SYNTHETIC_TEXTURE_POINTER, 0x90)
        .expect("read source texture block")
        .to_vec();
    resource
        .bytes_at_mut(SECOND_TEXTURE_POINTER, template.len())
        .expect("second texture block")
        .copy_from_slice(&template);

    resource
        .bytes_at_mut(SYSTEM_BASE + 0x28, 2)
        .expect("hash count")
        .copy_from_slice(&2_u16.to_le_bytes());
    resource
        .bytes_at_mut(SYSTEM_BASE + 0x2A, 2)
        .expect("hash capacity")
        .copy_from_slice(&2_u16.to_le_bytes());
    resource
        .bytes_at_mut(SYSTEM_BASE + 0x38, 2)
        .expect("texture count")
        .copy_from_slice(&2_u16.to_le_bytes());
    resource
        .bytes_at_mut(SYSTEM_BASE + 0x3A, 2)
        .expect("texture capacity")
        .copy_from_slice(&2_u16.to_le_bytes());
    resource
        .bytes_at_mut(SYSTEM_BASE + 0x44, 4)
        .expect("second hash")
        .copy_from_slice(&joaat(SECOND_NAME).to_le_bytes());
    resource
        .bytes_at_mut(SYSTEM_BASE + 0x50, 8)
        .expect("second texture pointer")
        .copy_from_slice(&SECOND_TEXTURE_POINTER.to_le_bytes());

    let encoded_name = b"second\0";
    resource
        .bytes_at_mut(SECOND_NAME_POINTER, encoded_name.len())
        .expect("second texture name")
        .copy_from_slice(encoded_name);
    resource
        .bytes_at_mut(SECOND_TEXTURE_POINTER + 0x28, 8)
        .expect("second name pointer")
        .copy_from_slice(&SECOND_NAME_POINTER.to_le_bytes());
    resource
        .bytes_at_mut(SECOND_TEXTURE_POINTER + 0x70, 8)
        .expect("second data pointer")
        .copy_from_slice(&SECOND_DATA_POINTER.to_le_bytes());

    let second_payload = [77_u8; 64];
    resource
        .bytes_at_mut(SECOND_DATA_POINTER, second_payload.len())
        .expect("second texture payload")
        .copy_from_slice(&second_payload);
    resource
        .to_bytes()
        .expect("encode synthetic two-texture YTD")
}

#[test]
fn parses_synthetic_legacy_ytd_end_to_end() {
    let ytd = Ytd::from_bytes(SIMPLE_YTD).expect("parse synthetic YTD");

    assert_eq!(ytd.resource_version, 13);
    assert_eq!(ytd.file_unknown, 1);
    assert_eq!(ytd.pages_info_pointer, SYSTEM_BASE + 0x140);
    assert_eq!(ytd.textures.len(), 1);

    let texture = &ytd.textures[0];
    assert_eq!(texture.dictionary_hash, 0x5245_AB9A);
    assert_eq!(texture.name, "synthetic_diffuse");
    assert_eq!(texture.name_hash, texture.dictionary_hash);
    assert!(texture.dictionary_hash_matches_name());
    assert_eq!((texture.width, texture.height, texture.depth), (4, 4, 1));
    assert_eq!(texture.stride, 16);
    assert_eq!(texture.format, TextureFormat::Rgba8);
    assert_eq!(texture.levels, 1);
    assert_eq!(texture.usage, 20);
    assert_eq!(texture.usage_flags, 0);
    assert_eq!(texture.data_length, 64);
    assert_eq!(texture.data_pointer, 0x6000_0000);
}

#[test]
fn compact_rebuild_preserves_semantics_payload_and_is_deterministic() {
    let before = Ytd::from_bytes(SIMPLE_YTD).expect("parse source YTD");
    let before_payload = Ytd::encoded_texture(SIMPLE_YTD, 0)
        .expect("read source payload")
        .data;

    let rebuilt = Ytd::rebuild_legacy_compact(SIMPLE_YTD).expect("compact rebuild YTD");
    let after = Ytd::from_bytes(&rebuilt).expect("reparse compact YTD");
    assert_eq!(after.resource_version, before.resource_version);
    assert_eq!(after.file_vft, before.file_vft);
    assert_eq!(after.file_unknown, before.file_unknown);
    assert_eq!(after.textures.len(), 1);
    let left = &before.textures[0];
    let right = &after.textures[0];
    assert_eq!(right.dictionary_hash, left.dictionary_hash);
    assert_eq!(right.name, left.name);
    assert_eq!(right.name_hash, left.name_hash);
    assert_eq!(
        (right.width, right.height, right.depth),
        (left.width, left.height, left.depth)
    );
    assert_eq!(right.stride, left.stride);
    assert_eq!(right.format, left.format);
    assert_eq!(right.levels, left.levels);
    assert_eq!(right.usage, left.usage);
    assert_eq!(right.usage_flags, left.usage_flags);
    assert_eq!(right.extra_flags, left.extra_flags);
    assert_eq!(right.data_length, left.data_length);
    assert_eq!(
        Ytd::encoded_texture(&rebuilt, 0)
            .expect("read compact payload")
            .data,
        before_payload
    );

    let rebuilt_again =
        Ytd::rebuild_legacy_compact(&rebuilt).expect("repeat compact rebuild deterministically");
    assert_eq!(rebuilt_again, rebuilt);
}

#[test]
fn compact_rebuild_preserves_two_texture_identity_and_payloads() {
    let source = synthetic_two_texture_ytd();
    let before = Ytd::from_bytes(&source).expect("parse two-texture source");
    let before_payloads = (0..before.textures.len())
        .map(|index| {
            Ytd::encoded_texture(&source, index)
                .expect("read source texture payload")
                .data
        })
        .collect::<Vec<_>>();

    let rebuilt = Ytd::rebuild_legacy_compact(&source).expect("compact two-texture YTD");
    let after = Ytd::from_bytes(&rebuilt).expect("reparse compact two-texture YTD");
    assert_eq!(after.textures.len(), before.textures.len());
    for (index, (left, right)) in before
        .textures
        .iter()
        .zip(after.textures.iter())
        .enumerate()
    {
        assert_eq!(right.dictionary_hash, left.dictionary_hash);
        assert_eq!(right.name, left.name);
        assert_eq!(right.name_hash, left.name_hash);
        assert_eq!(
            (right.width, right.height, right.depth),
            (left.width, left.height, left.depth)
        );
        assert_eq!(right.stride, left.stride);
        assert_eq!(right.format, left.format);
        assert_eq!(right.levels, left.levels);
        assert_eq!(right.usage, left.usage);
        assert_eq!(right.usage_flags, left.usage_flags);
        assert_eq!(right.extra_flags, left.extra_flags);
        assert_eq!(right.data_length, left.data_length);
        assert_eq!(
            Ytd::encoded_texture(&rebuilt, index)
                .expect("read compact texture payload")
                .data,
            before_payloads[index]
        );
    }
}

#[test]
fn compact_rebuild_rejects_unmodeled_descriptor_pointer() {
    let mut resource = Rsc7Resource::parse(SIMPLE_YTD).expect("parse synthetic resource");
    resource
        .bytes_at_mut(SYNTHETIC_TEXTURE_POINTER + 0x08, 8)
        .expect("opaque descriptor pointer slot")
        .copy_from_slice(&(SYSTEM_BASE + 0x1A0).to_le_bytes());
    let source = resource.to_bytes().expect("encode pointer-bearing fixture");

    let error = Ytd::rebuild_legacy_compact(&source)
        .expect_err("unmodeled descriptor resource pointers must fail closed");
    assert!(matches!(error, YtdError::Malformed(_)));
    assert!(error.to_string().contains("unmodeled resource pointer"));
}

#[test]
fn compact_rebuild_aligns_layout_and_page_info() {
    let source = synthetic_two_texture_ytd();
    let rebuilt = Ytd::rebuild_legacy_compact(&source).expect("compact two-texture YTD");
    let resource = Rsc7Resource::parse(&rebuilt).expect("parse compact resource");
    let parsed = Ytd::from_bytes(&rebuilt).expect("parse compact YTD");

    let hash_list = resource.read_u64(SYSTEM_BASE + 0x20).unwrap();
    let pointer_list = resource.read_u64(SYSTEM_BASE + 0x30).unwrap();
    assert_eq!((hash_list - SYSTEM_BASE) % 16, 0);
    assert_eq!((pointer_list - SYSTEM_BASE) % 16, 0);
    assert_eq!(resource.read_u16(SYSTEM_BASE + 0x28).unwrap(), 2);
    assert_eq!(resource.read_u16(SYSTEM_BASE + 0x2A).unwrap(), 2);
    assert_eq!(resource.read_u16(SYSTEM_BASE + 0x38).unwrap(), 2);
    assert_eq!(resource.read_u16(SYSTEM_BASE + 0x3A).unwrap(), 2);

    for index in 0..2_u64 {
        let descriptor = resource.read_u64(pointer_list + index * 8).unwrap();
        assert_eq!((descriptor - SYSTEM_BASE) % 16, 0);
        assert_ne!(descriptor, SYNTHETIC_TEXTURE_POINTER);
    }
    for texture in &parsed.textures {
        assert_eq!((texture.data_pointer - GRAPHICS_BASE) % 16, 0);
    }

    let page_info = parsed.pages_info_pointer;
    assert_eq!((page_info - SYSTEM_BASE) % 16, 0);
    assert_eq!(
        usize::from(resource.read_u8(page_info + 0x08).unwrap()),
        decode_page_count(resource.header.system_flags)
    );
    assert_eq!(
        usize::from(resource.read_u8(page_info + 0x09).unwrap()),
        decode_page_count(resource.header.graphics_flags)
    );
}

#[test]
fn compact_rebuild_preserves_duplicate_name_hash_entries_independently() {
    let source = synthetic_two_texture_ytd();
    let mut resource = Rsc7Resource::parse(&source).expect("parse two-texture resource");
    let first_hash = resource.read_u32(SYSTEM_BASE + 0x40).unwrap();
    let first_name_pointer = resource.read_u64(SYNTHETIC_TEXTURE_POINTER + 0x28).unwrap();
    resource
        .bytes_at_mut(SYSTEM_BASE + 0x44, 4)
        .unwrap()
        .copy_from_slice(&first_hash.to_le_bytes());
    resource
        .bytes_at_mut(SYSTEM_BASE + 0x160 + 0x28, 8)
        .unwrap()
        .copy_from_slice(&first_name_pointer.to_le_bytes());
    let duplicate_source = resource.to_bytes().expect("encode duplicate dictionary");
    let before_second_payload = Ytd::encoded_texture(&duplicate_source, 1)
        .expect("read duplicate second payload")
        .data;

    let rebuilt =
        Ytd::rebuild_legacy_compact(&duplicate_source).expect("rebuild duplicate dictionary");
    let after = Ytd::from_bytes(&rebuilt).expect("parse rebuilt duplicate dictionary");
    assert_eq!(after.textures.len(), 2);
    assert_eq!(
        after.textures[0].dictionary_hash,
        after.textures[1].dictionary_hash
    );
    assert_eq!(after.textures[0].name, after.textures[1].name);
    assert_eq!(
        Ytd::encoded_texture(&rebuilt, 1)
            .expect("read rebuilt duplicate second payload")
            .data,
        before_second_payload
    );
}

#[test]
fn compact_rebuild_rejects_corrupt_list_count_mismatch() {
    let mut resource = Rsc7Resource::parse(SIMPLE_YTD).expect("parse synthetic resource");
    resource
        .bytes_at_mut(SYSTEM_BASE + 0x28, 2)
        .unwrap()
        .copy_from_slice(&2_u16.to_le_bytes());
    resource
        .bytes_at_mut(SYSTEM_BASE + 0x2A, 2)
        .unwrap()
        .copy_from_slice(&2_u16.to_le_bytes());
    let corrupt = resource.to_bytes().expect("encode corrupt list fixture");

    let error = Ytd::rebuild_legacy_compact(&corrupt)
        .expect_err("hash/pointer list count mismatch must fail before serialization");
    assert!(matches!(error, YtdError::Malformed(_)));
    assert!(error.to_string().contains("hash count"));
}

#[test]
fn decodes_synthetic_rgba8_top_mip() {
    let decoded = Ytd::decode_top_mip_rgba(SIMPLE_YTD, 0).expect("decode synthetic texture");
    assert_eq!((decoded.width, decoded.height), (4, 4));
    assert_eq!(decoded.rgba.len(), 64);
    assert_eq!(&decoded.rgba[0..4], &[0, 0, 255, 255]);
    assert_eq!(&decoded.rgba[60..64], &[192, 192, 159, 255]);
}

#[test]
fn exports_synthetic_rgba8_as_classic_dds() {
    let encoded = Ytd::encoded_texture(SIMPLE_YTD, 0).expect("read encoded texture");
    let dds = Ytd::texture_dds(SIMPLE_YTD, 0).expect("export synthetic DDS");

    assert_eq!(encoded.data.len(), 64);
    assert_eq!(dds.len(), 128 + encoded.data.len());
    assert_eq!(&dds[0..4], b"DDS ");
    assert_eq!(u32::from_le_bytes(dds[12..16].try_into().unwrap()), 4);
    assert_eq!(u32::from_le_bytes(dds[16..20].try_into().unwrap()), 4);
    assert_eq!(u32::from_le_bytes(dds[20..24].try_into().unwrap()), 16);
    assert_eq!(u32::from_le_bytes(dds[88..92].try_into().unwrap()), 32);
    assert_eq!(
        u32::from_le_bytes(dds[92..96].try_into().unwrap()),
        0x0000_00FF
    );
    assert_eq!(
        u32::from_le_bytes(dds[96..100].try_into().unwrap()),
        0x0000_FF00
    );
    assert_eq!(
        u32::from_le_bytes(dds[100..104].try_into().unwrap()),
        0x00FF_0000
    );
    assert_eq!(
        u32::from_le_bytes(dds[104..108].try_into().unwrap()),
        0xFF00_0000
    );
    assert_eq!(&dds[128..], encoded.data);
}

#[test]
fn replaces_synthetic_texture_from_matching_dds_without_changing_metadata() {
    let before = Ytd::from_bytes(SIMPLE_YTD).expect("parse source YTD");
    let mut dds = Ytd::texture_dds(SIMPLE_YTD, 0).expect("export source DDS");
    dds[128..132].copy_from_slice(&[9, 8, 7, 6]);

    let rewritten = Ytd::replace_texture_from_dds(SIMPLE_YTD, 0, &dds)
        .expect("replace matching texture payload");
    let after = Ytd::from_bytes(&rewritten).expect("reparse rewritten YTD");
    assert_eq!(
        after, before,
        "layout-preserving replacement changed metadata"
    );

    let decoded = Ytd::decode_top_mip_rgba(&rewritten, 0).expect("decode replaced texture");
    assert_eq!(&decoded.rgba[0..4], &[9, 8, 7, 6]);
    let encoded = Ytd::encoded_texture(&rewritten, 0).expect("read replaced payload");
    assert_eq!(encoded.data, dds[128..]);
}

#[test]
fn relocates_resized_texture_and_updates_legacy_metadata() {
    let before = Ytd::from_bytes(SIMPLE_YTD).expect("parse source YTD");
    let source_texture = before.textures[0].clone();
    let mut dds = Ytd::texture_dds(SIMPLE_YTD, 0).expect("export source DDS");

    dds[12..16].copy_from_slice(&8_u32.to_le_bytes());
    dds[16..20].copy_from_slice(&8_u32.to_le_bytes());
    dds[20..24].copy_from_slice(&32_u32.to_le_bytes());
    dds[28..32].copy_from_slice(&2_u32.to_le_bytes());
    dds.truncate(128);
    let replacement_payload = (0..320)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    dds.extend_from_slice(&replacement_payload);

    let rewritten = Ytd::replace_texture_from_dds_relocated(SIMPLE_YTD, 0, &dds)
        .expect("relocate resized texture payload");
    let after = Ytd::from_bytes(&rewritten).expect("reparse relocated YTD");
    let texture = &after.textures[0];

    assert_eq!(texture.name, source_texture.name);
    assert_eq!(texture.dictionary_hash, source_texture.dictionary_hash);
    assert_eq!(texture.usage, source_texture.usage);
    assert_eq!(texture.usage_flags, source_texture.usage_flags);
    assert_eq!(texture.extra_flags, source_texture.extra_flags);
    assert_eq!((texture.width, texture.height, texture.depth), (8, 8, 1));
    assert_eq!(texture.stride, 32);
    assert_eq!(texture.format, TextureFormat::Rgba8);
    assert_eq!(texture.levels, 2);
    assert_eq!(texture.data_length, 320);
    assert_eq!(texture.data_pointer, 0x6000_0200);

    let resource = Rsc7Resource::parse(&rewritten).expect("parse relocated resource envelope");
    assert_eq!(decode_page_count(resource.header.graphics_flags), 1);
    assert_eq!(resource.read_u8(SYSTEM_BASE + 0x149).unwrap(), 1);

    let encoded = Ytd::encoded_texture(&rewritten, 0).expect("read relocated payload");
    assert_eq!(encoded.data, replacement_payload);
    let decoded = Ytd::decode_top_mip_rgba(&rewritten, 0).expect("decode resized top mip");
    assert_eq!((decoded.width, decoded.height), (8, 8));
    assert_eq!(decoded.rgba, replacement_payload[..256]);
}

#[test]
fn relocated_replacement_preserves_other_texture_metadata_and_payload() {
    let source = synthetic_two_texture_ytd();
    let before = Ytd::from_bytes(&source).expect("parse two-texture YTD");
    assert_eq!(before.textures.len(), 2);
    assert_eq!(before.textures[1].name, "second");
    let second_before = before.textures[1].clone();
    let second_payload_before = Ytd::encoded_texture(&source, 1)
        .expect("read second payload before repack")
        .data;

    let mut dds = Ytd::texture_dds(&source, 0).expect("export first texture DDS");
    dds[12..16].copy_from_slice(&8_u32.to_le_bytes());
    dds[16..20].copy_from_slice(&8_u32.to_le_bytes());
    dds[20..24].copy_from_slice(&32_u32.to_le_bytes());
    dds.truncate(128);
    dds.extend((0..256).map(|index| (index % 239) as u8));

    let rewritten = Ytd::replace_texture_from_dds_relocated(&source, 0, &dds)
        .expect("repack first texture in two-texture dictionary");
    let after = Ytd::from_bytes(&rewritten).expect("reparse two-texture YTD");
    assert_eq!(after.textures.len(), 2);
    assert_eq!(after.textures[1], second_before);
    assert_eq!(
        Ytd::encoded_texture(&rewritten, 1)
            .expect("read second payload after repack")
            .data,
        second_payload_before
    );
    assert_eq!((after.textures[0].width, after.textures[0].height), (8, 8));
}

#[test]
fn repacks_rgba8_texture_from_generated_full_mip_chain() {
    let top = vec![
        12_u8, 34, 56, 255, 12, 34, 56, 255, 12, 34, 56, 255, 12, 34, 56, 255, 12, 34, 56, 255, 12,
        34, 56, 255, 12, 34, 56, 255, 12, 34, 56, 255, 12, 34, 56, 255, 12, 34, 56, 255, 12, 34,
        56, 255, 12, 34, 56, 255, 12, 34, 56, 255, 12, 34, 56, 255, 12, 34, 56, 255, 12, 34, 56,
        255,
    ];
    let dds = Ytd::rgba8_dds_with_generated_mips(4, 4, &top)
        .expect("generate deterministic RGBA8 mip chain");
    assert_eq!(u32::from_le_bytes(dds[28..32].try_into().unwrap()), 3);

    let rewritten = Ytd::replace_texture_from_dds_relocated(SIMPLE_YTD, 0, &dds)
        .expect("repack generated RGBA8 mip chain");
    let after = Ytd::from_bytes(&rewritten).expect("reparse generated-mip YTD");
    let texture = &after.textures[0];
    assert_eq!((texture.width, texture.height), (4, 4));
    assert_eq!(texture.stride, 16);
    assert_eq!(texture.format, TextureFormat::Rgba8);
    assert_eq!(texture.levels, 3);
    assert_eq!(texture.data_length, 64 + 16 + 4);

    let encoded = Ytd::encoded_texture(&rewritten, 0).expect("read generated mip payload");
    assert_eq!(&encoded.data[..64], top.as_slice());
    assert!(encoded.data[64..]
        .chunks_exact(4)
        .all(|pixel| pixel == [12, 34, 56, 255]));
}

#[test]
fn repacks_bc1_texture_directly_from_rgba_with_generated_mips() {
    let source = synthetic_bc1_ytd();
    let rgba = [255_u8, 0, 0, 255].repeat(8 * 8);

    let rewritten = Ytd::replace_texture_from_rgba_relocated(&source, 0, 8, 8, &rgba)
        .expect("repack BC1 directly from RGBA");
    let repeated = Ytd::replace_texture_from_rgba_relocated(&source, 0, 8, 8, &rgba)
        .expect("repeat BC1 RGBA repack deterministically");
    assert_eq!(rewritten, repeated);
    let after = Ytd::from_bytes(&rewritten).expect("reparse RGBA->BC1 YTD");
    let texture = &after.textures[0];
    assert_eq!((texture.width, texture.height), (8, 8));
    assert_eq!(texture.format, TextureFormat::Bc1);
    assert_eq!(texture.levels, 4);
    assert_eq!(texture.data_length, 32 + 8 + 8 + 8);

    let decoded = Ytd::decode_top_mip_rgba(&rewritten, 0).expect("decode RGBA->BC1 top mip");
    assert_eq!(decoded.rgba, rgba);
}

#[test]
fn repacks_bc3_texture_directly_from_rgba_with_generated_mips() {
    let source = synthetic_bc3_ytd();
    let rgba = [0_u8, 0, 255, 128].repeat(8 * 8);

    let rewritten = Ytd::replace_texture_from_rgba_relocated(&source, 0, 8, 8, &rgba)
        .expect("repack BC3 directly from RGBA");
    let after = Ytd::from_bytes(&rewritten).expect("reparse RGBA->BC3 YTD");
    let texture = &after.textures[0];
    assert_eq!((texture.width, texture.height), (8, 8));
    assert_eq!(texture.format, TextureFormat::Bc3);
    assert_eq!(texture.levels, 4);
    assert_eq!(texture.data_length, 64 + 16 + 16 + 16);

    let decoded = Ytd::decode_top_mip_rgba(&rewritten, 0).expect("decode RGBA->BC3 top mip");
    assert_eq!(decoded.rgba, rgba);
}

#[test]
fn relocated_rgba_repack_preserves_dictionary_identity_and_unrelated_texture() {
    let source = synthetic_two_texture_ytd();
    let before = Ytd::from_bytes(&source).expect("parse two-texture source");
    let target_before = before.textures[0].clone();
    let unrelated_before = before.textures[1].clone();
    let unrelated_payload_before = Ytd::encoded_texture(&source, 1)
        .expect("read unrelated payload before RGBA repack")
        .data;
    let rgba = [11_u8, 22, 33, 44].repeat(8 * 4);

    let rewritten = Ytd::replace_texture_from_rgba_relocated(&source, 0, 8, 4, &rgba)
        .expect("repack first texture directly from RGBA");
    let after = Ytd::from_bytes(&rewritten).expect("reparse RGBA-repacked two-texture YTD");
    let target_after = &after.textures[0];

    assert_eq!(before.resource_version, after.resource_version);
    assert_eq!(before.file_vft, after.file_vft);
    assert_eq!(before.file_unknown, after.file_unknown);
    assert_eq!(before.pages_info_pointer, after.pages_info_pointer);
    assert_eq!(before.textures.len(), after.textures.len());
    assert_eq!(target_before.dictionary_hash, target_after.dictionary_hash);
    assert_eq!(target_before.name, target_after.name);
    assert_eq!(target_before.name_hash, target_after.name_hash);
    assert_eq!(target_before.depth, target_after.depth);
    assert_eq!(target_before.format, target_after.format);
    assert_eq!(target_before.usage, target_after.usage);
    assert_eq!(target_before.usage_flags, target_after.usage_flags);
    assert_eq!(target_before.extra_flags, target_after.extra_flags);
    assert_eq!((target_after.width, target_after.height), (8, 4));
    assert_eq!(target_after.stride, 32);
    assert_eq!(target_after.levels, 4);
    assert_eq!(target_after.data_length, 128 + 32 + 8 + 4);
    assert_eq!(after.textures[1], unrelated_before);
    assert_eq!(
        Ytd::encoded_texture(&rewritten, 1)
            .expect("read unrelated payload after RGBA repack")
            .data,
        unrelated_payload_before
    );
    assert_eq!(
        Ytd::decode_top_mip_rgba(&rewritten, 0)
            .expect("decode RGBA-repacked target")
            .rgba,
        rgba
    );
}

#[test]
fn relocated_rgba_repack_rejects_oversized_dimensions_before_allocation() {
    let error = Ytd::replace_texture_from_rgba_relocated(SIMPLE_YTD, 0, u16::MAX, u16::MAX, &[])
        .expect_err("oversized RGBA image must be rejected");
    assert!(matches!(error, YtdError::InvalidRgba(_)));
}

#[test]
fn relocated_rgba_repack_rejects_truncated_source() {
    let rgba = [255_u8, 0, 0, 255].repeat(4 * 4);
    let error = Ytd::replace_texture_from_rgba_relocated(&SIMPLE_YTD[..32], 0, 4, 4, &rgba)
        .expect_err("truncated source must be rejected before repack");
    assert!(matches!(error, YtdError::Resource(_)));
}

#[test]
fn relocated_rgba_repack_rejects_unsupported_target_format() {
    let mut resource = Rsc7Resource::parse(SIMPLE_YTD).expect("parse synthetic resource");
    resource
        .bytes_at_mut(SYNTHETIC_TEXTURE_POINTER + 0x58, 4)
        .expect("texture format")
        .copy_from_slice(&0x3354_5844_u32.to_le_bytes());
    let source = resource.to_bytes().expect("encode synthetic BC2 YTD");
    let rgba = [255_u8, 0, 0, 255].repeat(4 * 4);

    let error = Ytd::replace_texture_from_rgba_relocated(&source, 0, 4, 4, &rgba)
        .expect_err("BC2 target must be rejected by RGBA repack");
    assert!(matches!(
        error,
        YtdError::UnsupportedTextureFormat(TextureFormat::Bc2)
    ));
}

#[test]
fn relocates_resized_bc1_texture_with_legacy_compressed_stride() {
    let source = synthetic_bc1_ytd();
    let parsed = Ytd::from_bytes(&source).expect("parse synthetic BC1 YTD");
    assert_eq!(parsed.textures[0].format, TextureFormat::Bc1);
    assert_eq!(parsed.textures[0].stride, 2);
    assert_eq!(parsed.textures[0].data_length, 8);

    let mut dds = Ytd::texture_dds(&source, 0).expect("export BC1 DDS");
    dds[12..16].copy_from_slice(&8_u32.to_le_bytes());
    dds[16..20].copy_from_slice(&8_u32.to_le_bytes());
    dds[20..24].copy_from_slice(&32_u32.to_le_bytes());
    dds.truncate(128);

    let mut red_block = [0_u8; 8];
    red_block[0..2].copy_from_slice(&0xF800_u16.to_le_bytes());
    red_block[2..4].copy_from_slice(&0x07E0_u16.to_le_bytes());
    let replacement_payload = red_block.repeat(4);
    dds.extend_from_slice(&replacement_payload);

    let rewritten = Ytd::replace_texture_from_dds_relocated(&source, 0, &dds)
        .expect("relocate resized BC1 payload");
    let after = Ytd::from_bytes(&rewritten).expect("reparse resized BC1 YTD");
    let texture = &after.textures[0];
    assert_eq!((texture.width, texture.height), (8, 8));
    assert_eq!(texture.stride, 4);
    assert_eq!(texture.format, TextureFormat::Bc1);
    assert_eq!(texture.levels, 1);
    assert_eq!(texture.data_length, 32);
    assert_eq!(texture.data_pointer, 0x6000_0200);

    let encoded = Ytd::encoded_texture(&rewritten, 0).expect("read resized BC1 payload");
    assert_eq!(encoded.data, replacement_payload);
    let decoded = Ytd::decode_top_mip_rgba(&rewritten, 0).expect("decode resized BC1 texture");
    assert_eq!((decoded.width, decoded.height), (8, 8));
    assert!(decoded
        .rgba
        .chunks_exact(4)
        .all(|pixel| pixel == [255, 0, 0, 255]));
}

#[test]
fn rejects_replacement_dds_with_different_dimensions() {
    let mut dds = Ytd::texture_dds(SIMPLE_YTD, 0).expect("export source DDS");
    dds[12..16].copy_from_slice(&8_u32.to_le_bytes());
    dds[16..20].copy_from_slice(&2_u32.to_le_bytes());

    let error = Ytd::replace_texture_from_dds(SIMPLE_YTD, 0, &dds)
        .expect_err("different dimensions must be rejected");
    assert!(matches!(error, YtdError::ReplacementMismatch(_)));
}

#[test]
fn rejects_truncated_replacement_dds_payload() {
    let mut dds = Ytd::texture_dds(SIMPLE_YTD, 0).expect("export source DDS");
    dds.pop();

    let error = Ytd::replace_texture_from_dds(SIMPLE_YTD, 0, &dds)
        .expect_err("truncated DDS must be rejected");
    assert!(matches!(error, YtdError::InvalidDds(_)));
}

#[test]
fn rejects_gen9_version_until_separate_layout_is_implemented() {
    let mut bytes = SIMPLE_YTD.to_vec();
    bytes[4..8].copy_from_slice(&5_u32.to_le_bytes());

    let error = Ytd::from_bytes(&bytes).expect_err("version 5 must not use legacy layout");
    assert!(matches!(error, YtdError::UnsupportedResourceVersion(5)));
}

#[test]
fn rejects_truncated_resource() {
    let error = Ytd::from_bytes(&SIMPLE_YTD[..32]).expect_err("truncated fixture must fail");
    assert!(matches!(error, YtdError::Resource(_)));
}

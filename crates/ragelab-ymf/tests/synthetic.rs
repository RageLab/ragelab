use ragelab_meta::MetaHash;
use ragelab_ymf::Ymf;

const SYNTHETIC_YMF: &[u8] = include_bytes!("../../../fixtures/synthetic/_manifest.ymf");

#[test]
fn parses_synthetic_ymf_end_to_end() {
    let manifest = Ymf::from_bytes(SYNTHETIC_YMF).expect("synthetic YMF should parse");

    assert_eq!(manifest.maps.len(), 1);
    assert_eq!(manifest.maps[0].ymap, MetaHash(0x6C3A_E62A));
    assert_eq!(manifest.maps[0].ytyps, vec![MetaHash(0xAA2B_639B)]);
    assert!(manifest.ytyps.is_empty());
    assert!(manifest.interiors.is_empty());
}

#[test]
fn rewrites_synthetic_ymf_semantically() {
    let manifest = Ymf::from_bytes(SYNTHETIC_YMF).expect("synthetic YMF should parse");
    let rewritten = manifest.to_bytes().expect("synthetic YMF should write");
    let reparsed = Ymf::from_bytes(&rewritten).expect("rewritten YMF should parse");

    assert_eq!(reparsed, manifest);
}

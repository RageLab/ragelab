//! Jenkins hash utilities used by GTA V/RAGE resources.
//!
//! Two variants are deliberately exposed because GTA tooling uses both
//! conventions:
//!
//! - [`jenkins`] hashes bytes exactly as supplied and is required for META
//!   type/field names such as `CMapData` and `archetypeName`.
//! - [`joaat`] normalizes ASCII to lowercase first and is convenient for the
//!   common case-insensitive asset-name convention.

/// Computes Jenkins one-at-a-time over the bytes exactly as supplied.
pub fn jenkins(name: &str) -> u32 {
    jenkins_bytes(name.bytes())
}

/// Computes the lowercase-normalized Jenkins one-at-a-time hash commonly used
/// for GTA asset/archetype names.
///
/// ASCII bytes are normalized to lowercase before hashing. Non-ASCII bytes are
/// left untouched; GTA asset names are conventionally ASCII.
pub fn joaat(name: &str) -> u32 {
    jenkins_bytes(name.bytes().map(|byte| byte.to_ascii_lowercase()))
}

fn jenkins_bytes(bytes: impl IntoIterator<Item = u8>) -> u32 {
    let mut hash = 0_u32;

    for byte in bytes {
        hash = hash.wrapping_add(u32::from(byte));
        hash = hash.wrapping_add(hash << 10);
        hash ^= hash >> 6;
    }

    hash = hash.wrapping_add(hash << 3);
    hash ^= hash >> 11;
    hash = hash.wrapping_add(hash << 15);
    hash
}

#[cfg(test)]
mod tests {
    use super::{jenkins, joaat};

    #[test]
    fn empty_is_zero() {
        assert_eq!(jenkins(""), 0);
        assert_eq!(joaat(""), 0);
    }

    #[test]
    fn asset_hashing_is_ascii_case_insensitive() {
        assert_eq!(joaat("PROP_CHAIR_01A"), joaat("prop_chair_01a"));
    }

    #[test]
    fn meta_hashing_is_case_sensitive() {
        assert_ne!(jenkins("archetypeName"), jenkins("archetypename"));
        // Known META names used by CMapData/CEntityDef.
        assert_eq!(jenkins("CMapData"), 3_545_841_574);
        assert_eq!(jenkins("archetypeName"), 2_686_689_324);
        assert_eq!(jenkins("physicsDictionaries"), 949_589_348);
        assert_eq!(jenkins("parentIndex"), 3_633_459_645);
    }
}

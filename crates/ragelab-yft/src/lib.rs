//! Read-only GTA V Legacy YFT fragment preview parser.
//!
//! Scope is intentionally narrow: parse the Legacy v162 FragType root and
//! expose its pristine main fragDrawable through the shared YDR drawable
//! parser. Physics children, damaged variants and articulation remain out of
//! contract until their transforms are independently validated.

use std::{error::Error, fmt, sync::Arc};

use ragelab_resource::{ResourceError, Rsc7Resource, SYSTEM_BASE};
use ragelab_ydr::{DrawableReadLayout, YdrDocument, YdrError};

pub const YFT_LEGACY_VERSION: u32 = 162;
const FRAG_TYPE_MIN_SIZE: usize = 0x60;
const MAIN_DRAWABLE_POINTER_OFFSET: u64 = 0x30;
const NAME_POINTER_OFFSET: u64 = 0x58;
const MAX_NAME_LENGTH: usize = 1_024;

#[derive(Debug, Clone)]
pub struct YftDocument {
    pub name: Option<String>,
    pub main_drawable: Option<YdrDocument>,
}

impl YftDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, YftError> {
        let resource = Arc::new(Rsc7Resource::parse(bytes)?);
        if resource.header.version != YFT_LEGACY_VERSION {
            return Err(YftError::Unsupported(format!(
                "YFT resource version {} is unsupported; expected GTA V Legacy version {YFT_LEGACY_VERSION}",
                resource.header.version
            )));
        }

        resource.bytes_at(SYSTEM_BASE, FRAG_TYPE_MIN_SIZE)?;
        let drawable_pointer = resource.read_u64(SYSTEM_BASE + MAIN_DRAWABLE_POINTER_OFFSET)?;
        let name_pointer = resource.read_u64(SYSTEM_BASE + NAME_POINTER_OFFSET)?;
        let name = if name_pointer == 0 {
            None
        } else {
            Some(resource.read_c_string(name_pointer, MAX_NAME_LENGTH)?)
        };

        let main_drawable = if drawable_pointer == 0 {
            None
        } else {
            Some(YdrDocument::from_shared_resource_at_with_layout(
                resource,
                drawable_pointer,
                DrawableReadLayout::YFT_FRAGMENT,
            )?)
        };

        Ok(Self {
            name,
            main_drawable,
        })
    }
}

#[derive(Debug)]
pub enum YftError {
    Resource(ResourceError),
    Drawable(YdrError),
    Unsupported(String),
}

impl fmt::Display for YftError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resource(error) => write!(formatter, "{error}"),
            Self::Drawable(error) => write!(formatter, "YFT main drawable parse failed: {error}"),
            Self::Unsupported(message) => formatter.write_str(message),
        }
    }
}

impl Error for YftError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Resource(error) => Some(error),
            Self::Drawable(error) => Some(error),
            Self::Unsupported(_) => None,
        }
    }
}

impl From<ResourceError> for YftError {
    fn from(value: ResourceError) -> Self {
        Self::Resource(value)
    }
}

impl From<YdrError> for YftError {
    fn from(value: YdrError) -> Self {
        Self::Drawable(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_yft(version: u32, drawable_pointer: u64) -> Vec<u8> {
        let mut system = vec![0_u8; FRAG_TYPE_MIN_SIZE];
        system[MAIN_DRAWABLE_POINTER_OFFSET as usize..MAIN_DRAWABLE_POINTER_OFFSET as usize + 8]
            .copy_from_slice(&drawable_pointer.to_le_bytes());
        Rsc7Resource::from_segments(version, 0xA000_0000, 0, &system, &[])
            .expect("build synthetic YFT")
            .to_bytes()
            .expect("encode synthetic YFT")
    }

    #[test]
    fn accepts_legacy_fragment_without_main_drawable() {
        let document = YftDocument::from_bytes(&synthetic_yft(YFT_LEGACY_VERSION, 0))
            .expect("parse synthetic YFT");
        assert!(document.main_drawable.is_none());
        assert!(document.name.is_none());
    }

    #[test]
    fn rejects_non_legacy_fragment_version() {
        let error = YftDocument::from_bytes(&synthetic_yft(165, 0)).unwrap_err();
        assert!(error
            .to_string()
            .contains("expected GTA V Legacy version 162"));
    }

    #[test]
    fn rejects_invalid_main_drawable_pointer() {
        let error =
            YftDocument::from_bytes(&synthetic_yft(YFT_LEGACY_VERSION, 0x1234)).unwrap_err();
        assert!(error.to_string().contains("main drawable parse failed"));
    }

    #[test]
    #[ignore = "requires RAGELAB_TEST_YFT to point to a real GTA V Legacy YFT"]
    fn parses_real_fragment_main_drawable() {
        let path =
            std::env::var("RAGELAB_TEST_YFT").expect("RAGELAB_TEST_YFT must point to a real YFT");
        let bytes = std::fs::read(path).expect("read real YFT");
        let document = YftDocument::from_bytes(&bytes).expect("parse real YFT");
        let drawable = document
            .main_drawable
            .as_ref()
            .expect("real fixture should contain a main drawable");
        assert!(!drawable.model.primitives.is_empty());
        assert!(drawable.model.vertex_count() > 0);
        assert!(drawable.model.triangle_count() > 0);
    }
}

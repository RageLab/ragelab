//! GTA V PC YDD drawable-dictionary reader.
//!
//! The dictionary layer stays small: it enumerates hash -> Drawable entries
//! and reuses `ragelab-ydr` to decode a selected drawable from the same shared
//! RSC7 resource.

use std::{error::Error, fmt, sync::Arc};

use ragelab_resource::{ResourceError, Rsc7Resource, SYSTEM_BASE};
use ragelab_ydr::{
    DrawableBoundsProvenance, EditCapability, EditableShaderBinding, EditableTextureBinding,
    GeometryEditCapabilities, GeometryProvenance, ShaderBindingKey, TextureBindingKey, YdrDocument,
    YdrEditSession, YdrError,
};

const SUPPORTED_VERSION: u32 = 165;
const DRAWABLE_DICTIONARY_SIZE: usize = 0x40;
const DRAWABLE_SIZE: usize = 0xD0;
const MAX_DRAWABLES: usize = 65_535;
const MAX_NAME_LENGTH: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YddEntry {
    pub index: usize,
    pub name_hash: u32,
    pub name: Option<String>,
    drawable_pointer: u64,
}

#[derive(Debug, Clone)]
pub struct YddDictionary {
    entries: Vec<YddEntry>,
    resource: Arc<Rsc7Resource>,
}

#[derive(Debug, Clone)]
pub struct YddEditSession {
    entry: YddEntry,
    entries: Vec<YddEntry>,
    drawable: YdrEditSession,
}

impl YddDictionary {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, YddError> {
        let resource = Arc::new(Rsc7Resource::parse(bytes)?);
        if resource.header.version != SUPPORTED_VERSION {
            return Err(YddError::Unsupported(format!(
                "YDD resource version {} is unsupported; expected GTA V PC version {SUPPORTED_VERSION}",
                resource.header.version
            )));
        }

        let root = SYSTEM_BASE;
        resource.bytes_at(root, DRAWABLE_DICTIONARY_SIZE)?;
        let hashes = read_list_header(&resource, root + 0x20, "drawable hash list")?;
        let drawables = read_list_header(&resource, root + 0x30, "drawable pointer list")?;
        if hashes.count != drawables.count {
            return Err(YddError::Malformed(format!(
                "drawable hash count {} does not match drawable pointer count {}",
                hashes.count, drawables.count
            )));
        }
        validate_count(drawables.count)?;

        if drawables.count > 0 {
            if hashes.array_pointer == 0 {
                return Err(YddError::Malformed(
                    "drawable hash list has entries but a null array pointer".into(),
                ));
            }
            if drawables.array_pointer == 0 {
                return Err(YddError::Malformed(
                    "drawable pointer list has entries but a null array pointer".into(),
                ));
            }
            resource.bytes_at(
                hashes.array_pointer,
                checked_mul(hashes.count, 4, "drawable hash array")?,
            )?;
            resource.bytes_at(
                drawables.array_pointer,
                checked_mul(drawables.count, 8, "drawable pointer array")?,
            )?;
        }

        let mut entries = Vec::with_capacity(drawables.count);
        for index in 0..drawables.count {
            let hash_address = add_indexed(hashes.array_pointer, index, 4, "drawable hash")?;
            let pointer_address =
                add_indexed(drawables.array_pointer, index, 8, "drawable pointer")?;
            let name_hash = resource.read_u32(hash_address)?;
            let drawable_pointer = resource.read_u64(pointer_address)?;
            if drawable_pointer == 0 {
                return Err(YddError::Malformed(format!(
                    "drawable {index} has a null pointer"
                )));
            }
            resource.bytes_at(drawable_pointer, DRAWABLE_SIZE)?;
            let name_pointer = resource.read_u64(drawable_pointer + 0xa8)?;
            let name = if name_pointer == 0 {
                None
            } else {
                Some(resource.read_c_string(name_pointer, MAX_NAME_LENGTH)?)
            };
            entries.push(YddEntry {
                index,
                name_hash,
                name,
                drawable_pointer,
            });
        }

        Ok(Self { entries, resource })
    }

    pub fn entries(&self) -> &[YddEntry] {
        &self.entries
    }

    pub fn entry_by_hash(&self, hash: u32) -> Option<&YddEntry> {
        self.entries.iter().find(|entry| entry.name_hash == hash)
    }

    pub fn document(&self, index: usize) -> Result<YdrDocument, YddError> {
        let entry = self.entries.get(index).ok_or_else(|| {
            YddError::Malformed(format!(
                "drawable index {index} exceeds dictionary size {}",
                self.entries.len()
            ))
        })?;
        YdrDocument::from_shared_resource_at(self.resource.clone(), entry.drawable_pointer)
            .map_err(YddError::Drawable)
    }

    pub fn document_by_hash(&self, hash: u32) -> Result<YdrDocument, YddError> {
        let entry = self
            .entry_by_hash(hash)
            .ok_or_else(|| YddError::Malformed(format!("drawable hash 0x{hash:08X} not found")))?;
        self.document(entry.index)
    }
}

impl YddEditSession {
    pub fn from_bytes(bytes: &[u8], drawable_index: usize) -> Result<Self, YddError> {
        let dictionary = YddDictionary::from_bytes(bytes)?;
        let entry = dictionary
            .entries
            .get(drawable_index)
            .cloned()
            .ok_or_else(|| {
                YddError::Malformed(format!(
                    "drawable index {drawable_index} exceeds dictionary size {}",
                    dictionary.entries.len()
                ))
            })?;
        let entries = dictionary.entries.clone();
        let resource = Rsc7Resource::parse(bytes)?;
        let drawable = YdrEditSession::from_resource_at(resource, entry.drawable_pointer)
            .map_err(YddError::Drawable)?;
        Ok(Self {
            entry,
            entries,
            drawable,
        })
    }

    pub fn entry(&self) -> &YddEntry {
        &self.entry
    }

    pub fn shader_count(&self) -> usize {
        self.drawable.shader_count()
    }

    pub fn shader_bindings(&self) -> &[EditableShaderBinding] {
        self.drawable.shader_bindings()
    }

    pub fn texture_bindings(&self) -> &[EditableTextureBinding] {
        self.drawable.texture_bindings()
    }

    pub fn bounds_provenance(&self) -> DrawableBoundsProvenance {
        self.drawable.bounds_provenance()
    }

    pub fn geometry_provenance(&self) -> &[GeometryProvenance] {
        self.drawable.geometry_provenance()
    }

    pub fn structural_audit_error(&self) -> Option<&str> {
        self.drawable.structural_audit_error()
    }

    pub fn geometry_edit_capabilities(&self) -> Vec<GeometryEditCapabilities> {
        self.drawable.geometry_edit_capabilities()
    }

    pub fn rigid_translation_capability(&self) -> EditCapability {
        self.drawable.rigid_translation_capability()
    }

    pub fn translate_rigid_model(&mut self, delta: [f32; 3]) -> Result<(), YddError> {
        self.drawable
            .translate_rigid_model(delta)
            .map_err(YddError::Drawable)
    }

    pub fn rebind_shader(
        &mut self,
        source: ShaderBindingKey,
        target_shader_index: u16,
    ) -> Result<(), YddError> {
        self.drawable
            .rebind_shader(source, target_shader_index)
            .map_err(YddError::Drawable)
    }

    pub fn rebind_texture(
        &mut self,
        source: TextureBindingKey,
        target: TextureBindingKey,
    ) -> Result<(), YddError> {
        self.drawable
            .rebind_texture(source, target)
            .map_err(YddError::Drawable)
    }

    pub fn document(&self) -> Result<YdrDocument, YddError> {
        self.drawable.document().map_err(YddError::Drawable)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, YddError> {
        let bytes = self.drawable.to_bytes().map_err(YddError::Drawable)?;
        let reopened = YddDictionary::from_bytes(&bytes)?;
        if reopened.entries() != self.entries.as_slice() {
            return Err(YddError::Malformed(
                "edited YDD changed dictionary identity metadata after re-open".into(),
            ));
        }
        Ok(bytes)
    }
}

#[derive(Debug)]
pub enum YddError {
    Resource(ResourceError),
    Drawable(YdrError),
    Malformed(String),
    Unsupported(String),
}

impl fmt::Display for YddError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resource(error) => write!(f, "{error}"),
            Self::Drawable(error) => write!(f, "{error}"),
            Self::Malformed(message) => write!(f, "malformed YDD: {message}"),
            Self::Unsupported(message) => write!(f, "unsupported YDD feature: {message}"),
        }
    }
}

impl Error for YddError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Resource(error) => Some(error),
            Self::Drawable(error) => Some(error),
            Self::Malformed(_) | Self::Unsupported(_) => None,
        }
    }
}

impl From<ResourceError> for YddError {
    fn from(error: ResourceError) -> Self {
        Self::Resource(error)
    }
}

#[derive(Debug, Clone, Copy)]
struct ListHeader {
    array_pointer: u64,
    count: usize,
}

fn read_list_header(
    resource: &Rsc7Resource,
    pointer: u64,
    label: &str,
) -> Result<ListHeader, YddError> {
    resource.bytes_at(pointer, 16)?;
    let array_pointer = resource.read_u64(pointer)?;
    let count = usize::from(resource.read_u16(pointer + 0x08)?);
    let capacity = usize::from(resource.read_u16(pointer + 0x0a)?);
    if count > capacity {
        return Err(YddError::Malformed(format!(
            "{label} count {count} exceeds capacity {capacity}"
        )));
    }
    Ok(ListHeader {
        array_pointer,
        count,
    })
}

fn validate_count(count: usize) -> Result<(), YddError> {
    if count > MAX_DRAWABLES {
        return Err(YddError::Malformed(format!(
            "drawable count {count} exceeds safety limit {MAX_DRAWABLES}"
        )));
    }
    Ok(())
}

fn checked_mul(left: usize, right: usize, label: &str) -> Result<usize, YddError> {
    left.checked_mul(right)
        .ok_or_else(|| YddError::Malformed(format!("{label} byte length overflow")))
}

fn add_indexed(base: u64, index: usize, stride: usize, label: &str) -> Result<u64, YddError> {
    let offset = checked_mul(index, stride, label)?;
    base.checked_add(offset as u64)
        .ok_or_else(|| YddError::Malformed(format!("{label} address overflow")))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::{write::DeflateEncoder, Compression};
    use ragelab_resource::{RSC7_MAGIC, SYSTEM_BASE};
    use ragelab_ydr::TextureBindingKey;

    use super::{YddDictionary, YddEditSession, YddError};

    const SYSTEM_SIZE: usize = 2048;
    const SYSTEM_FLAGS: u32 = (1 << 26) | 1;

    fn ptr(offset: usize) -> u64 {
        SYSTEM_BASE + offset as u64
    }

    fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn build_fixture(mutate: impl FnOnce(&mut [u8])) -> Vec<u8> {
        let mut system = vec![0_u8; SYSTEM_SIZE];

        // DrawableDictionary root: ResourceSimpleList64 hashes + ResourcePointerList64 drawables.
        write_u64(&mut system, 0x20, ptr(0x100));
        write_u16(&mut system, 0x28, 1);
        write_u16(&mut system, 0x2a, 1);
        write_u64(&mut system, 0x30, ptr(0x110));
        write_u16(&mut system, 0x38, 1);
        write_u16(&mut system, 0x3a, 1);
        write_u32(&mut system, 0x100, 0x1234_5678);
        write_u64(&mut system, 0x110, ptr(0x180));

        // Only metadata required for dictionary enumeration.
        write_u64(&mut system, 0x180 + 0xa8, ptr(0x300));
        system[0x300..0x30e].copy_from_slice(b"test_drawable\0");

        mutate(&mut system);

        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&system).expect("compress fixture");
        let compressed = encoder.finish().expect("finish fixture compression");
        let mut output = Vec::new();
        output.extend_from_slice(&RSC7_MAGIC);
        output.extend_from_slice(&165_u32.to_le_bytes());
        output.extend_from_slice(&SYSTEM_FLAGS.to_le_bytes());
        output.extend_from_slice(&0_u32.to_le_bytes());
        output.extend_from_slice(&compressed);
        output
    }

    fn add_two_texture_shader(system: &mut [u8]) {
        // Selected drawable root is 0x180 in this fixture.
        write_u64(system, 0x180 + 0x10, ptr(0x380));
        write_u64(system, 0x380 + 0x10, ptr(0x3c0));
        write_u16(system, 0x380 + 0x18, 1);
        write_u16(system, 0x380 + 0x1a, 1);
        write_u64(system, 0x3c0, ptr(0x3d0));
        write_u64(system, 0x3d0, ptr(0x400));
        system[0x3d0 + 0x10] = 2;

        system[0x400] = 0;
        write_u64(system, 0x408, ptr(0x440));
        system[0x410] = 0;
        write_u64(system, 0x418, ptr(0x4a0));
        write_u32(system, 0x420, 0x1111_1111);
        write_u32(system, 0x424, 0x2222_2222);

        write_u64(system, 0x440 + 0x28, ptr(0x500));
        write_u64(system, 0x4a0 + 0x28, ptr(0x520));
        system[0x500..0x50d].copy_from_slice(b"dict_diffuse\0");
        system[0x520..0x52c].copy_from_slice(b"dict_normal\0");
    }

    #[test]
    fn parses_dictionary_metadata() {
        let bytes = build_fixture(|_| {});
        let dictionary = YddDictionary::from_bytes(&bytes).expect("valid synthetic YDD");
        assert_eq!(dictionary.entries().len(), 1);
        let entry = &dictionary.entries()[0];
        assert_eq!(entry.index, 0);
        assert_eq!(entry.name_hash, 0x1234_5678);
        assert_eq!(entry.name.as_deref(), Some("test_drawable"));
        assert_eq!(dictionary.entry_by_hash(0x1234_5678), Some(entry));
    }

    #[test]
    fn edit_session_rebinds_texture_inside_selected_drawable() {
        let bytes = build_fixture(|system| {
            add_two_texture_shader(system);
            write_u32(system, 0x424, 0x1111_1111);
        });
        let mut session =
            YddEditSession::from_bytes(&bytes, 0).expect("drawable should be editable");
        assert_eq!(session.entry().name_hash, 0x1234_5678);
        assert_eq!(session.texture_bindings().len(), 2);
        assert_eq!(
            session.texture_bindings()[0].texture_name.as_deref(),
            Some("dict_diffuse")
        );

        let source = TextureBindingKey {
            shader_index: 0,
            parameter_index: 0,
        };
        let target = TextureBindingKey {
            shader_index: 0,
            parameter_index: 1,
        };
        session
            .rebind_texture(source, target)
            .expect("existing dictionary texture pointer should be reusable");
        let output = session.to_bytes().expect("edited YDD should re-encode");

        let dictionary = YddDictionary::from_bytes(&output).expect("edited YDD should reopen");
        assert_eq!(dictionary.entries()[0].name_hash, 0x1234_5678);
        let reopened =
            YddEditSession::from_bytes(&output, 0).expect("edited drawable should remain editable");
        assert_eq!(
            reopened.texture_bindings()[0].texture_name.as_deref(),
            Some("dict_normal")
        );
        assert_eq!(reopened.texture_bindings()[0].parameter_hash, 0x1111_1111);
    }

    #[test]
    fn rejects_mismatched_dictionary_counts() {
        let bytes = build_fixture(|system| {
            write_u16(system, 0x38, 0);
            write_u16(system, 0x3a, 0);
        });
        let error = YddDictionary::from_bytes(&bytes).expect_err("counts must agree");
        assert!(
            matches!(error, YddError::Malformed(message) if message.contains("does not match"))
        );
    }

    #[test]
    fn rejects_other_resource_versions() {
        let mut bytes = build_fixture(|_| {});
        bytes[4..8].copy_from_slice(&159_u32.to_le_bytes());
        let error = YddDictionary::from_bytes(&bytes).expect_err("Gen9 must be explicit");
        assert!(matches!(error, YddError::Unsupported(message) if message.contains("159")));
    }
}

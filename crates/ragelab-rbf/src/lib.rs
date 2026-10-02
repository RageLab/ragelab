//! Bounded reader for the sequential RAGE RBF0 container used by some YMT files.
//!
//! RBF is descriptor-based and does not use an offset table: each record either
//! declares/reuses a descriptor, closes the current structure, or carries an
//! anonymous byte blob. The reader keeps the generic tree intact; consumers are
//! responsible for interpreting format-specific schemas such as `CMapParentTxds`.

use std::{error::Error, fmt};

pub const RBF_MAGIC: [u8; 4] = *b"RBF0";
const CLOSE_MARKER: u8 = 0xFF;
const BYTES_MARKER: u8 = 0xFD;
const MAX_DEPTH: usize = 256;
const MAX_NODES: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq)]
pub struct RbfFile {
    pub root: RbfStructure,
}

impl RbfFile {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, RbfError> {
        Parser::new(bytes).parse()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RbfStructure {
    pub name: String,
    pub attributes: Vec<RbfValue>,
    pub children: Vec<RbfValue>,
}

impl RbfStructure {
    pub fn child_structures<'a>(
        &'a self,
        name: &'a str,
    ) -> impl Iterator<Item = &'a RbfStructure> + 'a {
        self.children.iter().filter_map(move |value| match value {
            RbfValue::Structure(structure) if structure.name == name => Some(structure),
            _ => None,
        })
    }

    pub fn first_bytes(&self) -> Option<&[u8]> {
        self.children.iter().find_map(|value| match value {
            RbfValue::Bytes(bytes) => Some(bytes.as_slice()),
            _ => None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RbfValue {
    Structure(RbfStructure),
    Bytes(Vec<u8>),
    Uint32 { name: String, value: u32 },
    Boolean { name: String, value: bool },
    Float { name: String, value: f32 },
    Float3 { name: String, value: [f32; 3] },
    String { name: String, value: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RbfError {
    offset: usize,
    message: String,
}

impl RbfError {
    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for RbfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RBF parse error at 0x{:X}: {}",
            self.offset, self.message
        )
    }
}

impl Error for RbfError {}

#[derive(Debug, Clone)]
struct Descriptor {
    name: String,
    data_type: u8,
}

#[derive(Debug)]
struct Frame {
    structure: RbfStructure,
    pending_attributes: usize,
}

impl Frame {
    fn add(&mut self, value: RbfValue) {
        if self.pending_attributes > 0 {
            self.pending_attributes -= 1;
            self.structure.attributes.push(value);
        } else {
            self.structure.children.push(value);
        }
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    position: usize,
    descriptors: Vec<Descriptor>,
    stack: Vec<Frame>,
    nodes: usize,
}

impl<'a> Parser<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            position: 0,
            descriptors: Vec::new(),
            stack: Vec::new(),
            nodes: 0,
        }
    }

    fn parse(mut self) -> Result<RbfFile, RbfError> {
        let magic = self.read_exact(RBF_MAGIC.len())?;
        if magic != RBF_MAGIC {
            return self.fail_at(0, format!("invalid magic {magic:02X?}; expected RBF0"));
        }

        while self.position < self.bytes.len() {
            let record_offset = self.position;
            let descriptor_index = self.read_u8()?;
            match descriptor_index {
                CLOSE_MARKER => {
                    let second = self.read_u8()?;
                    if second != CLOSE_MARKER {
                        return self.fail_at(
                            record_offset,
                            format!("close marker must be FF FF, found FF {second:02X}"),
                        );
                    }
                    let frame = self.stack.pop().ok_or_else(|| {
                        self.error_at(record_offset, "close marker without an open structure")
                    })?;
                    if frame.pending_attributes != 0 {
                        return self.fail_at(
                            record_offset,
                            format!(
                                "structure {} closed with {} pending attributes",
                                frame.structure.name, frame.pending_attributes
                            ),
                        );
                    }
                    if let Some(parent) = self.stack.last_mut() {
                        parent.add(RbfValue::Structure(frame.structure));
                    } else {
                        if self.position != self.bytes.len() {
                            return self
                                .fail_at(self.position, "trailing bytes after the root structure");
                        }
                        return Ok(RbfFile {
                            root: frame.structure,
                        });
                    }
                }
                BYTES_MARKER => {
                    self.require_open_structure(record_offset, "byte blob")?;
                    let second = self.read_u8()?;
                    if second != CLOSE_MARKER {
                        return self.fail_at(
                            record_offset,
                            format!("byte marker must be FD FF, found FD {second:02X}"),
                        );
                    }
                    let length = self.read_i32()?;
                    if length < 0 {
                        return self
                            .fail_at(record_offset, format!("negative byte blob length {length}"));
                    }
                    let length = usize::try_from(length).map_err(|_| {
                        self.error_at(record_offset, "byte blob length does not fit usize")
                    })?;
                    let value = self.read_exact(length)?.to_vec();
                    self.bump_node(record_offset)?;
                    self.stack
                        .last_mut()
                        .expect("open structure checked above")
                        .add(RbfValue::Bytes(value));
                }
                index => {
                    let data_type = self.read_u8()?;
                    let descriptor = self.read_descriptor(record_offset, index, data_type)?;
                    self.parse_typed_record(record_offset, descriptor, data_type)?;
                }
            }
        }

        if self.stack.is_empty() {
            self.fail_at(self.position, "missing root structure")
        } else {
            self.fail_at(self.position, "unexpected end of input before root close")
        }
    }

    fn read_descriptor(
        &mut self,
        record_offset: usize,
        index: u8,
        data_type: u8,
    ) -> Result<Descriptor, RbfError> {
        let index = usize::from(index);
        if index > self.descriptors.len() {
            return self.fail_at(
                record_offset,
                format!(
                    "descriptor index {index} skips next descriptor {}",
                    self.descriptors.len()
                ),
            );
        }

        if index == self.descriptors.len() {
            if index >= usize::from(BYTES_MARKER) {
                return self.fail_at(
                    record_offset,
                    "descriptor table exhausted before reserved marker indices",
                );
            }
            let name_length = self.read_i16()?;
            if name_length <= 0 {
                return self.fail_at(
                    record_offset,
                    format!("descriptor name length must be positive, found {name_length}"),
                );
            }
            let name_length = usize::try_from(name_length).map_err(|_| {
                self.error_at(record_offset, "descriptor name length does not fit usize")
            })?;
            let name_offset = self.position;
            let name = decode_ascii(self.read_exact(name_length)?).map_err(|message| {
                self.error_at(name_offset, format!("invalid descriptor name: {message}"))
            })?;
            let descriptor = Descriptor { name, data_type };
            self.descriptors.push(descriptor.clone());
            Ok(descriptor)
        } else {
            let descriptor = self.descriptors[index].clone();
            if descriptor.data_type != data_type {
                return self.fail_at(
                    record_offset,
                    format!(
                        "descriptor {} ({}) changed type from 0x{:02X} to 0x{data_type:02X}",
                        index, descriptor.name, descriptor.data_type
                    ),
                );
            }
            Ok(descriptor)
        }
    }

    fn parse_typed_record(
        &mut self,
        record_offset: usize,
        descriptor: Descriptor,
        data_type: u8,
    ) -> Result<(), RbfError> {
        match data_type {
            0x00 => {
                if self.stack.len() >= MAX_DEPTH {
                    return self.fail_at(
                        record_offset,
                        format!("structure nesting exceeds {MAX_DEPTH}"),
                    );
                }
                let _unknown_1 = self.read_i16()?;
                let _unknown_2 = self.read_i16()?;
                let attribute_count = self.read_i16()?;
                if attribute_count < 0 {
                    return self.fail_at(
                        record_offset,
                        format!("negative attribute count {attribute_count}"),
                    );
                }
                self.bump_node(record_offset)?;
                self.stack.push(Frame {
                    structure: RbfStructure {
                        name: descriptor.name,
                        attributes: Vec::new(),
                        children: Vec::new(),
                    },
                    pending_attributes: usize::try_from(attribute_count).map_err(|_| {
                        self.error_at(record_offset, "attribute count does not fit usize")
                    })?,
                });
                Ok(())
            }
            0x10 => {
                self.require_open_structure(record_offset, "uint32")?;
                let value = self.read_u32()?;
                self.add_value(
                    record_offset,
                    RbfValue::Uint32 {
                        name: descriptor.name,
                        value,
                    },
                )
            }
            0x20 | 0x30 => {
                self.require_open_structure(record_offset, "boolean")?;
                self.add_value(
                    record_offset,
                    RbfValue::Boolean {
                        name: descriptor.name,
                        value: data_type == 0x20,
                    },
                )
            }
            0x40 => {
                self.require_open_structure(record_offset, "float")?;
                let value = f32::from_bits(self.read_u32()?);
                self.add_value(
                    record_offset,
                    RbfValue::Float {
                        name: descriptor.name,
                        value,
                    },
                )
            }
            0x50 => {
                self.require_open_structure(record_offset, "float3")?;
                let value = [
                    f32::from_bits(self.read_u32()?),
                    f32::from_bits(self.read_u32()?),
                    f32::from_bits(self.read_u32()?),
                ];
                self.add_value(
                    record_offset,
                    RbfValue::Float3 {
                        name: descriptor.name,
                        value,
                    },
                )
            }
            0x60 => {
                self.require_open_structure(record_offset, "string")?;
                let length = self.read_i16()?;
                if length < 0 {
                    return self.fail_at(record_offset, format!("negative string length {length}"));
                }
                let length = usize::try_from(length).map_err(|_| {
                    self.error_at(record_offset, "string length does not fit usize")
                })?;
                let value_offset = self.position;
                let value = decode_ascii(self.read_exact(length)?).map_err(|message| {
                    self.error_at(value_offset, format!("invalid ASCII string: {message}"))
                })?;
                self.add_value(
                    record_offset,
                    RbfValue::String {
                        name: descriptor.name,
                        value,
                    },
                )
            }
            other => self.fail_at(
                record_offset,
                format!("unsupported RBF data type 0x{other:02X}"),
            ),
        }
    }

    fn add_value(&mut self, offset: usize, value: RbfValue) -> Result<(), RbfError> {
        self.bump_node(offset)?;
        if self.stack.is_empty() {
            return self.fail_at(offset, "value outside a structure");
        }
        self.stack
            .last_mut()
            .expect("non-empty stack checked above")
            .add(value);
        Ok(())
    }

    fn bump_node(&mut self, offset: usize) -> Result<(), RbfError> {
        self.nodes = self
            .nodes
            .checked_add(1)
            .ok_or_else(|| self.error_at(offset, "node count overflow"))?;
        if self.nodes > MAX_NODES {
            return self.fail_at(offset, format!("node count exceeds {MAX_NODES}"));
        }
        Ok(())
    }

    fn require_open_structure(&self, offset: usize, kind: &str) -> Result<(), RbfError> {
        if self.stack.is_empty() {
            Err(self.error_at(offset, format!("{kind} value outside a structure")))
        } else {
            Ok(())
        }
    }

    fn read_exact(&mut self, length: usize) -> Result<&'a [u8], RbfError> {
        let start = self.position;
        let end = start
            .checked_add(length)
            .ok_or_else(|| self.error_at(start, "read length overflow"))?;
        let slice = self.bytes.get(start..end).ok_or_else(|| {
            self.error_at(
                start,
                format!(
                    "truncated input: need {length} bytes, {} remain",
                    self.bytes.len().saturating_sub(start)
                ),
            )
        })?;
        self.position = end;
        Ok(slice)
    }

    fn read_u8(&mut self) -> Result<u8, RbfError> {
        Ok(self.read_exact(1)?[0])
    }

    fn read_i16(&mut self) -> Result<i16, RbfError> {
        let bytes: [u8; 2] = self.read_exact(2)?.try_into().expect("fixed-size slice");
        Ok(i16::from_le_bytes(bytes))
    }

    fn read_i32(&mut self) -> Result<i32, RbfError> {
        let bytes: [u8; 4] = self.read_exact(4)?.try_into().expect("fixed-size slice");
        Ok(i32::from_le_bytes(bytes))
    }

    fn read_u32(&mut self) -> Result<u32, RbfError> {
        let bytes: [u8; 4] = self.read_exact(4)?.try_into().expect("fixed-size slice");
        Ok(u32::from_le_bytes(bytes))
    }

    fn error_at(&self, offset: usize, message: impl Into<String>) -> RbfError {
        RbfError {
            offset,
            message: message.into(),
        }
    }

    fn fail_at<T>(&self, offset: usize, message: impl Into<String>) -> Result<T, RbfError> {
        Err(self.error_at(offset, message))
    }
}

fn decode_ascii(bytes: &[u8]) -> Result<String, String> {
    if let Some((index, byte)) = bytes
        .iter()
        .copied()
        .enumerate()
        .find(|(_, byte)| !byte.is_ascii())
    {
        return Err(format!("byte 0x{byte:02X} at index {index} is not ASCII"));
    }
    Ok(bytes.iter().map(|byte| char::from(*byte)).collect())
}

#[cfg(test)]
mod tests {
    use super::{RbfFile, RbfValue, RBF_MAGIC};
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Builder {
        bytes: Vec<u8>,
        descriptors: BTreeMap<String, u8>,
    }

    impl Builder {
        fn new() -> Self {
            Self {
                bytes: RBF_MAGIC.to_vec(),
                descriptors: BTreeMap::new(),
            }
        }

        fn record(&mut self, name: &str, data_type: u8) {
            if let Some(index) = self.descriptors.get(name).copied() {
                self.bytes.push(index);
                self.bytes.push(data_type);
                return;
            }
            let index = u8::try_from(self.descriptors.len()).expect("small fixture");
            self.descriptors.insert(name.to_string(), index);
            self.bytes.push(index);
            self.bytes.push(data_type);
            self.bytes
                .extend_from_slice(&i16::try_from(name.len()).expect("small name").to_le_bytes());
            self.bytes.extend_from_slice(name.as_bytes());
        }

        fn open(&mut self, name: &str, attributes: i16) {
            self.record(name, 0x00);
            self.bytes.extend_from_slice(&0_i16.to_le_bytes());
            self.bytes.extend_from_slice(&0_i16.to_le_bytes());
            self.bytes.extend_from_slice(&attributes.to_le_bytes());
        }

        fn bytes(&mut self, value: &[u8]) {
            self.bytes.extend_from_slice(&[0xFD, 0xFF]);
            self.bytes.extend_from_slice(
                &i32::try_from(value.len())
                    .expect("small fixture")
                    .to_le_bytes(),
            );
            self.bytes.extend_from_slice(value);
        }

        fn uint32(&mut self, name: &str, value: u32) {
            self.record(name, 0x10);
            self.bytes.extend_from_slice(&value.to_le_bytes());
        }

        fn close(&mut self) {
            self.bytes.extend_from_slice(&[0xFF, 0xFF]);
        }

        fn finish(self) -> Vec<u8> {
            self.bytes
        }
    }

    fn parent_txd_fixture() -> Vec<u8> {
        let mut builder = Builder::new();
        builder.open("CMapParentTxds", 0);
        builder.open("txdRelationships", 0);
        builder.open("item", 0);
        builder.open("parent", 0);
        builder.bytes(b"parent_txd\0");
        builder.close();
        builder.open("child", 0);
        builder.bytes(b"child_txd\0");
        builder.close();
        builder.close();
        builder.close();
        builder.close();
        builder.finish()
    }

    #[test]
    fn parses_nested_rbf0_tree_and_bytes() {
        let file = RbfFile::from_bytes(&parent_txd_fixture()).expect("valid RBF fixture");
        assert_eq!(file.root.name, "CMapParentTxds");
        let relationships = file
            .root
            .child_structures("txdRelationships")
            .next()
            .expect("relationships");
        let item = relationships.child_structures("item").next().expect("item");
        assert_eq!(
            item.child_structures("parent")
                .next()
                .and_then(|structure| structure.first_bytes()),
            Some(b"parent_txd\0".as_slice())
        );
        assert_eq!(
            item.child_structures("child")
                .next()
                .and_then(|structure| structure.first_bytes()),
            Some(b"child_txd\0".as_slice())
        );
    }

    #[test]
    fn routes_declared_attributes_separately() {
        let mut builder = Builder::new();
        builder.open("root", 1);
        builder.uint32("flags", 7);
        builder.close();
        let file = RbfFile::from_bytes(&builder.finish()).expect("valid attribute fixture");
        assert!(file.root.children.is_empty());
        assert!(matches!(
            file.root.attributes.as_slice(),
            [RbfValue::Uint32 { name, value: 7 }] if name == "flags"
        ));
    }

    #[test]
    fn rejects_every_truncated_prefix() {
        let bytes = parent_txd_fixture();
        for end in 0..bytes.len() {
            assert!(
                RbfFile::from_bytes(&bytes[..end]).is_err(),
                "prefix {end}/{} unexpectedly parsed",
                bytes.len()
            );
        }
        assert!(RbfFile::from_bytes(&bytes).is_ok());
    }

    #[test]
    fn rejects_descriptor_index_gap() {
        let bytes = [b'R', b'B', b'F', b'0', 0x01, 0x00];
        let error = RbfFile::from_bytes(&bytes).expect_err("descriptor gap must fail");
        assert!(error.message().contains("descriptor index"));
    }

    #[test]
    fn rejects_negative_blob_length() {
        let mut builder = Builder::new();
        builder.open("root", 0);
        builder.bytes.extend_from_slice(&[0xFD, 0xFF]);
        builder.bytes.extend_from_slice(&(-1_i32).to_le_bytes());
        builder.close();
        let error = RbfFile::from_bytes(&builder.finish()).expect_err("negative length must fail");
        assert!(error.message().contains("negative byte blob length"));
    }

    #[test]
    fn rejects_unsatisfied_attribute_count() {
        let mut builder = Builder::new();
        builder.open("root", 1);
        builder.close();
        let error = RbfFile::from_bytes(&builder.finish()).expect_err("missing attr must fail");
        assert!(error.message().contains("pending attributes"));
    }

    #[test]
    fn rejects_trailing_bytes_after_root_close() {
        let mut bytes = parent_txd_fixture();
        bytes.push(0);
        let error = RbfFile::from_bytes(&bytes).expect_err("trailing bytes must fail");
        assert!(error.message().contains("trailing bytes"));
    }
}

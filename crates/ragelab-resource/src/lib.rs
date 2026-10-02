//! Low-level GTA V RAGE resource (`RSC7`) primitives.
//!
//! This crate owns only the generic resource envelope: header decoding,
//! DEFLATE decompression, system/graphics page sizing and virtual-address
//! resolution. Higher-level formats such as META/YMAP live in separate crates.

use std::{
    error::Error,
    fmt,
    io::{Cursor, Read, Write},
};

use flate2::{read::DeflateDecoder, write::DeflateEncoder, Compression};

pub const RSC7_MAGIC: [u8; 4] = *b"RSC7";
pub const RSC7_HEADER_SIZE: usize = 16;
pub const SYSTEM_BASE: u64 = 0x5000_0000;
pub const GRAPHICS_BASE: u64 = 0x6000_0000;
pub const ADDRESS_SPACE_SIZE: u64 = 0x1000_0000;

const PAGE_LAYOUT_MASK: u32 = 0x0FFF_FFFF;
const PAGE_RESERVED_MASK: u32 = !PAGE_LAYOUT_MASK;
const MAX_PAGE_UNITS: u64 = 5_663;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceError {
    TooSmall {
        expected_at_least: usize,
        actual: usize,
    },
    InvalidMagic([u8; 4]),
    Decompression(String),
    Compression(String),
    SegmentSizeMismatch {
        expected: usize,
        actual: usize,
    },
    InvalidPointer(u64),
    OutOfBounds {
        segment: &'static str,
        offset: usize,
        length: usize,
        segment_length: usize,
    },
    Malformed(String),
    Unsupported(&'static str),
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooSmall {
                expected_at_least,
                actual,
            } => write!(
                f,
                "resource is too small: expected at least {expected_at_least} bytes, got {actual}"
            ),
            Self::InvalidMagic(magic) => write!(f, "not an RSC7 resource (magic: {magic:02X?})"),
            Self::Decompression(message) => write!(f, "RSC7 decompression failed: {message}"),
            Self::Compression(message) => write!(f, "RSC7 compression failed: {message}"),
            Self::SegmentSizeMismatch { expected, actual } => write!(
                f,
                "RSC7 decompressed size mismatch: flags describe {expected} bytes, payload produced {actual}"
            ),
            Self::InvalidPointer(pointer) => write!(f, "invalid resource pointer 0x{pointer:016X}"),
            Self::OutOfBounds {
                segment,
                offset,
                length,
                segment_length,
            } => write!(
                f,
                "{segment} read is out of bounds: offset={offset}, length={length}, segment={segment_length}"
            ),
            Self::Malformed(message) => write!(f, "malformed resource: {message}"),
            Self::Unsupported(what) => write!(f, "unsupported resource feature: {what}"),
        }
    }
}

impl Error for ResourceError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rsc7Header {
    pub version: u32,
    pub system_flags: u32,
    pub graphics_flags: u32,
}

impl Rsc7Header {
    pub fn parse(bytes: &[u8]) -> Result<Self, ResourceError> {
        if bytes.len() < RSC7_HEADER_SIZE {
            return Err(ResourceError::TooSmall {
                expected_at_least: RSC7_HEADER_SIZE,
                actual: bytes.len(),
            });
        }

        let magic: [u8; 4] = bytes[0..4].try_into().expect("fixed four-byte magic slice");
        if magic != RSC7_MAGIC {
            return Err(ResourceError::InvalidMagic(magic));
        }

        Ok(Self {
            version: read_u32_le(bytes, 4)?,
            system_flags: read_u32_le(bytes, 8)?,
            graphics_flags: read_u32_le(bytes, 12)?,
        })
    }

    pub fn system_size(self) -> usize {
        decode_page_size(self.system_flags)
    }

    pub fn graphics_size(self) -> usize {
        decode_page_size(self.graphics_flags)
    }

    pub fn decompressed_size(self) -> usize {
        self.system_size().saturating_add(self.graphics_size())
    }
}

/// Decodes the packed RAGE page-size flags used by GTA V PC RSC7 resources.
///
/// The low nibble selects a base page (`0x200 << shift`) and the remaining
/// packed fields encode how many pages exist at successively larger sizes.
pub fn decode_page_size(flags: u32) -> usize {
    let page_counts = [
        (flags >> 27) & 0x1,
        ((flags >> 26) & 0x1) << 1,
        ((flags >> 25) & 0x1) << 2,
        ((flags >> 24) & 0x1) << 3,
        ((flags >> 17) & 0x7f) << 4,
        ((flags >> 11) & 0x3f) << 5,
        ((flags >> 7) & 0x0f) << 6,
        ((flags >> 5) & 0x03) << 7,
        ((flags >> 4) & 0x01) << 8,
    ];

    let page_units: u64 = page_counts.into_iter().map(u64::from).sum();
    let base_page = 0x200_u64 << (flags & 0x0f);
    (base_page.saturating_mul(page_units)) as usize
}

/// Returns the number of physical pages described by a packed RSC7 flag word.
///
/// This differs from [`decode_page_size`]: the size calculation weights each
/// page class by its capacity, while `ResourcePagesInfo` stores the raw number
/// of page records. CodeWalker uses this same raw count for its page-info block.
pub fn decode_page_count(flags: u32) -> usize {
    usize::try_from(
        ((flags >> 27) & 0x1)
            + ((flags >> 26) & 0x1)
            + ((flags >> 25) & 0x1)
            + ((flags >> 24) & 0x1)
            + ((flags >> 17) & 0x7f)
            + ((flags >> 11) & 0x3f)
            + ((flags >> 7) & 0x0f)
            + ((flags >> 5) & 0x03)
            + ((flags >> 4) & 0x01),
    )
    .expect("RSC7 page count fits usize")
}

/// Encodes a RAGE page layout that can hold at least `minimum_size` bytes.
///
/// Only the lower 28 page-layout bits are returned. Callers that resize an
/// existing segment must preserve any format-specific bits in the upper nibble
/// of the original flags. The returned allocation is page-aligned and never
/// exceeds the 256 MiB virtual address space reserved for one RSC7 segment.
pub fn encode_page_size_at_least(minimum_size: usize) -> Result<(u32, usize), ResourceError> {
    encode_page_size_at_least_with_max_pages(minimum_size, usize::MAX)
}

fn encode_page_size_at_least_with_max_pages(
    minimum_size: usize,
    max_pages: usize,
) -> Result<(u32, usize), ResourceError> {
    if minimum_size == 0 {
        return Ok((0, 0));
    }
    if minimum_size > ADDRESS_SPACE_SIZE as usize {
        return Err(ResourceError::Malformed(format!(
            "requested RSC7 segment size {minimum_size} exceeds address space {ADDRESS_SPACE_SIZE}"
        )));
    }

    let minimum_size_u64 = minimum_size as u64;
    let mut best: Option<(u32, usize, usize)> = None;
    for shift in 0_u32..=15 {
        let base_page = 0x200_u64 << shift;
        let units = minimum_size_u64.div_ceil(base_page);
        if units == 0 || units > MAX_PAGE_UNITS {
            continue;
        }

        let layout = encode_page_units(units)? | shift;
        let page_count = decode_page_count(layout);
        if page_count > max_pages {
            continue;
        }
        let allocation = base_page
            .checked_mul(units)
            .ok_or_else(|| ResourceError::Malformed("RSC7 page allocation overflows u64".into()))?;
        if allocation > ADDRESS_SPACE_SIZE {
            continue;
        }
        let allocation = usize::try_from(allocation).map_err(|_| {
            ResourceError::Malformed("RSC7 page allocation does not fit usize".into())
        })?;
        debug_assert_eq!(decode_page_size(layout), allocation);

        let candidate = (layout, allocation, page_count);
        let replace = match best.as_ref() {
            Some(current) => (allocation, page_count) < (current.1, current.2),
            None => true,
        };
        if replace {
            best = Some(candidate);
        }
    }

    best.map(|(layout, allocation, _)| (layout, allocation))
        .ok_or_else(|| {
            ResourceError::Malformed(format!(
                "cannot encode RSC7 page layout for {minimum_size} bytes within {max_pages} pages"
            ))
        })
}

fn encode_page_units(mut units: u64) -> Result<u32, ResourceError> {
    let fields = [
        (256_u64, 1_u64, 4_u32),
        (128, 3, 5),
        (64, 15, 7),
        (32, 63, 11),
        (16, 127, 17),
        (8, 1, 24),
        (4, 1, 25),
        (2, 1, 26),
        (1, 1, 27),
    ];
    let mut flags = 0_u32;
    for (weight, capacity, bit) in fields {
        let count = (units / weight).min(capacity);
        units -= count * weight;
        flags |= (count as u32) << bit;
    }
    if units != 0 {
        return Err(ResourceError::Malformed(
            "RSC7 page-unit count is not representable".into(),
        ));
    }
    Ok(flags)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rsc7Resource {
    pub header: Rsc7Header,
    system: Vec<u8>,
    graphics: Vec<u8>,
}

impl Rsc7Resource {
    pub fn parse(bytes: &[u8]) -> Result<Self, ResourceError> {
        let header = Rsc7Header::parse(bytes)?;

        let compressed = bytes
            .get(RSC7_HEADER_SIZE..)
            .ok_or(ResourceError::TooSmall {
                expected_at_least: RSC7_HEADER_SIZE,
                actual: bytes.len(),
            })?;

        let mut decoder = DeflateDecoder::new(Cursor::new(compressed));
        let mut decompressed = Vec::with_capacity(header.decompressed_size());
        decoder
            .read_to_end(&mut decompressed)
            .map_err(|error| ResourceError::Decompression(error.to_string()))?;

        let system_size = header.system_size();
        let graphics_size = header.graphics_size();
        let expected = system_size.saturating_add(graphics_size);

        if decompressed.len() != expected {
            return Err(ResourceError::SegmentSizeMismatch {
                expected,
                actual: decompressed.len(),
            });
        }

        let graphics = decompressed.split_off(system_size);
        let system = decompressed;

        Ok(Self {
            header,
            system,
            graphics,
        })
    }

    /// Builds a new RSC7 resource from raw system/graphics segment payloads.
    ///
    /// Segment byte slices are copied into the smallest valid RAGE page
    /// allocations that can contain them and padded with zeroes. Callers may
    /// provide format-specific reserved flag bits for the upper nibble; page
    /// layout bits are always derived from the requested segment lengths.
    pub fn from_segments(
        version: u32,
        system_reserved_flags: u32,
        graphics_reserved_flags: u32,
        system: &[u8],
        graphics: &[u8],
    ) -> Result<Self, ResourceError> {
        let (system_layout, system_allocation) = encode_page_size_at_least(system.len())?;
        let (graphics_layout, graphics_allocation) = encode_page_size_at_least(graphics.len())?;

        let mut system_bytes = vec![0; system_allocation];
        system_bytes[..system.len()].copy_from_slice(system);
        let mut graphics_bytes = vec![0; graphics_allocation];
        graphics_bytes[..graphics.len()].copy_from_slice(graphics);

        Ok(Self {
            header: Rsc7Header {
                version,
                system_flags: (system_reserved_flags & PAGE_RESERVED_MASK)
                    | (system_layout & PAGE_LAYOUT_MASK),
                graphics_flags: (graphics_reserved_flags & PAGE_RESERVED_MASK)
                    | (graphics_layout & PAGE_LAYOUT_MASK),
            },
            system: system_bytes,
            graphics: graphics_bytes,
        })
    }

    pub fn system(&self) -> &[u8] {
        &self.system
    }

    pub fn graphics(&self) -> &[u8] {
        &self.graphics
    }

    /// Grows the graphics segment to a valid page allocation that can hold at
    /// least `minimum_size` bytes, preserving existing bytes and reserved
    /// upper flag bits. Shrinking is intentionally not supported here.
    pub fn grow_graphics_to_fit(&mut self, minimum_size: usize) -> Result<usize, ResourceError> {
        if minimum_size <= self.graphics.len() {
            return Ok(self.graphics.len());
        }

        let (layout_flags, allocation) = encode_page_size_at_least(minimum_size)?;
        let reserved = self.header.graphics_flags & PAGE_RESERVED_MASK;
        self.header.graphics_flags = reserved | (layout_flags & PAGE_LAYOUT_MASK);
        self.graphics.resize(allocation, 0);
        Ok(allocation)
    }

    /// Grows the graphics segment while keeping an existing `ResourcePagesInfo`
    /// block consistent with the new RSC7 page flags.
    ///
    /// Legacy `ResourceFileBase` stores a page-info pointer at offset `0x08`.
    /// The pointed block starts with a 16-byte header whose bytes `0x08` and
    /// `0x09` are the system/graphics page counts, followed by eight reserved
    /// bytes per page. This method never increases the graphics page count
    /// beyond the block's original capacity, so no system-segment relocation is
    /// required when the graphics allocation grows.
    pub fn grow_graphics_to_fit_with_page_info(
        &mut self,
        minimum_size: usize,
        pages_info_pointer: u64,
    ) -> Result<usize, ResourceError> {
        if minimum_size <= self.graphics.len() {
            return Ok(self.graphics.len());
        }
        if pages_info_pointer == 0 {
            return Err(ResourceError::Malformed(
                "cannot grow graphics segment with a null ResourcePagesInfo pointer".into(),
            ));
        }

        let system_pages = usize::from(self.read_u8(checked_pointer_add(
            pages_info_pointer,
            0x08,
            "ResourcePagesInfo system page count",
        )?)?);
        let graphics_pages_pointer = checked_pointer_add(
            pages_info_pointer,
            0x09,
            "ResourcePagesInfo graphics page count",
        )?;
        let graphics_pages = usize::from(self.read_u8(graphics_pages_pointer)?);
        let header_system_pages = decode_page_count(self.header.system_flags);
        let header_graphics_pages = decode_page_count(self.header.graphics_flags);
        if system_pages != header_system_pages || graphics_pages != header_graphics_pages {
            return Err(ResourceError::Malformed(format!(
                "ResourcePagesInfo page counts {system_pages}/{graphics_pages} do not match RSC7 flags {header_system_pages}/{header_graphics_pages}"
            )));
        }

        let original_page_info_len = 16_usize
            .checked_add(
                system_pages
                    .checked_add(graphics_pages)
                    .and_then(|count| count.checked_mul(8))
                    .ok_or_else(|| {
                        ResourceError::Malformed(
                            "ResourcePagesInfo page-table length overflows usize".into(),
                        )
                    })?,
            )
            .ok_or_else(|| {
                ResourceError::Malformed("ResourcePagesInfo block length overflows usize".into())
            })?;
        self.bytes_at(pages_info_pointer, original_page_info_len)?;

        let (layout_flags, allocation) =
            encode_page_size_at_least_with_max_pages(minimum_size, graphics_pages)?;
        let new_graphics_pages = decode_page_count(layout_flags);
        let new_graphics_pages = u8::try_from(new_graphics_pages).map_err(|_| {
            ResourceError::Malformed("graphics page count does not fit ResourcePagesInfo u8".into())
        })?;

        let reserved = self.header.graphics_flags & PAGE_RESERVED_MASK;
        self.header.graphics_flags = reserved | (layout_flags & PAGE_LAYOUT_MASK);
        self.graphics.resize(allocation, 0);
        self.bytes_at_mut(graphics_pages_pointer, 1)?[0] = new_graphics_pages;
        Ok(allocation)
    }

    /// Re-encodes this resource without changing its virtual layout.
    ///
    /// The current system/graphics segment lengths must still match the sizes
    /// encoded by the original page flags. This is intentionally not a page
    /// allocator; callers may patch existing bytes but may not grow/shrink a
    /// segment through this API.
    pub fn to_bytes(&self) -> Result<Vec<u8>, ResourceError> {
        let expected_system = self.header.system_size();
        let expected_graphics = self.header.graphics_size();
        if self.system.len() != expected_system || self.graphics.len() != expected_graphics {
            return Err(ResourceError::Malformed(format!(
                "cannot re-encode RSC7 after segment resize: system {}/{} bytes, graphics {}/{} bytes",
                self.system.len(),
                expected_system,
                self.graphics.len(),
                expected_graphics
            )));
        }

        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(&self.system)
            .and_then(|_| encoder.write_all(&self.graphics))
            .map_err(|error| ResourceError::Compression(error.to_string()))?;
        let compressed = encoder
            .finish()
            .map_err(|error| ResourceError::Compression(error.to_string()))?;

        let mut output = Vec::with_capacity(RSC7_HEADER_SIZE + compressed.len());
        output.extend_from_slice(&RSC7_MAGIC);
        output.extend_from_slice(&self.header.version.to_le_bytes());
        output.extend_from_slice(&self.header.system_flags.to_le_bytes());
        output.extend_from_slice(&self.header.graphics_flags.to_le_bytes());
        output.extend_from_slice(&compressed);
        Ok(output)
    }

    pub fn bytes_at(&self, pointer: u64, length: usize) -> Result<&[u8], ResourceError> {
        if (SYSTEM_BASE..SYSTEM_BASE + ADDRESS_SPACE_SIZE).contains(&pointer) {
            let offset = usize::try_from(pointer - SYSTEM_BASE)
                .map_err(|_| ResourceError::InvalidPointer(pointer))?;
            return checked_slice("system", &self.system, offset, length);
        }

        if (GRAPHICS_BASE..GRAPHICS_BASE + ADDRESS_SPACE_SIZE).contains(&pointer) {
            let offset = usize::try_from(pointer - GRAPHICS_BASE)
                .map_err(|_| ResourceError::InvalidPointer(pointer))?;
            return checked_slice("graphics", &self.graphics, offset, length);
        }

        Err(ResourceError::InvalidPointer(pointer))
    }

    pub fn bytes_at_mut(
        &mut self,
        pointer: u64,
        length: usize,
    ) -> Result<&mut [u8], ResourceError> {
        if (SYSTEM_BASE..SYSTEM_BASE + ADDRESS_SPACE_SIZE).contains(&pointer) {
            let offset = usize::try_from(pointer - SYSTEM_BASE)
                .map_err(|_| ResourceError::InvalidPointer(pointer))?;
            return checked_slice_mut("system", &mut self.system, offset, length);
        }

        if (GRAPHICS_BASE..GRAPHICS_BASE + ADDRESS_SPACE_SIZE).contains(&pointer) {
            let offset = usize::try_from(pointer - GRAPHICS_BASE)
                .map_err(|_| ResourceError::InvalidPointer(pointer))?;
            return checked_slice_mut("graphics", &mut self.graphics, offset, length);
        }

        Err(ResourceError::InvalidPointer(pointer))
    }

    pub fn read_u8(&self, pointer: u64) -> Result<u8, ResourceError> {
        Ok(self.bytes_at(pointer, 1)?[0])
    }

    pub fn read_u16(&self, pointer: u64) -> Result<u16, ResourceError> {
        let bytes: [u8; 2] = self
            .bytes_at(pointer, 2)?
            .try_into()
            .expect("fixed two-byte slice");
        Ok(u16::from_le_bytes(bytes))
    }

    pub fn read_i16(&self, pointer: u64) -> Result<i16, ResourceError> {
        Ok(self.read_u16(pointer)? as i16)
    }

    pub fn read_u32(&self, pointer: u64) -> Result<u32, ResourceError> {
        let bytes: [u8; 4] = self
            .bytes_at(pointer, 4)?
            .try_into()
            .expect("fixed four-byte slice");
        Ok(u32::from_le_bytes(bytes))
    }

    pub fn read_i32(&self, pointer: u64) -> Result<i32, ResourceError> {
        Ok(self.read_u32(pointer)? as i32)
    }

    pub fn read_u64(&self, pointer: u64) -> Result<u64, ResourceError> {
        let bytes: [u8; 8] = self
            .bytes_at(pointer, 8)?
            .try_into()
            .expect("fixed eight-byte slice");
        Ok(u64::from_le_bytes(bytes))
    }

    pub fn read_i64(&self, pointer: u64) -> Result<i64, ResourceError> {
        Ok(self.read_u64(pointer)? as i64)
    }

    pub fn read_c_string(&self, pointer: u64, max_len: usize) -> Result<String, ResourceError> {
        let segment = if (SYSTEM_BASE..SYSTEM_BASE + ADDRESS_SPACE_SIZE).contains(&pointer) {
            (
                &self.system[..],
                usize::try_from(pointer - SYSTEM_BASE).unwrap_or(usize::MAX),
            )
        } else if (GRAPHICS_BASE..GRAPHICS_BASE + ADDRESS_SPACE_SIZE).contains(&pointer) {
            (
                &self.graphics[..],
                usize::try_from(pointer - GRAPHICS_BASE).unwrap_or(usize::MAX),
            )
        } else {
            return Err(ResourceError::InvalidPointer(pointer));
        };

        if segment.1 >= segment.0.len() {
            return Err(ResourceError::OutOfBounds {
                segment: if pointer >= GRAPHICS_BASE {
                    "graphics"
                } else {
                    "system"
                },
                offset: segment.1,
                length: 1,
                segment_length: segment.0.len(),
            });
        }

        let remaining = &segment.0[segment.1..];
        let bounded = &remaining[..remaining.len().min(max_len)];
        let end = bounded
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bounded.len());
        Ok(String::from_utf8_lossy(&bounded[..end]).into_owned())
    }
}

/// Backward-compatible light-weight header probe used by the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rsc7Probe {
    pub prefix: [u8; RSC7_HEADER_SIZE],
    pub header: Rsc7Header,
}

impl Rsc7Probe {
    pub fn parse(bytes: &[u8]) -> Result<Self, ResourceError> {
        let header = Rsc7Header::parse(bytes)?;
        let prefix = bytes[0..RSC7_HEADER_SIZE]
            .try_into()
            .expect("fixed RSC7 header slice");
        Ok(Self { prefix, header })
    }
}

fn checked_pointer_add(
    pointer: u64,
    offset: u64,
    label: &'static str,
) -> Result<u64, ResourceError> {
    pointer
        .checked_add(offset)
        .ok_or_else(|| ResourceError::Malformed(format!("{label} pointer overflows u64")))
}

fn checked_slice<'a>(
    segment_name: &'static str,
    segment: &'a [u8],
    offset: usize,
    length: usize,
) -> Result<&'a [u8], ResourceError> {
    let end = offset
        .checked_add(length)
        .ok_or(ResourceError::OutOfBounds {
            segment: segment_name,
            offset,
            length,
            segment_length: segment.len(),
        })?;

    segment.get(offset..end).ok_or(ResourceError::OutOfBounds {
        segment: segment_name,
        offset,
        length,
        segment_length: segment.len(),
    })
}

fn checked_slice_mut<'a>(
    segment_name: &'static str,
    segment: &'a mut [u8],
    offset: usize,
    length: usize,
) -> Result<&'a mut [u8], ResourceError> {
    let segment_length = segment.len();
    let end = offset
        .checked_add(length)
        .ok_or(ResourceError::OutOfBounds {
            segment: segment_name,
            offset,
            length,
            segment_length,
        })?;

    segment
        .get_mut(offset..end)
        .ok_or(ResourceError::OutOfBounds {
            segment: segment_name,
            offset,
            length,
            segment_length,
        })
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Result<u32, ResourceError> {
    let end = offset + 4;
    let slice = bytes.get(offset..end).ok_or(ResourceError::TooSmall {
        expected_at_least: end,
        actual: bytes.len(),
    })?;
    Ok(u32::from_le_bytes(
        slice.try_into().expect("fixed four-byte slice"),
    ))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::{write::DeflateEncoder, Compression};

    use super::{
        decode_page_count, decode_page_size, encode_page_size_at_least, ResourceError, Rsc7Header,
        Rsc7Probe, Rsc7Resource, ADDRESS_SPACE_SIZE, GRAPHICS_BASE, PAGE_RESERVED_MASK, RSC7_MAGIC,
        SYSTEM_BASE,
    };

    fn flags_for_single_512_page() -> u32 {
        1 << 27
    }

    fn build_resource(
        system: &[u8],
        graphics: &[u8],
        system_flags: u32,
        graphics_flags: u32,
    ) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(system);
        payload.extend_from_slice(graphics);

        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&payload).expect("compress fixture");
        let compressed = encoder.finish().expect("finish fixture compression");

        let mut output = Vec::new();
        output.extend_from_slice(&RSC7_MAGIC);
        output.extend_from_slice(&2_u32.to_le_bytes());
        output.extend_from_slice(&system_flags.to_le_bytes());
        output.extend_from_slice(&graphics_flags.to_le_bytes());
        output.extend_from_slice(&compressed);
        output
    }

    #[test]
    fn recognizes_header() {
        let flags = flags_for_single_512_page();
        let bytes = build_resource(&vec![0; 512], &[], flags, 0);
        let probe = Rsc7Probe::parse(&bytes).expect("valid RSC7 header");
        assert_eq!(probe.header.version, 2);
        assert_eq!(probe.header.system_size(), 512);
    }

    #[test]
    fn builds_new_resource_from_unpadded_segments() {
        let system = vec![0xA5; 513];
        let graphics = vec![0x5A; 17];
        let resource =
            Rsc7Resource::from_segments(13, 0xA000_0000, 0x5000_0000, &system, &graphics)
                .expect("build fresh resource");

        assert_eq!(resource.header.version, 13);
        assert_eq!(
            resource.header.system_flags & PAGE_RESERVED_MASK,
            0xA000_0000
        );
        assert_eq!(
            resource.header.graphics_flags & PAGE_RESERVED_MASK,
            0x5000_0000
        );
        assert!(resource.system().len() >= system.len());
        assert!(resource.graphics().len() >= graphics.len());
        assert_eq!(&resource.system()[..system.len()], system.as_slice());
        assert_eq!(&resource.graphics()[..graphics.len()], graphics.as_slice());
        assert!(resource.system()[system.len()..]
            .iter()
            .all(|byte| *byte == 0));
        assert!(resource.graphics()[graphics.len()..]
            .iter()
            .all(|byte| *byte == 0));

        let bytes = resource.to_bytes().expect("encode fresh resource");
        let reparsed = Rsc7Resource::parse(&bytes).expect("reparse fresh resource");
        assert_eq!(reparsed.header, resource.header);
        assert_eq!(reparsed.system(), resource.system());
        assert_eq!(reparsed.graphics(), resource.graphics());
    }

    #[test]
    fn page_flags_decode_single_512_page() {
        assert_eq!(decode_page_size(flags_for_single_512_page()), 512);
    }

    #[test]
    fn page_flags_decode_physical_page_count() {
        assert_eq!(decode_page_count(flags_for_single_512_page()), 1);
        assert_eq!(decode_page_count(0xD3B0_0385), 97);
    }

    #[test]
    fn page_flags_encode_minimum_capacity_without_underallocation() {
        for minimum in [1_usize, 512, 513, 1_048_576, 30_605_312] {
            let (flags, allocation) = encode_page_size_at_least(minimum).unwrap();
            assert!(allocation >= minimum);
            assert_eq!(decode_page_size(flags), allocation);
            assert_eq!(flags & 0xF000_0000, 0);
        }
        assert_eq!(encode_page_size_at_least(0).unwrap(), (0, 0));
    }

    #[test]
    fn page_flags_round_trip_every_base_unit_count() {
        for units in 1_usize..=5_663 {
            let minimum = units * 512;
            let (flags, allocation) = encode_page_size_at_least(minimum).unwrap();
            assert_eq!(allocation, minimum);
            assert_eq!(decode_page_size(flags), minimum);
        }
    }

    #[test]
    fn page_flags_reject_segment_larger_than_virtual_address_space() {
        let too_large = usize::try_from(ADDRESS_SPACE_SIZE).unwrap() + 1;
        assert!(matches!(
            encode_page_size_at_least(too_large),
            Err(ResourceError::Malformed(_))
        ));
    }

    #[test]
    fn grows_graphics_without_expanding_resource_pages_info_capacity() {
        let flags = flags_for_single_512_page();
        let pages_info_pointer = SYSTEM_BASE + 0x20;
        let mut system = vec![0_u8; 512];
        system[0x28] = 1;
        system[0x29] = 1;
        let mut graphics = vec![0_u8; 512];
        graphics[8] = 0xAB;
        let bytes = build_resource(&system, &graphics, flags, 0xD000_0000 | flags);
        let mut resource = Rsc7Resource::parse(&bytes).expect("parse synthetic resource");

        let allocation = resource
            .grow_graphics_to_fit_with_page_info(1536, pages_info_pointer)
            .expect("grow graphics with page info");
        assert_eq!(allocation, 2048);
        assert_eq!(decode_page_count(resource.header.graphics_flags), 1);
        assert_eq!(resource.read_u8(pages_info_pointer + 8).unwrap(), 1);
        assert_eq!(resource.read_u8(pages_info_pointer + 9).unwrap(), 1);
        assert_eq!(resource.graphics()[8], 0xAB);
        assert_eq!(resource.header.graphics_flags & 0xF000_0000, 0xD000_0000);

        let reencoded = resource.to_bytes().expect("re-encode grown resource");
        let reparsed = Rsc7Resource::parse(&reencoded).expect("reparse grown resource");
        assert_eq!(reparsed.graphics().len(), 2048);
        assert_eq!(reparsed.read_u8(pages_info_pointer + 9).unwrap(), 1);
        assert_eq!(decode_page_count(reparsed.header.graphics_flags), 1);
    }

    #[test]
    fn rejects_page_info_counts_that_disagree_with_header_flags() {
        let flags = flags_for_single_512_page();
        let pages_info_pointer = SYSTEM_BASE + 0x20;
        let mut system = vec![0_u8; 512];
        system[0x28] = 1;
        system[0x29] = 2;
        let bytes = build_resource(&system, &vec![0_u8; 512], flags, flags);
        let mut resource = Rsc7Resource::parse(&bytes).expect("parse synthetic resource");
        assert!(matches!(
            resource.grow_graphics_to_fit_with_page_info(1024, pages_info_pointer),
            Err(ResourceError::Malformed(message)) if message.contains("do not match RSC7 flags")
        ));
    }

    #[test]
    fn grows_graphics_preserving_bytes_and_reserved_flag_bits() {
        let flags = flags_for_single_512_page();
        let graphics_flags = 0xD000_0000 | flags;
        let mut graphics = vec![0_u8; 512];
        graphics[8] = 0xAB;
        let bytes = build_resource(&vec![0_u8; 512], &graphics, flags, graphics_flags);
        let mut resource = Rsc7Resource::parse(&bytes).expect("parse synthetic resource");

        let allocation = resource
            .grow_graphics_to_fit(700)
            .expect("grow graphics segment");
        assert!(allocation >= 700);
        assert_eq!(resource.graphics().len(), allocation);
        assert_eq!(resource.graphics()[8], 0xAB);
        assert_eq!(resource.header.graphics_flags & 0xF000_0000, 0xD000_0000);
        assert_eq!(resource.header.graphics_size(), allocation);

        let reencoded = resource.to_bytes().expect("re-encode grown resource");
        let reparsed = Rsc7Resource::parse(&reencoded).expect("reparse grown resource");
        assert_eq!(reparsed.graphics().len(), allocation);
        assert_eq!(reparsed.graphics()[8], 0xAB);
        assert_eq!(reparsed.header.graphics_flags & 0xF000_0000, 0xD000_0000);
    }

    #[test]
    fn decompresses_and_resolves_virtual_addresses() {
        let flags = flags_for_single_512_page();
        let mut system = vec![0_u8; 512];
        system[4..8].copy_from_slice(&0x1234_5678_u32.to_le_bytes());
        let mut graphics = vec![0_u8; 512];
        graphics[8] = 0xAB;

        let bytes = build_resource(&system, &graphics, flags, flags);
        let resource = Rsc7Resource::parse(&bytes).expect("valid synthetic resource");

        assert_eq!(resource.read_u32(SYSTEM_BASE + 4).unwrap(), 0x1234_5678);
        assert_eq!(resource.read_u8(GRAPHICS_BASE + 8).unwrap(), 0xAB);
    }

    #[test]
    fn reencodes_resource_preserving_header_and_payload() {
        let flags = flags_for_single_512_page();
        let mut system = vec![0_u8; 512];
        system[4..8].copy_from_slice(&0x1234_5678_u32.to_le_bytes());
        let mut graphics = vec![0_u8; 512];
        graphics[8] = 0xAB;

        let bytes = build_resource(&system, &graphics, flags, flags);
        let resource = Rsc7Resource::parse(&bytes).expect("parse synthetic resource");
        let reencoded = resource.to_bytes().expect("re-encode unchanged resource");
        let reparsed = Rsc7Resource::parse(&reencoded).expect("reparse re-encoded resource");

        assert_eq!(reparsed.header, resource.header);
        assert_eq!(reparsed.system(), resource.system());
        assert_eq!(reparsed.graphics(), resource.graphics());
    }

    #[test]
    fn patches_existing_virtual_bytes_and_reencodes() {
        let flags = flags_for_single_512_page();
        let bytes = build_resource(&vec![0_u8; 512], &vec![0_u8; 512], flags, flags);
        let mut resource = Rsc7Resource::parse(&bytes).expect("parse synthetic resource");
        resource.bytes_at_mut(GRAPHICS_BASE + 8, 1).unwrap()[0] = 0xCD;

        let reencoded = resource.to_bytes().expect("re-encode patched resource");
        let reparsed = Rsc7Resource::parse(&reencoded).expect("reparse patched resource");
        assert_eq!(reparsed.read_u8(GRAPHICS_BASE + 8).unwrap(), 0xCD);
    }

    #[test]
    fn rejects_other_magic() {
        let bytes = [0_u8; 16];
        assert!(matches!(
            Rsc7Header::parse(&bytes),
            Err(ResourceError::InvalidMagic(_))
        ));
    }
}

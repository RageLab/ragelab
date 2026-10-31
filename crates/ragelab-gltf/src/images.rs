use std::io::Cursor;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Serialize;
use serde_json::Value;

use crate::{
    inspect_glb,
    parser::{parse_container, GlbError},
};

const MAX_ENCODED_IMAGE_BYTES: usize = 128 * 1024 * 1024;
const MAX_TOTAL_IMAGE_BYTES: usize = 512 * 1024 * 1024;
const MAX_IMAGE_PIXELS: usize = 64 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: usize = 16_384;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodedImage {
    pub index: usize,
    pub name: String,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    #[serde(skip)]
    pub rgba8: Vec<u8>,
}

/// Extracts and decodes embedded PNG images from a GLB's BIN bufferViews or
/// base64 data URIs. External images and unsupported image codecs fail closed.
pub fn decode_glb_images(bytes: &[u8]) -> Result<Vec<DecodedImage>, GlbError> {
    inspect_glb(bytes, "embedded-images.glb")?;
    let container = parse_container(bytes)?;
    let root = &container.json;
    let images = root
        .get("images")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let views = root
        .get("bufferViews")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let buffers = root
        .get("buffers")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    let mut total_encoded = 0usize;
    let mut total_decoded = 0usize;
    let mut decoded = Vec::with_capacity(images.len());
    for (index, image) in images.iter().enumerate() {
        let info = image_info(index, image, views, buffers, container.bin)?;
        if info.0.len() > MAX_ENCODED_IMAGE_BYTES {
            return Err(GlbError::Unsupported(format!(
                "image {index} exceeds the {MAX_ENCODED_IMAGE_BYTES} byte encoded-image limit"
            )));
        }
        total_encoded = total_encoded.checked_add(info.0.len()).ok_or_else(|| {
            GlbError::Unsupported("total embedded image bytes overflow usize".into())
        })?;
        if total_encoded > MAX_TOTAL_IMAGE_BYTES {
            return Err(GlbError::Unsupported(format!(
                "embedded images exceed the {MAX_TOTAL_IMAGE_BYTES} byte aggregate limit"
            )));
        }
        if info.1 != "image/png" {
            return Err(GlbError::Unsupported(format!(
                "image {index} uses MIME type {}; this importer currently decodes embedded PNG only",
                info.1
            )));
        }
        let (width, height, rgba8) = decode_png(index, &info.0)?;
        total_decoded = total_decoded.checked_add(rgba8.len()).ok_or_else(|| {
            GlbError::Unsupported("total decoded RGBA bytes overflow usize".into())
        })?;
        if total_decoded > MAX_TOTAL_IMAGE_BYTES {
            return Err(GlbError::Unsupported(format!(
                "decoded RGBA images exceed the {MAX_TOTAL_IMAGE_BYTES} byte aggregate limit"
            )));
        }
        let name = image
            .get("name")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("image_{index}"));
        decoded.push(DecodedImage {
            index,
            name,
            mime_type: info.1,
            width,
            height,
            rgba8,
        });
    }
    Ok(decoded)
}

fn image_info(
    index: usize,
    image: &Value,
    views: &[Value],
    buffers: &[Value],
    bin: Option<&[u8]>,
) -> Result<(Vec<u8>, String), GlbError> {
    let mime_from_field = image.get("mimeType").and_then(Value::as_str);
    if let Some(view_index) = image.get("bufferView").and_then(Value::as_u64) {
        let view_index = usize::try_from(view_index).map_err(|_| {
            GlbError::InvalidDocument(format!("images[{index}].bufferView does not fit usize"))
        })?;
        let view = views.get(view_index).ok_or_else(|| {
            GlbError::InvalidDocument(format!(
                "images[{index}] references missing bufferView {view_index}"
            ))
        })?;
        let buffer_index = view
            .get("buffer")
            .and_then(Value::as_u64)
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(|| {
                GlbError::InvalidDocument(format!("bufferViews[{view_index}].buffer is invalid"))
            })?;
        let buffer = buffers.get(buffer_index).ok_or_else(|| {
            GlbError::InvalidDocument(format!(
                "bufferViews[{view_index}] references missing buffer {buffer_index}"
            ))
        })?;
        if buffer_index != 0 || buffer.get("uri").is_some() {
            return Err(GlbError::Unsupported(format!(
                "image {index} is stored in an external buffer; only the GLB BIN chunk is supported"
            )));
        }
        let offset = view
            .get("byteOffset")
            .and_then(Value::as_u64)
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(0);
        let length = view
            .get("byteLength")
            .and_then(Value::as_u64)
            .and_then(|v| usize::try_from(v).ok())
            .ok_or_else(|| {
                GlbError::InvalidDocument(format!(
                    "bufferViews[{view_index}].byteLength is invalid"
                ))
            })?;
        let end = offset.checked_add(length).ok_or_else(|| {
            GlbError::InvalidDocument(format!("images[{index}] bufferView range overflows usize"))
        })?;
        let data = bin
            .and_then(|value| value.get(offset..end))
            .ok_or_else(|| {
                GlbError::InvalidDocument(format!("image {index} exceeds the GLB BIN chunk"))
            })?;
        let mime = mime_from_field.ok_or_else(|| {
            GlbError::InvalidDocument(format!("images[{index}] bufferView has no mimeType"))
        })?;
        return Ok((data.to_vec(), mime.to_string()));
    }

    let uri = image.get("uri").and_then(Value::as_str).ok_or_else(|| {
        GlbError::InvalidDocument(format!("image {index} has neither bufferView nor URI"))
    })?;
    if !uri.starts_with("data:") {
        return Err(GlbError::Unsupported(format!(
            "image {index} references external URI {uri}; external files are not resolved during export"
        )));
    }
    let (metadata, payload) = uri.split_once(',').ok_or_else(|| {
        GlbError::InvalidDocument(format!(
            "image {index} data URI is missing a comma separator"
        ))
    })?;
    let mime = metadata
        .strip_prefix("data:")
        .and_then(|value| value.split(';').next())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            GlbError::InvalidDocument(format!("image {index} data URI has no MIME type"))
        })?;
    if !metadata.split(';').any(|part| part == "base64") {
        return Err(GlbError::Unsupported(format!(
            "image {index} data URI is not base64-encoded"
        )));
    }
    let data = STANDARD.decode(payload).map_err(|error| {
        GlbError::InvalidDocument(format!(
            "image {index} data URI has invalid base64: {error}"
        ))
    })?;
    if let Some(declared) = mime_from_field {
        if declared != mime {
            return Err(GlbError::InvalidDocument(format!(
                "image {index} mimeType {declared} conflicts with data URI MIME type {mime}"
            )));
        }
    }
    Ok((data, mime.to_string()))
}

fn decode_png(index: usize, encoded: &[u8]) -> Result<(u32, u32, Vec<u8>), GlbError> {
    if encoded.len() < 8 || &encoded[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err(GlbError::InvalidDocument(format!(
            "image {index} is not a valid PNG stream"
        )));
    }
    let decoder = png::Decoder::new(Cursor::new(encoded));
    let mut decoder = decoder;
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|error| {
        GlbError::InvalidDocument(format!("image {index} PNG header is invalid: {error}"))
    })?;
    let width = reader.info().width;
    let height = reader.info().height;
    validate_dimensions(index, width, height)?;
    let pixel_count = usize::try_from(width)
        .ok()
        .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
        .ok_or_else(|| {
            GlbError::Unsupported(format!("image {index} pixel count overflows usize"))
        })?;
    let rgba_len = pixel_count.checked_mul(4).ok_or_else(|| {
        GlbError::Unsupported(format!("image {index} decoded RGBA length overflows usize"))
    })?;
    if rgba_len > MAX_ENCODED_IMAGE_BYTES {
        return Err(GlbError::Unsupported(format!(
            "image {index} decoded RGBA payload exceeds {MAX_ENCODED_IMAGE_BYTES} bytes"
        )));
    }
    let output_size = reader.output_buffer_size();
    if output_size > MAX_ENCODED_IMAGE_BYTES {
        return Err(GlbError::Unsupported(format!(
            "image {index} PNG output buffer exceeds {MAX_ENCODED_IMAGE_BYTES} bytes"
        )));
    }
    let mut frame = vec![0; output_size];
    let output = reader.next_frame(&mut frame).map_err(|error| {
        GlbError::InvalidDocument(format!("image {index} PNG data is invalid: {error}"))
    })?;
    let frame = &frame[..output.buffer_size()];
    let rgba8 = to_rgba8(index, output.color_type, frame, pixel_count)?;
    Ok((width, height, rgba8))
}

fn validate_dimensions(index: usize, width: u32, height: u32) -> Result<(), GlbError> {
    if width == 0
        || height == 0
        || width as usize > MAX_IMAGE_DIMENSION
        || height as usize > MAX_IMAGE_DIMENSION
    {
        return Err(GlbError::Unsupported(format!(
            "image {index} dimensions {width}x{height} exceed the supported range 1..={MAX_IMAGE_DIMENSION}"
        )));
    }
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| {
            GlbError::Unsupported(format!("image {index} pixel count overflows usize"))
        })?;
    if pixels > MAX_IMAGE_PIXELS {
        return Err(GlbError::Unsupported(format!(
            "image {index} has {pixels} pixels; safety limit is {MAX_IMAGE_PIXELS}"
        )));
    }
    Ok(())
}

fn to_rgba8(
    index: usize,
    color: png::ColorType,
    data: &[u8],
    pixel_count: usize,
) -> Result<Vec<u8>, GlbError> {
    let channels = match color {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => {
            return Err(GlbError::Unsupported(format!(
                "image {index} PNG palette was not expanded by the decoder"
            )))
        }
    };
    let expected = pixel_count.checked_mul(channels).ok_or_else(|| {
        GlbError::Unsupported(format!("image {index} PNG channel byte count overflow"))
    })?;
    if data.len() != expected {
        return Err(GlbError::InvalidDocument(format!(
            "image {index} PNG produced {} bytes; expected {expected}",
            data.len()
        )));
    }
    if channels == 4 {
        return Ok(data.to_vec());
    }
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    for pixel in data.chunks_exact(channels) {
        match channels {
            1 => rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], 255]),
            2 => rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]),
            3 => rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]),
            _ => unreachable!(),
        }
    }
    Ok(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn png_rgba(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut output, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(pixels).unwrap();
        }
        output
    }

    fn glb(json_value: Value, bin: &[u8]) -> Vec<u8> {
        let mut json_bytes = serde_json::to_vec(&json_value).unwrap();
        while json_bytes.len() % 4 != 0 {
            json_bytes.push(b' ');
        }
        let mut bin_bytes = bin.to_vec();
        while bin_bytes.len() % 4 != 0 {
            bin_bytes.push(0);
        }
        let total = 12 + 8 + json_bytes.len() + 8 + bin_bytes.len();
        let mut result = Vec::with_capacity(total);
        result.extend_from_slice(b"glTF");
        result.extend_from_slice(&2u32.to_le_bytes());
        result.extend_from_slice(&(total as u32).to_le_bytes());
        result.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        result.extend_from_slice(&0x4E4F534Au32.to_le_bytes());
        result.extend_from_slice(&json_bytes);
        result.extend_from_slice(&(bin_bytes.len() as u32).to_le_bytes());
        result.extend_from_slice(&0x004E4942u32.to_le_bytes());
        result.extend_from_slice(&bin_bytes);
        result
    }

    #[test]
    fn extracts_and_decodes_png_from_bin_buffer_view() {
        let pixels = [255, 90, 20, 255, 10, 20, 30, 128];
        let encoded = png_rgba(2, 1, &pixels);
        let json = json!({
            "asset":{"version":"2.0"},
            "buffers":[{"byteLength":encoded.len()}],
            "bufferViews":[{"buffer":0,"byteOffset":0,"byteLength":encoded.len()}],
            "images":[{"name":"pumpkin_albedo","mimeType":"image/png","bufferView":0}]
        });
        let bytes = glb(json, &encoded);
        let images = decode_glb_images(&bytes).unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].name, "pumpkin_albedo");
        assert_eq!((images[0].width, images[0].height), (2, 1));
        assert_eq!(images[0].rgba8, pixels);
    }

    #[test]
    fn extracts_png_from_base64_data_uri() {
        let pixels = [40, 50, 60, 255];
        let encoded = png_rgba(1, 1, &pixels);
        let uri = format!("data:image/png;base64,{}", STANDARD.encode(&encoded));
        let bytes = glb(
            json!({"asset":{"version":"2.0"},"images":[{"uri":uri}]}),
            &[],
        );
        let images = decode_glb_images(&bytes).unwrap();
        assert_eq!(images[0].rgba8, pixels);
    }

    #[test]
    fn rejects_external_images_and_unsupported_mime_types() {
        let external = glb(
            json!({"asset":{"version":"2.0"},"images":[{"uri":"albedo.png"}]}),
            &[],
        );
        assert!(decode_glb_images(&external)
            .unwrap_err()
            .to_string()
            .contains("external URI"));
        let jpeg = glb(
            json!({"asset":{"version":"2.0"},"images":[{"uri":"data:image/jpeg;base64,AAAA"}]}),
            &[],
        );
        assert!(decode_glb_images(&jpeg)
            .unwrap_err()
            .to_string()
            .contains("PNG only"));
    }

    #[test]
    fn rejects_oversized_image_dimensions_before_decoding_pixels() {
        let encoded = png_rgba(1, 1, &[0, 0, 0, 0]);
        let mut malformed = encoded;
        malformed[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        let bytes = glb(
            json!({
                "asset":{"version":"2.0"},
                "buffers":[{"byteLength":malformed.len()}],
                "bufferViews":[{"buffer":0,"byteLength":malformed.len()}],
                "images":[{"mimeType":"image/png","bufferView":0}]
            }),
            &malformed,
        );
        assert!(decode_glb_images(&bytes).is_err());
    }
}

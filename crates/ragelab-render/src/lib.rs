//! Native renderer for RageLab's renderer-neutral `RenderPackage` contract.
//!
//! This crate intentionally does not depend on RAGE format crates. Parsing,
//! dependency resolution and material semantics remain owned by `ragelab-engine`.

mod gpu;

use std::{error::Error, fmt, fs, io, path::Path};

use ragelab_engine::RenderPackage;
use serde::Serialize;
use sha2::{Digest, Sha256};

pub use gpu::{OffscreenRenderer, SurfaceRenderer};

pub const SCREENSHOT_METADATA_SCHEMA_VERSION: u32 = 1;

#[derive(Debug)]
pub enum RenderError {
    Io(io::Error),
    InvalidInput(String),
    Unsupported(String),
    Gpu(String),
    Png(String),
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::InvalidInput(message) => write!(formatter, "{message}"),
            Self::Unsupported(message) => write!(formatter, "{message}"),
            Self::Gpu(message) => write!(formatter, "{message}"),
            Self::Png(message) => write!(formatter, "{message}"),
        }
    }
}

impl Error for RenderError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidInput(_) | Self::Unsupported(_) | Self::Gpu(_) | Self::Png(_) => None,
        }
    }
}

impl From<io::Error> for RenderError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

pub type RenderResult<T> = Result<T, RenderError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RenderView {
    Auto,
    Front,
    Back,
    Left,
    Right,
    Top,
    Isometric,
}

impl RenderView {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "front" => Some(Self::Front),
            "back" => Some(Self::Back),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "top" => Some(Self::Top),
            "isometric" | "iso" => Some(Self::Isometric),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Front => "front",
            Self::Back => "back",
            Self::Left => "left",
            Self::Right => "right",
            Self::Top => "top",
            Self::Isometric => "isometric",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Projection {
    Perspective,
    Orthographic,
}

impl Projection {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "perspective" | "persp" => Some(Self::Perspective),
            "orthographic" | "ortho" => Some(Self::Orthographic),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Perspective => "perspective",
            Self::Orthographic => "orthographic",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OffscreenOptions {
    pub width: u32,
    pub height: u32,
    pub view: RenderView,
    pub projection: Projection,
    pub transparent: bool,
    pub grid: bool,
    pub wireframe: bool,
    pub bounds: bool,
}

impl Default for OffscreenOptions {
    fn default() -> Self {
        Self {
            width: 1024,
            height: 1024,
            view: RenderView::Auto,
            projection: Projection::Perspective,
            transparent: false,
            grid: false,
            wireframe: false,
            bounds: false,
        }
    }
}

impl OffscreenOptions {
    pub fn validate(self) -> RenderResult<Self> {
        if self.width == 0 || self.height == 0 {
            return Err(RenderError::InvalidInput(
                "render width and height must be greater than zero".into(),
            ));
        }
        if self.width > 8192 || self.height > 8192 {
            return Err(RenderError::InvalidInput(
                "render width and height must not exceed 8192".into(),
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewportOptions {
    pub width: u32,
    pub height: u32,
    pub projection: Projection,
    pub grid: bool,
    pub wireframe: bool,
    pub bounds: bool,
}

impl Default for ViewportOptions {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            projection: Projection::Perspective,
            grid: true,
            wireframe: false,
            bounds: false,
        }
    }
}

impl ViewportOptions {
    pub fn validate(self) -> RenderResult<Self> {
        if self.width == 0 || self.height == 0 {
            return Err(RenderError::InvalidInput(
                "viewport width and height must be greater than zero".into(),
            ));
        }
        if self.width > 8192 || self.height > 8192 {
            return Err(RenderError::InvalidInput(
                "viewport width and height must not exceed 8192".into(),
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraSnapshot {
    pub target: [f32; 3],
    pub eye: [f32; 3],
    pub yaw_radians: f32,
    pub pitch_radians: f32,
    pub distance: f32,
    pub projection: Projection,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickResult {
    pub node_index: u32,
    pub distance: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuCacheBudget {
    pub max_assets: u32,
    pub max_textures: u32,
    pub max_asset_bytes: u64,
    pub max_texture_bytes: u64,
}

impl Default for GpuCacheBudget {
    fn default() -> Self {
        Self {
            max_assets: 2_048,
            max_textures: 4_096,
            max_asset_bytes: 384 * 1024 * 1024,
            max_texture_bytes: 256 * 1024 * 1024,
        }
    }
}

impl GpuCacheBudget {
    pub fn validate(self) -> RenderResult<Self> {
        if self.max_assets == 0
            || self.max_textures == 0
            || self.max_asset_bytes == 0
            || self.max_texture_bytes == 0
        {
            return Err(RenderError::InvalidInput(
                "GPU cache budgets must be greater than zero".into(),
            ));
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewportStats {
    pub width: u32,
    pub height: u32,
    pub instances: u32,
    pub assets: u32,
    pub meshes: u32,
    pub materials: u32,
    pub textures: u32,
    pub gpu_asset_cache: u32,
    pub gpu_texture_cache: u32,
    pub gpu_asset_cache_bytes: u64,
    pub gpu_texture_cache_bytes: u64,
    pub gpu_cache_hits: u64,
    pub gpu_cache_misses: u64,
    pub gpu_evictions: u64,
    pub gpu_budget_overflow: bool,
    pub uploaded_payload_bytes: u64,
    pub scene_load_ms: f64,
    pub last_frame_ms: f64,
    pub selected_node_index: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl RenderedImage {
    pub fn write_png(&self, path: &Path) -> RenderResult<()> {
        let expected = usize::try_from(self.width)
            .ok()
            .and_then(|width| {
                usize::try_from(self.height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| RenderError::InvalidInput("image dimensions overflow usize".into()))?;
        if self.rgba.len() != expected {
            return Err(RenderError::InvalidInput(format!(
                "RGBA buffer length {} does not match {}x{}",
                self.rgba.len(),
                self.width,
                self.height
            )));
        }

        let file = fs::File::create(path)?;
        let writer = io::BufWriter::new(file);
        let mut encoder = png::Encoder::new(writer, self.width, self.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| RenderError::Png(error.to_string()))?;
        writer
            .write_image_data(&self.rgba)
            .map_err(|error| RenderError::Png(error.to_string()))
    }

    pub fn sha256(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(&self.rgba);
        format!("{:X}", hash.finalize())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotMetadata {
    pub schema: &'static str,
    pub schema_version: u32,
    pub width: u32,
    pub height: u32,
    pub view: RenderView,
    pub projection: Projection,
    pub transparent: bool,
    pub grid: bool,
    pub wireframe: bool,
    pub bounds: bool,
    pub package_schema_version: u32,
    pub instances: u32,
    pub assets: u32,
    pub meshes: u32,
    pub materials: u32,
    pub textures: u32,
    pub image_sha256: String,
}

impl ScreenshotMetadata {
    pub fn new(package: &RenderPackage, options: OffscreenOptions, image: &RenderedImage) -> Self {
        Self {
            schema: "ragelab.render.screenshot",
            schema_version: SCREENSHOT_METADATA_SCHEMA_VERSION,
            width: image.width,
            height: image.height,
            view: options.view,
            projection: options.projection,
            transparent: options.transparent,
            grid: options.grid,
            wireframe: options.wireframe,
            bounds: options.bounds,
            package_schema_version: package.descriptor.schema_version,
            instances: package.descriptor.summary.instances,
            assets: package.descriptor.summary.assets,
            meshes: package.descriptor.summary.meshes,
            materials: package.descriptor.summary.materials,
            textures: package.descriptor.summary.textures,
            image_sha256: image.sha256(),
        }
    }

    pub fn write_json(&self, path: &Path) -> RenderResult<()> {
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| RenderError::InvalidInput(error.to_string()))?;
        fs::write(path, bytes)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualDiff {
    pub pixels: u64,
    pub changed_pixels: u64,
    pub max_channel_delta: u8,
    pub mean_absolute_channel_error: f64,
    pub tolerance: u8,
    pub within_tolerance: bool,
}

pub fn compare_rgba(
    expected: &RenderedImage,
    actual: &RenderedImage,
    tolerance: u8,
) -> RenderResult<VisualDiff> {
    if expected.width != actual.width || expected.height != actual.height {
        return Err(RenderError::InvalidInput(format!(
            "image dimensions differ: expected {}x{}, actual {}x{}",
            expected.width, expected.height, actual.width, actual.height
        )));
    }
    if expected.rgba.len() != actual.rgba.len() {
        return Err(RenderError::InvalidInput(
            "RGBA buffer lengths differ".into(),
        ));
    }

    let mut changed_pixels = 0_u64;
    let mut max_delta = 0_u8;
    let mut delta_sum = 0_u64;

    for (expected_pixel, actual_pixel) in expected
        .rgba
        .chunks_exact(4)
        .zip(actual.rgba.chunks_exact(4))
    {
        let mut changed = false;
        for channel in 0..4 {
            let delta = expected_pixel[channel].abs_diff(actual_pixel[channel]);
            max_delta = max_delta.max(delta);
            delta_sum = delta_sum.saturating_add(u64::from(delta));
            changed |= delta > tolerance;
        }
        if changed {
            changed_pixels = changed_pixels.saturating_add(1);
        }
    }

    let pixels = u64::from(expected.width) * u64::from(expected.height);
    let channels = pixels.saturating_mul(4).max(1);
    Ok(VisualDiff {
        pixels,
        changed_pixels,
        max_channel_delta: max_delta,
        mean_absolute_channel_error: delta_sum as f64 / channels as f64,
        tolerance,
        within_tolerance: changed_pixels == 0,
    })
}

pub fn read_png(path: &Path) -> RenderResult<RenderedImage> {
    let file = fs::File::open(path)?;
    let decoder = png::Decoder::new(io::BufReader::new(file));
    let mut reader = decoder
        .read_info()
        .map_err(|error| RenderError::Png(error.to_string()))?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|error| RenderError::Png(error.to_string()))?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(RenderError::Unsupported(format!(
            "visual regression expects RGBA8 PNG; found {:?} {:?}",
            info.color_type, info.bit_depth
        )));
    }
    buffer.truncate(info.buffer_size());
    Ok(RenderedImage {
        width: info.width,
        height: info.height,
        rgba: buffer,
    })
}

pub fn compare_png_files(
    expected: &Path,
    actual: &Path,
    tolerance: u8,
) -> RenderResult<VisualDiff> {
    let expected = read_png(expected)?;
    let actual = read_png(actual)?;
    compare_rgba(&expected, &actual, tolerance)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_diff_respects_per_channel_tolerance() {
        let expected = RenderedImage {
            width: 1,
            height: 1,
            rgba: vec![10, 20, 30, 255],
        };
        let actual = RenderedImage {
            width: 1,
            height: 1,
            rgba: vec![11, 22, 27, 255],
        };
        let strict = compare_rgba(&expected, &actual, 0).unwrap();
        assert!(!strict.within_tolerance);
        assert_eq!(strict.changed_pixels, 1);
        assert_eq!(strict.max_channel_delta, 3);

        let tolerant = compare_rgba(&expected, &actual, 3).unwrap();
        assert!(tolerant.within_tolerance);
        assert_eq!(tolerant.changed_pixels, 0);
    }

    #[test]
    fn options_reject_zero_or_unbounded_dimensions() {
        assert!(OffscreenOptions {
            width: 0,
            ..OffscreenOptions::default()
        }
        .validate()
        .is_err());
        assert!(OffscreenOptions {
            width: 9000,
            ..OffscreenOptions::default()
        }
        .validate()
        .is_err());
    }
}

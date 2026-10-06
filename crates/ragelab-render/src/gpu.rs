use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    sync::mpsc,
    time::Instant,
};

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Quat, Vec3};
use ragelab_engine::{
    RenderAssetDescriptor, RenderAssetState, RenderBufferView, RenderElementType, RenderPackage,
    RenderTextureDescriptor,
};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use wgpu::util::DeviceExt;

use crate::{
    CameraSnapshot, GpuCacheBudget, OffscreenOptions, PickResult, Projection, RenderError,
    RenderResult, RenderView, RenderedImage, ViewportOptions, ViewportStats,
};

const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

const MESH_SHADER: &str = r#"
struct Camera {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    camera_pos: vec4<f32>,
};

struct Model {
    model: mat4x4<f32>,
    normal: mat4x4<f32>,
    style: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

@group(1) @binding(0)
var<uniform> model: Model;

@group(2) @binding(0)
var diffuse_texture: texture_2d<f32>;

@group(2) @binding(1)
var diffuse_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv0: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) uv0: vec2<f32>,
    @location(2) @interpolate(flat) style: vec2<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    let world_position = model.model * vec4<f32>(input.position, 1.0);
    output.position = camera.view_proj * world_position;
    output.world_normal = normalize((model.normal * vec4<f32>(input.normal, 0.0)).xyz);
    output.uv0 = input.uv0;
    output.style = model.style.xy;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let base = textureSample(diffuse_texture, diffuse_sampler, input.uv0);
    let normal = normalize(input.world_normal);
    let diffuse = max(dot(normal, normalize(camera.light_dir.xyz)), 0.0);
    let lighting = 0.34 + 0.66 * diffuse;
    var shaded = base.rgb * lighting;
    if (input.style.y > 0.5) {
        shaded = mix(shaded, vec3<f32>(0.35, 0.48, 0.62), 0.10);
    }
    if (input.style.x > 0.5) {
        shaded = mix(shaded, vec3<f32>(0.30, 0.62, 1.0), 0.58) + vec3<f32>(0.08, 0.10, 0.14);
    }
    return vec4<f32>(shaded, base.a);
}
"#;

const LINE_SHADER: &str = r#"
struct Camera {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    camera_pos: vec4<f32>,
};

struct Model {
    model: mat4x4<f32>,
    normal: mat4x4<f32>,
    style: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

@group(1) @binding(0)
var<uniform> model: Model;

@vertex
fn vs_line(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return camera.view_proj * model.model * vec4<f32>(position, 1.0);
}

@fragment
fn fs_line() -> @location(0) vec4<f32> {
    return vec4<f32>(0.95, 0.62, 0.16, 0.82);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    light_dir: [f32; 4],
    camera_pos: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ModelUniform {
    model: [[f32; 4]; 4],
    normal: [[f32; 4]; 4],
    style: [f32; 4],
}

struct ModelBinding {
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

struct GpuTexture {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}

struct GpuMaterial {
    bind_group: wgpu::BindGroup,
}

struct GpuMesh {
    positions: wgpu::Buffer,
    normals: wgpu::Buffer,
    uv0: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    wire_indices: wgpu::Buffer,
    wire_index_count: u32,
    material_ref: Option<u32>,
}

struct GpuAsset {
    meshes: Vec<GpuMesh>,
    materials: Vec<GpuMaterial>,
    fallback_material: GpuMaterial,
}

#[derive(Debug, Clone)]
struct GpuAssetCacheMeta {
    bytes: u64,
    last_used: u64,
    texture_keys: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct GpuTextureCacheMeta {
    bytes: u64,
    last_used: u64,
}

struct DrawInstance {
    asset_key: String,
    model: Mat4,
    model_binding: ModelBinding,
    node_index: Option<u32>,
    local_overlay: bool,
    world_bounds: Aabb,
}

#[derive(Debug, Clone, Copy)]
struct Aabb {
    min: Vec3,
    max: Vec3,
}

impl Aabb {
    fn empty() -> Self {
        Self {
            min: Vec3::splat(f32::INFINITY),
            max: Vec3::splat(f32::NEG_INFINITY),
        }
    }

    fn include(&mut self, point: Vec3) {
        self.min = self.min.min(point);
        self.max = self.max.max(point);
    }

    fn is_valid(self) -> bool {
        self.min.is_finite() && self.max.is_finite() && self.min.cmple(self.max).all()
    }

    fn center(self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    fn extent(self) -> Vec3 {
        (self.max - self.min).max(Vec3::splat(0.001))
    }

    fn radius(self) -> f32 {
        (self.extent().length() * 0.5).max(0.01)
    }
}

pub struct OffscreenRenderer {
    _instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    camera_layout: wgpu::BindGroupLayout,
    model_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    mesh_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    white_texture: GpuTexture,
    asset_cache: HashMap<String, GpuAsset>,
    texture_cache: HashMap<String, GpuTexture>,
    asset_cache_meta: HashMap<String, GpuAssetCacheMeta>,
    texture_cache_meta: HashMap<String, GpuTextureCacheMeta>,
    cache_epoch: u64,
    cache_hits: u64,
    cache_misses: u64,
    cache_evictions: u64,
}

impl OffscreenRenderer {
    pub fn new() -> RenderResult<Self> {
        pollster::block_on(Self::new_async())
    }

    async fn new_async() -> RenderResult<Self> {
        Self::new_async_for_instance(wgpu::Instance::default(), None).await
    }

    async fn new_async_for_instance(
        instance: wgpu::Instance,
        compatible_surface: Option<&wgpu::Surface<'_>>,
    ) -> RenderResult<Self> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface,
                force_fallback_adapter: false,
            })
            .await
            .ok_or_else(|| RenderError::Gpu("no compatible wgpu adapter was found".into()))?;

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("RageLab offscreen device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults(),
                    memory_hints: wgpu::MemoryHints::Performance,
                },
                None,
            )
            .await
            .map_err(|error| RenderError::Gpu(error.to_string()))?;

        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("RageLab camera layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let model_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("RageLab model layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("RageLab material layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("RageLab diffuse sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let white_texture = create_texture(
            &device,
            &queue,
            "RageLab fallback white",
            1,
            1,
            &[230, 230, 230, 255],
        )?;

        let mesh_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("RageLab mesh shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(MESH_SHADER)),
        });
        let line_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("RageLab line shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(LINE_SHADER)),
        });

        let mesh_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("RageLab mesh pipeline layout"),
            bind_group_layouts: &[&camera_layout, &model_layout, &material_layout],
            push_constant_ranges: &[],
        });
        let line_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("RageLab line pipeline layout"),
            bind_group_layouts: &[&camera_layout, &model_layout],
            push_constant_ranges: &[],
        });

        const POSITION_ATTRIBUTES: [wgpu::VertexAttribute; 1] =
            wgpu::vertex_attr_array![0 => Float32x3];
        const NORMAL_ATTRIBUTES: [wgpu::VertexAttribute; 1] =
            wgpu::vertex_attr_array![1 => Float32x3];
        const UV_ATTRIBUTES: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![2 => Float32x2];

        let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("RageLab mesh pipeline"),
            layout: Some(&mesh_layout),
            vertex: wgpu::VertexState {
                module: &mesh_shader,
                entry_point: "vs_main",
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: 12,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &POSITION_ATTRIBUTES,
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: 12,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &NORMAL_ATTRIBUTES,
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: 8,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &UV_ATTRIBUTES,
                    },
                ],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &mesh_shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview: None,
            cache: None,
        });

        let line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("RageLab line pipeline"),
            layout: Some(&line_layout),
            vertex: wgpu::VertexState {
                module: &line_shader,
                entry_point: "vs_line",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &POSITION_ATTRIBUTES,
                }],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &line_shader,
                entry_point: "fs_line",
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview: None,
            cache: None,
        });

        Ok(Self {
            _instance: instance,
            adapter,
            device,
            queue,
            camera_layout,
            model_layout,
            material_layout,
            mesh_pipeline,
            line_pipeline,
            sampler,
            white_texture,
            asset_cache: HashMap::new(),
            texture_cache: HashMap::new(),
            asset_cache_meta: HashMap::new(),
            texture_cache_meta: HashMap::new(),
            cache_epoch: 0,
            cache_hits: 0,
            cache_misses: 0,
            cache_evictions: 0,
        })
    }

    pub fn render(
        &mut self,
        package: &RenderPackage,
        options: OffscreenOptions,
    ) -> RenderResult<RenderedImage> {
        let options = options.validate()?;
        package
            .validate()
            .map_err(|error| RenderError::InvalidInput(error.to_string()))?;

        for texture in &package.descriptor.textures {
            self.ensure_texture(package, texture)?;
        }
        for asset in &package.descriptor.assets {
            if asset.state == RenderAssetState::Ready {
                self.ensure_asset(package, asset)?;
            }
        }

        let draws = self.build_draw_instances(package, false)?;
        if draws.is_empty() {
            return Err(RenderError::Unsupported(
                "render package has no ready drawable instances".into(),
            ));
        }

        let world_bounds = package_world_bounds(package, &draws)?;
        let camera = camera_for_bounds(world_bounds, options);
        let camera_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("RageLab camera uniform"),
                contents: bytemuck::bytes_of(&camera),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let camera_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("RageLab camera bind group"),
            layout: &self.camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        let identity_model = self.create_model_binding(Mat4::IDENTITY, [0.0; 4]);
        let overlay_vertices = build_overlay_vertices(world_bounds, options);
        let overlay_buffer = (!overlay_vertices.is_empty()).then(|| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("RageLab overlay lines"),
                    contents: bytemuck::cast_slice(&overlay_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                })
        });

        let color = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("RageLab offscreen color"),
            size: wgpu::Extent3d {
                width: options.width,
                height: options.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        let depth = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("RageLab offscreen depth"),
            size: wgpu::Extent3d {
                width: options.width,
                height: options.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());

        let unpadded_bytes_per_row = options
            .width
            .checked_mul(4)
            .ok_or_else(|| RenderError::InvalidInput("image row size overflow".into()))?;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(align).saturating_mul(align);
        let readback_size = u64::from(padded_bytes_per_row)
            .checked_mul(u64::from(options.height))
            .ok_or_else(|| RenderError::InvalidInput("readback size overflow".into()))?;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("RageLab offscreen readback"),
            size: readback_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("RageLab offscreen encoder"),
            });
        {
            let clear = if options.transparent {
                wgpu::Color::TRANSPARENT
            } else {
                wgpu::Color {
                    r: 0.035,
                    g: 0.045,
                    b: 0.06,
                    a: 1.0,
                }
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("RageLab offscreen pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            pass.set_bind_group(0, &camera_bind_group, &[]);
            pass.set_pipeline(&self.mesh_pipeline);
            for draw in &draws {
                let Some(asset) = self.asset_cache.get(&draw.asset_key) else {
                    return Err(RenderError::Gpu(format!(
                        "GPU asset cache missing {}",
                        draw.asset_key
                    )));
                };
                pass.set_bind_group(1, &draw.model_binding.bind_group, &[]);
                for mesh in &asset.meshes {
                    let material = mesh
                        .material_ref
                        .and_then(|index| asset.materials.get(index as usize))
                        .unwrap_or(&asset.fallback_material);
                    pass.set_bind_group(2, &material.bind_group, &[]);
                    pass.set_vertex_buffer(0, mesh.positions.slice(..));
                    pass.set_vertex_buffer(1, mesh.normals.slice(..));
                    pass.set_vertex_buffer(2, mesh.uv0.slice(..));
                    pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                }
            }

            if options.wireframe {
                pass.set_pipeline(&self.line_pipeline);
                for draw in &draws {
                    let Some(asset) = self.asset_cache.get(&draw.asset_key) else {
                        continue;
                    };
                    pass.set_bind_group(1, &draw.model_binding.bind_group, &[]);
                    for mesh in &asset.meshes {
                        if mesh.wire_index_count == 0 {
                            continue;
                        }
                        pass.set_vertex_buffer(0, mesh.positions.slice(..));
                        pass.set_index_buffer(
                            mesh.wire_indices.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..mesh.wire_index_count, 0, 0..1);
                    }
                }
            }

            if let Some(buffer) = overlay_buffer.as_ref() {
                pass.set_pipeline(&self.line_pipeline);
                pass.set_bind_group(1, &identity_model.bind_group, &[]);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..overlay_vertices.len() as u32, 0..1);
            }
        }

        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &readback,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(options.height),
                },
            },
            wgpu::Extent3d {
                width: options.width,
                height: options.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));

        let slice = readback.slice(..);
        let (sender, receiver) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device.poll(wgpu::Maintain::Wait);
        receiver
            .recv()
            .map_err(|error| RenderError::Gpu(error.to_string()))?
            .map_err(|error| RenderError::Gpu(error.to_string()))?;

        let mapped = slice.get_mapped_range();
        let output_len = usize::try_from(options.width)
            .ok()
            .and_then(|width| {
                usize::try_from(options.height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| RenderError::InvalidInput("image buffer size overflow".into()))?;
        let mut rgba = vec![0_u8; output_len];
        let source_stride = padded_bytes_per_row as usize;
        let target_stride = unpadded_bytes_per_row as usize;
        for row in 0..options.height as usize {
            let source_start = row * source_stride;
            let target_start = row * target_stride;
            rgba[target_start..target_start + target_stride]
                .copy_from_slice(&mapped[source_start..source_start + target_stride]);
        }
        drop(mapped);
        readback.unmap();

        Ok(RenderedImage {
            width: options.width,
            height: options.height,
            rgba,
        })
    }

    fn ensure_texture(
        &mut self,
        package: &RenderPackage,
        descriptor: &RenderTextureDescriptor,
    ) -> RenderResult<()> {
        let key = texture_key(descriptor);
        self.cache_epoch = self.cache_epoch.saturating_add(1);
        let epoch = self.cache_epoch;
        if self.texture_cache.contains_key(&key) {
            self.cache_hits = self.cache_hits.saturating_add(1);
            if let Some(meta) = self.texture_cache_meta.get_mut(&key) {
                meta.last_used = epoch;
            }
            return Ok(());
        }
        self.cache_misses = self.cache_misses.saturating_add(1);
        let bytes = view_bytes(
            package,
            descriptor.data,
            RenderElementType::U8,
            4,
            "texture",
        )?;
        let texture = create_texture(
            &self.device,
            &self.queue,
            &format!("RageLab texture {}", descriptor.name),
            u32::from(descriptor.width),
            u32::from(descriptor.height),
            bytes,
        )?;
        self.texture_cache.insert(key.clone(), texture);
        self.texture_cache_meta.insert(
            key,
            GpuTextureCacheMeta {
                bytes: descriptor.data.byte_length,
                last_used: epoch,
            },
        );
        Ok(())
    }

    fn ensure_asset(
        &mut self,
        package: &RenderPackage,
        descriptor: &RenderAssetDescriptor,
    ) -> RenderResult<()> {
        let key = asset_key(descriptor);
        self.cache_epoch = self.cache_epoch.saturating_add(1);
        let epoch = self.cache_epoch;
        if self.asset_cache.contains_key(&key) {
            self.cache_hits = self.cache_hits.saturating_add(1);
            if let Some(meta) = self.asset_cache_meta.get_mut(&key) {
                meta.last_used = epoch;
            }
            return Ok(());
        }
        self.cache_misses = self.cache_misses.saturating_add(1);

        let texture_keys = descriptor
            .materials
            .iter()
            .filter_map(|material| material.diffuse_texture_ref)
            .filter_map(|texture_ref| package.descriptor.textures.get(texture_ref as usize))
            .map(texture_key)
            .collect::<Vec<_>>();
        let mut gpu_bytes = 0_u64;
        let mut materials = Vec::with_capacity(descriptor.materials.len());
        for material in &descriptor.materials {
            let view = if let Some(texture_ref) = material.diffuse_texture_ref {
                let texture = package
                    .descriptor
                    .textures
                    .get(texture_ref as usize)
                    .ok_or_else(|| {
                        RenderError::InvalidInput(format!(
                            "asset {} material references missing texture {}",
                            descriptor.asset_ref, texture_ref
                        ))
                    })?;
                let texture_key = texture_key(texture);
                &self
                    .texture_cache
                    .get(&texture_key)
                    .ok_or_else(|| {
                        RenderError::Gpu(format!(
                            "texture cache missing {} for asset {}",
                            texture_key, descriptor.asset_ref
                        ))
                    })?
                    .view
            } else {
                &self.white_texture.view
            };
            materials.push(self.create_material(view));
        }
        let fallback_material = self.create_material(&self.white_texture.view);

        let mut meshes = Vec::with_capacity(descriptor.meshes.len());
        for mesh in &descriptor.meshes {
            let positions = view_bytes(
                package,
                mesh.positions,
                RenderElementType::F32,
                3,
                "positions",
            )?;
            let vertex_count = mesh.positions.count as usize;
            let normals_owned;
            let normals = if let Some(view) = mesh.normals {
                view_bytes(package, view, RenderElementType::F32, 3, "normals")?
            } else {
                normals_owned = repeated_f32x3(vertex_count, [0.0, 0.0, 1.0]);
                &normals_owned
            };
            let uv_owned;
            let uv0 = if let Some(view) = mesh.uv0 {
                view_bytes(package, view, RenderElementType::F32, 2, "uv0")?
            } else {
                uv_owned = repeated_f32x2(vertex_count, [0.0, 0.0]);
                &uv_owned
            };
            let indices = view_bytes(package, mesh.indices, RenderElementType::U32, 1, "indices")?;
            let wire = wire_indices(indices)?;
            gpu_bytes = gpu_bytes
                .saturating_add(mesh.positions.byte_length)
                .saturating_add(mesh.indices.byte_length)
                .saturating_add(
                    mesh.normals
                        .map(|view| view.byte_length)
                        .unwrap_or((vertex_count as u64).saturating_mul(12)),
                )
                .saturating_add(
                    mesh.uv0
                        .map(|view| view.byte_length)
                        .unwrap_or((vertex_count as u64).saturating_mul(8)),
                )
                .saturating_add((wire.len() as u64).saturating_mul(4));

            meshes.push(GpuMesh {
                positions: self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("RageLab positions"),
                        contents: positions,
                        usage: wgpu::BufferUsages::VERTEX,
                    }),
                normals: self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("RageLab normals"),
                        contents: normals,
                        usage: wgpu::BufferUsages::VERTEX,
                    }),
                uv0: self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("RageLab uv0"),
                        contents: uv0,
                        usage: wgpu::BufferUsages::VERTEX,
                    }),
                indices: self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("RageLab indices"),
                        contents: indices,
                        usage: wgpu::BufferUsages::INDEX,
                    }),
                index_count: mesh.indices.count,
                wire_indices: self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("RageLab wire indices"),
                        contents: bytemuck::cast_slice(&wire),
                        usage: wgpu::BufferUsages::INDEX,
                    }),
                wire_index_count: wire.len().try_into().map_err(|_| {
                    RenderError::InvalidInput("wireframe index count exceeds u32".into())
                })?,
                material_ref: mesh.material_ref,
            });
        }

        self.asset_cache.insert(
            key.clone(),
            GpuAsset {
                meshes,
                materials,
                fallback_material,
            },
        );
        self.asset_cache_meta.insert(
            key,
            GpuAssetCacheMeta {
                bytes: gpu_bytes,
                last_used: epoch,
                texture_keys,
            },
        );
        Ok(())
    }

    fn cache_bytes(&self) -> (u64, u64) {
        let asset_bytes = self
            .asset_cache_meta
            .values()
            .map(|meta| meta.bytes)
            .fold(0_u64, u64::saturating_add);
        let texture_bytes = self
            .texture_cache_meta
            .values()
            .map(|meta| meta.bytes)
            .fold(0_u64, u64::saturating_add);
        (asset_bytes, texture_bytes)
    }

    fn evict_to_budget(&mut self, package: &RenderPackage, budget: GpuCacheBudget) -> bool {
        let protected_assets = package
            .descriptor
            .assets
            .iter()
            .filter(|asset| asset.state == RenderAssetState::Ready)
            .map(asset_key)
            .collect::<HashSet<_>>();

        loop {
            let (asset_bytes, _) = self.cache_bytes();
            if self.asset_cache.len() <= budget.max_assets as usize
                && asset_bytes <= budget.max_asset_bytes
            {
                break;
            }
            let victim = self
                .asset_cache_meta
                .iter()
                .filter(|(key, _)| !protected_assets.contains(*key))
                .map(|(key, meta)| (meta.last_used, key.as_str()))
                .min_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(right.1)))
                .map(|(_, key)| key.to_string());
            let Some(key) = victim else {
                break;
            };
            self.asset_cache.remove(&key);
            self.asset_cache_meta.remove(&key);
            self.cache_evictions = self.cache_evictions.saturating_add(1);
        }

        let mut protected_textures = package
            .descriptor
            .textures
            .iter()
            .map(texture_key)
            .collect::<HashSet<_>>();
        for meta in self.asset_cache_meta.values() {
            protected_textures.extend(meta.texture_keys.iter().cloned());
        }

        loop {
            let (_, texture_bytes) = self.cache_bytes();
            if self.texture_cache.len() <= budget.max_textures as usize
                && texture_bytes <= budget.max_texture_bytes
            {
                break;
            }
            let victim = self
                .texture_cache_meta
                .iter()
                .filter(|(key, _)| !protected_textures.contains(*key))
                .map(|(key, meta)| (meta.last_used, key.as_str()))
                .min_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(right.1)))
                .map(|(_, key)| key.to_string());
            let Some(key) = victim else {
                break;
            };
            self.texture_cache.remove(&key);
            self.texture_cache_meta.remove(&key);
            self.cache_evictions = self.cache_evictions.saturating_add(1);
        }

        let (asset_bytes, texture_bytes) = self.cache_bytes();
        self.asset_cache.len() > budget.max_assets as usize
            || self.texture_cache.len() > budget.max_textures as usize
            || asset_bytes > budget.max_asset_bytes
            || texture_bytes > budget.max_texture_bytes
    }

    fn create_material(&self, view: &wgpu::TextureView) -> GpuMaterial {
        GpuMaterial {
            bind_group: self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("RageLab material bind group"),
                layout: &self.material_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            }),
        }
    }

    fn create_model_binding(&self, model: Mat4, style: [f32; 4]) -> ModelBinding {
        let uniform = model_uniform(model, style);
        let buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("RageLab model uniform"),
                contents: bytemuck::bytes_of(&uniform),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("RageLab model bind group"),
            layout: &self.model_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        ModelBinding { buffer, bind_group }
    }

    fn update_model_binding(
        &self,
        draw: &DrawInstance,
        selected_node: Option<u32>,
        emphasize_local: bool,
    ) {
        let selected = draw.node_index.is_some() && draw.node_index == selected_node;
        let style = [
            if selected { 1.0 } else { 0.0 },
            if emphasize_local && draw.local_overlay {
                1.0
            } else {
                0.0
            },
            0.0,
            0.0,
        ];
        let uniform = model_uniform(draw.model, style);
        self.queue
            .write_buffer(&draw.model_binding.buffer, 0, bytemuck::bytes_of(&uniform));
    }

    fn build_draw_instances(
        &self,
        package: &RenderPackage,
        emphasize_local: bool,
    ) -> RenderResult<Vec<DrawInstance>> {
        let mut draws = Vec::new();

        if package.descriptor.scene.instances.is_empty() {
            for asset in &package.descriptor.assets {
                if asset.state != RenderAssetState::Ready {
                    continue;
                }
                let model = Mat4::IDENTITY;
                let world_bounds = asset_world_bounds(asset, model)?;
                draws.push(DrawInstance {
                    asset_key: asset_key(asset),
                    model,
                    model_binding: self.create_model_binding(model, [0.0; 4]),
                    node_index: None,
                    local_overlay: false,
                    world_bounds,
                });
            }
            return Ok(draws);
        }

        for instance in &package.descriptor.scene.instances {
            let Some(asset_ref) = instance.asset_ref else {
                continue;
            };
            let Some(asset) = package.descriptor.assets.get(asset_ref as usize) else {
                return Err(RenderError::InvalidInput(format!(
                    "scene instance {} references missing asset {}",
                    instance.node_index, asset_ref
                )));
            };
            if asset.state != RenderAssetState::Ready {
                continue;
            }
            let transform = instance.transform.ok_or_else(|| {
                RenderError::Unsupported(format!(
                    "scene instance {} has no proven render transform",
                    instance.node_index
                ))
            })?;
            let scale = transform.scale.ok_or_else(|| {
                RenderError::Unsupported(format!(
                    "scene instance {} has no proven scale; renderer will not assume identity",
                    instance.node_index
                ))
            })?;
            let rotation = Quat::from_xyzw(
                transform.rotation[0],
                transform.rotation[1],
                transform.rotation[2],
                transform.rotation[3],
            );
            if !rotation.is_finite() || rotation.length_squared() <= 1.0e-12 {
                return Err(RenderError::InvalidInput(format!(
                    "scene instance {} has invalid rotation",
                    instance.node_index
                )));
            }
            let model = Mat4::from_scale_rotation_translation(
                Vec3::from_array(scale),
                rotation.normalize(),
                Vec3::from_array(transform.translation),
            );
            let local_overlay = asset.source.source_type == "loose";
            let style = [
                0.0,
                if emphasize_local && local_overlay {
                    1.0
                } else {
                    0.0
                },
                0.0,
                0.0,
            ];
            draws.push(DrawInstance {
                asset_key: asset_key(asset),
                model,
                model_binding: self.create_model_binding(model, style),
                node_index: Some(instance.node_index),
                local_overlay,
                world_bounds: asset_world_bounds(asset, model)?,
            });
        }

        draws.sort_by_key(|draw| draw.local_overlay);
        Ok(draws)
    }
}

struct OrbitCamera {
    target: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
    radius: f32,
    projection: Projection,
}

impl OrbitCamera {
    fn fit(bounds: Aabb, projection: Projection) -> Self {
        let direction = Vec3::new(1.0, -1.0, 0.78).normalize();
        let yaw = direction.y.atan2(direction.x);
        let pitch = direction.z.asin();
        let radius = bounds.radius();
        let fov = 45.0_f32.to_radians();
        let distance = (radius / (fov * 0.5).tan()).max(radius * 2.0) * 1.25;
        Self {
            target: bounds.center(),
            yaw,
            pitch,
            distance,
            radius,
            projection,
        }
    }

    fn eye(&self) -> Vec3 {
        let cos_pitch = self.pitch.cos();
        let direction = Vec3::new(
            cos_pitch * self.yaw.cos(),
            cos_pitch * self.yaw.sin(),
            self.pitch.sin(),
        );
        self.target + direction * self.distance
    }

    fn forward(&self) -> Vec3 {
        (self.target - self.eye()).normalize_or_zero()
    }

    fn right(&self) -> Vec3 {
        self.forward().cross(Vec3::Z).normalize_or_zero()
    }

    fn view_projection(&self, width: u32, height: u32) -> Mat4 {
        let eye = self.eye();
        let view = Mat4::look_at_rh(eye, self.target, Vec3::Z);
        let aspect = (width as f32 / height.max(1) as f32).max(0.0001);
        let near = (self.radius * 0.01).max(0.01);
        let far = self.distance + self.radius * 6.0 + 1.0;
        let projection = match self.projection {
            Projection::Perspective => {
                Mat4::perspective_rh(45.0_f32.to_radians(), aspect, near, far)
            }
            Projection::Orthographic => {
                let half_y = self.radius * 1.35 * (self.distance / self.fit_distance()).max(0.05);
                let half_x = half_y * aspect;
                Mat4::orthographic_rh(-half_x, half_x, -half_y, half_y, near, far)
            }
        };
        projection * view
    }

    fn fit_distance(&self) -> f32 {
        let fov = 45.0_f32.to_radians();
        (self.radius / (fov * 0.5).tan()).max(self.radius * 2.0) * 1.25
    }

    fn uniform(&self, width: u32, height: u32) -> CameraUniform {
        let eye = self.eye();
        CameraUniform {
            view_proj: self.view_projection(width, height).to_cols_array_2d(),
            light_dir: [0.45, -0.65, 0.62, 0.0],
            camera_pos: [eye.x, eye.y, eye.z, 1.0],
        }
    }

    fn orbit(&mut self, delta_x: f32, delta_y: f32) {
        if delta_x.is_finite() {
            self.yaw -= delta_x * 2.5;
        }
        if delta_y.is_finite() {
            self.pitch = (self.pitch + delta_y * 2.0).clamp(-1.45, 1.45);
        }
    }

    fn pan(&mut self, delta_x: f32, delta_y: f32) {
        if !delta_x.is_finite() || !delta_y.is_finite() {
            return;
        }
        let forward = self.forward();
        let right = self.right();
        let up = right.cross(forward).normalize_or_zero();
        let scale = self.distance * 1.6;
        self.target += (-right * delta_x + up * delta_y) * scale;
    }

    fn zoom(&mut self, delta: f32) {
        if !delta.is_finite() {
            return;
        }
        let min_distance = (self.radius * 0.05).max(0.01);
        let max_distance = (self.radius * 100.0).max(min_distance * 2.0);
        self.distance = (self.distance * (delta * 0.16).exp()).clamp(min_distance, max_distance);
    }

    fn fly(&mut self, forward: f32, right: f32, up: f32) {
        if !forward.is_finite() || !right.is_finite() || !up.is_finite() {
            return;
        }
        let speed = (self.radius * 0.08).max(0.05);
        let movement = self.forward() * forward + self.right() * right + Vec3::Z * up;
        self.target += movement * speed;
    }

    fn snapshot(&self) -> CameraSnapshot {
        CameraSnapshot {
            target: self.target.to_array(),
            eye: self.eye().to_array(),
            yaw_radians: self.yaw,
            pitch_radians: self.pitch,
            distance: self.distance,
            projection: self.projection,
        }
    }

    fn ray(&self, width: u32, height: u32, x: f32, y: f32) -> Option<(Vec3, Vec3)> {
        if !x.is_finite()
            || !y.is_finite()
            || !(0.0..=1.0).contains(&x)
            || !(0.0..=1.0).contains(&y)
        {
            return None;
        }
        let inverse = self.view_projection(width, height).inverse();
        if !inverse.is_finite() {
            return None;
        }
        let ndc_x = x * 2.0 - 1.0;
        let ndc_y = 1.0 - y * 2.0;
        let near = inverse.project_point3(Vec3::new(ndc_x, ndc_y, 0.0));
        let far = inverse.project_point3(Vec3::new(ndc_x, ndc_y, 1.0));
        let direction = (far - near).normalize_or_zero();
        (direction.length_squared() > 0.0).then_some((near, direction))
    }
}

pub struct SurfaceRenderer {
    gpu: OffscreenRenderer,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    depth_texture: wgpu::Texture,
    depth_view: wgpu::TextureView,
    mesh_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    options: ViewportOptions,
    package: Option<RenderPackage>,
    draws: Vec<DrawInstance>,
    world_bounds: Option<Aabb>,
    camera: Option<OrbitCamera>,
    selected_node_index: Option<u32>,
    gpu_cache_budget: GpuCacheBudget,
    gpu_budget_overflow: bool,
    scene_load_ms: f64,
    last_frame_ms: f64,
}

impl SurfaceRenderer {
    pub fn new<T>(target: T, options: ViewportOptions) -> RenderResult<Self>
    where
        T: HasWindowHandle + HasDisplayHandle + Send + Sync + 'static,
    {
        pollster::block_on(Self::new_async(target, options))
    }

    async fn new_async<T>(target: T, options: ViewportOptions) -> RenderResult<Self>
    where
        T: HasWindowHandle + HasDisplayHandle + Send + Sync + 'static,
    {
        let options = options.validate()?;
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(target)
            .map_err(|error| RenderError::Gpu(error.to_string()))?;
        let gpu = OffscreenRenderer::new_async_for_instance(instance, Some(&surface)).await?;
        let capabilities = surface.get_capabilities(&gpu.adapter);
        if capabilities.formats.is_empty() {
            return Err(RenderError::Gpu(
                "native surface reported no compatible color formats".into(),
            ));
        }
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(capabilities.formats[0]);
        let mut config = surface
            .get_default_config(&gpu.adapter, options.width, options.height)
            .ok_or_else(|| {
                RenderError::Gpu("native surface has no default configuration".into())
            })?;
        config.format = format;
        config.present_mode = if capabilities
            .present_modes
            .contains(&wgpu::PresentMode::Fifo)
        {
            wgpu::PresentMode::Fifo
        } else {
            config.present_mode
        };
        if capabilities
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::Opaque)
        {
            config.alpha_mode = wgpu::CompositeAlphaMode::Opaque;
        }
        surface.configure(&gpu.device, &config);
        let (depth_texture, depth_view) =
            create_depth_target(&gpu.device, options.width, options.height);
        let (mesh_pipeline, line_pipeline) = create_pipelines_for_format(
            &gpu.device,
            &gpu.camera_layout,
            &gpu.model_layout,
            &gpu.material_layout,
            format,
        );

        Ok(Self {
            gpu,
            surface,
            config,
            depth_texture,
            depth_view,
            mesh_pipeline,
            line_pipeline,
            options,
            package: None,
            draws: Vec::new(),
            world_bounds: None,
            camera: None,
            selected_node_index: None,
            gpu_cache_budget: GpuCacheBudget::default(),
            gpu_budget_overflow: false,
            scene_load_ms: 0.0,
            last_frame_ms: 0.0,
        })
    }

    pub fn set_package(&mut self, package: RenderPackage) -> RenderResult<()> {
        self.replace_package(package, false)
    }

    pub fn update_streaming_package(&mut self, package: RenderPackage) -> RenderResult<()> {
        self.replace_package(package, true)
    }

    pub fn set_gpu_cache_budget(&mut self, budget: GpuCacheBudget) -> RenderResult<()> {
        let budget = budget.validate()?;
        self.gpu_cache_budget = budget;
        if let Some(package) = self.package.as_ref() {
            self.gpu_budget_overflow = self.gpu.evict_to_budget(package, budget);
        }
        Ok(())
    }

    pub const fn gpu_cache_budget(&self) -> GpuCacheBudget {
        self.gpu_cache_budget
    }

    fn replace_package(
        &mut self,
        package: RenderPackage,
        preserve_camera: bool,
    ) -> RenderResult<()> {
        let started = Instant::now();
        package
            .validate()
            .map_err(|error| RenderError::InvalidInput(error.to_string()))?;
        for texture in &package.descriptor.textures {
            self.gpu.ensure_texture(&package, texture)?;
        }
        for asset in &package.descriptor.assets {
            if asset.state == RenderAssetState::Ready {
                self.gpu.ensure_asset(&package, asset)?;
            }
        }
        let draws = self.gpu.build_draw_instances(&package, true)?;
        if draws.is_empty() {
            return Err(RenderError::Unsupported(
                "render package has no ready drawable instances".into(),
            ));
        }
        let world_bounds = package_world_bounds(&package, &draws)?;
        let camera = if preserve_camera {
            self.camera
                .take()
                .unwrap_or_else(|| OrbitCamera::fit(world_bounds, self.options.projection))
        } else {
            OrbitCamera::fit(world_bounds, self.options.projection)
        };
        let gpu_budget_overflow = self.gpu.evict_to_budget(&package, self.gpu_cache_budget);
        self.package = Some(package);
        self.draws = draws;
        self.world_bounds = Some(world_bounds);
        self.camera = Some(camera);
        self.selected_node_index = None;
        self.gpu_budget_overflow = gpu_budget_overflow;
        self.scene_load_ms = started.elapsed().as_secs_f64() * 1000.0;
        Ok(())
    }

    pub fn resize(&mut self, width: u32, height: u32) -> RenderResult<()> {
        let options = ViewportOptions {
            width,
            height,
            ..self.options
        }
        .validate()?;
        self.options = options;
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.gpu.device, &self.config);
        let (depth_texture, depth_view) = create_depth_target(&self.gpu.device, width, height);
        self.depth_texture = depth_texture;
        self.depth_view = depth_view;
        Ok(())
    }

    pub fn set_projection(&mut self, projection: Projection) {
        self.options.projection = projection;
        if let Some(camera) = self.camera.as_mut() {
            camera.projection = projection;
        }
    }

    pub fn set_overlays(&mut self, grid: bool, wireframe: bool, bounds: bool) {
        self.options.grid = grid;
        self.options.wireframe = wireframe;
        self.options.bounds = bounds;
    }

    pub fn orbit(&mut self, delta_x: f32, delta_y: f32) {
        if let Some(camera) = self.camera.as_mut() {
            camera.orbit(delta_x, delta_y);
        }
    }

    pub fn pan(&mut self, delta_x: f32, delta_y: f32) {
        if let Some(camera) = self.camera.as_mut() {
            camera.pan(delta_x, delta_y);
        }
    }

    pub fn zoom(&mut self, delta: f32) {
        if let Some(camera) = self.camera.as_mut() {
            camera.zoom(delta);
        }
    }

    pub fn fly(&mut self, forward: f32, right: f32, up: f32) {
        if let Some(camera) = self.camera.as_mut() {
            camera.fly(forward, right, up);
        }
    }

    pub fn pick(&self, x: f32, y: f32) -> Option<PickResult> {
        let camera = self.camera.as_ref()?;
        let (origin, direction) = camera.ray(self.options.width, self.options.height, x, y)?;
        self.draws
            .iter()
            .filter_map(|draw| {
                let node_index = draw.node_index?;
                let distance = ray_aabb(origin, direction, draw.world_bounds)?;
                Some(PickResult {
                    node_index,
                    distance,
                })
            })
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
    }

    pub fn select(&mut self, node_index: Option<u32>) {
        self.selected_node_index = node_index;
        for draw in &self.draws {
            self.gpu.update_model_binding(draw, node_index, true);
        }
    }

    pub fn camera_snapshot(&self) -> Option<CameraSnapshot> {
        self.camera.as_ref().map(OrbitCamera::snapshot)
    }

    pub fn stats(&self) -> ViewportStats {
        let (instances, assets, meshes, materials, textures, uploaded_payload_bytes) =
            if let Some(package) = self.package.as_ref() {
                (
                    package.descriptor.summary.instances,
                    package.descriptor.summary.assets,
                    package.descriptor.summary.meshes,
                    package.descriptor.summary.materials,
                    package.descriptor.summary.textures,
                    package.blob.len() as u64,
                )
            } else {
                (0, 0, 0, 0, 0, 0)
            };
        let (gpu_asset_cache_bytes, gpu_texture_cache_bytes) = self.gpu.cache_bytes();
        ViewportStats {
            width: self.options.width,
            height: self.options.height,
            instances,
            assets,
            meshes,
            materials,
            textures,
            gpu_asset_cache: self.gpu.asset_cache.len().try_into().unwrap_or(u32::MAX),
            gpu_texture_cache: self.gpu.texture_cache.len().try_into().unwrap_or(u32::MAX),
            gpu_asset_cache_bytes,
            gpu_texture_cache_bytes,
            gpu_cache_hits: self.gpu.cache_hits,
            gpu_cache_misses: self.gpu.cache_misses,
            gpu_evictions: self.gpu.cache_evictions,
            gpu_budget_overflow: self.gpu_budget_overflow,
            uploaded_payload_bytes,
            scene_load_ms: self.scene_load_ms,
            last_frame_ms: self.last_frame_ms,
            selected_node_index: self.selected_node_index,
        }
    }

    pub fn render_frame(&mut self) -> RenderResult<()> {
        let started = Instant::now();
        let package = self.package.as_ref().ok_or_else(|| {
            RenderError::InvalidInput("native viewport has no loaded render package".into())
        })?;
        let camera = self.camera.as_ref().ok_or_else(|| {
            RenderError::InvalidInput("native viewport has no camera state".into())
        })?;
        let world_bounds = self.world_bounds.ok_or_else(|| {
            RenderError::InvalidInput("native viewport has no world bounds".into())
        })?;

        let camera_uniform = camera.uniform(self.options.width, self.options.height);
        let camera_buffer = self
            .gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("RageLab viewport camera uniform"),
                contents: bytemuck::bytes_of(&camera_uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let camera_bind_group = self
            .gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("RageLab viewport camera bind group"),
                layout: &self.gpu.camera_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: camera_buffer.as_entire_binding(),
                }],
            });
        let identity_model = self.gpu.create_model_binding(Mat4::IDENTITY, [0.0; 4]);
        let overlay_options = OffscreenOptions {
            width: self.options.width,
            height: self.options.height,
            view: RenderView::Auto,
            projection: self.options.projection,
            transparent: false,
            grid: self.options.grid,
            wireframe: self.options.wireframe,
            bounds: self.options.bounds,
        };
        let overlay_vertices = build_overlay_vertices(world_bounds, overlay_options);
        let overlay_buffer = (!overlay_vertices.is_empty()).then(|| {
            self.gpu
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("RageLab viewport overlay lines"),
                    contents: bytemuck::cast_slice(&overlay_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                })
        });

        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.gpu.device, &self.config);
                self.surface
                    .get_current_texture()
                    .map_err(|error| RenderError::Gpu(error.to_string()))?
            }
            Err(wgpu::SurfaceError::Timeout) => return Ok(()),
            Err(error) => return Err(RenderError::Gpu(error.to_string())),
        };
        let color_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("RageLab viewport encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("RageLab native viewport pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.035,
                            g: 0.045,
                            b: 0.06,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            pass.set_bind_group(0, &camera_bind_group, &[]);
            pass.set_pipeline(&self.mesh_pipeline);
            for draw in &self.draws {
                let asset = self.gpu.asset_cache.get(&draw.asset_key).ok_or_else(|| {
                    RenderError::Gpu(format!("GPU asset cache missing {}", draw.asset_key))
                })?;
                pass.set_bind_group(1, &draw.model_binding.bind_group, &[]);
                for mesh in &asset.meshes {
                    let material = mesh
                        .material_ref
                        .and_then(|index| asset.materials.get(index as usize))
                        .unwrap_or(&asset.fallback_material);
                    pass.set_bind_group(2, &material.bind_group, &[]);
                    pass.set_vertex_buffer(0, mesh.positions.slice(..));
                    pass.set_vertex_buffer(1, mesh.normals.slice(..));
                    pass.set_vertex_buffer(2, mesh.uv0.slice(..));
                    pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                }
            }

            if self.options.wireframe {
                pass.set_pipeline(&self.line_pipeline);
                for draw in &self.draws {
                    let Some(asset) = self.gpu.asset_cache.get(&draw.asset_key) else {
                        continue;
                    };
                    pass.set_bind_group(1, &draw.model_binding.bind_group, &[]);
                    for mesh in &asset.meshes {
                        if mesh.wire_index_count == 0 {
                            continue;
                        }
                        pass.set_vertex_buffer(0, mesh.positions.slice(..));
                        pass.set_index_buffer(
                            mesh.wire_indices.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..mesh.wire_index_count, 0, 0..1);
                    }
                }
            }

            if let Some(buffer) = overlay_buffer.as_ref() {
                pass.set_pipeline(&self.line_pipeline);
                pass.set_bind_group(1, &identity_model.bind_group, &[]);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..overlay_vertices.len() as u32, 0..1);
            }
        }

        self.gpu.queue.submit(Some(encoder.finish()));
        frame.present();
        self.last_frame_ms = started.elapsed().as_secs_f64() * 1000.0;
        let _ = package;
        Ok(())
    }
}

fn create_depth_target(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("RageLab native viewport depth"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn create_pipelines_for_format(
    device: &wgpu::Device,
    camera_layout: &wgpu::BindGroupLayout,
    model_layout: &wgpu::BindGroupLayout,
    material_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> (wgpu::RenderPipeline, wgpu::RenderPipeline) {
    let mesh_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("RageLab native mesh shader"),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(MESH_SHADER)),
    });
    let line_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("RageLab native line shader"),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(LINE_SHADER)),
    });
    let mesh_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("RageLab native mesh pipeline layout"),
        bind_group_layouts: &[camera_layout, model_layout, material_layout],
        push_constant_ranges: &[],
    });
    let line_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("RageLab native line pipeline layout"),
        bind_group_layouts: &[camera_layout, model_layout],
        push_constant_ranges: &[],
    });
    const POSITION_ATTRIBUTES: [wgpu::VertexAttribute; 1] =
        wgpu::vertex_attr_array![0 => Float32x3];
    const NORMAL_ATTRIBUTES: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![1 => Float32x3];
    const UV_ATTRIBUTES: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![2 => Float32x2];

    let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("RageLab native mesh pipeline"),
        layout: Some(&mesh_layout),
        vertex: wgpu::VertexState {
            module: &mesh_shader,
            entry_point: "vs_main",
            buffers: &[
                wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &POSITION_ATTRIBUTES,
                },
                wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &NORMAL_ATTRIBUTES,
                },
                wgpu::VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &UV_ATTRIBUTES,
                },
            ],
            compilation_options: Default::default(),
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &mesh_shader,
            entry_point: "fs_main",
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        multiview: None,
        cache: None,
    });

    let line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("RageLab native line pipeline"),
        layout: Some(&line_layout),
        vertex: wgpu::VertexState {
            module: &line_shader,
            entry_point: "vs_line",
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: 12,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &POSITION_ATTRIBUTES,
            }],
            compilation_options: Default::default(),
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::LineList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &line_shader,
            entry_point: "fs_line",
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        multiview: None,
        cache: None,
    });

    (mesh_pipeline, line_pipeline)
}

fn ray_aabb(origin: Vec3, direction: Vec3, bounds: Aabb) -> Option<f32> {
    let inv = Vec3::new(
        if direction.x.abs() > 1.0e-8 {
            1.0 / direction.x
        } else {
            f32::INFINITY
        },
        if direction.y.abs() > 1.0e-8 {
            1.0 / direction.y
        } else {
            f32::INFINITY
        },
        if direction.z.abs() > 1.0e-8 {
            1.0 / direction.z
        } else {
            f32::INFINITY
        },
    );
    let t0 = (bounds.min - origin) * inv;
    let t1 = (bounds.max - origin) * inv;
    let near = t0.min(t1);
    let far = t0.max(t1);
    let t_min = near.x.max(near.y).max(near.z);
    let t_max = far.x.min(far.y).min(far.z);
    if t_max < 0.0 || t_min > t_max {
        None
    } else {
        Some(t_min.max(0.0))
    }
}

fn create_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> RenderResult<GpuTexture> {
    if width == 0 || height == 0 {
        return Err(RenderError::InvalidInput(format!(
            "{label}: texture dimensions must be non-zero"
        )));
    }
    let expected = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| RenderError::InvalidInput(format!("{label}: texture size overflow")))?;
    if rgba.len() != expected {
        return Err(RenderError::InvalidInput(format!(
            "{label}: RGBA length {} does not match {width}x{height}",
            rgba.len()
        )));
    }

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(width.saturating_mul(4)),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Ok(GpuTexture {
        _texture: texture,
        view,
    })
}

fn asset_key(asset: &RenderAssetDescriptor) -> String {
    asset.stable_key()
}

fn texture_key(texture: &RenderTextureDescriptor) -> String {
    texture.stable_key()
}

fn view_bytes<'a>(
    package: &'a RenderPackage,
    view: RenderBufferView,
    element_type: RenderElementType,
    components: u8,
    label: &str,
) -> RenderResult<&'a [u8]> {
    if view.element_type != element_type || view.components != components {
        return Err(RenderError::InvalidInput(format!(
            "{label} view has unexpected type/components"
        )));
    }
    let start = usize::try_from(view.offset)
        .map_err(|_| RenderError::InvalidInput(format!("{label} offset overflow")))?;
    let length = usize::try_from(view.byte_length)
        .map_err(|_| RenderError::InvalidInput(format!("{label} length overflow")))?;
    let end = start
        .checked_add(length)
        .ok_or_else(|| RenderError::InvalidInput(format!("{label} range overflow")))?;
    package
        .blob
        .get(start..end)
        .ok_or_else(|| RenderError::InvalidInput(format!("{label} view exceeds blob")))
}

fn repeated_f32x3(count: usize, value: [f32; 3]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(count.saturating_mul(12));
    for _ in 0..count {
        for component in value {
            bytes.extend_from_slice(&component.to_le_bytes());
        }
    }
    bytes
}

fn repeated_f32x2(count: usize, value: [f32; 2]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(count.saturating_mul(8));
    for _ in 0..count {
        for component in value {
            bytes.extend_from_slice(&component.to_le_bytes());
        }
    }
    bytes
}

fn wire_indices(bytes: &[u8]) -> RenderResult<Vec<u32>> {
    if bytes.len() % 4 != 0 {
        return Err(RenderError::InvalidInput(
            "index buffer byte length is not divisible by four".into(),
        ));
    }
    let indices = bytes
        .chunks_exact(4)
        .map(|chunk| u32::from_le_bytes(chunk.try_into().unwrap()))
        .collect::<Vec<_>>();
    let mut wire = Vec::with_capacity(indices.len().saturating_mul(2));
    for triangle in indices.chunks_exact(3) {
        wire.extend_from_slice(&[
            triangle[0],
            triangle[1],
            triangle[1],
            triangle[2],
            triangle[2],
            triangle[0],
        ]);
    }
    Ok(wire)
}

fn model_uniform(model: Mat4, style: [f32; 4]) -> ModelUniform {
    let determinant = model.determinant();
    let normal = if determinant.is_finite() && determinant.abs() > 1.0e-8 {
        model.inverse().transpose()
    } else {
        Mat4::IDENTITY
    };
    ModelUniform {
        model: model.to_cols_array_2d(),
        normal: normal.to_cols_array_2d(),
        style,
    }
}

fn asset_world_bounds(asset: &RenderAssetDescriptor, model: Mat4) -> RenderResult<Aabb> {
    let local = asset.bounds.ok_or_else(|| {
        RenderError::Unsupported(format!(
            "asset {} has no proven drawable bounds",
            asset.asset_ref
        ))
    })?;
    let mut bounds = Aabb::empty();
    for corner in aabb_corners(Vec3::from_array(local.min), Vec3::from_array(local.max)) {
        bounds.include(model.transform_point3(corner));
    }
    if !bounds.is_valid() {
        return Err(RenderError::InvalidInput(format!(
            "asset {} produced invalid world bounds",
            asset.asset_ref
        )));
    }
    Ok(bounds)
}

fn package_world_bounds(_package: &RenderPackage, draws: &[DrawInstance]) -> RenderResult<Aabb> {
    let mut bounds = Aabb::empty();
    for draw in draws {
        bounds.include(draw.world_bounds.min);
        bounds.include(draw.world_bounds.max);
    }
    if !bounds.is_valid() {
        return Err(RenderError::InvalidInput(
            "render package produced invalid world bounds".into(),
        ));
    }
    Ok(bounds)
}

fn aabb_corners(min: Vec3, max: Vec3) -> [Vec3; 8] {
    [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(min.x, max.y, max.z),
        Vec3::new(max.x, max.y, max.z),
    ]
}

fn camera_for_bounds(bounds: Aabb, options: OffscreenOptions) -> CameraUniform {
    let center = bounds.center();
    let radius = bounds.radius();
    let aspect = options.width as f32 / options.height as f32;
    let direction = match options.view {
        RenderView::Front => Vec3::new(0.0, -1.0, 0.0),
        RenderView::Back => Vec3::new(0.0, 1.0, 0.0),
        RenderView::Left => Vec3::new(-1.0, 0.0, 0.0),
        RenderView::Right => Vec3::new(1.0, 0.0, 0.0),
        RenderView::Top => Vec3::new(0.0, 0.0, 1.0),
        RenderView::Isometric | RenderView::Auto => Vec3::new(1.0, -1.0, 0.78).normalize(),
    };
    let up = if options.view == RenderView::Top {
        Vec3::Y
    } else {
        Vec3::Z
    };
    let fov = 45.0_f32.to_radians();
    let distance = (radius / (fov * 0.5).tan()).max(radius * 2.0) * 1.25;
    let eye = center + direction * distance;
    let view = Mat4::look_at_rh(eye, center, up);
    let near = (radius * 0.01).max(0.01);
    let far = distance + radius * 4.0 + 1.0;
    let projection = match options.projection {
        Projection::Perspective => Mat4::perspective_rh(fov, aspect, near, far),
        Projection::Orthographic => {
            let half_y = radius * 1.35;
            let half_x = half_y * aspect;
            Mat4::orthographic_rh(-half_x, half_x, -half_y, half_y, near, far)
        }
    };
    CameraUniform {
        view_proj: (projection * view).to_cols_array_2d(),
        light_dir: [0.45, -0.65, 0.62, 0.0],
        camera_pos: [eye.x, eye.y, eye.z, 1.0],
    }
}

fn build_overlay_vertices(bounds: Aabb, options: OffscreenOptions) -> Vec<[f32; 3]> {
    let mut vertices = Vec::new();

    if options.grid {
        let center = bounds.center();
        let extent = bounds.extent();
        let span = extent.x.max(extent.y).max(1.0) * 0.75;
        let spacing = nice_grid_spacing(span / 10.0);
        let count = 10_i32;
        let z = bounds.min.z;
        for step in -count..=count {
            let offset = step as f32 * spacing;
            vertices.push([center.x - span, center.y + offset, z]);
            vertices.push([center.x + span, center.y + offset, z]);
            vertices.push([center.x + offset, center.y - span, z]);
            vertices.push([center.x + offset, center.y + span, z]);
        }
    }

    if options.bounds {
        let corners = aabb_corners(bounds.min, bounds.max);
        const EDGES: [(usize, usize); 12] = [
            (0, 1),
            (0, 2),
            (1, 3),
            (2, 3),
            (4, 5),
            (4, 6),
            (5, 7),
            (6, 7),
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ];
        for (a, b) in EDGES {
            vertices.push(corners[a].to_array());
            vertices.push(corners[b].to_array());
        }
    }

    vertices
}

fn nice_grid_spacing(target: f32) -> f32 {
    if !target.is_finite() || target <= 0.0 {
        return 1.0;
    }
    let exponent = target.log10().floor();
    let base = 10.0_f32.powf(exponent);
    let normalized = target / base;
    let step = if normalized <= 1.0 {
        1.0
    } else if normalized <= 2.0 {
        2.0
    } else if normalized <= 5.0 {
        5.0
    } else {
        10.0
    };
    step * base
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orbit_camera_fit_and_controls_remain_finite() {
        let bounds = Aabb {
            min: Vec3::new(-2.0, -1.0, 0.0),
            max: Vec3::new(4.0, 3.0, 5.0),
        };
        let mut camera = OrbitCamera::fit(bounds, Projection::Perspective);
        let initial = camera.snapshot();
        assert!(initial.distance.is_finite());
        assert!(initial.distance > 0.0);

        camera.orbit(0.2, -0.15);
        camera.pan(0.1, -0.05);
        camera.zoom(-0.5);
        camera.fly(1.0, -0.25, 0.5);
        let moved = camera.snapshot();
        assert!(moved.eye.iter().all(|value| value.is_finite()));
        assert!(moved.target.iter().all(|value| value.is_finite()));
        assert!(moved.distance > 0.0);
        assert_ne!(moved.eye, initial.eye);
    }

    #[test]
    fn ray_aabb_selects_forward_box_and_rejects_miss() {
        let bounds = Aabb {
            min: Vec3::new(-1.0, -1.0, -1.0),
            max: Vec3::new(1.0, 1.0, 1.0),
        };
        let hit = ray_aabb(Vec3::new(0.0, -5.0, 0.0), Vec3::Y, bounds)
            .expect("forward ray should hit box");
        assert!((hit - 4.0).abs() < 1.0e-5);
        assert!(ray_aabb(Vec3::new(5.0, -5.0, 0.0), Vec3::Y, bounds).is_none());
    }

    #[test]
    fn camera_center_ray_hits_scene_bounds() {
        let bounds = Aabb {
            min: Vec3::new(-2.0, -2.0, -1.0),
            max: Vec3::new(2.0, 2.0, 3.0),
        };
        let camera = OrbitCamera::fit(bounds, Projection::Perspective);
        let (origin, direction) = camera.ray(1280, 720, 0.5, 0.5).expect("center ray");
        assert!(ray_aabb(origin, direction, bounds).is_some());
    }
}

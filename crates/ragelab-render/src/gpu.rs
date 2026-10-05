use std::{borrow::Cow, collections::HashMap, sync::mpsc};

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Quat, Vec3};
use ragelab_engine::{
    RenderAssetDescriptor, RenderAssetState, RenderBufferView, RenderElementType, RenderPackage,
    RenderTextureDescriptor,
};
use wgpu::util::DeviceExt;

use crate::{OffscreenOptions, Projection, RenderError, RenderResult, RenderView, RenderedImage};

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
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    let world_position = model.model * vec4<f32>(input.position, 1.0);
    output.position = camera.view_proj * world_position;
    output.world_normal = normalize((model.normal * vec4<f32>(input.normal, 0.0)).xyz);
    output.uv0 = input.uv0;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let base = textureSample(diffuse_texture, diffuse_sampler, input.uv0);
    let normal = normalize(input.world_normal);
    let diffuse = max(dot(normal, normalize(camera.light_dir.xyz)), 0.0);
    let lighting = 0.34 + 0.66 * diffuse;
    return vec4<f32>(base.rgb * lighting, base.a);
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
}

struct ModelBinding {
    _buffer: wgpu::Buffer,
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

struct DrawInstance {
    asset_key: String,
    model: Mat4,
    model_binding: ModelBinding,
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
}

impl OffscreenRenderer {
    pub fn new() -> RenderResult<Self> {
        pollster::block_on(Self::new_async())
    }

    async fn new_async() -> RenderResult<Self> {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
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

        let draws = self.build_draw_instances(package)?;
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

        let identity_model = self.create_model_binding(Mat4::IDENTITY);
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
        if self.texture_cache.contains_key(&key) {
            return Ok(());
        }
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
        self.texture_cache.insert(key, texture);
        Ok(())
    }

    fn ensure_asset(
        &mut self,
        package: &RenderPackage,
        descriptor: &RenderAssetDescriptor,
    ) -> RenderResult<()> {
        let key = asset_key(descriptor);
        if self.asset_cache.contains_key(&key) {
            return Ok(());
        }

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
            key,
            GpuAsset {
                meshes,
                materials,
                fallback_material,
            },
        );
        Ok(())
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

    fn create_model_binding(&self, model: Mat4) -> ModelBinding {
        let determinant = model.determinant();
        let normal = if determinant.is_finite() && determinant.abs() > 1.0e-8 {
            model.inverse().transpose()
        } else {
            Mat4::IDENTITY
        };
        let uniform = ModelUniform {
            model: model.to_cols_array_2d(),
            normal: normal.to_cols_array_2d(),
        };
        let buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("RageLab model uniform"),
                contents: bytemuck::bytes_of(&uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("RageLab model bind group"),
            layout: &self.model_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        ModelBinding {
            _buffer: buffer,
            bind_group,
        }
    }

    fn build_draw_instances(&self, package: &RenderPackage) -> RenderResult<Vec<DrawInstance>> {
        let mut draws = Vec::new();

        if package.descriptor.scene.instances.is_empty() {
            for asset in &package.descriptor.assets {
                if asset.state != RenderAssetState::Ready {
                    continue;
                }
                draws.push(DrawInstance {
                    asset_key: asset_key(asset),
                    model: Mat4::IDENTITY,
                    model_binding: self.create_model_binding(Mat4::IDENTITY),
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
            draws.push(DrawInstance {
                asset_key: asset_key(asset),
                model,
                model_binding: self.create_model_binding(model),
            });
        }

        Ok(draws)
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
    let selector = asset
        .selector
        .as_ref()
        .map(|selector| {
            format!(
                "{}:{}:{:08X}:{}",
                selector.selector_type,
                selector.index,
                selector.name_hash.unwrap_or(0),
                selector.name.as_deref().unwrap_or("")
            )
        })
        .unwrap_or_default();
    format!(
        "{}|{}|{:08X}|{}",
        asset.source.source_type,
        asset.source.path,
        asset.hash.unwrap_or(0),
        selector
    )
}

fn texture_key(texture: &RenderTextureDescriptor) -> String {
    format!(
        "{}|{}|{:08X}|{}x{}",
        texture.source, texture.source_path, texture.name_hash, texture.width, texture.height
    )
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

fn package_world_bounds(package: &RenderPackage, draws: &[DrawInstance]) -> RenderResult<Aabb> {
    let mut bounds = Aabb::empty();
    for draw in draws {
        let asset = package
            .descriptor
            .assets
            .iter()
            .find(|asset| asset_key(asset) == draw.asset_key)
            .ok_or_else(|| {
                RenderError::InvalidInput(format!(
                    "draw references missing asset {}",
                    draw.asset_key
                ))
            })?;
        let local = asset.bounds.ok_or_else(|| {
            RenderError::Unsupported(format!(
                "asset {} has no proven drawable bounds",
                asset.asset_ref
            ))
        })?;
        for corner in aabb_corners(Vec3::from_array(local.min), Vec3::from_array(local.max)) {
            bounds.include(draw.model.transform_point3(corner));
        }
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

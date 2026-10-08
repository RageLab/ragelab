use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    sync::{mpsc, Arc, OnceLock},
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
    RenderResult, RenderView, RenderedImage, TransformGizmoMode, ViewportOptions, ViewportStats,
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

const GIZMO_SHADER: &str = r#"
struct Camera {
    view_proj: mat4x4<f32>,
    light_dir: vec4<f32>,
    camera_pos: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

struct GizmoVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_gizmo(
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
) -> GizmoVertexOutput {
    var output: GizmoVertexOutput;
    output.position = camera.view_proj * vec4<f32>(position, 1.0);
    output.color = color;
    return output;
}

@fragment
fn fs_gizmo(input: GizmoVertexOutput) -> @location(0) vec4<f32> {
    return input.color;
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

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, PartialEq)]
struct GizmoVertex {
    position: [f32; 3],
    color: [f32; 4],
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

fn select_lru_key<'a>(
    entries: impl Iterator<Item = (&'a String, u64)>,
    protected: &HashSet<String>,
) -> Option<String> {
    entries
        .filter(|(key, _)| !protected.contains(*key))
        .min_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(right.0)))
        .map(|(key, _)| key.clone())
}

fn textures_referenced_by_assets<'a>(
    assets: impl Iterator<Item = &'a GpuAssetCacheMeta>,
) -> HashSet<String> {
    assets
        .flat_map(|meta| meta.texture_keys.iter().cloned())
        .collect()
}

fn select_asset_releasing_texture<'a>(
    entries: impl Iterator<Item = (&'a String, &'a GpuAssetCacheMeta)>,
    protected_assets: &HashSet<String>,
    active_textures: &HashSet<String>,
    resident_textures: &HashSet<String>,
) -> Option<String> {
    select_lru_key(
        entries
            .filter(|(_, meta)| {
                meta.texture_keys
                    .iter()
                    .any(|key| !active_textures.contains(key) && resident_textures.contains(key))
            })
            .map(|(key, meta)| (key, meta.last_used)),
        protected_assets,
    )
}

struct DrawInstance {
    asset_key: String,
    model: Mat4,
    model_binding: ModelBinding,
    node_index: Option<u32>,
    local_overlay: bool,
    collision_overlay: bool,
    world_bounds: Aabb,
    lod_dist: Option<f32>,
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

// Source YMAP and original entity index remain stable when a YMAP is paginated,
// sorted by camera proximity, or reloaded at a different package position.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LodEntityKey {
    source_ymap: String,
    entity_index: u32,
}

#[derive(Debug, Clone)]
struct LodEntity {
    key: LodEntityKey,
    node_index: u32,
    map_hash: Option<u32>,
    parent_map_hash: Option<u32>,
    parent_index: Option<i32>,
    num_children: Option<u32>,
    child_lod_dist: Option<f32>,
    lod_dist: Option<f32>,
    bounds: Option<Aabb>,
}

#[derive(Default)]
struct LodHierarchy {
    nodes: Vec<LodEntity>,
    children: Vec<Vec<usize>>,
    roots: Vec<usize>,
}

impl LodHierarchy {
    fn from_packages(
        packages: &[Arc<RenderPackage>],
        draws: &[DrawInstance],
        gpu_assets: &HashMap<String, GpuAsset>,
    ) -> Self {
        let bounds_by_node = draws
            .iter()
            .filter(|draw| !draw.collision_overlay)
            .filter(|draw| {
                gpu_assets
                    .get(&draw.asset_key)
                    .is_some_and(|asset| asset.meshes.iter().any(|mesh| mesh.index_count > 0))
            })
            .filter_map(|draw| draw.node_index.map(|index| (index, draw.world_bounds)))
            .collect::<HashMap<_, _>>();
        let bases = package_node_bases(packages);
        let nodes = packages
            .iter()
            .enumerate()
            .flat_map(|(package_index, package)| {
                let base = bases[package_index];
                let bounds_by_node = &bounds_by_node;
                package
                    .descriptor
                    .scene
                    .instances
                    .iter()
                    .map(move |instance| {
                        let node_index = base.saturating_add(instance.node_index);
                        LodEntity {
                            key: LodEntityKey {
                                source_ymap: instance.source_ymap.clone(),
                                entity_index: instance.entity_index,
                            },
                            node_index,
                            map_hash: package.descriptor.scene.root.name_hash,
                            parent_map_hash: instance.parent_map_hash,
                            parent_index: instance.parent_index,
                            num_children: instance.num_children,
                            child_lod_dist: instance.child_lod_dist,
                            lod_dist: instance.lod_dist,
                            bounds: bounds_by_node.get(&node_index).copied(),
                        }
                    })
            })
            .collect();
        Self::from_nodes(nodes)
    }

    fn from_nodes(nodes: Vec<LodEntity>) -> Self {
        // GTA YMAP parentIndex normally indexes an entity of the *parent YMAP*,
        // designated by the map's parent hash, not the child's own YMAP.
        // Pages from the same map can arrive independently and in any order.
        let mut by_map_entity = HashMap::new();
        let mut by_source_entity = HashMap::new();
        for (index, node) in nodes.iter().enumerate() {
            // Interior nodes may share an index with their owning entity.
            by_source_entity.entry(node.key.clone()).or_insert(index);
            if let Some(map_hash) = node.map_hash {
                by_map_entity
                    .entry((map_hash, node.key.entity_index))
                    .or_insert(index);
            }
        }
        let mut children = vec![Vec::new(); nodes.len()];
        let mut has_parent = vec![false; nodes.len()];
        for (index, node) in nodes.iter().enumerate() {
            let Some(parent_index) = node
                .parent_index
                .and_then(|value| u32::try_from(value).ok())
            else {
                continue;
            };
            let parent = if let Some(parent_map_hash) = node.parent_map_hash {
                // Never connect a child to a coincidentally matching local
                // index while its actual parent map is not resident.
                by_map_entity.get(&(parent_map_hash, parent_index)).copied()
            } else {
                let parent_key = LodEntityKey {
                    source_ymap: node.key.source_ymap.clone(),
                    entity_index: parent_index,
                };
                by_source_entity.get(&parent_key).copied()
            };
            if let Some(parent) = parent.filter(|&parent| parent != index) {
                children[parent].push(index);
                has_parent[index] = true;
            }
        }
        let roots = has_parent
            .iter()
            .enumerate()
            .filter_map(|(index, linked)| (!linked).then_some(index))
            .collect();
        Self {
            nodes,
            children,
            roots,
        }
    }

    // Returns nodes hidden by *hierarchy* only. Frustum, user visibility and
    // max lodDist are still independently applied in the draw loop.
    fn suppressed(
        &self,
        camera: &OrbitCamera,
        selections: &mut HashMap<LodEntityKey, bool>,
    ) -> HashSet<u32> {
        let mut suppressed = HashSet::new();
        let mut visited = vec![false; self.nodes.len()];
        let mut stack = self
            .roots
            .iter()
            .rev()
            .map(|&index| (index, true))
            .collect::<Vec<_>>();
        // Invalid/cyclic data must never make entities permanently disappear.
        for start in 0..self.nodes.len() {
            if stack.is_empty() && !visited[start] {
                stack.push((start, true));
            }
            while let Some((index, enabled)) = stack.pop() {
                if std::mem::replace(&mut visited[index], true) {
                    continue;
                }
                let node = &self.nodes[index];
                let expected = node.num_children.unwrap_or(0) as usize;
                let threshold = node.child_lod_dist.filter(|d| d.is_finite() && *d > 0.0);
                let managed =
                    expected > 0 && threshold.is_some() && !self.children[index].is_empty();
                let parent_ready = node
                    .bounds
                    .is_some_and(|bounds| camera.lod_visible(bounds, node.lod_dist));
                // A loaded mesh is not necessarily a drawable LOD at this
                // distance. Suppressing the proxy while its children fail
                // their own lodDist leaves a hole between LOD rings.
                let children_ready = managed
                    && self.children[index].len() == expected
                    && self.children[index].iter().all(|&child| {
                        let descendant = &self.nodes[child];
                        descendant
                            .bounds
                            .is_some_and(|bounds| camera.lod_visible(bounds, descendant.lod_dist))
                    });
                let use_children = if !parent_ready {
                    // Never suppress a drawable child behind a missing parent.
                    true
                } else if children_ready {
                    let bounds = node.bounds.expect("ready parent has bounds");
                    let distance =
                        ((bounds.center() - camera.eye()).length() - bounds.radius()).max(0.0);
                    let threshold = threshold.expect("managed branch has threshold");
                    let use_detail = match selections.get(&node.key) {
                        Some(true) => distance <= threshold * 1.10,
                        Some(false) => distance <= threshold * 0.90,
                        None => distance <= threshold,
                    };
                    selections.insert(node.key.clone(), use_detail);
                    use_detail
                } else {
                    // No metadata: preserve legacy rendering. Incomplete
                    // declared hierarchy: keep the proxy until every child is ready.
                    !managed
                };
                if !enabled || (managed && use_children) {
                    suppressed.insert(node.node_index);
                }
                let children_enabled = enabled && (!managed || use_children);
                for &child in self.children[index].iter().rev() {
                    stack.push((child, children_enabled));
                }
            }
        }
        suppressed
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
            mipmap_filter: wgpu::FilterMode::Linear,
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

        let draws = self.build_draw_instances(package, false, 0)?;
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
                bytes: gpu_texture_mip_bytes(
                    u32::from(descriptor.width),
                    u32::from(descriptor.height),
                ),
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

    fn evict_to_budget(&mut self, packages: &[Arc<RenderPackage>], budget: GpuCacheBudget) -> bool {
        let protected_assets = packages
            .iter()
            .flat_map(|package| package.descriptor.assets.iter())
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
            let victim = select_lru_key(
                self.asset_cache_meta
                    .iter()
                    .map(|(key, meta)| (key, meta.last_used)),
                &protected_assets,
            );
            let Some(key) = victim else {
                break;
            };
            self.asset_cache.remove(&key);
            self.asset_cache_meta.remove(&key);
            self.cache_evictions = self.cache_evictions.saturating_add(1);
        }

        let active_textures = packages
            .iter()
            .flat_map(|package| package.descriptor.textures.iter())
            .map(texture_key)
            .collect::<HashSet<_>>();

        loop {
            let (_, texture_bytes) = self.cache_bytes();
            if self.texture_cache.len() <= budget.max_textures as usize
                && texture_bytes <= budget.max_texture_bytes
            {
                break;
            }

            let referenced_textures = textures_referenced_by_assets(self.asset_cache_meta.values());
            let mut protected_textures = active_textures.clone();
            protected_textures.extend(referenced_textures);

            if let Some(key) = select_lru_key(
                self.texture_cache_meta
                    .iter()
                    .map(|(key, meta)| (key, meta.last_used)),
                &protected_textures,
            ) {
                self.texture_cache.remove(&key);
                self.texture_cache_meta.remove(&key);
                self.cache_evictions = self.cache_evictions.saturating_add(1);
                continue;
            }

            let resident_textures = self
                .texture_cache_meta
                .keys()
                .cloned()
                .collect::<HashSet<_>>();
            let releasable_asset = select_asset_releasing_texture(
                self.asset_cache_meta.iter(),
                &protected_assets,
                &active_textures,
                &resident_textures,
            );
            let Some(key) = releasable_asset else {
                break;
            };
            self.asset_cache.remove(&key);
            self.asset_cache_meta.remove(&key);
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
        node_base: u32,
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
                    collision_overlay: false,
                    world_bounds,
                    lod_dist: None,
                });
            }
            return Ok(draws);
        }

        for instance in &package.descriptor.scene.instances {
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

            if let Some(asset_ref) = instance.asset_ref {
                let Some(asset) = package.descriptor.assets.get(asset_ref as usize) else {
                    return Err(RenderError::InvalidInput(format!(
                        "scene instance {} references missing asset {}",
                        instance.node_index, asset_ref
                    )));
                };
                if asset.state == RenderAssetState::Ready {
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
                        node_index: Some(node_base.saturating_add(instance.node_index)),
                        local_overlay,
                        collision_overlay: false,
                        world_bounds: asset_world_bounds(asset, model)?,
                        lod_dist: instance.lod_dist,
                    });
                }
            }

            if let Some(collision_ref) = instance
                .collision
                .as_ref()
                .and_then(|collision| collision.asset_ref)
            {
                let Some(asset) = package.descriptor.assets.get(collision_ref as usize) else {
                    return Err(RenderError::InvalidInput(format!(
                        "scene instance {} collision references missing asset {}",
                        instance.node_index, collision_ref
                    )));
                };
                if asset.state == RenderAssetState::Ready {
                    let local_overlay = asset.source.source_type == "loose";
                    draws.push(DrawInstance {
                        asset_key: asset_key(asset),
                        model,
                        model_binding: self.create_model_binding(model, [0.0; 4]),
                        node_index: Some(node_base.saturating_add(instance.node_index)),
                        local_overlay,
                        collision_overlay: true,
                        world_bounds: asset_world_bounds(asset, model)?,
                        lod_dist: instance.lod_dist,
                    });
                }
            }
        }

        draws.sort_by_key(|draw| (draw.local_overlay, draw.collision_overlay));
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
    far_radius_multiplier: f32,
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
            far_radius_multiplier: 6.0,
        }
    }

    fn around_world_position(target: [f32; 3], local_radius: f32, projection: Projection) -> Self {
        let direction = Vec3::new(1.0, -1.0, 0.68).normalize();
        let yaw = direction.y.atan2(direction.x);
        let pitch = direction.z.asin();
        let radius = local_radius.max(64.0);
        let distance = (radius * 1.35).max(120.0);
        Self {
            target: Vec3::from_array(target),
            yaw,
            pitch,
            distance,
            radius,
            projection,
            // Native GTA world uses a 120m focus radius; the default 6x
            // multiplier clipped distant city/terrain at roughly 880m.
            // Keep long-range proxy LODs in the frustum while the stream
            // controls actual residency and the entity's lodDist controls draws.
            far_radius_multiplier: 22.0,
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

    fn far_distance(&self) -> f32 {
        self.distance + self.radius * self.far_radius_multiplier + 1.0
    }

    fn view_projection(&self, width: u32, height: u32) -> Mat4 {
        let eye = self.eye();
        let view = Mat4::look_at_rh(eye, self.target, Vec3::Z);
        let aspect = (width as f32 / height.max(1) as f32).max(0.0001);
        let near = (self.radius * 0.01).max(0.01);
        let far = self.far_distance();
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

    fn lod_visible(&self, bounds: Aabb, lod_dist: Option<f32>) -> bool {
        let Some(lod_dist) = lod_dist.filter(|value| value.is_finite() && *value > 0.0) else {
            return true;
        };
        let surface_distance = ((bounds.center() - self.eye()).length() - bounds.radius()).max(0.0);
        surface_distance <= lod_dist * 1.15
    }

    fn aabb_visible(&self, bounds: Aabb, width: u32, height: u32) -> bool {
        let eye = self.eye();
        let forward = self.forward();
        let right = self.right();
        let up = right.cross(forward).normalize_or_zero();
        let to_center = bounds.center() - eye;
        let radius = bounds.radius();
        let depth = to_center.dot(forward);
        let near = (self.radius * 0.01).max(0.01);
        let far = self.far_distance();

        if depth + radius < near || depth - radius > far {
            return false;
        }

        let aspect = (width as f32 / height.max(1) as f32).max(0.0001);
        let horizontal = to_center.dot(right).abs();
        let vertical = to_center.dot(up).abs();

        match self.projection {
            Projection::Perspective => {
                let half_vertical = depth.max(near) * (45.0_f32.to_radians() * 0.5).tan();
                let half_horizontal = half_vertical * aspect;
                horizontal <= half_horizontal + radius && vertical <= half_vertical + radius
            }
            Projection::Orthographic => {
                let half_vertical =
                    self.radius * 1.35 * (self.distance / self.fit_distance()).max(0.05);
                let half_horizontal = half_vertical * aspect;
                horizontal <= half_horizontal + radius && vertical <= half_vertical + radius
            }
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

    fn look(&mut self, delta_x: f32, delta_y: f32) {
        let eye = self.eye();
        self.orbit(delta_x, delta_y);
        let cos_pitch = self.pitch.cos();
        let eye_direction = Vec3::new(
            cos_pitch * self.yaw.cos(),
            cos_pitch * self.yaw.sin(),
            self.pitch.sin(),
        );
        self.target = eye - eye_direction * self.distance;
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

fn rebase_package_node(old_node: u32, old_base: u32, new_base: u32) -> u32 {
    old_node.saturating_sub(old_base).saturating_add(new_base)
}

fn package_node_bases(packages: &[Arc<RenderPackage>]) -> Vec<u32> {
    let mut next = 0_u32;
    packages
        .iter()
        .map(|package| {
            let start = next;
            next = next.saturating_add(
                u32::try_from(package.descriptor.scene.instances.len()).unwrap_or(u32::MAX),
            );
            start
        })
        .collect()
}

fn package_index_at_node(
    packages: &[Arc<RenderPackage>],
    bases: &[u32],
    node: u32,
) -> Option<usize> {
    packages.iter().enumerate().find_map(|(index, package)| {
        let start = bases[index];
        let end = start.saturating_add(
            u32::try_from(package.descriptor.scene.instances.len()).unwrap_or(u32::MAX),
        );
        (node >= start && node < end).then_some(index)
    })
}

pub struct SurfaceRenderer {
    gpu: OffscreenRenderer,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    identity_model: ModelBinding,
    depth_texture: wgpu::Texture,
    depth_view: wgpu::TextureView,
    mesh_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    gizmo_pipeline: wgpu::RenderPipeline,
    options: ViewportOptions,
    packages: Vec<Arc<RenderPackage>>,
    draws: Vec<DrawInstance>,
    lod_hierarchy: LodHierarchy,
    lod_selections: HashMap<LodEntityKey, bool>,
    lod_suppressed: HashSet<u32>,
    world_bounds: Option<Aabb>,
    camera: Option<OrbitCamera>,
    selected_node_index: Option<u32>,
    gizmo_mode: Option<TransformGizmoMode>,
    hidden_node_indices: HashSet<u32>,
    show_base_game: bool,
    show_local_overlays: bool,
    show_collision: bool,
    gpu_cache_budget: GpuCacheBudget,
    gpu_budget_overflow: bool,
    scene_load_ms: f64,
    last_frame_ms: f64,
    last_visible_instances: u32,
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
        let (mesh_pipeline, line_pipeline, gizmo_pipeline) = create_pipelines_for_format(
            &gpu.device,
            &gpu.camera_layout,
            &gpu.model_layout,
            &gpu.material_layout,
            format,
        );

        let camera_buffer = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("RageLab persistent viewport camera uniform"),
                contents: bytemuck::bytes_of(&CameraUniform::zeroed()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let camera_bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("RageLab persistent viewport camera bind group"),
            layout: &gpu.camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let identity_model = gpu.create_model_binding(Mat4::IDENTITY, [0.0; 4]);

        Ok(Self {
            gpu,
            surface,
            config,
            camera_buffer,
            camera_bind_group,
            identity_model,
            depth_texture,
            depth_view,
            mesh_pipeline,
            line_pipeline,
            gizmo_pipeline,
            options,
            packages: Vec::new(),
            draws: Vec::new(),
            lod_hierarchy: LodHierarchy::default(),
            lod_selections: HashMap::new(),
            lod_suppressed: HashSet::new(),
            world_bounds: None,
            camera: None,
            selected_node_index: None,
            gizmo_mode: None,
            hidden_node_indices: HashSet::new(),
            show_base_game: true,
            show_local_overlays: true,
            show_collision: false,
            gpu_cache_budget: GpuCacheBudget::default(),
            gpu_budget_overflow: false,
            scene_load_ms: 0.0,
            last_frame_ms: 0.0,
            last_visible_instances: 0,
        })
    }

    pub fn set_package(&mut self, package: RenderPackage) -> RenderResult<()> {
        self.replace_packages(vec![Arc::new(package)], false)
    }

    pub fn update_streaming_package(&mut self, package: RenderPackage) -> RenderResult<()> {
        self.replace_packages(vec![Arc::new(package)], true)
    }

    pub fn update_streaming_packages(
        &mut self,
        packages: Vec<Arc<RenderPackage>>,
    ) -> RenderResult<()> {
        self.replace_packages(packages, true)
    }

    pub fn set_gpu_cache_budget(&mut self, budget: GpuCacheBudget) -> RenderResult<()> {
        let budget = budget.validate()?;
        self.gpu_cache_budget = budget;
        self.gpu_budget_overflow = self.gpu.evict_to_budget(&self.packages, budget);
        Ok(())
    }

    pub const fn gpu_cache_budget(&self) -> GpuCacheBudget {
        self.gpu_cache_budget
    }

    fn replace_packages(
        &mut self,
        packages: Vec<Arc<RenderPackage>>,
        preserve_camera: bool,
    ) -> RenderResult<()> {
        let started = Instant::now();

        if packages.is_empty() {
            self.packages.clear();
            self.draws.clear();
            self.lod_hierarchy = LodHierarchy::default();
            self.lod_selections.clear();
            self.lod_suppressed.clear();
            self.world_bounds = None;
            self.selected_node_index = None;
            self.hidden_node_indices.clear();
            if !preserve_camera {
                self.camera = None;
            }
            self.gpu_budget_overflow = self
                .gpu
                .evict_to_budget(&self.packages, self.gpu_cache_budget);
            self.scene_load_ms = started.elapsed().as_secs_f64() * 1000.0;
            return Ok(());
        }

        // Build only new package draws. Model buffers/bind groups belonging to
        // unchanged Arc<RenderPackage>s survive residency updates.
        let previous_bases = package_node_bases(&self.packages);
        let next_bases = package_node_bases(&packages);
        let mut claimed = HashSet::new();
        let previous_matches = packages
            .iter()
            .map(|package| {
                if package.descriptor.scene.instances.is_empty() {
                    return None;
                }
                self.packages
                    .iter()
                    .enumerate()
                    .find(|(index, old)| !claimed.contains(index) && Arc::ptr_eq(package, old))
                    .map(|(index, _)| {
                        claimed.insert(index);
                        index
                    })
            })
            .collect::<Vec<_>>();

        let mut new_draws = Vec::with_capacity(packages.len());
        for (index, package) in packages.iter().enumerate() {
            if previous_matches[index].is_some() {
                // This exact package remained resident across updates. GPU
                // eviction protects active packages, so its assets, textures
                // and model bindings do not need to be rebuilt or revalidated.
                new_draws.push(None);
                continue;
            }

            package
                .validate()
                .map_err(|error| RenderError::InvalidInput(error.to_string()))?;
            for texture in &package.descriptor.textures {
                self.gpu.ensure_texture(package, texture)?;
            }
            for asset in &package.descriptor.assets {
                if asset.state == RenderAssetState::Ready {
                    self.gpu.ensure_asset(package, asset)?;
                }
            }
            new_draws.push(Some(self.gpu.build_draw_instances(
                package,
                true,
                next_bases[index],
            )?));
        }

        let reusable_old = previous_matches
            .iter()
            .flatten()
            .copied()
            .collect::<HashSet<_>>();
        let reuse_count = self
            .draws
            .iter()
            .filter(|draw| {
                draw.node_index.is_some_and(|node| {
                    package_index_at_node(&self.packages, &previous_bases, node)
                        .is_some_and(|index| reusable_old.contains(&index))
                })
            })
            .count();
        let fresh_count = new_draws
            .iter()
            .filter_map(Option::as_ref)
            .map(Vec::len)
            .sum::<usize>();
        if reuse_count == 0 && fresh_count == 0 {
            return Err(RenderError::Unsupported(
                "render packages have no ready drawable instances".into(),
            ));
        }

        // Validate the *future* scene before moving model bindings out of
        // self.draws. A failed update must not erase the last good frame.
        let mut world_bounds = Aabb::empty();
        for draw in &self.draws {
            if draw.node_index.is_some_and(|node| {
                package_index_at_node(&self.packages, &previous_bases, node)
                    .is_some_and(|index| reusable_old.contains(&index))
            }) {
                world_bounds.include(draw.world_bounds.min);
                world_bounds.include(draw.world_bounds.max);
            }
        }
        for draw in new_draws.iter().filter_map(Option::as_ref).flatten() {
            world_bounds.include(draw.world_bounds.min);
            world_bounds.include(draw.world_bounds.max);
        }
        if !world_bounds.is_valid() {
            return Err(RenderError::InvalidInput(
                "render packages produced invalid world bounds".into(),
            ));
        }

        let mut previous_draws = std::iter::repeat_with(Vec::new)
            .take(self.packages.len())
            .collect::<Vec<Vec<DrawInstance>>>();
        for draw in std::mem::take(&mut self.draws) {
            if let Some(index) = draw
                .node_index
                .and_then(|node| package_index_at_node(&self.packages, &previous_bases, node))
            {
                previous_draws[index].push(draw);
            }
        }

        let mut draws = Vec::with_capacity(reuse_count + fresh_count);
        for (index, previous_index) in previous_matches.into_iter().enumerate() {
            if let Some(old_index) = previous_index {
                for mut draw in std::mem::take(&mut previous_draws[old_index]) {
                    if let Some(old_node) = draw.node_index {
                        draw.node_index = Some(rebase_package_node(
                            old_node,
                            previous_bases[old_index],
                            next_bases[index],
                        ));
                    }
                    draws.push(draw);
                }
            } else if let Some(mut added) = new_draws[index].take() {
                draws.append(&mut added);
            }
        }
        draws.sort_by_key(|draw| (draw.local_overlay, draw.collision_overlay));

        let camera = if preserve_camera {
            self.camera
                .take()
                .unwrap_or_else(|| OrbitCamera::fit(world_bounds, self.options.projection))
        } else {
            OrbitCamera::fit(world_bounds, self.options.projection)
        };
        let selected_node_index = if preserve_camera {
            self.selected_node_index
                .and_then(|old_node| {
                    let old_package_index =
                        package_index_at_node(&self.packages, &previous_bases, old_node)?;
                    let old_package = &self.packages[old_package_index];
                    let new_package_index = packages
                        .iter()
                        .position(|package| Arc::ptr_eq(package, old_package))?;
                    Some(rebase_package_node(
                        old_node,
                        previous_bases[old_package_index],
                        next_bases[new_package_index],
                    ))
                })
                .filter(|selected| draws.iter().any(|draw| draw.node_index == Some(*selected)))
        } else {
            None
        };
        let gpu_budget_overflow = self.gpu.evict_to_budget(&packages, self.gpu_cache_budget);

        self.lod_hierarchy = LodHierarchy::from_packages(&packages, &draws, &self.gpu.asset_cache);
        let active_keys = self
            .lod_hierarchy
            .nodes
            .iter()
            .map(|node| node.key.clone())
            .collect::<HashSet<_>>();
        self.lod_selections
            .retain(|key, _| active_keys.contains(key));
        self.lod_suppressed.clear();
        self.packages = packages;
        self.draws = draws;
        self.world_bounds = Some(world_bounds);
        self.camera = Some(camera);
        self.selected_node_index = selected_node_index;
        self.hidden_node_indices.clear();
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

    pub fn set_collision_overlay(&mut self, visible: bool) {
        self.show_collision = visible;
    }

    pub fn set_gizmo_mode(&mut self, mode: Option<TransformGizmoMode>) {
        self.gizmo_mode = mode;
    }

    pub fn orbit(&mut self, delta_x: f32, delta_y: f32) {
        if let Some(camera) = self.camera.as_mut() {
            camera.orbit(delta_x, delta_y);
        }
    }

    pub fn look(&mut self, delta_x: f32, delta_y: f32) {
        if let Some(camera) = self.camera.as_mut() {
            camera.look(delta_x, delta_y);
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

    pub fn focus_world_position(&mut self, target: [f32; 3], local_radius: f32) {
        self.camera = Some(OrbitCamera::around_world_position(
            target,
            local_radius,
            self.options.projection,
        ));
    }

    pub fn pick(&self, x: f32, y: f32) -> Option<PickResult> {
        let camera = self.camera.as_ref()?;
        let (origin, direction) = camera.ray(self.options.width, self.options.height, x, y)?;
        self.draws
            .iter()
            .filter(|draw| self.draw_visible(draw))
            .filter(|draw| {
                !draw
                    .node_index
                    .is_some_and(|node| self.lod_suppressed.contains(&node))
            })
            .filter(|draw| camera.lod_visible(draw.world_bounds, draw.lod_dist))
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

    pub fn focus_node(&mut self, node_index: u32) -> bool {
        let Some(draw) = self
            .draws
            .iter()
            .find(|draw| draw.node_index == Some(node_index) && self.draw_visible(draw))
        else {
            return false;
        };
        self.camera = Some(OrbitCamera::fit(draw.world_bounds, self.options.projection));
        true
    }

    pub fn set_node_visible(&mut self, node_index: u32, visible: bool) -> bool {
        if !self
            .draws
            .iter()
            .any(|draw| draw.node_index == Some(node_index))
        {
            return false;
        }
        if visible {
            self.hidden_node_indices.remove(&node_index);
        } else {
            self.hidden_node_indices.insert(node_index);
            if self.selected_node_index == Some(node_index) {
                self.select(None);
            }
        }
        true
    }

    pub fn isolate_node(&mut self, node_index: u32) -> bool {
        if !self
            .draws
            .iter()
            .any(|draw| draw.node_index == Some(node_index))
        {
            return false;
        }
        self.hidden_node_indices = self
            .draws
            .iter()
            .filter_map(|draw| draw.node_index.filter(|candidate| *candidate != node_index))
            .collect();
        true
    }

    pub fn show_all_nodes(&mut self) {
        self.hidden_node_indices.clear();
    }

    pub fn set_layer_visibility(&mut self, base_game: bool, local_overlays: bool) {
        self.show_base_game = base_game;
        self.show_local_overlays = local_overlays;
        if self.selected_node_index.is_some_and(|selected| {
            self.draws
                .iter()
                .find(|draw| draw.node_index == Some(selected))
                .is_some_and(|draw| !self.draw_visible(draw))
        }) {
            self.select(None);
        }
    }

    fn draw_visible(&self, draw: &DrawInstance) -> bool {
        (!draw.collision_overlay || self.show_collision)
            && draw_visibility(
                draw.node_index,
                draw.local_overlay,
                &self.hidden_node_indices,
                self.show_base_game,
                self.show_local_overlays,
            )
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
            self.packages.iter().fold(
                (0_u32, 0_u32, 0_u32, 0_u32, 0_u32, 0_u64),
                |(instances, assets, meshes, materials, textures, bytes), package| {
                    let summary = package.descriptor.summary;
                    (
                        instances.saturating_add(summary.instances),
                        assets.saturating_add(summary.assets),
                        meshes.saturating_add(summary.meshes),
                        materials.saturating_add(summary.materials),
                        textures.saturating_add(summary.textures),
                        bytes.saturating_add(package.blob.len() as u64),
                    )
                },
            );
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
            visible_instances: self.last_visible_instances,
            selected_node_index: self.selected_node_index,
        }
    }

    pub fn render_frame(&mut self) -> RenderResult<()> {
        let started = Instant::now();
        if self.packages.is_empty() {
            return Err(RenderError::InvalidInput(
                "native viewport has no loaded render packages".into(),
            ));
        }
        let camera = self.camera.as_ref().ok_or_else(|| {
            RenderError::InvalidInput("native viewport has no camera state".into())
        })?;
        self.lod_suppressed = self
            .lod_hierarchy
            .suppressed(camera, &mut self.lod_selections);
        let world_bounds = self.world_bounds.ok_or_else(|| {
            RenderError::InvalidInput("native viewport has no world bounds".into())
        })?;

        let camera_uniform = camera.uniform(self.options.width, self.options.height);
        self.gpu
            .queue
            .write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&camera_uniform));
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
        let gizmo_vertices = self
            .gizmo_mode
            .and_then(|mode| {
                let selected = self.selected_node_index?;
                let draw = self.draws.iter().find(|draw| {
                    draw.node_index == Some(selected)
                        && self.draw_visible(draw)
                        && !self.lod_suppressed.contains(&selected)
                })?;
                let center = draw.world_bounds.center();
                let distance = (camera.eye() - center).length().max(0.1);
                let size = draw
                    .world_bounds
                    .radius()
                    .max(distance * 0.08)
                    .clamp(0.15, distance * 0.3);
                Some(build_transform_gizmo_vertices(center, size, mode))
            })
            .unwrap_or_default();
        let mut visible_instances = 0_u32;
        let gizmo_buffer = (!gizmo_vertices.is_empty()).then(|| {
            self.gpu
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("RageLab transform gizmo lines"),
                    contents: bytemuck::cast_slice(&gizmo_vertices),
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

            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            pass.set_pipeline(&self.mesh_pipeline);
            for draw in &self.draws {
                if draw.collision_overlay
                    || !self.draw_visible(draw)
                    || draw
                        .node_index
                        .is_some_and(|node| self.lod_suppressed.contains(&node))
                    || !camera.aabb_visible(
                        draw.world_bounds,
                        self.options.width,
                        self.options.height,
                    )
                    || !camera.lod_visible(draw.world_bounds, draw.lod_dist)
                {
                    continue;
                }
                visible_instances = visible_instances.saturating_add(1);
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

            if self.options.wireframe || self.show_collision {
                pass.set_pipeline(&self.line_pipeline);
                for draw in &self.draws {
                    if !self.draw_visible(draw)
                        || draw
                            .node_index
                            .is_some_and(|node| self.lod_suppressed.contains(&node))
                        || !camera.aabb_visible(
                            draw.world_bounds,
                            self.options.width,
                            self.options.height,
                        )
                        || !camera.lod_visible(draw.world_bounds, draw.lod_dist)
                        || (!self.options.wireframe && !draw.collision_overlay)
                    {
                        continue;
                    }
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
                pass.set_bind_group(1, &self.identity_model.bind_group, &[]);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..overlay_vertices.len() as u32, 0..1);
            }

            if let Some(buffer) = gizmo_buffer.as_ref() {
                pass.set_pipeline(&self.gizmo_pipeline);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..gizmo_vertices.len() as u32, 0..1);
            }
        }

        self.gpu.queue.submit(Some(encoder.finish()));
        frame.present();
        self.last_visible_instances = visible_instances;
        self.last_frame_ms = started.elapsed().as_secs_f64() * 1000.0;
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
) -> (
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
) {
    let mesh_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("RageLab native mesh shader"),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(MESH_SHADER)),
    });
    let line_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("RageLab native line shader"),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(LINE_SHADER)),
    });
    let gizmo_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("RageLab native gizmo shader"),
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(GIZMO_SHADER)),
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
    let gizmo_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("RageLab native gizmo pipeline layout"),
        bind_group_layouts: &[camera_layout],
        push_constant_ranges: &[],
    });
    const POSITION_ATTRIBUTES: [wgpu::VertexAttribute; 1] =
        wgpu::vertex_attr_array![0 => Float32x3];
    const NORMAL_ATTRIBUTES: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![1 => Float32x3];
    const UV_ATTRIBUTES: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![2 => Float32x2];
    const GIZMO_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4];

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

    let gizmo_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("RageLab native gizmo pipeline"),
        layout: Some(&gizmo_layout),
        vertex: wgpu::VertexState {
            module: &gizmo_shader,
            entry_point: "vs_gizmo",
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<GizmoVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &GIZMO_ATTRIBUTES,
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
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &gizmo_shader,
            entry_point: "fs_gizmo",
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

    (mesh_pipeline, line_pipeline, gizmo_pipeline)
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

fn gpu_texture_mip_count(width: u32, height: u32) -> u32 {
    32 - width.max(height).max(1).leading_zeros()
}

fn gpu_texture_mip_bytes(width: u32, height: u32) -> u64 {
    let (mut width, mut height) = (width.max(1), height.max(1));
    let mut total = 0_u64;
    loop {
        total = total.saturating_add(u64::from(width) * u64::from(height) * 4);
        if width == 1 && height == 1 {
            break;
        }
        width = (width / 2).max(1);
        height = (height / 2).max(1);
    }
    total
}

fn srgb_decode_table() -> &'static [f32; 256] {
    static TABLE: OnceLock<[f32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        std::array::from_fn(|index| {
            let value = index as f32 / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        })
    })
}

fn linear_to_srgb8(linear: f32) -> u8 {
    // Generate the nonlinear transfer curve once. A lookup avoids three
    // powf calls per RGBA texel on the GPU upload/render thread.
    const STEPS: usize = 8192;
    static TABLE: OnceLock<[u8; STEPS + 1]> = OnceLock::new();
    let encode = TABLE.get_or_init(|| {
        std::array::from_fn(|index| {
            let value = index as f32 / STEPS as f32;
            let encoded = if value <= 0.0031308 {
                value * 12.92
            } else {
                1.055 * value.powf(1.0 / 2.4) - 0.055
            };
            (encoded * 255.0).round() as u8
        })
    });
    let index = (linear.clamp(0.0, 1.0) * STEPS as f32).round() as usize;
    encode[index]
}

// RGBA8 albedo mips are generated in linear-light and with alpha weighting,
// to avoid dark halos on GTA vegetation/window cutout texture edges.
fn downsample_rgba_srgb(source: &[u8], width: u32, height: u32) -> Vec<u8> {
    let next_width = (width / 2).max(1);
    let next_height = (height / 2).max(1);
    let mut output = vec![0_u8; next_width as usize * next_height as usize * 4];
    let decode = srgb_decode_table();
    for y in 0..next_height {
        for x in 0..next_width {
            let mut alpha_sum = 0.0_f32;
            let mut linear_premultiplied = [0.0_f32; 3];
            for oy in 0..2 {
                for ox in 0..2 {
                    let sx = (x * 2 + ox).min(width - 1);
                    let sy = (y * 2 + oy).min(height - 1);
                    let source_index = ((sy * width + sx) * 4) as usize;
                    let alpha = source[source_index + 3] as f32 / 255.0;
                    alpha_sum += alpha;
                    for channel in 0..3 {
                        linear_premultiplied[channel] +=
                            decode[source[source_index + channel] as usize] * alpha;
                    }
                }
            }
            let out_index = ((y * next_width + x) * 4) as usize;
            for channel in 0..3 {
                output[out_index + channel] = if alpha_sum > 0.0 {
                    linear_to_srgb8(linear_premultiplied[channel] / alpha_sum)
                } else {
                    0
                };
            }
            output[out_index + 3] = (alpha_sum * 0.25 * 255.0).round() as u8;
        }
    }
    output
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
        mip_level_count: gpu_texture_mip_count(width, height),
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let upload_mip = |mip_level: u32, width: u32, height: u32, pixels: &[u8]| {
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
    };
    upload_mip(0, width, height, rgba);
    // Keep only the current reduced mip in CPU memory: do not clone level 0
    // or retain every mipmap during texture uploads.
    let (mut mip_width, mut mip_height) = (width, height);
    let mut mip_pixels = Vec::new();
    for level in 1..gpu_texture_mip_count(width, height) {
        let new_pixels = downsample_rgba_srgb(
            if level == 1 { rgba } else { &mip_pixels },
            mip_width,
            mip_height,
        );
        mip_width = (mip_width / 2).max(1);
        mip_height = (mip_height / 2).max(1);
        upload_mip(level, mip_width, mip_height, &new_pixels);
        mip_pixels = new_pixels;
    }
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

fn draw_visibility(
    node_index: Option<u32>,
    local_overlay: bool,
    hidden_node_indices: &HashSet<u32>,
    show_base_game: bool,
    show_local_overlays: bool,
) -> bool {
    let layer_visible = if local_overlay {
        show_local_overlays
    } else {
        show_base_game
    };
    layer_visible
        && node_index
            .map(|node_index| !hidden_node_indices.contains(&node_index))
            .unwrap_or(true)
}

fn package_world_bounds(_package: &RenderPackage, draws: &[DrawInstance]) -> RenderResult<Aabb> {
    draws_world_bounds(draws)
}

fn draws_world_bounds(draws: &[DrawInstance]) -> RenderResult<Aabb> {
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

fn build_transform_gizmo_vertices(
    center: Vec3,
    size: f32,
    mode: TransformGizmoMode,
) -> Vec<GizmoVertex> {
    const X_COLOR: [f32; 4] = [0.96, 0.22, 0.18, 1.0];
    const Y_COLOR: [f32; 4] = [0.28, 0.86, 0.34, 1.0];
    const Z_COLOR: [f32; 4] = [0.22, 0.48, 1.0, 1.0];
    let size = size.max(0.001);
    let axes = [
        (Vec3::X, Vec3::Y, Vec3::Z, X_COLOR),
        (Vec3::Y, Vec3::Z, Vec3::X, Y_COLOR),
        (Vec3::Z, Vec3::X, Vec3::Y, Z_COLOR),
    ];
    let mut vertices = Vec::new();

    match mode {
        TransformGizmoMode::Translate => {
            for (axis, side_a, side_b, color) in axes {
                let tip = center + axis * size;
                push_gizmo_line(&mut vertices, center, tip, color);
                let back = tip - axis * (size * 0.18);
                let wing = size * 0.08;
                for offset in [side_a * wing, -side_a * wing, side_b * wing, -side_b * wing] {
                    push_gizmo_line(&mut vertices, tip, back + offset, color);
                }
            }
        }
        TransformGizmoMode::Rotate => {
            const SEGMENTS: usize = 48;
            let radius = size * 0.78;
            for (_axis, side_a, side_b, color) in axes {
                for segment in 0..SEGMENTS {
                    let a0 = segment as f32 * std::f32::consts::TAU / SEGMENTS as f32;
                    let a1 = (segment + 1) as f32 * std::f32::consts::TAU / SEGMENTS as f32;
                    let p0 = center + (side_a * a0.cos() + side_b * a0.sin()) * radius;
                    let p1 = center + (side_a * a1.cos() + side_b * a1.sin()) * radius;
                    push_gizmo_line(&mut vertices, p0, p1, color);
                }
            }
        }
        TransformGizmoMode::Scale => {
            for (axis, side_a, side_b, color) in axes {
                let tip = center + axis * size;
                push_gizmo_line(&mut vertices, center, tip, color);
                let marker = size * 0.075;
                let a = tip + side_a * marker + side_b * marker;
                let b = tip - side_a * marker + side_b * marker;
                let c = tip - side_a * marker - side_b * marker;
                let d = tip + side_a * marker - side_b * marker;
                for (from, to) in [(a, b), (b, c), (c, d), (d, a)] {
                    push_gizmo_line(&mut vertices, from, to, color);
                }
            }
        }
    }

    vertices
}

fn push_gizmo_line(vertices: &mut Vec<GizmoVertex>, from: Vec3, to: Vec3, color: [f32; 4]) {
    vertices.push(GizmoVertex {
        position: from.to_array(),
        color,
    });
    vertices.push(GizmoVertex {
        position: to.to_array(),
        color,
    });
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

    fn synthetic_lod_entity(
        ymap: &str,
        entity_index: u32,
        parent_index: Option<i32>,
        num_children: Option<u32>,
        child_lod_dist: Option<f32>,
        center: Vec3,
        ready: bool,
    ) -> LodEntity {
        LodEntity {
            key: LodEntityKey {
                source_ymap: ymap.into(),
                entity_index,
            },
            node_index: entity_index,
            map_hash: None,
            parent_map_hash: None,
            parent_index,
            num_children,
            child_lod_dist,
            lod_dist: Some(500.0),
            bounds: ready.then_some(Aabb {
                min: center - Vec3::splat(1.0),
                max: center + Vec3::splat(1.0),
            }),
        }
    }

    #[test]
    fn lod_parent_survives_missing_and_unready_paginated_children() {
        let camera = OrbitCamera::around_world_position([0.0; 3], 120.0, Projection::Perspective);
        let center = camera.eye() + camera.forward() * 80.0;
        let root = synthetic_lod_entity("map.ymap", 0, None, Some(2), Some(100.0), center, true);
        let child_a = synthetic_lod_entity("map.ymap", 1, Some(0), None, None, center, true);
        let child_b = synthetic_lod_entity("map.ymap", 2, Some(0), None, None, center, true);
        let mut state = HashMap::new();

        // Only the first page is resident; it must not remove the proxy.
        let partial = LodHierarchy::from_nodes(vec![root.clone(), child_a.clone()]);
        assert_eq!(partial.suppressed(&camera, &mut state), HashSet::from([1]));

        // The second page is present but its drawable was not resolved.
        let mut unresolved = child_b.clone();
        unresolved.bounds = None;
        let pending = LodHierarchy::from_nodes(vec![root.clone(), child_a.clone(), unresolved]);
        assert_eq!(
            pending.suppressed(&camera, &mut state),
            HashSet::from([1, 2])
        );

        // Both pages become drawable: switch atomically to both HD children.
        let complete = LodHierarchy::from_nodes(vec![root, child_a, child_b]);
        assert_eq!(complete.suppressed(&camera, &mut state), HashSet::from([0]));

        // Losing a child after a successful switch restores the parent immediately.
        assert_eq!(
            pending.suppressed(&camera, &mut state),
            HashSet::from([1, 2])
        );
    }

    #[test]
    fn lod_proxy_stays_visible_when_all_loaded_children_expire_at_distance() {
        let camera = OrbitCamera::around_world_position([0.0; 3], 120.0, Projection::Perspective);
        let center = camera.eye() + camera.forward() * 220.0;
        let parent = synthetic_lod_entity("map.ymap", 0, None, Some(2), Some(280.0), center, true);
        let mut child_a = synthetic_lod_entity("map.ymap", 1, Some(0), None, None, center, true);
        let mut child_b = synthetic_lod_entity("map.ymap", 2, Some(0), None, None, center, true);
        child_a.lod_dist = Some(70.0);
        child_b.lod_dist = Some(70.0);
        let tree = LodHierarchy::from_nodes(vec![parent, child_a, child_b]);
        let mut state = HashMap::new();
        assert_eq!(tree.suppressed(&camera, &mut state), HashSet::from([1, 2]));
    }

    #[test]
    fn lod_hysteresis_avoids_thrashing_and_returns_to_detail() {
        let mut camera =
            OrbitCamera::around_world_position([0.0; 3], 120.0, Projection::Perspective);
        let direction = camera.forward();
        let center = camera.eye() + direction * 90.0;
        let tree = LodHierarchy::from_nodes(vec![
            synthetic_lod_entity("map", 0, None, Some(1), Some(100.0), center, true),
            synthetic_lod_entity("map", 1, Some(0), None, None, center, true),
        ]);
        let mut state = HashMap::new();
        assert_eq!(tree.suppressed(&camera, &mut state), HashSet::from([0]));
        camera.target -= direction * 16.0;
        assert_eq!(tree.suppressed(&camera, &mut state), HashSet::from([0]));
        camera.target -= direction * 16.0;
        assert_eq!(tree.suppressed(&camera, &mut state), HashSet::from([1]));
        camera.target += direction * 16.0;
        assert_eq!(tree.suppressed(&camera, &mut state), HashSet::from([1]));
        camera.target += direction * 16.0;
        assert_eq!(tree.suppressed(&camera, &mut state), HashSet::from([0]));
    }

    #[test]
    fn lod_nested_branches_and_unlinked_ymaps_are_safe() {
        let camera = OrbitCamera::around_world_position([0.0; 3], 120.0, Projection::Perspective);
        let center = camera.eye() + camera.forward() * 80.0;
        let tree = LodHierarchy::from_nodes(vec![
            synthetic_lod_entity("a.ymap", 0, None, Some(1), Some(100.0), center, true),
            synthetic_lod_entity("a.ymap", 1, Some(0), Some(1), Some(100.0), center, true),
            synthetic_lod_entity("a.ymap", 2, Some(1), None, None, center, true),
            synthetic_lod_entity("b.ymap", 3, Some(0), None, None, center, true),
        ]);
        let mut state = HashMap::new();
        assert_eq!(tree.suppressed(&camera, &mut state), HashSet::from([0, 1]));
        // A child from another YMAP cannot satisfy the parent's child count.
        let incomplete = LodHierarchy::from_nodes(vec![
            synthetic_lod_entity("a.ymap", 0, None, Some(2), Some(100.0), center, true),
            synthetic_lod_entity("a.ymap", 1, Some(0), None, None, center, true),
            synthetic_lod_entity("b.ymap", 3, Some(0), None, None, center, true),
        ]);
        assert_eq!(
            incomplete.suppressed(&camera, &mut state),
            HashSet::from([1])
        );
    }

    #[test]
    fn lod_switches_across_santamon_style_parent_ymap_chain() {
        let camera = OrbitCamera::around_world_position([0.0; 3], 120.0, Projection::Perspective);
        let center = camera.eye() + camera.forward() * 80.0;
        let mut slod =
            synthetic_lod_entity("sm_lod.ymap", 20, None, Some(1), Some(130.0), center, true);
        slod.map_hash = Some(0x100);
        slod.node_index = 0;
        let mut proxy = synthetic_lod_entity(
            "sm_23.ymap",
            2,
            Some(20),
            Some(2),
            Some(100.0),
            center,
            true,
        );
        proxy.map_hash = Some(0x200);
        proxy.parent_map_hash = Some(0x100);
        proxy.node_index = 1;
        let mut detail_a =
            synthetic_lod_entity("sm_23_strm_0.ymap", 0, Some(2), None, None, center, true);
        detail_a.map_hash = Some(0x300);
        detail_a.parent_map_hash = Some(0x200);
        detail_a.node_index = 2;
        let mut detail_b =
            synthetic_lod_entity("sm_23_strm_1.ymap", 0, Some(2), None, None, center, true);
        detail_b.map_hash = Some(0x301);
        detail_b.parent_map_hash = Some(0x200);
        detail_b.node_index = 3;

        let mut state = HashMap::new();
        let initial = LodHierarchy::from_nodes(vec![slod.clone(), proxy.clone()]);
        assert_eq!(initial.suppressed(&camera, &mut state), HashSet::from([0]));

        let one_page =
            LodHierarchy::from_nodes(vec![slod.clone(), proxy.clone(), detail_a.clone()]);
        assert_eq!(
            one_page.suppressed(&camera, &mut state),
            HashSet::from([0, 2])
        );

        let complete = LodHierarchy::from_nodes(vec![slod, proxy, detail_a, detail_b]);
        assert_eq!(
            complete.suppressed(&camera, &mut state),
            HashSet::from([0, 1])
        );
    }

    #[test]
    fn lod_parent_map_hash_prevents_false_local_parent_attachment() {
        let camera = OrbitCamera::around_world_position([0.0; 3], 120.0, Projection::Perspective);
        let center = camera.eye() + camera.forward() * 80.0;
        let mut local = synthetic_lod_entity(
            "sm_23_strm_0.ymap",
            2,
            None,
            Some(1),
            Some(100.0),
            center,
            true,
        );
        local.map_hash = Some(0x300);
        local.node_index = 9;
        let mut external_child =
            synthetic_lod_entity("sm_23_strm_0.ymap", 0, Some(2), None, None, center, true);
        external_child.map_hash = Some(0x300);
        external_child.parent_map_hash = Some(0x200);
        external_child.node_index = 10;
        let mut state = HashMap::new();
        let no_parent = LodHierarchy::from_nodes(vec![local, external_child.clone()]);
        assert!(no_parent.suppressed(&camera, &mut state).is_empty());

        let mut real_parent =
            synthetic_lod_entity("sm_23.ymap", 2, None, Some(1), Some(100.0), center, true);
        real_parent.map_hash = Some(0x200);
        real_parent.node_index = 11;
        let linked = LodHierarchy::from_nodes(vec![real_parent, external_child]);
        assert_eq!(linked.suppressed(&camera, &mut state), HashSet::from([11]));
    }

    #[test]
    fn texture_mipmap_chain_size_matches_gpu_allocation() {
        assert_eq!(gpu_texture_mip_count(1, 1), 1);
        assert_eq!(gpu_texture_mip_count(3, 5), 3);
        assert_eq!(gpu_texture_mip_count(256, 128), 9);
        assert_eq!(gpu_texture_mip_bytes(1, 1), 4);
        assert_eq!(gpu_texture_mip_bytes(4, 4), 84);
        assert_eq!(gpu_texture_mip_bytes(3, 5), 60 + 8 + 4);
    }

    #[test]
    fn texture_mips_average_srgb_in_linear_light() {
        // 2x2 black/white checkerboard, all pixels opaque.
        let pixels = [
            0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255,
        ];
        let mip = downsample_rgba_srgb(&pixels, 2, 2);
        assert_eq!(mip.len(), 4);
        // Linear-light average of black/white is ~188 sRGB, not 128.
        assert!((mip[0] as i32 - 188).abs() <= 1);
        assert_eq!(mip[0], mip[1]);
        assert_eq!(mip[1], mip[2]);
        assert_eq!(mip[3], 255);
    }

    #[test]
    fn texture_mips_preserve_opaque_color_under_transparent_cutout() {
        let pixels = [255, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let mip = downsample_rgba_srgb(&pixels, 2, 2);
        assert_eq!(&mip[..3], &[255, 0, 0]);
        assert_eq!(mip[3], 64);
    }

    #[test]
    fn package_node_rebasing_preserves_local_identity_when_page_inserted() {
        // A page of three instances was formerly at base 10. Two new
        // instances are inserted before it, so its base moves to 12.
        assert_eq!(rebase_package_node(10, 10, 12), 12);
        assert_eq!(rebase_package_node(11, 10, 12), 13);
        assert_eq!(rebase_package_node(12, 10, 12), 14);
        assert_eq!(rebase_package_node(14, 12, 3), 5);
    }

    #[test]
    fn gpu_eviction_selection_is_lru_stable_and_can_release_texture_pressure() {
        let active_asset = "active".to_string();
        let old_asset = "asset-a".to_string();
        let new_asset = "asset-b".to_string();
        let active_texture = "texture-active".to_string();
        let old_texture = "texture-old".to_string();
        let new_texture = "texture-new".to_string();

        let mut assets = HashMap::new();
        assets.insert(
            active_asset.clone(),
            GpuAssetCacheMeta {
                bytes: 64,
                last_used: 1,
                texture_keys: vec![active_texture.clone()],
            },
        );
        assets.insert(
            old_asset.clone(),
            GpuAssetCacheMeta {
                bytes: 64,
                last_used: 2,
                texture_keys: vec![old_texture.clone()],
            },
        );
        assets.insert(
            new_asset.clone(),
            GpuAssetCacheMeta {
                bytes: 64,
                last_used: 3,
                texture_keys: vec![new_texture.clone()],
            },
        );

        let protected_assets = HashSet::from([active_asset]);
        let active_textures = HashSet::from([active_texture]);
        let resident_textures = HashSet::from([old_texture, new_texture]);

        assert_eq!(
            select_asset_releasing_texture(
                assets.iter(),
                &protected_assets,
                &active_textures,
                &resident_textures,
            ),
            Some(old_asset)
        );

        let left = "a".to_string();
        let right = "b".to_string();
        let protected = HashSet::new();
        assert_eq!(
            select_lru_key([(&right, 7), (&left, 7)].into_iter(), &protected),
            Some(left)
        );
    }

    #[test]
    fn world_camera_keeps_distant_proxy_geometry_in_frustum() {
        let camera = OrbitCamera::around_world_position([0.0; 3], 120.0, Projection::Perspective);
        assert!(camera.far_distance() >= 2_500.0);
        let proxy_center = camera.eye() + camera.forward() * 1_850.0;
        let proxy_bounds = Aabb {
            min: proxy_center - Vec3::splat(25.0),
            max: proxy_center + Vec3::splat(25.0),
        };
        assert!(camera.aabb_visible(proxy_bounds, 1280, 800));
        assert!(camera.lod_visible(proxy_bounds, Some(4_000.0)));
    }

    #[test]
    fn orbit_camera_respects_entity_lod_distance_conservatively() {
        let camera =
            OrbitCamera::around_world_position([0.0, 0.0, 0.0], 120.0, Projection::Perspective);
        let eye = camera.eye();
        let forward = camera.forward();

        let near_center = eye + forward * 80.0;
        let near = Aabb {
            min: near_center - Vec3::splat(5.0),
            max: near_center + Vec3::splat(5.0),
        };
        assert!(camera.lod_visible(near, Some(100.0)));
        assert!(camera.lod_visible(near, None));
        assert!(camera.lod_visible(near, Some(0.0)));

        let far_center = eye + forward * 220.0;
        let far = Aabb {
            min: far_center - Vec3::splat(5.0),
            max: far_center + Vec3::splat(5.0),
        };
        assert!(!camera.lod_visible(far, Some(100.0)));
    }

    #[test]
    fn orbit_camera_culls_bounds_behind_and_outside_frustum() {
        let camera =
            OrbitCamera::around_world_position([0.0, 0.0, 0.0], 450.0, Projection::Perspective);
        let near_target = Aabb {
            min: Vec3::splat(-5.0),
            max: Vec3::splat(5.0),
        };
        assert!(camera.aabb_visible(near_target, 1280, 800));

        let eye = camera.eye();
        let forward = camera.forward();
        let right = camera.right();
        let behind_center = eye - forward * 100.0;
        let behind = Aabb {
            min: behind_center - Vec3::splat(2.0),
            max: behind_center + Vec3::splat(2.0),
        };
        assert!(!camera.aabb_visible(behind, 1280, 800));

        let side_center = eye + forward * 150.0 + right * 800.0;
        let outside_side = Aabb {
            min: side_center - Vec3::splat(2.0),
            max: side_center + Vec3::splat(2.0),
        };
        assert!(!camera.aabb_visible(outside_side, 1280, 800));
    }

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
        let orbit_eye = camera.eye();
        camera.look(-0.1, 0.08);
        assert!((camera.eye() - orbit_eye).length() < 1.0e-4);
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
    fn draw_visibility_respects_layer_and_hidden_node_state() {
        let mut hidden = HashSet::new();
        hidden.insert(7);

        assert!(draw_visibility(Some(1), false, &hidden, true, true));
        assert!(!draw_visibility(Some(7), false, &hidden, true, true));
        assert!(!draw_visibility(Some(1), false, &hidden, false, true));
        assert!(draw_visibility(Some(1), true, &hidden, false, true));
        assert!(!draw_visibility(Some(1), true, &hidden, true, false));
        assert!(draw_visibility(None, false, &hidden, true, true));
    }

    #[test]
    fn transform_gizmo_geometry_is_deterministic_per_mode() {
        let center = Vec3::new(10.0, -5.0, 2.0);
        let translate = build_transform_gizmo_vertices(center, 2.0, TransformGizmoMode::Translate);
        let rotate = build_transform_gizmo_vertices(center, 2.0, TransformGizmoMode::Rotate);
        let scale = build_transform_gizmo_vertices(center, 2.0, TransformGizmoMode::Scale);

        assert_eq!(translate.len(), 30);
        assert_eq!(rotate.len(), 288);
        assert_eq!(scale.len(), 30);
        assert_eq!(translate[0].position, center.to_array());
        assert_eq!(translate[1].position, (center + Vec3::X * 2.0).to_array());
        assert_eq!(translate[0].color, [0.96, 0.22, 0.18, 1.0]);
        assert!(rotate
            .iter()
            .all(|vertex| vertex.position.iter().all(|value| value.is_finite())));
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

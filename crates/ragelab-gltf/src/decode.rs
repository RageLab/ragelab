use std::collections::HashSet;

use serde::Serialize;
use serde_json::Value;

use crate::{
    inspect_glb,
    parser::{parse_container, GlbError},
};

const MAX_VERTICES_PER_PRIMITIVE: usize = 2_000_000;
const MAX_INDICES_PER_PRIMITIVE: usize = 6_000_000;
const GEOMETRY_SCHEMA: &str = "ragelab.glb.geometry";
const GLTF_TO_GTA_Z_UP: bool = true;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodedGeometryReport {
    pub schema: &'static str,
    pub schema_version: u32,
    pub source_sha256: String,
    pub source_byte_length: usize,
    pub selected_scene: Option<usize>,
    pub primitives: Vec<DecodedPrimitive>,
    pub uninstantiated_meshes: Vec<usize>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecodedPrimitive {
    pub node_index: usize,
    pub node_name: Option<String>,
    pub mesh_index: usize,
    pub mesh_name: Option<String>,
    pub primitive_index: usize,
    pub material_index: Option<usize>,
    pub source_mode: u32,
    pub vertex_count: usize,
    pub index_count: usize,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    pub tangents: Option<Vec<[f32; 4]>>,
    pub texcoord_0: Option<Vec<[f32; 2]>>,
    pub texcoord_1: Option<Vec<[f32; 2]>>,
    pub colors_0: Option<Vec<[f32; 4]>>,
    pub indices: Vec<u32>,
    pub unsupported_attributes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Mat4([[f64; 4]; 4]);

impl Mat4 {
    const IDENTITY: Self = Self([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]);

    fn mul(self, rhs: Self) -> Self {
        let mut result = [[0.0; 4]; 4];
        for (row, output_row) in result.iter_mut().enumerate() {
            for (column, output) in output_row.iter_mut().enumerate() {
                *output = (0..4)
                    .map(|index| self.0[row][index] * rhs.0[index][column])
                    .sum();
            }
        }
        Self(result)
    }

    fn transform_position(self, value: [f64; 3]) -> Result<[f64; 3], GlbError> {
        let input = [value[0], value[1], value[2], 1.0];
        let mut output = [0.0; 4];
        for (row, target) in output.iter_mut().enumerate() {
            *target = (0..4)
                .map(|column| self.0[row][column] * input[column])
                .sum();
        }
        let w = output[3];
        if !w.is_finite() || w.abs() < 1.0e-12 {
            return Err(GlbError::Unsupported(
                "node transform produces a zero or non-finite homogeneous w".into(),
            ));
        }
        let transformed = [output[0] / w, output[1] / w, output[2] / w];
        if transformed.iter().any(|value| !value.is_finite()) {
            return Err(GlbError::Unsupported(
                "node transform produces non-finite vertex positions".into(),
            ));
        }
        Ok(transformed)
    }

    fn transform_vector(self, value: [f64; 3]) -> Result<[f64; 3], GlbError> {
        let transformed = [
            self.0[0][0] * value[0] + self.0[0][1] * value[1] + self.0[0][2] * value[2],
            self.0[1][0] * value[0] + self.0[1][1] * value[1] + self.0[1][2] * value[2],
            self.0[2][0] * value[0] + self.0[2][1] * value[1] + self.0[2][2] * value[2],
        ];
        normalize3(transformed)
    }

    fn determinant3(self) -> f64 {
        let m = self.0;
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }

    fn transform_normal(self, value: [f64; 3]) -> Result<[f64; 3], GlbError> {
        let m = self.0;
        let determinant = self.determinant3();
        if !determinant.is_finite() || determinant.abs() < 1.0e-12 {
            return Err(GlbError::Unsupported(
                "normal transform is singular; degenerate node scale cannot be exported".into(),
            ));
        }
        // Inverse-transpose of the upper-left 3x3, expanded as cofactors.
        let inv_det = 1.0 / determinant;
        let normal_matrix = [
            [
                (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv_det,
                (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv_det,
                (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv_det,
            ],
            [
                (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv_det,
                (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv_det,
                (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv_det,
            ],
            [
                (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv_det,
                (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv_det,
                (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv_det,
            ],
        ];
        // Cofactor matrix is already inverse-transpose.
        let transformed = [
            normal_matrix[0][0] * value[0]
                + normal_matrix[0][1] * value[1]
                + normal_matrix[0][2] * value[2],
            normal_matrix[1][0] * value[0]
                + normal_matrix[1][1] * value[1]
                + normal_matrix[1][2] * value[2],
            normal_matrix[2][0] * value[0]
                + normal_matrix[2][1] * value[1]
                + normal_matrix[2][2] * value[2],
        ];
        normalize3(transformed)
    }

    fn from_node(node: &Value, index: usize) -> Result<Self, GlbError> {
        if let Some(matrix) = node.get("matrix") {
            let values = read_float_array(matrix, 16, &format!("nodes[{index}].matrix"))?;
            let mut output = [[0.0; 4]; 4];
            for row in 0..4 {
                for column in 0..4 {
                    output[row][column] = values[column * 4 + row];
                }
            }
            if output[3][0].abs() > 1.0e-12
                || output[3][1].abs() > 1.0e-12
                || output[3][2].abs() > 1.0e-12
                || (output[3][3] - 1.0).abs() > 1.0e-12
            {
                return Err(GlbError::Unsupported(format!(
                    "nodes[{index}].matrix is not an affine transform"
                )));
            }
            return Ok(Self(output));
        }

        let translation = optional_float_array(node.get("translation"), 3, "translation")?
            .unwrap_or_else(|| vec![0.0, 0.0, 0.0]);
        let rotation = optional_float_array(node.get("rotation"), 4, "rotation")?
            .unwrap_or_else(|| vec![0.0, 0.0, 0.0, 1.0]);
        let scale = optional_float_array(node.get("scale"), 3, "scale")?
            .unwrap_or_else(|| vec![1.0, 1.0, 1.0]);

        let quaternion_length = rotation
            .iter()
            .map(|value| value * value)
            .sum::<f64>()
            .sqrt();
        if !quaternion_length.is_finite() || quaternion_length < 1.0e-12 {
            return Err(GlbError::InvalidDocument(format!(
                "nodes[{index}].rotation is a zero or non-finite quaternion"
            )));
        }
        let x = rotation[0] / quaternion_length;
        let y = rotation[1] / quaternion_length;
        let z = rotation[2] / quaternion_length;
        let w = rotation[3] / quaternion_length;
        let rotation_matrix = Self([
            [
                1.0 - 2.0 * (y * y + z * z),
                2.0 * (x * y - z * w),
                2.0 * (x * z + y * w),
                0.0,
            ],
            [
                2.0 * (x * y + z * w),
                1.0 - 2.0 * (x * x + z * z),
                2.0 * (y * z - x * w),
                0.0,
            ],
            [
                2.0 * (x * z - y * w),
                2.0 * (y * z + x * w),
                1.0 - 2.0 * (x * x + y * y),
                0.0,
            ],
            [0.0, 0.0, 0.0, 1.0],
        ]);
        let scale_matrix = Self([
            [scale[0], 0.0, 0.0, 0.0],
            [0.0, scale[1], 0.0, 0.0],
            [0.0, 0.0, scale[2], 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ]);
        let translation_matrix = Self([
            [1.0, 0.0, 0.0, translation[0]],
            [0.0, 1.0, 0.0, translation[1]],
            [0.0, 0.0, 1.0, translation[2]],
            [0.0, 0.0, 0.0, 1.0],
        ]);
        Ok(translation_matrix.mul(rotation_matrix).mul(scale_matrix))
    }
}

pub fn decode_glb_geometry(
    bytes: &[u8],
    file_name: impl Into<String>,
) -> Result<DecodedGeometryReport, GlbError> {
    let file_name = file_name.into();
    let inspection = inspect_glb(bytes, file_name)?;
    if !inspection.unsupported_required_extensions.is_empty() {
        return Err(GlbError::Unsupported(format!(
            "required extensions are not supported for geometry decoding: {}",
            inspection.unsupported_required_extensions.join(", ")
        )));
    }
    let container = parse_container(bytes)?;
    let root = &container.json;
    let nodes = array(root, "nodes");
    let meshes = array(root, "meshes");
    let scenes = array(root, "scenes");
    let accessors = array(root, "accessors");
    let buffer_views = array(root, "bufferViews");
    let buffers = array(root, "buffers");

    let selected_scene = root
        .get("scene")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .or_else(|| (!scenes.is_empty()).then_some(0));
    let roots = if let Some(scene_index) = selected_scene {
        scenes
            .get(scene_index)
            .ok_or_else(|| {
                GlbError::InvalidDocument(format!("scene index {scene_index} is out of range"))
            })?
            .get("nodes")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                GlbError::InvalidDocument(format!("scenes[{scene_index}].nodes must be an array"))
            })?
            .iter()
            .map(|value| {
                value.as_u64().map(|value| value as usize).ok_or_else(|| {
                    GlbError::InvalidDocument("scene root node is not a valid index".into())
                })
            })
            .collect::<Result<Vec<_>, _>>()?
    } else {
        let mut child_nodes = HashSet::new();
        for node in nodes {
            if let Some(children) = node.get("children").and_then(Value::as_array) {
                for child in children {
                    let index = child.as_u64().ok_or_else(|| {
                        GlbError::InvalidDocument("node child is not a valid index".into())
                    })? as usize;
                    child_nodes.insert(index);
                }
            }
        }
        (0..nodes.len())
            .filter(|index| !child_nodes.contains(index))
            .collect()
    };

    let mut report = DecodedGeometryReport {
        schema: GEOMETRY_SCHEMA,
        schema_version: 1,
        source_sha256: inspection.source.sha256,
        source_byte_length: inspection.source.byte_length,
        selected_scene,
        primitives: Vec::new(),
        uninstantiated_meshes: Vec::new(),
        warnings: Vec::new(),
    };
    let mut referenced_meshes = HashSet::new();
    let mut recursion = HashSet::new();
    let mut visited_nodes = HashSet::new();
    for node_index in roots {
        visit_node(
            node_index,
            Mat4::IDENTITY,
            nodes,
            meshes,
            accessors,
            buffer_views,
            buffers,
            container.bin,
            &mut recursion,
            &mut visited_nodes,
            &mut referenced_meshes,
            &mut report,
        )?;
    }

    for mesh_index in 0..meshes.len() {
        if !referenced_meshes.contains(&mesh_index) {
            report.uninstantiated_meshes.push(mesh_index);
        }
    }
    if !report.uninstantiated_meshes.is_empty() {
        report.warnings.push(
            "some meshes are not referenced by nodes in the selected scene and were not decoded"
                .into(),
        );
    }
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
fn visit_node(
    node_index: usize,
    parent: Mat4,
    nodes: &[Value],
    meshes: &[Value],
    accessors: &[Value],
    buffer_views: &[Value],
    buffers: &[Value],
    bin: Option<&[u8]>,
    recursion: &mut HashSet<usize>,
    visited: &mut HashSet<usize>,
    referenced_meshes: &mut HashSet<usize>,
    report: &mut DecodedGeometryReport,
) -> Result<(), GlbError> {
    if !recursion.insert(node_index) {
        return Err(GlbError::InvalidDocument(format!(
            "node hierarchy contains a cycle at node {node_index}"
        )));
    }
    if !visited.insert(node_index) {
        return Err(GlbError::Unsupported(format!(
            "node {node_index} is referenced more than once in the selected scene"
        )));
    }
    let node = nodes.get(node_index).ok_or_else(|| {
        GlbError::InvalidDocument(format!("node index {node_index} is out of range"))
    })?;
    if node.get("skin").is_some() {
        return Err(GlbError::Unsupported(format!(
            "node {node_index} is skinned; skinning is not supported by the initial static YDR importer"
        )));
    }
    let world = parent.mul(Mat4::from_node(node, node_index)?);
    if let Some(mesh_value) = node.get("mesh") {
        let mesh_index = mesh_value.as_u64().ok_or_else(|| {
            GlbError::InvalidDocument(format!("nodes[{node_index}].mesh must be an index"))
        })? as usize;
        let mesh = meshes.get(mesh_index).ok_or_else(|| {
            GlbError::InvalidDocument(format!("mesh index {mesh_index} is out of range"))
        })?;
        referenced_meshes.insert(mesh_index);
        let primitive_values = mesh
            .get("primitives")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                GlbError::InvalidDocument(format!(
                    "meshes[{mesh_index}].primitives must be an array"
                ))
            })?;
        for (primitive_index, primitive) in primitive_values.iter().enumerate() {
            if primitive
                .get("extensions")
                .and_then(Value::as_object)
                .is_some_and(|extensions| extensions.contains_key("KHR_draco_mesh_compression"))
            {
                return Err(GlbError::Unsupported(format!(
                    "mesh {mesh_index} primitive {primitive_index} uses KHR_draco_mesh_compression"
                )));
            }
            if primitive
                .get("targets")
                .and_then(Value::as_array)
                .is_some_and(|values| !values.is_empty())
            {
                return Err(GlbError::Unsupported(format!(
                    "mesh {mesh_index} primitive {primitive_index} uses morph targets"
                )));
            }
            let mode = primitive.get("mode").and_then(Value::as_u64).unwrap_or(4) as u32;
            if !matches!(mode, 4..=6) {
                return Err(GlbError::Unsupported(format!(
                    "mesh {mesh_index} primitive {primitive_index} uses non-triangle mode {mode}"
                )));
            }
            let attributes = primitive
                .get("attributes")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    GlbError::InvalidDocument(format!(
                        "mesh {mesh_index} primitive {primitive_index} has no attributes object"
                    ))
                })?;
            let position_index = attributes
                .get("POSITION")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    GlbError::InvalidDocument(format!(
                    "mesh {mesh_index} primitive {primitive_index} has no valid POSITION accessor"
                ))
                })? as usize;
            let positions_raw = decode_vec_accessor(
                position_index,
                accessors,
                buffer_views,
                buffers,
                bin,
                3,
                "POSITION",
                true,
            )?;
            let positions_source = positions_raw
                .iter()
                .map(|value| [value[0], value[1], value[2]])
                .collect::<Vec<_>>();
            if positions_source.len() > MAX_VERTICES_PER_PRIMITIVE {
                return Err(GlbError::Unsupported(format!(
                    "mesh {mesh_index} primitive {primitive_index} exceeds the vertex safety limit"
                )));
            }

            let indices_source = if let Some(index_value) = primitive.get("indices") {
                let index = index_value.as_u64().ok_or_else(|| {
                    GlbError::InvalidDocument("primitive indices must be an accessor index".into())
                })? as usize;
                decode_indices(index, accessors, buffer_views, buffers, bin)?
            } else {
                (0..positions_source.len() as u32).collect()
            };
            let mut indices = triangulate(mode, &indices_source, mesh_index, primitive_index)?;
            if indices.len() > MAX_INDICES_PER_PRIMITIVE {
                return Err(GlbError::Unsupported(format!(
                    "mesh {mesh_index} primitive {primitive_index} exceeds the index safety limit"
                )));
            }
            for index in &indices {
                if *index as usize >= positions_source.len() {
                    return Err(GlbError::InvalidDocument(format!(
                        "mesh {mesh_index} primitive {primitive_index} index {index} exceeds vertex count {}",
                        positions_source.len()
                    )));
                }
            }

            let determinant = world.determinant3();
            if !determinant.is_finite() || determinant.abs() < 1.0e-12 {
                return Err(GlbError::Unsupported(format!(
                    "node {node_index} has a singular world transform"
                )));
            }
            if determinant < 0.0 {
                for triangle in indices.chunks_exact_mut(3) {
                    triangle.swap(1, 2);
                }
            }
            let positions = positions_source
                .iter()
                .map(|position| {
                    let transformed = world.transform_position(*position)?;
                    as_f32(to_gta_z_up(transformed), "POSITION")
                })
                .collect::<Result<Vec<_>, GlbError>>()?;
            let (bounds_min, bounds_max) = calculate_bounds(&positions)?;

            let normals = decode_optional_vec(
                "NORMAL",
                3,
                attributes,
                accessors,
                buffer_views,
                buffers,
                bin,
            )?
            .map(|values| {
                values
                    .into_iter()
                    .map(|value| {
                        let transformed = world.transform_normal([value[0], value[1], value[2]])?;
                        as_f32(to_gta_z_up(transformed), "NORMAL")
                    })
                    .collect::<Result<Vec<_>, GlbError>>()
            })
            .transpose()?;

            let tangents = decode_optional_vec(
                "TANGENT",
                4,
                attributes,
                accessors,
                buffer_views,
                buffers,
                bin,
            )?
            .map(|values| {
                values
                    .into_iter()
                    .map(|value| {
                        let transformed = world.transform_vector([value[0], value[1], value[2]])?;
                        let direction = to_gta_z_up(transformed);
                        Ok([
                            to_f32(direction[0], "TANGENT")?,
                            to_f32(direction[1], "TANGENT")?,
                            to_f32(direction[2], "TANGENT")?,
                            to_f32(
                                if determinant < 0.0 {
                                    -value[3]
                                } else {
                                    value[3]
                                },
                                "TANGENT handedness",
                            )?,
                        ])
                    })
                    .collect::<Result<Vec<_>, GlbError>>()
            })
            .transpose()?;

            // Preserve glTF UV coordinates. Texture row orientation is resolved
            // when image payloads are decoded/repacked, not by mutating UVs.
            let texcoord_0 = decode_optional_vec(
                "TEXCOORD_0",
                2,
                attributes,
                accessors,
                buffer_views,
                buffers,
                bin,
            )?
            .map(|values| {
                values
                    .into_iter()
                    .map(|value| {
                        Ok([
                            to_f32(value[0], "TEXCOORD_0")?,
                            to_f32(value[1], "TEXCOORD_0")?,
                        ])
                    })
                    .collect::<Result<Vec<_>, GlbError>>()
            })
            .transpose()?;
            let texcoord_1 = decode_optional_vec(
                "TEXCOORD_1",
                2,
                attributes,
                accessors,
                buffer_views,
                buffers,
                bin,
            )?
            .map(|values| {
                values
                    .into_iter()
                    .map(|value| {
                        Ok([
                            to_f32(value[0], "TEXCOORD_1")?,
                            to_f32(value[1], "TEXCOORD_1")?,
                        ])
                    })
                    .collect::<Result<Vec<_>, GlbError>>()
            })
            .transpose()?;
            let colors_0 =
                decode_color_accessor(attributes, accessors, buffer_views, buffers, bin)?;

            for (semantic, count) in [
                ("NORMAL", normals.as_ref().map(Vec::len)),
                ("TANGENT", tangents.as_ref().map(Vec::len)),
                ("TEXCOORD_0", texcoord_0.as_ref().map(Vec::len)),
                ("TEXCOORD_1", texcoord_1.as_ref().map(Vec::len)),
                ("COLOR_0", colors_0.as_ref().map(Vec::len)),
            ] {
                if let Some(count) = count {
                    if count != positions.len() {
                        return Err(GlbError::InvalidDocument(format!(
                            "mesh {mesh_index} primitive {primitive_index} {semantic} count {count} does not match POSITION count {}",
                            positions.len()
                        )));
                    }
                }
            }

            for (semantic, accessor) in attributes {
                let semantic = semantic.as_str();
                if !matches!(
                    semantic,
                    "POSITION" | "NORMAL" | "TANGENT" | "TEXCOORD_0" | "TEXCOORD_1" | "COLOR_0"
                ) {
                    report.warnings.push(format!(
                        "node {node_index} mesh {mesh_index} primitive {primitive_index} attribute {semantic} was not imported"
                    ));
                    let _ = accessor;
                }
            }
            if normals.is_none() {
                report.warnings.push(format!(
                    "node {node_index} mesh {mesh_index} primitive {primitive_index} has no NORMAL attribute; a future writer must generate normals or choose a verified shader that does not require them"
                ));
            }

            let material_index = primitive
                .get("material")
                .and_then(Value::as_u64)
                .map(|value| value as usize);
            report.primitives.push(DecodedPrimitive {
                node_index,
                node_name: node.get("name").and_then(Value::as_str).map(str::to_string),
                mesh_index,
                mesh_name: mesh.get("name").and_then(Value::as_str).map(str::to_string),
                primitive_index,
                material_index,
                source_mode: mode,
                vertex_count: positions.len(),
                index_count: indices.len(),
                bounds_min,
                bounds_max,
                positions,
                normals,
                tangents,
                texcoord_0,
                texcoord_1,
                colors_0,
                indices,
                unsupported_attributes: attributes
                    .keys()
                    .filter(|key| {
                        !matches!(
                            key.as_str(),
                            "POSITION"
                                | "NORMAL"
                                | "TANGENT"
                                | "TEXCOORD_0"
                                | "TEXCOORD_1"
                                | "COLOR_0"
                        )
                    })
                    .cloned()
                    .collect(),
            });
        }
    }
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            let child_index = child.as_u64().ok_or_else(|| {
                GlbError::InvalidDocument(format!(
                    "nodes[{node_index}].children contains a non-index"
                ))
            })? as usize;
            visit_node(
                child_index,
                world,
                nodes,
                meshes,
                accessors,
                buffer_views,
                buffers,
                bin,
                recursion,
                visited,
                referenced_meshes,
                report,
            )?;
        }
    }
    recursion.remove(&node_index);
    Ok(())
}

fn decode_optional_vec(
    semantic: &str,
    components: usize,
    attributes: &serde_json::Map<String, Value>,
    accessors: &[Value],
    buffer_views: &[Value],
    buffers: &[Value],
    bin: Option<&[u8]>,
) -> Result<Option<Vec<Vec<f64>>>, GlbError> {
    let Some(value) = attributes.get(semantic) else {
        return Ok(None);
    };
    let index = value
        .as_u64()
        .ok_or_else(|| GlbError::InvalidDocument(format!("{semantic} accessor must be an index")))?
        as usize;
    let values = decode_vec_accessor(
        index,
        accessors,
        buffer_views,
        buffers,
        bin,
        components,
        semantic,
        false,
    )?;
    Ok(Some(values))
}

fn decode_color_accessor(
    attributes: &serde_json::Map<String, Value>,
    accessors: &[Value],
    buffer_views: &[Value],
    buffers: &[Value],
    bin: Option<&[u8]>,
) -> Result<Option<Vec<[f32; 4]>>, GlbError> {
    let Some(value) = attributes.get("COLOR_0") else {
        return Ok(None);
    };
    let index = value
        .as_u64()
        .ok_or_else(|| GlbError::InvalidDocument("COLOR_0 accessor must be an index".into()))?
        as usize;
    let accessor = accessors.get(index).ok_or_else(|| {
        GlbError::InvalidDocument(format!("COLOR_0 accessor {index} is out of range"))
    })?;
    let components = match accessor.get("type").and_then(Value::as_str) {
        Some("VEC3") => 3,
        Some("VEC4") => 4,
        Some(kind) => {
            return Err(GlbError::InvalidDocument(format!(
                "COLOR_0 accessor {index} must be VEC3 or VEC4, got {kind}"
            )))
        }
        None => {
            return Err(GlbError::InvalidDocument(format!(
                "COLOR_0 accessor {index} has no type"
            )))
        }
    };
    let values = decode_vec_accessor(
        index,
        accessors,
        buffer_views,
        buffers,
        bin,
        components,
        "COLOR_0",
        false,
    )?;
    values
        .into_iter()
        .map(|value| {
            Ok([
                to_f32(value[0], "COLOR_0")?,
                to_f32(value[1], "COLOR_0")?,
                to_f32(value[2], "COLOR_0")?,
                if components == 4 {
                    to_f32(value[3], "COLOR_0")?
                } else {
                    1.0
                },
            ])
        })
        .collect::<Result<Vec<_>, GlbError>>()
        .map(Some)
}

#[allow(clippy::too_many_arguments)]
fn decode_vec_accessor(
    index: usize,
    accessors: &[Value],
    views: &[Value],
    buffers: &[Value],
    bin: Option<&[u8]>,
    components: usize,
    semantic: &str,
    require_float: bool,
) -> Result<Vec<Vec<f64>>, GlbError> {
    let accessor = accessors.get(index).ok_or_else(|| {
        GlbError::InvalidDocument(format!("{semantic} accessor {index} is out of range"))
    })?;
    if accessor.get("sparse").is_some() {
        return Err(GlbError::Unsupported(format!(
            "{semantic} accessor {index} uses sparse overrides"
        )));
    }
    let kind = accessor.get("type").and_then(Value::as_str).unwrap_or("");
    let actual_components = match kind {
        "SCALAR" => 1,
        "VEC2" => 2,
        "VEC3" => 3,
        "VEC4" => 4,
        _ => {
            return Err(GlbError::Unsupported(format!(
                "{semantic} accessor {index} has type {kind}"
            )))
        }
    };
    if actual_components != components {
        return Err(GlbError::InvalidDocument(format!(
            "{semantic} accessor {index} requires VEC{components}, got {kind}"
        )));
    }
    let component = accessor
        .get("componentType")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            GlbError::InvalidDocument(format!("{semantic} accessor {index} has no componentType"))
        })? as u32;
    let normalized = accessor
        .get("normalized")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if require_float && component != 5126 {
        return Err(GlbError::Unsupported(format!(
            "{semantic} must use FLOAT components"
        )));
    }
    let legal_component = match semantic {
        "POSITION" => component == 5126,
        "NORMAL" | "TANGENT" => {
            component == 5126 || (normalized && matches!(component, 5120 | 5122))
        }
        "TEXCOORD_0" | "TEXCOORD_1" | "COLOR_0" => {
            component == 5126 || (normalized && matches!(component, 5121 | 5123))
        }
        _ => true,
    };
    if !legal_component {
        return Err(GlbError::Unsupported(format!(
            "{semantic} accessor {index} has componentType {component} with normalized={normalized}, which is not a permitted glTF vertex attribute encoding"
        )));
    }
    let count = accessor
        .get("count")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            GlbError::InvalidDocument(format!("{semantic} accessor {index} has no count"))
        })? as usize;
    if count > MAX_VERTICES_PER_PRIMITIVE {
        return Err(GlbError::Unsupported(format!(
            "{semantic} accessor {index} exceeds the vertex safety limit"
        )));
    }
    let (bytes, stride) = accessor_bytes(
        index, accessor, views, buffers, bin, components, component, true,
    )?;
    let element_size = component_size(component)? * components;
    let mut result = Vec::with_capacity(count);
    for item in 0..count {
        let offset = item.checked_mul(stride).ok_or_else(|| {
            GlbError::InvalidDocument("accessor stride multiplication overflow".into())
        })?;
        let element = bytes.get(offset..offset + element_size).ok_or_else(|| {
            GlbError::InvalidDocument(format!("{semantic} accessor {index} data is truncated"))
        })?;
        let mut vector = Vec::with_capacity(components);
        for part in 0..components {
            let start = part * component_size(component)?;
            vector.push(read_component(
                &element[start..start + component_size(component)?],
                component,
                normalized,
            )?);
        }
        result.push(vector);
    }
    Ok(result)
}

fn decode_indices(
    index: usize,
    accessors: &[Value],
    views: &[Value],
    buffers: &[Value],
    bin: Option<&[u8]>,
) -> Result<Vec<u32>, GlbError> {
    let accessor = accessors.get(index).ok_or_else(|| {
        GlbError::InvalidDocument(format!("indices accessor {index} is out of range"))
    })?;
    if accessor.get("sparse").is_some() {
        return Err(GlbError::Unsupported(format!(
            "indices accessor {index} uses sparse overrides"
        )));
    }
    if accessor.get("type").and_then(Value::as_str) != Some("SCALAR") {
        return Err(GlbError::InvalidDocument(format!(
            "indices accessor {index} must be SCALAR"
        )));
    }
    let component = accessor
        .get("componentType")
        .and_then(Value::as_u64)
        .ok_or_else(|| GlbError::InvalidDocument("indices accessor has no componentType".into()))?
        as u32;
    if !matches!(component, 5121 | 5123 | 5125) {
        return Err(GlbError::Unsupported(format!(
            "indices accessor {index} componentType {component} is not an unsigned integer"
        )));
    }
    if accessor
        .get("normalized")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Err(GlbError::InvalidDocument(
            "indices accessors cannot be normalized".into(),
        ));
    }
    let (bytes, stride) =
        accessor_bytes(index, accessor, views, buffers, bin, 1, component, false)?;
    let count = accessor
        .get("count")
        .and_then(Value::as_u64)
        .ok_or_else(|| GlbError::InvalidDocument("indices accessor has no count".into()))?
        as usize;
    if count > MAX_INDICES_PER_PRIMITIVE {
        return Err(GlbError::Unsupported(
            "index accessor exceeds the safety limit".into(),
        ));
    }
    let size = component_size(component)?;
    let mut indices = Vec::with_capacity(count);
    for item in 0..count {
        let offset = item
            .checked_mul(stride)
            .ok_or_else(|| GlbError::InvalidDocument("index stride overflow".into()))?;
        let value = bytes
            .get(offset..offset + size)
            .ok_or_else(|| GlbError::InvalidDocument("index accessor data is truncated".into()))?;
        indices.push(match component {
            5121 => value[0] as u32,
            5123 => u16::from_le_bytes(value.try_into().unwrap()) as u32,
            5125 => u32::from_le_bytes(value.try_into().unwrap()),
            _ => unreachable!(),
        });
    }
    Ok(indices)
}

#[allow(clippy::too_many_arguments)]
fn accessor_bytes<'a>(
    accessor_index: usize,
    accessor: &Value,
    views: &'a [Value],
    buffers: &[Value],
    bin: Option<&'a [u8]>,
    components: usize,
    component_type: u32,
    require_vertex_alignment: bool,
) -> Result<(&'a [u8], usize), GlbError> {
    let view_index = accessor
        .get("bufferView")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            GlbError::Unsupported(format!("accessor {accessor_index} has no base bufferView"))
        })? as usize;
    let view = views.get(view_index).ok_or_else(|| {
        GlbError::InvalidDocument(format!(
            "accessor {accessor_index} bufferView is out of range"
        ))
    })?;
    if view
        .get("extensions")
        .and_then(Value::as_object)
        .is_some_and(|extensions| extensions.contains_key("EXT_meshopt_compression"))
    {
        return Err(GlbError::Unsupported(format!(
            "accessor {accessor_index} uses EXT_meshopt_compression"
        )));
    }
    let buffer_index = view
        .get("buffer")
        .and_then(Value::as_u64)
        .ok_or_else(|| GlbError::InvalidDocument("bufferView has no buffer index".into()))?
        as usize;
    let buffer = buffers.get(buffer_index).ok_or_else(|| {
        GlbError::InvalidDocument("bufferView references a missing buffer".into())
    })?;
    if buffer_index != 0 || buffer.get("uri").is_some() {
        return Err(GlbError::Unsupported(format!(
            "accessor {accessor_index} references external buffer {buffer_index}; only embedded GLB BIN data is supported"
        )));
    }
    let data = bin.ok_or_else(|| GlbError::InvalidDocument("GLB BIN chunk is missing".into()))?;
    let view_offset = view.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let view_length = view
        .get("byteLength")
        .and_then(Value::as_u64)
        .ok_or_else(|| GlbError::InvalidDocument("bufferView has no byteLength".into()))?
        as usize;
    let accessor_offset = accessor
        .get("byteOffset")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let component_bytes = component_size(component_type)?;
    let start = view_offset
        .checked_add(accessor_offset)
        .ok_or_else(|| GlbError::InvalidDocument("accessor offset overflow".into()))?;
    if start % component_bytes != 0 {
        return Err(GlbError::InvalidDocument(format!(
            "accessor {accessor_index} absolute offset is not aligned to its component size"
        )));
    }
    if require_vertex_alignment && start % 4 != 0 {
        return Err(GlbError::InvalidDocument(format!(
            "vertex accessor {accessor_index} is not 4-byte aligned"
        )));
    }
    let element_size = component_bytes * components;
    let stride = view
        .get("byteStride")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(element_size);
    if stride < element_size || stride % component_bytes != 0 {
        return Err(GlbError::InvalidDocument(format!(
            "accessor {accessor_index} has an invalid byteStride {stride}"
        )));
    }
    let count = accessor
        .get("count")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            GlbError::InvalidDocument(format!("accessor {accessor_index} has no count"))
        })? as usize;
    let needed = if count == 0 {
        0
    } else {
        (count - 1)
            .checked_mul(stride)
            .and_then(|value| value.checked_add(element_size))
            .and_then(|value| value.checked_add(accessor_offset))
            .ok_or_else(|| {
                GlbError::InvalidDocument(format!("accessor {accessor_index} range overflows"))
            })?
    };
    if needed > view_length {
        return Err(GlbError::InvalidDocument(format!(
            "accessor {accessor_index} exceeds its bufferView"
        )));
    }
    let end = start
        .checked_add(needed.saturating_sub(accessor_offset))
        .ok_or_else(|| GlbError::InvalidDocument("accessor end overflow".into()))?;
    let slice = data.get(start..end).ok_or_else(|| {
        GlbError::InvalidDocument(format!("accessor {accessor_index} exceeds the BIN chunk"))
    })?;
    Ok((slice, stride))
}

fn component_size(component: u32) -> Result<usize, GlbError> {
    match component {
        5120 | 5121 => Ok(1),
        5122 | 5123 => Ok(2),
        5125 | 5126 => Ok(4),
        _ => Err(GlbError::Unsupported(format!(
            "accessor componentType {component} is not supported"
        ))),
    }
}

fn read_component(bytes: &[u8], component: u32, normalized: bool) -> Result<f64, GlbError> {
    let value = match component {
        5120 => {
            let raw = bytes[0] as i8;
            if normalized {
                (raw as f64 / 127.0).max(-1.0)
            } else {
                raw as f64
            }
        }
        5121 => {
            let raw = bytes[0];
            if normalized {
                raw as f64 / 255.0
            } else {
                raw as f64
            }
        }
        5122 => {
            let raw = i16::from_le_bytes(bytes.try_into().unwrap());
            if normalized {
                (raw as f64 / 32767.0).max(-1.0)
            } else {
                raw as f64
            }
        }
        5123 => {
            let raw = u16::from_le_bytes(bytes.try_into().unwrap());
            if normalized {
                raw as f64 / 65535.0
            } else {
                raw as f64
            }
        }
        5125 => {
            let raw = u32::from_le_bytes(bytes.try_into().unwrap());
            if normalized {
                raw as f64 / u32::MAX as f64
            } else {
                raw as f64
            }
        }
        5126 => f32::from_le_bytes(bytes.try_into().unwrap()) as f64,
        _ => {
            return Err(GlbError::Unsupported(format!(
                "componentType {component} is not supported"
            )))
        }
    };
    if !value.is_finite() {
        return Err(GlbError::InvalidDocument(
            "accessor contains a non-finite value".into(),
        ));
    }
    Ok(value)
}

pub(crate) fn triangulate(
    mode: u32,
    source: &[u32],
    mesh: usize,
    primitive: usize,
) -> Result<Vec<u32>, GlbError> {
    let output_count = match mode {
        4 => source.len(),
        5 | 6 if source.len() >= 3 => (source.len() - 2)
            .checked_mul(3)
            .ok_or_else(|| GlbError::InvalidDocument("triangle index count overflow".into()))?,
        5 | 6 => {
            return Err(GlbError::InvalidDocument(format!(
                "mesh {mesh} primitive {primitive} triangle topology has fewer than 3 indices"
            )))
        }
        _ => {
            return Err(GlbError::Unsupported(format!(
                "primitive mode {mode} is not a triangle topology"
            )))
        }
    };
    if output_count > MAX_INDICES_PER_PRIMITIVE {
        return Err(GlbError::Unsupported(format!(
            "mesh {mesh} primitive {primitive} expands beyond the index safety limit"
        )));
    }
    let mut indices = Vec::with_capacity(output_count);
    match mode {
        4 => {
            if source.len() % 3 != 0 {
                return Err(GlbError::InvalidDocument(format!(
                    "mesh {mesh} primitive {primitive} triangle index count is not divisible by 3"
                )));
            }
            indices.extend_from_slice(source);
        }
        5 => {
            if source.len() < 3 {
                return Err(GlbError::InvalidDocument(format!(
                    "mesh {mesh} primitive {primitive} triangle strip has fewer than 3 indices"
                )));
            }
            for index in 0..source.len() - 2 {
                if index % 2 == 0 {
                    indices.extend_from_slice(&[
                        source[index],
                        source[index + 1],
                        source[index + 2],
                    ]);
                } else {
                    indices.extend_from_slice(&[
                        source[index + 1],
                        source[index],
                        source[index + 2],
                    ]);
                }
            }
        }
        6 => {
            if source.len() < 3 {
                return Err(GlbError::InvalidDocument(format!(
                    "mesh {mesh} primitive {primitive} triangle fan has fewer than 3 indices"
                )));
            }
            for index in 1..source.len() - 1 {
                indices.extend_from_slice(&[source[0], source[index], source[index + 1]]);
            }
        }
        _ => {
            return Err(GlbError::Unsupported(format!(
                "primitive mode {mode} is not a triangle topology"
            )))
        }
    }
    Ok(indices)
}

fn to_gta_z_up(value: [f64; 3]) -> [f64; 3] {
    if GLTF_TO_GTA_Z_UP {
        [value[0], -value[2], value[1]]
    } else {
        value
    }
}

fn calculate_bounds(points: &[[f32; 3]]) -> Result<([f32; 3], [f32; 3]), GlbError> {
    let Some(first) = points.first().copied() else {
        return Err(GlbError::InvalidDocument(
            "POSITION accessor contains no vertices".into(),
        ));
    };
    let mut min = first;
    let mut max = first;
    for point in &points[1..] {
        for axis in 0..3 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }
    Ok((min, max))
}

fn as_f32(value: [f64; 3], semantic: &str) -> Result<[f32; 3], GlbError> {
    Ok([
        to_f32(value[0], semantic)?,
        to_f32(value[1], semantic)?,
        to_f32(value[2], semantic)?,
    ])
}

fn to_f32(value: f64, semantic: &str) -> Result<f32, GlbError> {
    let converted = value as f32;
    if !converted.is_finite() {
        return Err(GlbError::Unsupported(format!(
            "{semantic} value {value} is outside the finite f32 range required by the YDR pipeline"
        )));
    }
    Ok(converted)
}

fn normalize3(value: [f64; 3]) -> Result<[f64; 3], GlbError> {
    let length = value.iter().map(|v| v * v).sum::<f64>().sqrt();
    if !length.is_finite() || length < 1.0e-12 {
        return Err(GlbError::InvalidDocument(
            "normal or tangent has zero or non-finite length".into(),
        ));
    }
    Ok([value[0] / length, value[1] / length, value[2] / length])
}

fn read_float_array(value: &Value, expected: usize, label: &str) -> Result<Vec<f64>, GlbError> {
    let values = value
        .as_array()
        .ok_or_else(|| GlbError::InvalidDocument(format!("{label} must be an array")))?;
    if values.len() != expected {
        return Err(GlbError::InvalidDocument(format!(
            "{label} must contain {expected} numbers"
        )));
    }
    values
        .iter()
        .map(|value| {
            value
                .as_f64()
                .filter(|number| number.is_finite())
                .ok_or_else(|| {
                    GlbError::InvalidDocument(format!(
                        "{label} contains a non-finite or non-numeric value"
                    ))
                })
        })
        .collect()
}

fn optional_float_array(
    value: Option<&Value>,
    expected: usize,
    label: &str,
) -> Result<Option<Vec<f64>>, GlbError> {
    value
        .map(|value| read_float_array(value, expected, label))
        .transpose()
}

fn array<'a>(root: &'a Value, key: &str) -> &'a [Value] {
    root.get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_oversized_vertex_accessor_before_allocating_decoded_values() {
        let accessors = vec![json!({
            "bufferView": 0,
            "componentType": 5126,
            "count": 2_000_001,
            "type": "VEC3"
        })];
        let views = vec![json!({"buffer": 0, "byteLength": 0})];
        let buffers = vec![json!({"byteLength": 0})];

        let error = decode_vec_accessor(
            0,
            &accessors,
            &views,
            &buffers,
            Some(&[]),
            3,
            "POSITION",
            true,
        )
        .unwrap_err();

        assert!(matches!(error, GlbError::Unsupported(_)));
        assert!(error.to_string().contains("vertex safety limit"));
    }

    #[test]
    fn rejects_unaligned_vertex_accessor_offset() {
        let accessor = json!({
            "bufferView": 0,
            "byteOffset": 2,
            "count": 1,
            "type": "VEC3"
        });
        let views = vec![json!({"buffer": 0, "byteLength": 16})];
        let buffers = vec![json!({"byteLength": 16})];
        let binary = [0u8; 16];

        let error = accessor_bytes(0, &accessor, &views, &buffers, Some(&binary), 3, 5126, true)
            .unwrap_err();

        assert!(matches!(error, GlbError::InvalidDocument(_)));
        assert!(error.to_string().contains("not aligned"));
    }

    #[test]
    fn permits_byte_aligned_u8_index_accessor_without_vertex_alignment() {
        let accessor = json!({
            "bufferView": 0,
            "byteOffset": 1,
            "count": 1,
            "type": "SCALAR"
        });
        let views = vec![json!({"buffer": 0, "byteLength": 4})];
        let buffers = vec![json!({"byteLength": 4})];
        let binary = [0u8, 7, 8, 9];

        let (bytes, stride) = accessor_bytes(
            0,
            &accessor,
            &views,
            &buffers,
            Some(&binary),
            1,
            5121,
            false,
        )
        .unwrap();

        assert_eq!(stride, 1);
        assert_eq!(bytes, &[7]);
    }

    #[test]
    fn rejects_strip_expansion_over_index_limit_before_allocating_output() {
        let source = vec![0u32; 2_000_003];
        let error = triangulate(5, &source, 0, 0).unwrap_err();

        assert!(matches!(error, GlbError::Unsupported(_)));
        assert!(error.to_string().contains("index safety limit"));
    }
}

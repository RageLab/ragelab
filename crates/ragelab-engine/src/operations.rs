use std::{
    error::Error,
    fmt,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use ragelab_ybn::{
    repack_polygon_edits, CollisionShape, YbnCollision, YbnPolygonEdit, YbnPolygonKind,
};
use ragelab_ydd::{YddDictionary, YddEditSession};
use ragelab_ydr::{ShaderBindingKey, TextureBindingKey, YdrDocument, YdrEditSession};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

pub const OPERATION_DOCUMENT_SCHEMA: &str = "ragelab.operation";
pub const OPERATION_DOCUMENT_SCHEMA_VERSION: u64 = 1;
pub const OPERATION_PLAN_SCHEMA: &str = "ragelab.operation.plan";
pub const OPERATION_APPLY_SCHEMA: &str = "ragelab.operation.apply";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationDocument {
    pub schema: String,
    pub schema_version: u64,
    pub source: PathBuf,
    pub output: PathBuf,
    pub operations: Vec<OperationSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationSpec {
    #[serde(rename = "type")]
    pub operation_type: String,
    #[serde(flatten)]
    pub parameters: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationPlan {
    pub schema: &'static str,
    pub schema_version: u64,
    pub source: PathBuf,
    pub output: PathBuf,
    pub asset_type: String,
    pub allowed: bool,
    pub non_destructive: bool,
    pub output_exists: bool,
    pub source_bytes: usize,
    pub operations: Vec<PlannedOperation>,
    pub reasons: Vec<String>,
    pub writes: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedOperation {
    pub index: usize,
    #[serde(rename = "type")]
    pub operation_type: String,
    pub allowed: bool,
    pub reason: Option<String>,
    pub details: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationApplyResult {
    pub schema: &'static str,
    pub schema_version: u64,
    pub source: PathBuf,
    pub output: PathBuf,
    pub asset_type: String,
    pub operations_applied: usize,
    pub bytes_written: usize,
    pub non_destructive: bool,
    pub validation: ApplyValidation,
    pub details: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyValidation {
    pub semantic_reopen: bool,
    pub source_unchanged: bool,
}

#[derive(Debug)]
pub enum OperationError {
    InvalidDocument(String),
    InvalidSource(String),
    Rejected(String),
    Writer(String),
    Io(io::Error),
}

impl OperationError {
    pub fn error_kind(&self) -> io::ErrorKind {
        match self {
            Self::InvalidDocument(_) => io::ErrorKind::InvalidInput,
            Self::InvalidSource(_) => io::ErrorKind::InvalidData,
            Self::Rejected(_) => io::ErrorKind::Unsupported,
            Self::Writer(_) => io::ErrorKind::Other,
            Self::Io(error) => error.kind(),
        }
    }
}

impl fmt::Display for OperationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDocument(message) => {
                write!(formatter, "invalid operation document: {message}")
            }
            Self::InvalidSource(message) => write!(formatter, "invalid source asset: {message}"),
            Self::Rejected(message) => write!(formatter, "operation rejected: {message}"),
            Self::Writer(message) => write!(formatter, "operation writer failed: {message}"),
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl Error for OperationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for OperationError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub fn parse_operation_document(input: &str) -> Result<OperationDocument, OperationError> {
    let document = serde_json::from_str::<OperationDocument>(input)
        .map_err(|error| OperationError::InvalidDocument(error.to_string()))?;

    if document.schema != OPERATION_DOCUMENT_SCHEMA {
        return Err(OperationError::InvalidDocument(format!(
            "unsupported schema {:?}; expected {:?}",
            document.schema, OPERATION_DOCUMENT_SCHEMA
        )));
    }
    if document.schema_version != OPERATION_DOCUMENT_SCHEMA_VERSION {
        return Err(OperationError::InvalidDocument(format!(
            "unsupported schemaVersion {}; expected {}",
            document.schema_version, OPERATION_DOCUMENT_SCHEMA_VERSION
        )));
    }
    if document.source.as_os_str().is_empty() {
        return Err(OperationError::InvalidDocument(
            "source must not be empty".into(),
        ));
    }
    if document.output.as_os_str().is_empty() {
        return Err(OperationError::InvalidDocument(
            "output must not be empty".into(),
        ));
    }
    if document.operations.is_empty() {
        return Err(OperationError::InvalidDocument(
            "operations must contain at least one operation".into(),
        ));
    }

    Ok(document)
}

pub fn plan_operation_document(
    document: &OperationDocument,
    base_dir: &Path,
) -> Result<OperationPlan, OperationError> {
    let source = resolve_path(base_dir, &document.source);
    let output = resolve_path(base_dir, &document.output);
    let source_bytes = fs::read(&source)?;
    let asset_type = asset_type(&source).to_string();
    let output_exists = output.exists();
    let mut reasons = Vec::new();

    if output_exists {
        reasons.push(format!(
            "output already exists; apply never overwrites existing files: {}",
            output.display()
        ));
    }

    let ybn_collision = if asset_type == "YBN" {
        Some(
            YbnCollision::from_bytes(&source_bytes)
                .map_err(|error| OperationError::InvalidSource(error.to_string()))?,
        )
    } else {
        None
    };

    let operations = if asset_type == "YDR" {
        plan_ydr_operations(&source_bytes, &document.operations, &mut reasons)?
    } else if asset_type == "YDD" {
        plan_ydd_operations(&source_bytes, &document.operations, &mut reasons)?
    } else {
        let mut operations = Vec::with_capacity(document.operations.len());
        for (index, operation) in document.operations.iter().enumerate() {
            let planned = match operation.operation_type.as_str() {
                "ydr.translate" | "ydr.rebind-texture" | "ydr.rebind-shader" | "ydd.translate"
                | "ydd.rebind-texture" | "ydd.rebind-shader" => PlannedOperation {
                    index,
                    operation_type: operation.operation_type.clone(),
                    allowed: false,
                    reason: Some(format!(
                        "{} requires a {} source, found {asset_type}",
                        operation.operation_type,
                        if operation.operation_type.starts_with("ydd.") {
                            "YDD"
                        } else {
                            "YDR"
                        }
                    )),
                    details: Value::Null,
                },
                "ybn.edit-polygon" => {
                    plan_ybn_edit_polygon(index, operation, &asset_type, ybn_collision.as_ref())
                }
                other => PlannedOperation {
                    index,
                    operation_type: other.to_string(),
                    allowed: false,
                    reason: Some(format!("unsupported operation type: {other}")),
                    details: Value::Null,
                },
            };
            operations.push(planned);
        }
        operations
    };

    if asset_type == "YBN" && operations.iter().all(|operation| operation.allowed) {
        let collision = ybn_collision
            .as_ref()
            .expect("YBN collision should be available after successful parse");
        let edits = document
            .operations
            .iter()
            .map(|operation| parse_ybn_polygon_edit(operation, collision))
            .collect::<Result<Vec<_>, _>>();
        match edits {
            Ok(edits) => {
                if let Err(error) = repack_polygon_edits(&source_bytes, &edits) {
                    reasons.push(format!("YBN writer rejected planned edits: {error}"));
                }
            }
            Err(reason) => reasons.push(reason),
        }
    }

    let allowed = reasons.is_empty() && operations.iter().all(|operation| operation.allowed);

    Ok(OperationPlan {
        schema: OPERATION_PLAN_SCHEMA,
        schema_version: OPERATION_DOCUMENT_SCHEMA_VERSION,
        source,
        output: output.clone(),
        asset_type,
        allowed,
        non_destructive: true,
        output_exists,
        source_bytes: source_bytes.len(),
        operations,
        reasons,
        writes: vec![output],
    })
}

pub fn apply_operation_document(
    document: &OperationDocument,
    base_dir: &Path,
) -> Result<OperationApplyResult, OperationError> {
    let plan = plan_operation_document(document, base_dir)?;
    if !plan.allowed {
        let mut reasons = plan.reasons.clone();
        reasons.extend(
            plan.operations
                .iter()
                .filter_map(|operation| operation.reason.clone()),
        );
        return Err(OperationError::Rejected(reasons.join("; ")));
    }

    match plan.asset_type.as_str() {
        "YDR" => apply_ydr_operations(document, &plan),
        "YDD" => apply_ydd_operations(document, &plan),
        "YBN" => apply_ybn_operations(document, &plan),
        other => Err(OperationError::Rejected(format!(
            "no declarative writer is available for asset type {other}"
        ))),
    }
}

fn plan_ydr_operations(
    source_bytes: &[u8],
    specs: &[OperationSpec],
    reasons: &mut Vec<String>,
) -> Result<Vec<PlannedOperation>, OperationError> {
    let mut session = YdrEditSession::from_bytes(source_bytes)
        .map_err(|error| OperationError::InvalidSource(error.to_string()))?;
    let mut planned = Vec::with_capacity(specs.len());

    for (index, operation) in specs.iter().enumerate() {
        let result = match operation.operation_type.as_str() {
            "ydr.translate" => parse_translation_delta(operation).and_then(|delta| {
                session
                    .translate_rigid_model(delta)
                    .map_err(|error| error.to_string())?;
                Ok(json!({ "delta": delta }))
            }),
            "ydr.rebind-texture" => plan_ydr_texture_rebind(&mut session, operation),
            "ydr.rebind-shader" => plan_ydr_shader_rebind(&mut session, operation),
            other => Err(format!("unsupported operation type: {other}")),
        };

        match result {
            Ok(details) => planned.push(PlannedOperation {
                index,
                operation_type: operation.operation_type.clone(),
                allowed: true,
                reason: None,
                details,
            }),
            Err(reason) => planned.push(PlannedOperation {
                index,
                operation_type: operation.operation_type.clone(),
                allowed: false,
                reason: Some(reason),
                details: Value::Null,
            }),
        }
    }

    if planned.iter().all(|operation| operation.allowed) {
        if let Err(error) = session.to_bytes() {
            reasons.push(format!("YDR writer rejected planned edits: {error}"));
        }
    }

    Ok(planned)
}

fn plan_ydr_texture_rebind(
    session: &mut YdrEditSession,
    operation: &OperationSpec,
) -> Result<Value, String> {
    let (source, target) = parse_ydr_texture_rebind(operation)?;
    let target_binding = session
        .texture_bindings()
        .iter()
        .find(|binding| binding.key == target)
        .cloned()
        .ok_or_else(|| {
            format!(
                "target texture binding shader {} parameter {} was not found",
                target.shader_index, target.parameter_index
            )
        })?;

    session
        .rebind_texture(source, target)
        .map_err(|error| error.to_string())?;

    Ok(json!({
        "source": {
            "shaderIndex": source.shader_index,
            "parameterIndex": source.parameter_index,
        },
        "target": {
            "shaderIndex": target.shader_index,
            "parameterIndex": target.parameter_index,
        },
        "targetParameterHash": format!("0x{:08X}", target_binding.parameter_hash),
        "targetTexture": target_binding.texture_name,
    }))
}

fn plan_ydr_shader_rebind(
    session: &mut YdrEditSession,
    operation: &OperationSpec,
) -> Result<Value, String> {
    let (binding, target_shader_index) = parse_ydr_shader_rebind(operation)?;
    let current = session
        .shader_bindings()
        .iter()
        .find(|candidate| candidate.key == binding)
        .copied()
        .ok_or_else(|| {
            format!(
                "shader binding model {} geometry {} was not found",
                binding.model_index, binding.geometry_index
            )
        })?;
    if current.shader_index == target_shader_index {
        return Err("source geometry already uses target shader".into());
    }

    session
        .rebind_shader(binding, target_shader_index)
        .map_err(|error| error.to_string())?;

    Ok(json!({
        "binding": {
            "modelIndex": binding.model_index,
            "geometryIndex": binding.geometry_index,
        },
        "sourceShaderIndex": current.shader_index,
        "targetShaderIndex": target_shader_index,
    }))
}

fn parse_ydr_texture_rebind(
    operation: &OperationSpec,
) -> Result<(TextureBindingKey, TextureBindingKey), String> {
    const ALLOWED: &[&str] = &[
        "sourceShader",
        "sourceParameter",
        "targetShader",
        "targetParameter",
    ];
    reject_unknown_parameters(operation, ALLOWED)?;

    let source = TextureBindingKey {
        shader_index: required_usize(&operation.parameters, "sourceShader", "ydr.rebind-texture")?,
        parameter_index: required_usize(
            &operation.parameters,
            "sourceParameter",
            "ydr.rebind-texture",
        )?,
    };
    let target = TextureBindingKey {
        shader_index: required_usize(&operation.parameters, "targetShader", "ydr.rebind-texture")?,
        parameter_index: required_usize(
            &operation.parameters,
            "targetParameter",
            "ydr.rebind-texture",
        )?,
    };

    if source == target {
        return Err("ydr.rebind-texture source and target bindings must differ".into());
    }

    Ok((source, target))
}

fn parse_ydr_shader_rebind(operation: &OperationSpec) -> Result<(ShaderBindingKey, u16), String> {
    const ALLOWED: &[&str] = &["modelIndex", "geometryIndex", "targetShaderIndex"];
    reject_unknown_parameters(operation, ALLOWED)?;

    let binding = ShaderBindingKey {
        model_index: required_usize(&operation.parameters, "modelIndex", "ydr.rebind-shader")?,
        geometry_index: required_usize(
            &operation.parameters,
            "geometryIndex",
            "ydr.rebind-shader",
        )?,
    };
    let target = required_usize(
        &operation.parameters,
        "targetShaderIndex",
        "ydr.rebind-shader",
    )?;
    let target_shader_index = u16::try_from(target)
        .map_err(|_| "ydr.rebind-shader targetShaderIndex must fit in u16".to_string())?;

    Ok((binding, target_shader_index))
}

fn reject_unknown_parameters(operation: &OperationSpec, allowed: &[&str]) -> Result<(), String> {
    for key in operation.parameters.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!(
                "{} has unknown parameter: {key}",
                operation.operation_type
            ));
        }
    }
    Ok(())
}

fn plan_ydd_operations(
    source_bytes: &[u8],
    specs: &[OperationSpec],
    reasons: &mut Vec<String>,
) -> Result<Vec<PlannedOperation>, OperationError> {
    let original = YddDictionary::from_bytes(source_bytes)
        .map_err(|error| OperationError::InvalidSource(error.to_string()))?;
    let original_entries = original.entries().to_vec();
    let mut working_bytes = source_bytes.to_vec();
    let mut planned = Vec::with_capacity(specs.len());

    for (index, operation) in specs.iter().enumerate() {
        match apply_ydd_operation_to_bytes(&working_bytes, operation) {
            Ok((rewritten, details)) => {
                working_bytes = rewritten;
                planned.push(PlannedOperation {
                    index,
                    operation_type: operation.operation_type.clone(),
                    allowed: true,
                    reason: None,
                    details,
                });
            }
            Err(reason) => planned.push(PlannedOperation {
                index,
                operation_type: operation.operation_type.clone(),
                allowed: false,
                reason: Some(reason),
                details: Value::Null,
            }),
        }
    }

    if planned.iter().all(|operation| operation.allowed) {
        match YddDictionary::from_bytes(&working_bytes) {
            Ok(reopened) if reopened.entries() == original_entries.as_slice() => {}
            Ok(_) => reasons.push(
                "YDD writer changed dictionary identity metadata during planned edits".into(),
            ),
            Err(error) => reasons.push(format!(
                "YDD writer failed semantic re-open after planned edits: {error}"
            )),
        }
    }

    Ok(planned)
}

fn apply_ydd_operation_to_bytes(
    bytes: &[u8],
    operation: &OperationSpec,
) -> Result<(Vec<u8>, Value), String> {
    match operation.operation_type.as_str() {
        "ydd.translate" => {
            let (drawable_index, delta) = parse_ydd_translation(operation)?;
            let mut session = YddEditSession::from_bytes(bytes, drawable_index)
                .map_err(|error| error.to_string())?;
            let entry = session.entry().clone();
            session
                .translate_rigid_model(delta)
                .map_err(|error| error.to_string())?;
            let rewritten = session.to_bytes().map_err(|error| error.to_string())?;
            Ok((
                rewritten,
                json!({
                    "drawableIndex": drawable_index,
                    "drawableHash": format!("0x{:08X}", entry.name_hash),
                    "drawableName": entry.name,
                    "delta": delta,
                }),
            ))
        }
        "ydd.rebind-texture" => {
            let (drawable_index, source, target) = parse_ydd_texture_rebind(operation)?;
            let mut session = YddEditSession::from_bytes(bytes, drawable_index)
                .map_err(|error| error.to_string())?;
            let entry = session.entry().clone();
            let target_binding = session
                .texture_bindings()
                .iter()
                .find(|binding| binding.key == target)
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "target texture binding shader {} parameter {} was not found",
                        target.shader_index, target.parameter_index
                    )
                })?;
            session
                .rebind_texture(source, target)
                .map_err(|error| error.to_string())?;
            let rewritten = session.to_bytes().map_err(|error| error.to_string())?;
            Ok((
                rewritten,
                json!({
                    "drawableIndex": drawable_index,
                    "drawableHash": format!("0x{:08X}", entry.name_hash),
                    "drawableName": entry.name,
                    "source": {
                        "shaderIndex": source.shader_index,
                        "parameterIndex": source.parameter_index,
                    },
                    "target": {
                        "shaderIndex": target.shader_index,
                        "parameterIndex": target.parameter_index,
                    },
                    "targetParameterHash": format!("0x{:08X}", target_binding.parameter_hash),
                    "targetTexture": target_binding.texture_name,
                }),
            ))
        }
        "ydd.rebind-shader" => {
            let (drawable_index, binding, target_shader_index) =
                parse_ydd_shader_rebind(operation)?;
            let mut session = YddEditSession::from_bytes(bytes, drawable_index)
                .map_err(|error| error.to_string())?;
            let entry = session.entry().clone();
            let current = session
                .shader_bindings()
                .iter()
                .find(|candidate| candidate.key == binding)
                .copied()
                .ok_or_else(|| {
                    format!(
                        "shader binding model {} geometry {} was not found",
                        binding.model_index, binding.geometry_index
                    )
                })?;
            if current.shader_index == target_shader_index {
                return Err("source geometry already uses target shader".into());
            }
            session
                .rebind_shader(binding, target_shader_index)
                .map_err(|error| error.to_string())?;
            let rewritten = session.to_bytes().map_err(|error| error.to_string())?;
            Ok((
                rewritten,
                json!({
                    "drawableIndex": drawable_index,
                    "drawableHash": format!("0x{:08X}", entry.name_hash),
                    "drawableName": entry.name,
                    "binding": {
                        "modelIndex": binding.model_index,
                        "geometryIndex": binding.geometry_index,
                    },
                    "sourceShaderIndex": current.shader_index,
                    "targetShaderIndex": target_shader_index,
                }),
            ))
        }
        other => Err(format!("unsupported operation type: {other}")),
    }
}

fn parse_ydd_translation(operation: &OperationSpec) -> Result<(usize, [f32; 3]), String> {
    const ALLOWED: &[&str] = &["drawableIndex", "delta"];
    reject_unknown_parameters(operation, ALLOWED)?;
    let drawable_index = required_usize(&operation.parameters, "drawableIndex", "ydd.translate")?;
    let delta = parse_delta_parameter(&operation.parameters, "ydd.translate")?;
    Ok((drawable_index, delta))
}

fn parse_ydd_texture_rebind(
    operation: &OperationSpec,
) -> Result<(usize, TextureBindingKey, TextureBindingKey), String> {
    const ALLOWED: &[&str] = &[
        "drawableIndex",
        "sourceShader",
        "sourceParameter",
        "targetShader",
        "targetParameter",
    ];
    reject_unknown_parameters(operation, ALLOWED)?;

    let drawable_index =
        required_usize(&operation.parameters, "drawableIndex", "ydd.rebind-texture")?;
    let source = TextureBindingKey {
        shader_index: required_usize(&operation.parameters, "sourceShader", "ydd.rebind-texture")?,
        parameter_index: required_usize(
            &operation.parameters,
            "sourceParameter",
            "ydd.rebind-texture",
        )?,
    };
    let target = TextureBindingKey {
        shader_index: required_usize(&operation.parameters, "targetShader", "ydd.rebind-texture")?,
        parameter_index: required_usize(
            &operation.parameters,
            "targetParameter",
            "ydd.rebind-texture",
        )?,
    };
    if source == target {
        return Err("ydd.rebind-texture source and target bindings must differ".into());
    }
    Ok((drawable_index, source, target))
}

fn parse_ydd_shader_rebind(
    operation: &OperationSpec,
) -> Result<(usize, ShaderBindingKey, u16), String> {
    const ALLOWED: &[&str] = &[
        "drawableIndex",
        "modelIndex",
        "geometryIndex",
        "targetShaderIndex",
    ];
    reject_unknown_parameters(operation, ALLOWED)?;

    let drawable_index =
        required_usize(&operation.parameters, "drawableIndex", "ydd.rebind-shader")?;
    let binding = ShaderBindingKey {
        model_index: required_usize(&operation.parameters, "modelIndex", "ydd.rebind-shader")?,
        geometry_index: required_usize(
            &operation.parameters,
            "geometryIndex",
            "ydd.rebind-shader",
        )?,
    };
    let target = required_usize(
        &operation.parameters,
        "targetShaderIndex",
        "ydd.rebind-shader",
    )?;
    let target_shader_index = u16::try_from(target)
        .map_err(|_| "ydd.rebind-shader targetShaderIndex must fit in u16".to_string())?;

    Ok((drawable_index, binding, target_shader_index))
}

fn plan_ybn_edit_polygon(
    index: usize,
    operation: &OperationSpec,
    asset_type: &str,
    collision: Option<&YbnCollision>,
) -> PlannedOperation {
    let mut reason = None;
    let mut details = Value::Null;

    if asset_type != "YBN" {
        reason = Some(format!(
            "ybn.edit-polygon requires a YBN source, found {asset_type}"
        ));
    } else if let Some(collision) = collision {
        match parse_ybn_polygon_edit(operation, collision) {
            Ok(_) => details = Value::Object(operation.parameters.clone()),
            Err(message) => reason = Some(message),
        }
    } else {
        reason = Some("YBN source could not be parsed".into());
    }

    PlannedOperation {
        index,
        operation_type: operation.operation_type.clone(),
        allowed: reason.is_none(),
        reason,
        details,
    }
}

fn parse_ybn_polygon_edit(
    operation: &OperationSpec,
    collision: &YbnCollision,
) -> Result<YbnPolygonEdit, String> {
    const ALLOWED: &[&str] = &[
        "childIndex",
        "polygonIndex",
        "kind",
        "radius",
        "materialIndex",
    ];

    for key in operation.parameters.keys() {
        if !ALLOWED.contains(&key.as_str()) {
            return Err(format!("ybn.edit-polygon has unknown parameter: {key}"));
        }
    }

    let child_index = required_usize(&operation.parameters, "childIndex", "ybn.edit-polygon")?;
    let polygon_index = required_usize(&operation.parameters, "polygonIndex", "ybn.edit-polygon")?;
    let kind = operation
        .parameters
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| "ybn.edit-polygon kind must be a string".to_string())?;
    let expected_kind = parse_ybn_polygon_kind(kind)?;

    let primitive = collision
        .shape_primitives
        .iter()
        .find(|primitive| {
            primitive.child_index == child_index && primitive.polygon_index == Some(polygon_index)
        })
        .ok_or_else(|| {
            format!("child {child_index} polygon {polygon_index} is not an editable shape polygon")
        })?;
    let actual_kind = collision_shape_kind(&primitive.shape);
    if actual_kind != expected_kind {
        return Err(format!(
            "child {child_index} polygon {polygon_index} is {}, not {}",
            ybn_kind_name(actual_kind),
            ybn_kind_name(expected_kind)
        ));
    }

    let radius = match operation.parameters.get("radius") {
        Some(value) => {
            let radius = value.as_f64().ok_or_else(|| {
                "ybn.edit-polygon radius must be a finite number greater than zero".to_string()
            })?;
            if !radius.is_finite()
                || radius <= 0.0
                || radius < f32::MIN as f64
                || radius > f32::MAX as f64
            {
                return Err(
                    "ybn.edit-polygon radius must be a finite f32 greater than zero".into(),
                );
            }
            if expected_kind == YbnPolygonKind::Box {
                return Err("ybn.edit-polygon box polygons do not expose radius edits".into());
            }
            Some(radius as f32)
        }
        None => None,
    };

    let local_material = match operation.parameters.get("materialIndex") {
        Some(value) => {
            let material_index = json_usize(value, "materialIndex", "ybn.edit-polygon")?;
            let material = collision
                .materials
                .iter()
                .find(|material| material.index == material_index)
                .ok_or_else(|| {
                    format!(
                        "ybn.edit-polygon materialIndex {material_index} does not exist in the source"
                    )
                })?;
            if material.child_index != child_index {
                return Err(format!(
                    "ybn.edit-polygon materialIndex {material_index} belongs to child {}, not child {child_index}",
                    material.child_index
                ));
            }
            Some(material.local_index)
        }
        None => None,
    };

    if radius.is_none() && local_material.is_none() {
        return Err(
            "ybn.edit-polygon requires at least one mutable field: radius or materialIndex".into(),
        );
    }

    Ok(YbnPolygonEdit {
        child_index,
        polygon_index,
        expected_kind,
        radius,
        local_material,
    })
}

fn required_usize(
    parameters: &Map<String, Value>,
    key: &str,
    operation: &str,
) -> Result<usize, String> {
    let value = parameters
        .get(key)
        .ok_or_else(|| format!("{operation} requires {key}"))?;
    json_usize(value, key, operation)
}

fn json_usize(value: &Value, key: &str, operation: &str) -> Result<usize, String> {
    let value = value
        .as_u64()
        .ok_or_else(|| format!("{operation} {key} must be a non-negative integer"))?;
    usize::try_from(value).map_err(|_| format!("{operation} {key} is too large"))
}

fn parse_ybn_polygon_kind(value: &str) -> Result<YbnPolygonKind, String> {
    match value.to_ascii_lowercase().as_str() {
        "sphere" => Ok(YbnPolygonKind::Sphere),
        "capsule" => Ok(YbnPolygonKind::Capsule),
        "box" => Ok(YbnPolygonKind::Box),
        "cylinder" => Ok(YbnPolygonKind::Cylinder),
        _ => Err(format!(
            "unsupported ybn.edit-polygon kind {value:?}; expected sphere, capsule, box, or cylinder"
        )),
    }
}

fn collision_shape_kind(shape: &CollisionShape) -> YbnPolygonKind {
    match shape {
        CollisionShape::Sphere { .. } => YbnPolygonKind::Sphere,
        CollisionShape::Capsule { .. } => YbnPolygonKind::Capsule,
        CollisionShape::Box { .. } => YbnPolygonKind::Box,
        CollisionShape::Cylinder { .. } => YbnPolygonKind::Cylinder,
    }
}

fn ybn_kind_name(kind: YbnPolygonKind) -> &'static str {
    match kind {
        YbnPolygonKind::Sphere => "sphere",
        YbnPolygonKind::Capsule => "capsule",
        YbnPolygonKind::Box => "box",
        YbnPolygonKind::Cylinder => "cylinder",
    }
}

fn apply_ybn_operations(
    document: &OperationDocument,
    plan: &OperationPlan,
) -> Result<OperationApplyResult, OperationError> {
    let source_bytes = fs::read(&plan.source)?;
    let collision = YbnCollision::from_bytes(&source_bytes)
        .map_err(|error| OperationError::InvalidSource(error.to_string()))?;
    let edits = document
        .operations
        .iter()
        .map(|operation| parse_ybn_polygon_edit(operation, &collision))
        .collect::<Result<Vec<_>, _>>()
        .map_err(OperationError::Rejected)?;
    let rewritten = repack_polygon_edits(&source_bytes, &edits)
        .map_err(|error| OperationError::Writer(error.to_string()))?;
    let reopened = YbnCollision::from_bytes(&rewritten)
        .map_err(|error| OperationError::Writer(error.to_string()))?;

    if let Some(parent) = plan
        .output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let mut file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&plan.output)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(OperationError::Rejected(format!(
                "output already exists; apply never overwrites existing files: {}",
                plan.output.display()
            )))
        }
        Err(error) => return Err(OperationError::Io(error)),
    };
    file.write_all(&rewritten)?;
    file.flush()?;

    let source_unchanged = fs::read(&plan.source)? == source_bytes;
    if !source_unchanged {
        return Err(OperationError::Writer(
            "source asset changed during non-destructive apply".into(),
        ));
    }

    Ok(OperationApplyResult {
        schema: OPERATION_APPLY_SCHEMA,
        schema_version: OPERATION_DOCUMENT_SCHEMA_VERSION,
        source: plan.source.clone(),
        output: plan.output.clone(),
        asset_type: plan.asset_type.clone(),
        operations_applied: document.operations.len(),
        bytes_written: rewritten.len(),
        non_destructive: true,
        validation: ApplyValidation {
            semantic_reopen: true,
            source_unchanged,
        },
        details: json!({
            "shapePrimitives": reopened.shape_primitive_count(),
            "materials": reopened.materials.len(),
            "polygonEdits": edits.len(),
        }),
    })
}

fn apply_ydd_operations(
    document: &OperationDocument,
    plan: &OperationPlan,
) -> Result<OperationApplyResult, OperationError> {
    let source_bytes = fs::read(&plan.source)?;
    let original = YddDictionary::from_bytes(&source_bytes)
        .map_err(|error| OperationError::InvalidSource(error.to_string()))?;
    let original_entries = original.entries().to_vec();

    let mut working_bytes = source_bytes.clone();
    let mut translation_count = 0_usize;
    let mut texture_rebind_count = 0_usize;
    let mut shader_rebind_count = 0_usize;
    let mut drawable_indices = Vec::<usize>::new();

    for operation in &document.operations {
        let (rewritten, details) = apply_ydd_operation_to_bytes(&working_bytes, operation)
            .map_err(OperationError::Writer)?;
        working_bytes = rewritten;

        if let Some(index) = details.get("drawableIndex").and_then(Value::as_u64) {
            let index = usize::try_from(index)
                .map_err(|_| OperationError::Writer("drawable index overflow".into()))?;
            if !drawable_indices.contains(&index) {
                drawable_indices.push(index);
            }
        }

        match operation.operation_type.as_str() {
            "ydd.translate" => translation_count += 1,
            "ydd.rebind-texture" => texture_rebind_count += 1,
            "ydd.rebind-shader" => shader_rebind_count += 1,
            other => {
                return Err(OperationError::Rejected(format!(
                    "unsupported operation type: {other}"
                )))
            }
        }
    }

    let reopened = YddDictionary::from_bytes(&working_bytes)
        .map_err(|error| OperationError::Writer(error.to_string()))?;
    if reopened.entries() != original_entries.as_slice() {
        return Err(OperationError::Writer(
            "edited YDD changed dictionary identity metadata after semantic re-open".into(),
        ));
    }

    if let Some(parent) = plan
        .output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let mut file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&plan.output)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(OperationError::Rejected(format!(
                "output already exists; apply never overwrites existing files: {}",
                plan.output.display()
            )))
        }
        Err(error) => return Err(OperationError::Io(error)),
    };
    file.write_all(&working_bytes)?;
    file.flush()?;

    let source_unchanged = fs::read(&plan.source)? == source_bytes;
    if !source_unchanged {
        return Err(OperationError::Writer(
            "source asset changed during non-destructive apply".into(),
        ));
    }

    drawable_indices.sort_unstable();

    Ok(OperationApplyResult {
        schema: OPERATION_APPLY_SCHEMA,
        schema_version: OPERATION_DOCUMENT_SCHEMA_VERSION,
        source: plan.source.clone(),
        output: plan.output.clone(),
        asset_type: plan.asset_type.clone(),
        operations_applied: document.operations.len(),
        bytes_written: working_bytes.len(),
        non_destructive: true,
        validation: ApplyValidation {
            semantic_reopen: true,
            source_unchanged,
        },
        details: json!({
            "drawables": reopened.entries().len(),
            "editedDrawableIndices": drawable_indices,
            "translations": translation_count,
            "textureRebinds": texture_rebind_count,
            "shaderRebinds": shader_rebind_count,
        }),
    })
}

fn apply_ydr_operations(
    document: &OperationDocument,
    plan: &OperationPlan,
) -> Result<OperationApplyResult, OperationError> {
    let source_bytes = fs::read(&plan.source)?;
    let mut session = YdrEditSession::from_bytes(&source_bytes)
        .map_err(|error| OperationError::InvalidSource(error.to_string()))?;
    let before = session
        .document()
        .map_err(|error| OperationError::InvalidSource(error.to_string()))?;

    let mut effective_delta = [0.0_f32; 3];
    let mut translation_count = 0_usize;
    let mut texture_rebind_count = 0_usize;
    let mut shader_rebind_count = 0_usize;

    for operation in &document.operations {
        match operation.operation_type.as_str() {
            "ydr.translate" => {
                let delta = parse_translation_delta(operation).map_err(OperationError::Rejected)?;
                for index in 0..3 {
                    effective_delta[index] += delta[index];
                    if !effective_delta[index].is_finite() {
                        return Err(OperationError::Rejected(
                            "combined translation delta is not finite".into(),
                        ));
                    }
                }
                session
                    .translate_rigid_model(delta)
                    .map_err(|error| OperationError::Writer(error.to_string()))?;
                translation_count += 1;
            }
            "ydr.rebind-texture" => {
                plan_ydr_texture_rebind(&mut session, operation).map_err(OperationError::Writer)?;
                texture_rebind_count += 1;
            }
            "ydr.rebind-shader" => {
                plan_ydr_shader_rebind(&mut session, operation).map_err(OperationError::Writer)?;
                shader_rebind_count += 1;
            }
            other => {
                return Err(OperationError::Rejected(format!(
                    "unsupported operation type: {other}"
                )))
            }
        }
    }

    let expected_shader_bindings = session.shader_bindings().to_vec();
    let expected_texture_bindings = session.texture_bindings().to_vec();
    let rewritten = session
        .to_bytes()
        .map_err(|error| OperationError::Writer(error.to_string()))?;
    let reopened = YdrEditSession::from_bytes(&rewritten)
        .map_err(|error| OperationError::Writer(error.to_string()))?;
    let after = reopened
        .document()
        .map_err(|error| OperationError::Writer(error.to_string()))?;

    if reopened.shader_bindings() != expected_shader_bindings
        || reopened.texture_bindings() != expected_texture_bindings
    {
        return Err(OperationError::Writer(
            "edited YDR binding state changed after semantic re-open".into(),
        ));
    }

    verify_ydr_operation_result(
        &before,
        &after,
        effective_delta,
        texture_rebind_count > 0,
        shader_rebind_count > 0,
    )
    .map_err(|error| OperationError::Writer(error.to_string()))?;

    if let Some(parent) = plan
        .output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }

    let mut file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&plan.output)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(OperationError::Rejected(format!(
                "output already exists; apply never overwrites existing files: {}",
                plan.output.display()
            )))
        }
        Err(error) => return Err(OperationError::Io(error)),
    };
    file.write_all(&rewritten)?;
    file.flush()?;

    let source_unchanged = fs::read(&plan.source)? == source_bytes;
    if !source_unchanged {
        return Err(OperationError::Writer(
            "source asset changed during non-destructive apply".into(),
        ));
    }

    Ok(OperationApplyResult {
        schema: OPERATION_APPLY_SCHEMA,
        schema_version: OPERATION_DOCUMENT_SCHEMA_VERSION,
        source: plan.source.clone(),
        output: plan.output.clone(),
        asset_type: plan.asset_type.clone(),
        operations_applied: document.operations.len(),
        bytes_written: rewritten.len(),
        non_destructive: true,
        validation: ApplyValidation {
            semantic_reopen: true,
            source_unchanged,
        },
        details: json!({
            "effectiveTranslation": effective_delta,
            "translations": translation_count,
            "textureRebinds": texture_rebind_count,
            "shaderRebinds": shader_rebind_count,
            "shaderCount": reopened.shader_count(),
            "textureBindings": reopened.texture_bindings().len(),
            "vertices": after.model.vertex_count(),
        }),
    })
}

fn parse_translation_delta(operation: &OperationSpec) -> Result<[f32; 3], String> {
    if operation.parameters.len() != 1 || !operation.parameters.contains_key("delta") {
        return Err("ydr.translate accepts exactly one parameter: delta".into());
    }
    parse_delta_parameter(&operation.parameters, "ydr.translate")
}

fn parse_delta_parameter(
    parameters: &Map<String, Value>,
    operation_type: &str,
) -> Result<[f32; 3], String> {
    let values = parameters
        .get("delta")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            format!("{operation_type} delta must be an array of three finite numbers")
        })?;
    if values.len() != 3 {
        return Err(format!(
            "{operation_type} delta must contain exactly three values"
        ));
    }

    let mut delta = [0.0_f32; 3];
    for (index, value) in values.iter().enumerate() {
        let value = value
            .as_f64()
            .ok_or_else(|| format!("{operation_type} delta must contain only numbers"))?;
        if !value.is_finite() || value < f32::MIN as f64 || value > f32::MAX as f64 {
            return Err(format!(
                "{operation_type} delta must contain only finite f32 values"
            ));
        }
        delta[index] = value as f32;
    }

    if delta == [0.0, 0.0, 0.0] {
        return Err(format!("{operation_type} delta must not be zero"));
    }

    Ok(delta)
}

fn verify_ydr_operation_result(
    before: &YdrDocument,
    after: &YdrDocument,
    delta: [f32; 3],
    allow_texture_rebind: bool,
    allow_shader_rebind: bool,
) -> Result<(), io::Error> {
    let before_model = &before.model;
    let after_model = &after.model;
    if before_model.name != after_model.name
        || before_model.lod != after_model.lod
        || before_model.coordinate_convention != after_model.coordinate_convention
        || (!allow_texture_rebind && before_model.shaders != after_model.shaders)
        || before_model.primitives.len() != after_model.primitives.len()
        || before_model.bounds.radius != after_model.bounds.radius
        || translated_vec3(before_model.bounds.center, delta) != after_model.bounds.center
        || translated_vec3(before_model.bounds.min, delta) != after_model.bounds.min
        || translated_vec3(before_model.bounds.max, delta) != after_model.bounds.max
    {
        return Err(io::Error::other(
            "rigid translation changed drawable metadata or bounds unexpectedly",
        ));
    }

    let embedded_textures_match = match (
        before.embedded_textures.as_ref(),
        after.embedded_textures.as_ref(),
    ) {
        (None, None) => true,
        (Some(left), Some(right)) => left.textures() == right.textures(),
        _ => false,
    };
    if !embedded_textures_match {
        return Err(io::Error::other(
            "rigid translation changed embedded texture metadata",
        ));
    }

    for (before_primitive, after_primitive) in before_model
        .primitives
        .iter()
        .zip(after_model.primitives.iter())
    {
        if before_primitive.model_index != after_primitive.model_index
            || before_primitive.geometry_index != after_primitive.geometry_index
            || (!allow_shader_rebind
                && before_primitive.shader_index != after_primitive.shader_index)
            || before_primitive.topology != after_primitive.topology
            || before_primitive.normals != after_primitive.normals
            || before_primitive.uv0 != after_primitive.uv0
            || before_primitive.indices != after_primitive.indices
            || before_primitive.declaration != after_primitive.declaration
            || before_primitive.positions.len() != after_primitive.positions.len()
        {
            return Err(io::Error::other(
                "rigid translation changed primitive topology or non-position attributes",
            ));
        }
        for (before_position, after_position) in before_primitive
            .positions
            .iter()
            .zip(after_primitive.positions.iter())
        {
            if translated_vec3(*before_position, delta) != *after_position {
                return Err(io::Error::other(
                    "rigid translation produced an unexpected vertex position",
                ));
            }
        }
    }

    Ok(())
}

fn translated_vec3(value: [f32; 3], delta: [f32; 3]) -> [f32; 3] {
    [
        value[0] + delta[0],
        value[1] + delta[1],
        value[2] + delta[2],
    ]
}

fn resolve_path(base_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

fn asset_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "ydr" => "YDR",
        "ydd" => "YDD",
        "ytd" => "YTD",
        "ybn" => "YBN",
        "ymap" => "YMAP",
        "ytyp" => "YTYP",
        "ymf" => "YMF",
        "" => "UNKNOWN",
        _ => "UNKNOWN",
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        process,
        time::{SystemTime, UNIX_EPOCH},
    };

    use ragelab_ybn::{CollisionShape, YbnCollision};
    use ragelab_ydd::{YddDictionary, YddEditSession};
    use ragelab_ydr::{YdrDocument, YdrEditSession};
    use serde_json::json;

    use super::{
        apply_operation_document, parse_operation_document, plan_operation_document,
        OPERATION_DOCUMENT_SCHEMA,
    };

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic/ydr/simple.ydr")
    }

    fn editable_ydr_fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic/ydr/editable.ydr")
    }

    fn ydd_fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic/ydd/editable.ydd")
    }

    fn ybn_fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/synthetic/ybn/simple.ybn")
    }

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "ragelab-operation-{label}-{}-{nonce}",
            process::id()
        ))
    }

    fn document_json(source: &str, output: &str) -> String {
        json!({
            "schema": OPERATION_DOCUMENT_SCHEMA,
            "schemaVersion": 1,
            "source": source,
            "output": output,
            "operations": [
                {
                    "type": "ydr.translate",
                    "delta": [1.0, 2.0, 3.0]
                }
            ]
        })
        .to_string()
    }

    #[test]
    fn parses_and_plans_supported_ydr_translation() {
        let root = temp_root("plan");
        fs::create_dir_all(&root).unwrap();
        fs::copy(fixture(), root.join("source.ydr")).unwrap();

        let document =
            parse_operation_document(&document_json("source.ydr", "output.ydr")).unwrap();
        let plan = plan_operation_document(&document, &root).unwrap();

        assert!(plan.allowed);
        assert!(plan.non_destructive);
        assert!(!plan.output_exists);
        assert_eq!(plan.asset_type, "YDR");
        assert_eq!(plan.operations.len(), 1);
        assert!(plan.operations[0].allowed);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn plan_fails_closed_for_unknown_operation() {
        let root = temp_root("unknown");
        fs::create_dir_all(&root).unwrap();
        fs::copy(fixture(), root.join("source.ydr")).unwrap();

        let body = json!({
            "schema": OPERATION_DOCUMENT_SCHEMA,
            "schemaVersion": 1,
            "source": "source.ydr",
            "output": "output.ydr",
            "operations": [{"type": "ydr.rotate", "degrees": 90}]
        })
        .to_string();
        let document = parse_operation_document(&body).unwrap();
        let plan = plan_operation_document(&document, &root).unwrap();

        assert!(!plan.allowed);
        assert_eq!(
            plan.operations[0].reason.as_deref(),
            Some("unsupported operation type: ydr.rotate")
        );
        assert!(!root.join("output.ydr").exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn apply_writes_new_asset_and_preserves_source() {
        let root = temp_root("apply");
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source.ydr");
        let output = root.join("output.ydr");
        fs::copy(fixture(), &source).unwrap();
        let source_before = fs::read(&source).unwrap();

        let document =
            parse_operation_document(&document_json("source.ydr", "output.ydr")).unwrap();
        let result = apply_operation_document(&document, &root).unwrap();

        assert!(output.is_file());
        assert_eq!(fs::read(&source).unwrap(), source_before);
        assert!(result.validation.semantic_reopen);
        assert!(result.validation.source_unchanged);

        let before = YdrDocument::from_bytes(&source_before).unwrap();
        let after = YdrDocument::from_bytes(&fs::read(&output).unwrap()).unwrap();
        assert_eq!(before.model.primitives[0].positions[0], [0.0, 0.0, 0.0]);
        assert_eq!(after.model.primitives[0].positions[0], [1.0, 2.0, 3.0]);

        let second = apply_operation_document(&document, &root).unwrap_err();
        assert!(second.to_string().contains("never overwrites"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn plans_and_applies_mixed_ydr_rebind_operations() {
        let root = temp_root("ydr-rebind");
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source.ydr");
        let output = root.join("output.ydr");
        fs::copy(editable_ydr_fixture(), &source).unwrap();
        let source_before = fs::read(&source).unwrap();

        let body = json!({
            "schema": OPERATION_DOCUMENT_SCHEMA,
            "schemaVersion": 1,
            "source": "source.ydr",
            "output": "output.ydr",
            "operations": [
                {
                    "type": "ydr.translate",
                    "delta": [1.0, 0.0, 0.0]
                },
                {
                    "type": "ydr.rebind-texture",
                    "sourceShader": 0,
                    "sourceParameter": 0,
                    "targetShader": 0,
                    "targetParameter": 1
                },
                {
                    "type": "ydr.rebind-shader",
                    "modelIndex": 0,
                    "geometryIndex": 0,
                    "targetShaderIndex": 1
                }
            ]
        })
        .to_string();
        let document = parse_operation_document(&body).unwrap();
        let plan = plan_operation_document(&document, &root).unwrap();

        assert!(plan.allowed);
        assert_eq!(plan.operations.len(), 3);
        assert!(plan.operations.iter().all(|operation| operation.allowed));
        assert_eq!(plan.operations[1].details["targetTexture"], "test_normal");
        assert_eq!(plan.operations[2].details["targetShaderIndex"], 1);
        assert!(!output.exists());

        let result = apply_operation_document(&document, &root).unwrap();
        assert!(output.is_file());
        assert!(result.validation.semantic_reopen);
        assert!(result.validation.source_unchanged);
        assert_eq!(result.details["translations"], 1);
        assert_eq!(result.details["textureRebinds"], 1);
        assert_eq!(result.details["shaderRebinds"], 1);
        assert_eq!(fs::read(&source).unwrap(), source_before);

        let output_bytes = fs::read(&output).unwrap();
        let session = YdrEditSession::from_bytes(&output_bytes).unwrap();
        assert_eq!(
            session.texture_bindings()[0].texture_name.as_deref(),
            Some("test_normal")
        );
        assert_eq!(session.shader_bindings()[0].shader_index, 1);

        let after = session.document().unwrap();
        assert_eq!(after.model.primitives[0].positions[0], [1.0, 0.0, 0.0]);
        assert_eq!(after.model.primitives[0].shader_index, Some(1));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ydr_rebind_plan_fails_closed_for_invalid_selection() {
        let root = temp_root("ydr-rebind-invalid");
        fs::create_dir_all(&root).unwrap();
        fs::copy(editable_ydr_fixture(), root.join("source.ydr")).unwrap();

        let body = json!({
            "schema": OPERATION_DOCUMENT_SCHEMA,
            "schemaVersion": 1,
            "source": "source.ydr",
            "output": "output.ydr",
            "operations": [
                {
                    "type": "ydr.rebind-texture",
                    "sourceShader": 0,
                    "sourceParameter": 0,
                    "targetShader": 0,
                    "targetParameter": 0
                }
            ]
        })
        .to_string();
        let document = parse_operation_document(&body).unwrap();
        let plan = plan_operation_document(&document, &root).unwrap();

        assert!(!plan.allowed);
        assert!(!plan.operations[0].allowed);
        assert_eq!(
            plan.operations[0].reason.as_deref(),
            Some("ydr.rebind-texture source and target bindings must differ")
        );
        assert!(!root.join("output.ydr").exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn plans_and_applies_mixed_ydd_operations() {
        let root = temp_root("ydd-apply");
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source.ydd");
        let output = root.join("output.ydd");
        fs::copy(ydd_fixture(), &source).unwrap();
        let source_before = fs::read(&source).unwrap();

        let body = json!({
            "schema": OPERATION_DOCUMENT_SCHEMA,
            "schemaVersion": 1,
            "source": "source.ydd",
            "output": "output.ydd",
            "operations": [
                {
                    "type": "ydd.translate",
                    "drawableIndex": 0,
                    "delta": [1.0, 0.0, 0.0]
                },
                {
                    "type": "ydd.rebind-texture",
                    "drawableIndex": 0,
                    "sourceShader": 0,
                    "sourceParameter": 0,
                    "targetShader": 0,
                    "targetParameter": 1
                },
                {
                    "type": "ydd.rebind-shader",
                    "drawableIndex": 0,
                    "modelIndex": 0,
                    "geometryIndex": 0,
                    "targetShaderIndex": 1
                }
            ]
        })
        .to_string();
        let document = parse_operation_document(&body).unwrap();
        let plan = plan_operation_document(&document, &root).unwrap();

        assert!(plan.allowed);
        assert_eq!(plan.asset_type, "YDD");
        assert_eq!(plan.operations.len(), 3);
        assert!(plan.operations.iter().all(|operation| operation.allowed));
        assert_eq!(plan.operations[0].details["drawableIndex"], 0);
        assert_eq!(plan.operations[1].details["targetTexture"], "dict_normal");
        assert_eq!(plan.operations[2].details["targetShaderIndex"], 1);
        assert!(!output.exists());

        let result = apply_operation_document(&document, &root).unwrap();
        assert!(output.is_file());
        assert!(result.validation.semantic_reopen);
        assert!(result.validation.source_unchanged);
        assert_eq!(result.details["translations"], 1);
        assert_eq!(result.details["textureRebinds"], 1);
        assert_eq!(result.details["shaderRebinds"], 1);
        assert_eq!(result.details["editedDrawableIndices"][0], 0);
        assert_eq!(fs::read(&source).unwrap(), source_before);

        let output_bytes = fs::read(&output).unwrap();
        let dictionary = YddDictionary::from_bytes(&output_bytes).unwrap();
        assert_eq!(dictionary.entries().len(), 1);
        assert_eq!(dictionary.entries()[0].name_hash, 0x1234_5678);

        let session = YddEditSession::from_bytes(&output_bytes, 0).unwrap();
        assert_eq!(
            session.texture_bindings()[0].texture_name.as_deref(),
            Some("dict_normal")
        );
        assert_eq!(session.shader_bindings()[0].shader_index, 1);
        let document = session.document().unwrap();
        assert_eq!(document.model.primitives[0].positions[0], [1.0, 0.0, 0.0]);
        assert_eq!(document.model.primitives[0].shader_index, Some(1));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ydd_plan_fails_closed_for_invalid_drawable_selection() {
        let root = temp_root("ydd-invalid");
        fs::create_dir_all(&root).unwrap();
        fs::copy(ydd_fixture(), root.join("source.ydd")).unwrap();

        let body = json!({
            "schema": OPERATION_DOCUMENT_SCHEMA,
            "schemaVersion": 1,
            "source": "source.ydd",
            "output": "output.ydd",
            "operations": [
                {
                    "type": "ydd.translate",
                    "drawableIndex": 9,
                    "delta": [1.0, 0.0, 0.0]
                }
            ]
        })
        .to_string();
        let document = parse_operation_document(&body).unwrap();
        let plan = plan_operation_document(&document, &root).unwrap();

        assert!(!plan.allowed);
        assert!(!plan.operations[0].allowed);
        assert!(plan.operations[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("drawable index 9"));
        assert!(!root.join("output.ydd").exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn plans_and_applies_ybn_polygon_radius_and_material_edit() {
        let root = temp_root("ybn-apply");
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source.ybn");
        let output = root.join("output.ybn");
        fs::copy(ybn_fixture(), &source).unwrap();
        let source_before = fs::read(&source).unwrap();

        let body = json!({
            "schema": OPERATION_DOCUMENT_SCHEMA,
            "schemaVersion": 1,
            "source": "source.ybn",
            "output": "output.ybn",
            "operations": [
                {
                    "type": "ybn.edit-polygon",
                    "childIndex": 0,
                    "polygonIndex": 0,
                    "kind": "sphere",
                    "radius": 2.75,
                    "materialIndex": 1
                }
            ]
        })
        .to_string();
        let document = parse_operation_document(&body).unwrap();
        let plan = plan_operation_document(&document, &root).unwrap();

        assert!(plan.allowed);
        assert_eq!(plan.asset_type, "YBN");
        assert_eq!(plan.operations.len(), 1);
        assert!(plan.operations[0].allowed);
        assert!(!output.exists());

        let result = apply_operation_document(&document, &root).unwrap();
        assert!(output.is_file());
        assert_eq!(fs::read(&source).unwrap(), source_before);
        assert!(result.validation.semantic_reopen);
        assert!(result.validation.source_unchanged);

        let collision = YbnCollision::from_bytes(&fs::read(&output).unwrap()).unwrap();
        assert_eq!(collision.shape_primitives[0].material_index, 1);
        assert_eq!(
            collision.shape_primitives[0].shape,
            CollisionShape::Sphere {
                center: [11.0, 20.0, 30.0],
                radius: 2.75,
            }
        );

        fs::remove_dir_all(root).unwrap();
    }
}

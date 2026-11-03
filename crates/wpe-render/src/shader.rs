use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum UniformValue {
    Float(f32),
    Int(i32),
    Uint(u32),
    Vector(Vec<f32>),
    Matrix {
        columns: u8,
        rows: u8,
        values: Vec<f32>,
    },
    Array(Vec<UniformValue>),
    Struct(BTreeMap<String, UniformValue>),
}
impl UniformValue {
    pub fn from_json(value: &serde_json::Value) -> Result<Self> {
        value_from_json(value).ok_or_else(|| {
            Error::parse(
                0,
                "shader value is not numeric/vector/matrix/array/struct data",
            )
        })
    }
    /// Numeric JSON used by SceneScript material metadata, without enum tags.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Float(v) => serde_json::json!(v),
            Self::Int(v) => serde_json::json!(v),
            Self::Uint(v) => serde_json::json!(v),
            Self::Vector(v) | Self::Matrix { values: v, .. } => serde_json::json!(v),
            Self::Array(v) => serde_json::Value::Array(v.iter().map(Self::to_json).collect()),
            Self::Struct(v) => {
                serde_json::Value::Object(v.iter().map(|(k, v)| (k.clone(), v.to_json())).collect())
            }
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompiledShader {
    pub name: String,
    pub stage: ShaderStage,
    pub msl: String,
    pub entry_point: String,
    pub bindings: Vec<ShaderBinding>,
    pub uniforms: Vec<UniformMember>,
    pub uniform_size: u32,
    pub attributes: Vec<ShaderAttribute>,
    pub preprocessed_glsl: String,
    pub default_uniforms: BTreeMap<String, UniformValue>,
    pub uniform_aliases: BTreeMap<String, Vec<String>>,
    pub resource_hints: Vec<ResourceHint>,
    pub source_names: Vec<String>,
    pub profile_notes: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResourceHint {
    pub name: String,
    pub slot: u32,
    pub default_texture: Option<String>,
    pub metadata: serde_json::Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetadataReflection {
    pub defaults: BTreeMap<String, UniformValue>,
    pub uniform_aliases: BTreeMap<String, Vec<String>>,
    pub resource_hints: Vec<ResourceHint>,
    pub source_names: Vec<String>,
    pub defines: BTreeMap<String, i32>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ShaderStage {
    Vertex,
    Fragment,
    Compute,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TextureDimension {
    D1,
    D2,
    D3,
    Cube,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShaderBinding {
    pub name: String,
    pub group: u32,
    pub binding: u32,
    /// Logical Material texture/sampler slot; physical Metal samplers are dense.
    #[serde(default)]
    pub resource_slot: Option<u32>,
    pub texture_slot: Option<u32>,
    pub sampler_slot: Option<u32>,
    pub buffer_slot: Option<u32>,
    pub dimension: Option<TextureDimension>,
    pub depth: bool,
    pub comparison: bool,
    pub arrayed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UniformMember {
    pub name: String,
    pub offset: u32,
    pub bytes: u32,
    pub ty: String,
    pub layout: UniformType,
    pub aliases: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShaderAttribute {
    pub name: String,
    pub location: u32,
    pub components: u8,
    pub integer: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum ScalarType {
    Float,
    Int,
    Uint,
    Bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum UniformType {
    Scalar(ScalarType),
    Vector {
        kind: ScalarType,
        size: u8,
    },
    Matrix {
        columns: u8,
        rows: u8,
        column_stride: u32,
    },
    Array {
        count: u32,
        stride: u32,
        element: Box<UniformType>,
    },
    Struct {
        size: u32,
        members: Vec<UniformMember>,
    },
}
impl CompiledShader {
    /// Each stage has its own reflected std140 layout. GPU vertex data never shares buffer1.
    pub fn pack_uniforms(&self, values: &BTreeMap<String, UniformValue>) -> Result<Vec<u8>> {
        let mut bytes = vec![0; self.uniform_size as usize];
        for member in &self.uniforms {
            if let Some(value) = values
                .get(&member.name)
                .or_else(|| member.aliases.iter().find_map(|name| values.get(name)))
            {
                let projected = project_engine_matrix(&member.name, &member.layout, value)?;
                pack_value(
                    projected.as_ref().unwrap_or(value),
                    &member.layout,
                    &mut bytes,
                    member.offset as usize,
                )?;
            } else if let Some(value) = self.default_uniforms.get(&member.name) {
                // Some shipped editor annotations omit unused vector channels.
                // Raw metadata is preserved; this compatibility profile initializes
                // their missing storage components to zero, never uninitialized bytes.
                let mut value = value.clone();
                if let (UniformValue::Vector(v), UniformType::Vector { size, .. }) =
                    (&mut value, &member.layout)
                    && v.len() < *size as usize
                {
                    v.resize(*size as usize, 0.);
                }
                pack_value(&value, &member.layout, &mut bytes, member.offset as usize)?;
            }
        }
        Ok(bytes)
    }
}
/// Engine transforms use a canonical column-major 4x4 host value. Some shipped
/// stages declare a smaller matrix, including an unused mat3 g_ModelMatrix.
/// Only these named host transforms receive a top-left projection. User material
/// matrices retain strict shape checking, including values reached by aliases.
fn project_engine_matrix(
    name: &str,
    layout: &UniformType,
    value: &UniformValue,
) -> Result<Option<UniformValue>> {
    if !matches!(
        name,
        "g_ModelMatrix"
            | "g_ModelMatrixInverse"
            | "g_ViewMatrix"
            | "g_ViewMatrixInverse"
            | "g_ProjectionMatrix"
            | "g_ProjectionMatrixInverse"
            | "g_ModelViewMatrix"
            | "g_ModelViewMatrixInverse"
            | "g_ViewProjectionMatrix"
            | "g_ViewProjectionMatrixInverse"
            | "g_ModelViewProjectionMatrix"
            | "g_ModelViewProjectionMatrixInverse"
            | "g_NormalModelMatrix"
            | "g_EffectModelMatrix"
            | "g_EffectModelViewProjectionMatrix"
            | "g_EffectModelViewProjectionMatrixInverse"
            | "g_EffectTextureProjectionMatrix"
            | "g_EffectTextureProjectionMatrixInverse"
    ) {
        return Ok(None);
    }
    let (
        UniformType::Matrix { columns, rows, .. },
        UniformValue::Matrix {
            columns: 4,
            rows: 4,
            values,
        },
    ) = (layout, value)
    else {
        return Ok(None);
    };
    if *columns == 4 && *rows == 4 {
        return Ok(None);
    }
    if !(2..=4).contains(columns) || !(2..=4).contains(rows) || values.len() != 16 {
        return Err(Error::parse(0, "engine matrix shape mismatch"));
    }
    if values.iter().any(|v| !v.is_finite()) {
        return Err(Error::parse(0, "nonfinite engine matrix uniform"));
    }
    let mut projected = Vec::with_capacity(*columns as usize * *rows as usize);
    for column in 0..*columns as usize {
        projected.extend_from_slice(&values[column * 4..column * 4 + *rows as usize]);
    }
    Ok(Some(UniformValue::Matrix {
        columns: *columns,
        rows: *rows,
        values: projected,
    }))
}
fn scalar(value: &UniformValue, kind: ScalarType) -> Result<[u8; 4]> {
    let number = match value {
        UniformValue::Float(x) => *x as f64,
        UniformValue::Int(x) => *x as f64,
        UniformValue::Uint(x) => *x as f64,
        UniformValue::Vector(v) if v.len() == 1 => v[0] as f64,
        _ => return Err(Error::parse(0, "scalar uniform requires a number")),
    };
    if !number.is_finite() {
        return Err(Error::parse(0, "nonfinite uniform"));
    }
    Ok(match kind {
        ScalarType::Float => (number as f32).to_le_bytes(),
        ScalarType::Int => (number as i32).to_le_bytes(),
        ScalarType::Uint => (number as u32).to_le_bytes(),
        ScalarType::Bool => u32::from(number != 0.).to_le_bytes(),
    })
}
fn pack_value(
    value: &UniformValue,
    layout: &UniformType,
    bytes: &mut [u8],
    offset: usize,
) -> Result<()> {
    let write = |target: &mut [u8], at: usize, value: [u8; 4]| -> Result<()> {
        let end = at
            .checked_add(4)
            .ok_or_else(|| Error::limit(at, "uniform offset overflow"))?;
        target
            .get_mut(at..end)
            .ok_or_else(|| Error::parse(at, "uniform exceeds block"))?
            .copy_from_slice(&value);
        Ok(())
    };
    match layout {
        UniformType::Scalar(kind) => write(bytes, offset, scalar(value, *kind)?)?,
        UniformType::Vector { kind, size } => {
            for i in 0..*size as usize {
                let v = match value {
                    UniformValue::Vector(v) => UniformValue::Float(
                        *v.get(i)
                            .ok_or_else(|| Error::parse(offset, "uniform vector too short"))?,
                    ),
                    UniformValue::Float(_) | UniformValue::Int(_) | UniformValue::Uint(_) => {
                        value.clone()
                    }
                    _ => {
                        return Err(Error::parse(
                            offset,
                            "uniform vector requires numeric components",
                        ));
                    }
                };
                write(bytes, offset + i * 4, scalar(&v, *kind)?)?;
            }
        }
        UniformType::Matrix {
            columns,
            rows,
            column_stride,
        } => {
            let values = match value {
                UniformValue::Matrix {
                    columns: c,
                    rows: r,
                    values,
                } if c == columns && r == rows => values,
                UniformValue::Vector(v) => v,
                _ => return Err(Error::parse(offset, "matrix uniform shape mismatch")),
            };
            if values.iter().any(|v| !v.is_finite()) {
                return Err(Error::parse(offset, "nonfinite matrix uniform"));
            }
            if values.len() != *columns as usize * *rows as usize {
                return Err(Error::parse(
                    offset,
                    "matrix uniform element count mismatch",
                ));
            }
            for c in 0..*columns as usize {
                for r in 0..*rows as usize {
                    write(
                        bytes,
                        offset + c * *column_stride as usize + r * 4,
                        values[c * *rows as usize + r].to_le_bytes(),
                    )?;
                }
            }
        }
        UniformType::Array {
            count,
            stride,
            element,
        } => {
            let converted;
            let array = match value {
                UniformValue::Array(a) => a,
                UniformValue::Vector(v) if matches!(element.as_ref(), UniformType::Scalar(_)) => {
                    converted = v
                        .iter()
                        .map(|n| UniformValue::Float(*n))
                        .collect::<Vec<_>>();
                    &converted
                }
                _ => return Err(Error::parse(offset, "uniform array requires Array values")),
            };
            if array.len() > *count as usize {
                return Err(Error::parse(offset, "uniform array exceeds declared count"));
            }
            for (i, v) in array.iter().enumerate() {
                pack_value(v, element, bytes, offset + i * *stride as usize)?;
            }
        }
        UniformType::Struct { members, .. } => {
            let values = match value {
                UniformValue::Struct(m) => m,
                _ => {
                    return Err(Error::parse(
                        offset,
                        "uniform struct requires named Struct values",
                    ));
                }
            };
            for member in members {
                if let Some(value) = values.get(&member.name) {
                    pack_value(
                        value,
                        &member.layout,
                        bytes,
                        offset + member.offset as usize,
                    )?;
                }
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Default)]
pub struct SourceLibrary {
    sources: BTreeMap<String, String>,
}
impl SourceLibrary {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn insert(&mut self, name: impl Into<String>, source: impl Into<String>) {
        self.sources.insert(normalize(&name.into()), source.into());
    }
    pub fn get(&self, name: &str) -> Option<&str> {
        self.sources.get(&normalize(name)).map(String::as_str)
    }
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.sources.keys().map(String::as_str)
    }
    pub fn from_directory(root: &Path) -> Result<Self> {
        fn walk(
            root: &Path,
            dir: &Path,
            out: &mut SourceLibrary,
            total: &mut u64,
            depth: usize,
        ) -> Result<()> {
            if depth > 64 {
                return Err(Error::limit(0, "shader directory depth exceeds 64"));
            }
            for entry in std::fs::read_dir(dir)
                .map_err(|e| Error::parse(0, format!("shader directory: {e}")))?
            {
                let entry = entry.map_err(|e| Error::parse(0, e.to_string()))?;
                let path = entry.path();
                let kind = entry
                    .file_type()
                    .map_err(|e| Error::parse(0, e.to_string()))?;
                if kind.is_dir() {
                    walk(root, &path, out, total, depth + 1)?;
                } else if kind.is_file()
                    && path
                        .extension()
                        .and_then(|x| x.to_str())
                        .is_some_and(|e| ["vert", "frag", "geom", "h", "glsl"].contains(&e))
                {
                    let bytes = entry
                        .metadata()
                        .map_err(|e| Error::parse(0, e.to_string()))?
                        .len();
                    *total = total
                        .checked_add(bytes)
                        .ok_or_else(|| Error::limit(0, "shader library size overflow"))?;
                    if bytes > 4 * 1024 * 1024
                        || *total > 256 * 1024 * 1024
                        || out.sources.len() >= 65536
                    {
                        return Err(Error::limit(
                            0,
                            "shader library exceeds file/count/total policy",
                        ));
                    }
                    let name = path
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    let source = std::fs::read_to_string(&path)
                        .map_err(|e| Error::parse(0, format!("{}: {e}", path.display())))?;
                    out.insert(name, source);
                }
            }
            Ok(())
        }
        let mut out = Self::new();
        walk(root, root, &mut out, &mut 0, 0)?;
        Ok(out)
    }
    fn resolve(&self, owner: &str, include: &str) -> Option<String> {
        let parent = owner.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        [
            format!("{parent}/{include}"),
            include.to_owned(),
            format!("assets/shaders/{include}"),
            format!("shaders/{include}"),
        ]
        .into_iter()
        .map(|s| normalize(&s))
        .find(|s| self.sources.contains_key(s))
    }
}
fn normalize(path: &str) -> String {
    let path = path.replace('\\', "/");
    let mut components = Vec::new();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            _ => components.push(p),
        }
    }
    components.join("/")
}
#[derive(Clone, Debug)]
pub struct ShaderCompiler {
    pub library: SourceLibrary,
}
pub type Compiler = ShaderCompiler;
#[derive(Default)]
struct ShaderMetadata {
    defaults: BTreeMap<String, UniformValue>,
    aliases: BTreeMap<String, Vec<String>>,
    annotations: BTreeMap<String, serde_json::Value>,
}
struct Annotation {
    name: String,
    default: Option<UniformValue>,
    aliases: Vec<String>,
    metadata: serde_json::Value,
}
struct Prepared {
    tokens: Vec<String>,
    metadata: ShaderMetadata,
    names: Vec<String>,
    defines: BTreeMap<String, i32>,
}
fn charge_expansion(total: &mut usize, bytes: usize) -> Result<()> {
    *total = total
        .checked_add(bytes)
        .ok_or_else(|| Error::limit(0, "shader include size overflow"))?;
    if *total > 4 * 1024 * 1024 {
        return Err(Error::limit(0, "shader include expansion exceeds 4 MiB"));
    }
    Ok(())
}
impl ShaderCompiler {
    pub fn new(library: SourceLibrary) -> Self {
        Self { library }
    }
    /// Resolve includes/macros and active annotations without parsing Naga IR or emitting MSL.
    pub fn reflect_metadata(
        &self,
        name: &str,
        _stage: ShaderStage,
        defines: &BTreeMap<String, i32>,
    ) -> Result<MetadataReflection> {
        let prepared = self.prepare(name, defines)?;
        let mut uniform_aliases: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (name, aliases) in &prepared.metadata.aliases {
            for alias in aliases {
                uniform_aliases
                    .entry(alias.clone())
                    .or_default()
                    .push(name.clone());
            }
        }
        let mut resource_hints = Vec::new();
        let mut next_slot = 16;
        for i in 0..prepared.tokens.len().saturating_sub(2) {
            if prepared.tokens[i] == "uniform" && prepared.tokens[i + 1].starts_with("sampler") {
                let name = prepared.tokens[i + 2].clone();
                let slot = name
                    .strip_prefix("g_Texture")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_else(|| {
                        let slot = next_slot;
                        next_slot += 1;
                        slot
                    });
                let metadata = prepared
                    .metadata
                    .annotations
                    .get(&name)
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                resource_hints.push(ResourceHint {
                    name,
                    slot,
                    default_texture: metadata["default"].as_str().map(str::to_owned),
                    metadata,
                });
            }
        }
        Ok(MetadataReflection {
            defaults: prepared.metadata.defaults,
            uniform_aliases,
            resource_hints,
            source_names: prepared.names,
            defines: prepared.defines,
        })
    }
    pub fn compile(
        &self,
        name: &str,
        stage: ShaderStage,
        defines: &BTreeMap<String, i32>,
    ) -> Result<CompiledShader> {
        let prepared = self.prepare(name, defines)?;
        let varyings = varying_locations([&prepared]);
        self.finish(name, stage, prepared, &varyings)
    }
    pub fn compile_pair(
        &self,
        vertex: &str,
        fragment: &str,
        defines: &BTreeMap<String, i32>,
    ) -> Result<(CompiledShader, CompiledShader)> {
        let mut v = self.prepare(vertex, defines)?;
        let mut f = self.prepare(fragment, defines)?;
        let mut shared = v.defines.clone();
        shared.extend(f.defines.clone());
        shared.extend(defines.clone());
        if v.defines != shared {
            v = self.prepare(vertex, &shared)?;
        }
        if f.defines != shared {
            f = self.prepare(fragment, &shared)?;
        }
        let varying = varying_locations([&v, &f]);
        Ok((
            self.finish(vertex, ShaderStage::Vertex, v, &varying)?,
            self.finish(fragment, ShaderStage::Fragment, f, &varying)?,
        ))
    }
    fn prepare(&self, name: &str, defines: &BTreeMap<String, i32>) -> Result<Prepared> {
        let mut names = BTreeSet::new();
        let mut source = String::from(PRELUDE);
        let mut metadata = ShaderMetadata::default();
        let mut annotations = Vec::new();
        let mut combo_defaults = BTreeMap::new();
        let expanded = self.expand(
            &normalize(name),
            &mut BTreeSet::new(),
            &mut names,
            &mut annotations,
            &mut combo_defaults,
            0,
            &mut 0,
        )?;
        // The native Windows branch uses top-down screen/depth texture coordinates,
        // matching Metal. GLSL remains enabled for syntax utility implementations.
        if defines.get("WPE_GLSL_COORDINATES").copied().unwrap_or(0) == 0 {
            combo_defaults.insert("HLSL".into(), 1);
        }
        for (key, value) in defines {
            combo_defaults.insert(key.clone(), *value);
        }
        for (key, value) in &combo_defaults {
            if !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return Err(Error::parse(0, "invalid combo identifier"));
            }
            source.push_str(&format!("#define {key} {value}\n"));
        }
        source.push_str(&expanded);
        if source.len() > 4 * 1024 * 1024 {
            return Err(Error::limit(0, "expanded shader exceeds4 MiB"));
        }
        let mut tokens = Vec::new();
        for token in pp_rs::pp::Preprocessor::new(&source) {
            let token = token.map_err(|(e, l)| {
                Error::parse(
                    l.start as usize,
                    format!("{name}: GLSL preprocessing: {e:?}"),
                )
            })?;
            if let Some(mut t) = token_text(token.value) {
                if let Some(index) = t
                    .strip_prefix("_WPE_META_")
                    .and_then(|s| s.parse::<usize>().ok())
                {
                    let annotation = annotations.get(index).ok_or_else(|| {
                        Error::parse(0, "reserved shader metadata token collision")
                    })?;
                    if let Some(value) = &annotation.default {
                        metadata
                            .defaults
                            .insert(annotation.name.clone(), value.clone());
                    }
                    metadata
                        .annotations
                        .insert(annotation.name.clone(), annotation.metadata.clone());
                    let aliases = metadata.aliases.entry(annotation.name.clone()).or_default();
                    for alias in &annotation.aliases {
                        if !aliases.contains(alias) {
                            aliases.push(alias.clone());
                        }
                    }
                    continue;
                }
                if t == "sample" {
                    t = "_wpe_sample".into();
                }
                if t == "gl_FragColor" {
                    t = "_wpe_FragColor".into();
                }
                tokens.push(t);
            }
            if tokens.len() > 1_000_000 {
                return Err(Error::limit(0, "shader token count exceeds one million"));
            }
        }
        let mut depth = 0usize;
        let mut ternaries = 0usize;
        let mut unary = 0usize;
        for token in &tokens {
            if ["(", "[", "{"].contains(&token.as_str()) {
                depth += 1;
            }
            if [")", "]", "}"].contains(&token.as_str()) {
                depth = depth.saturating_sub(1);
            }
            if token == "?" {
                ternaries += 1;
            }
            if token == ";" {
                ternaries = 0;
            }
            unary = if ["!", "~", "-", "+", "++", "--"].contains(&token.as_str()) {
                unary + 1
            } else {
                0
            };
            if depth > 256 || ternaries > 256 || unary > 128 {
                return Err(Error::limit(
                    0,
                    "shader expression nesting exceeds portable compiler policy",
                ));
            }
        }
        // The Windows dialect may supply its own matrix inverse. Preserve that
        // later definition and rename only the now-unused portable prelude overload.
        let mut inverse_definitions: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for i in 0..tokens.len().saturating_sub(6) {
            if tokens[i + 1] == "wpe_inverse"
                && tokens[i + 2] == "("
                && tokens[i + 3].starts_with("mat")
                && tokens[i + 5] == ")"
                && tokens[i + 6] == "{"
            {
                inverse_definitions
                    .entry(tokens[i + 3].clone())
                    .or_default()
                    .push(i + 1);
            }
        }
        for (ty, indices) in inverse_definitions {
            for (index, position) in indices
                .iter()
                .take(indices.len().saturating_sub(1))
                .enumerate()
            {
                tokens[*position] = format!("_wpe_unused_inverse_{ty}_{index}");
            }
        }
        let normalized = crate::dialect::normalize(&tokens).join(" ");
        let tokens = pp_rs::pp::Preprocessor::new(&normalized)
            .filter_map(|token| match token {
                Ok(t) => token_text(t.value).map(Ok),
                Err((e, l)) => Some(Err(Error::parse(
                    l.start as usize,
                    format!("dialect tokenization: {e:?}"),
                ))),
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Prepared {
            tokens,
            metadata,
            names: names.into_iter().collect(),
            defines: combo_defaults,
        })
    }
    #[allow(clippy::too_many_arguments)]
    fn expand(
        &self,
        name: &str,
        stack: &mut BTreeSet<String>,
        names: &mut BTreeSet<String>,
        annotations: &mut Vec<Annotation>,
        combos: &mut BTreeMap<String, i32>,
        depth: usize,
        expanded_bytes: &mut usize,
    ) -> Result<String> {
        if depth > 64 {
            return Err(Error::limit(0, "shader include depth exceeds64"));
        }
        let source = self
            .library
            .get(name)
            .ok_or_else(|| Error::parse(0, format!("shader source not found: {name}")))?;
        if stack.contains(name) {
            return Ok(String::new());
        }
        stack.insert(name.to_owned());
        names.insert(name.to_owned());
        let guard = format!("WPE_INC_{:016x}", fnv(name.as_bytes()));
        let mut out = format!("\n#ifndef {guard}\n#define {guard}\n");
        charge_expansion(expanded_bytes, out.len())?;
        for line in source.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("#version") {
                continue;
            }
            if let Some(requirement) = trimmed.strip_prefix("#require ") {
                let requirement = requirement.trim();
                if let Some(resolved) = self
                    .library
                    .resolve(name, &format!("requirements/{requirement}.glsl"))
                {
                    out.push_str(&self.expand(
                        &resolved,
                        stack,
                        names,
                        annotations,
                        combos,
                        depth + 1,
                        expanded_bytes,
                    )?);
                } else if requirement == "LightingV1" {
                    names.insert("@builtin/LightingV1-point-profile".into());
                    charge_expansion(expanded_bytes, LIGHTING_V1.len())?;
                    out.push_str(LIGHTING_V1);
                } else {
                    let error = format!("\n#error unsupported_requirement_{requirement}\n");
                    charge_expansion(expanded_bytes, error.len())?;
                    out.push_str(&error);
                }
                continue;
            }
            if trimmed.starts_with("#include") {
                let include = trimmed
                    .split('"')
                    .nth(1)
                    .or_else(|| trimmed.split('<').nth(1).and_then(|x| x.split('>').next()));
                if let Some(include) = include {
                    if let Some(resolved) = self.library.resolve(name, include) {
                        out.push_str(&self.expand(
                            &resolved,
                            stack,
                            names,
                            annotations,
                            combos,
                            depth + 1,
                            expanded_bytes,
                        )?);
                    } else {
                        let error = format!(
                            "\n#error missing_include_{}\n",
                            include.replace(['.', '/', '-'], "_")
                        );
                        charge_expansion(expanded_bytes, error.len())?;
                        out.push_str(&error);
                    }
                } else {
                    return Err(Error::parse(0, format!("{name}: malformed include {line}")));
                }
                continue;
            }
            let mut sentinel = None;
            if let Some(start) = line.find('{')
                && let Ok(meta) = serde_json::from_str::<serde_json::Value>(&line[start..])
            {
                if line.contains("[COMBO]") {
                    if let Some(combo) = meta["combo"].as_str() {
                        let value = meta["default"].as_i64().unwrap_or(0);
                        combos.entry(combo.into()).or_insert(value as i32);
                    }
                } else if trimmed.starts_with("uniform ") {
                    let mut words = line[..start].split_whitespace().skip(1);
                    let mut ty = words.next();
                    while ty.is_some_and(|s| ["highp", "mediump", "lowp"].contains(&s)) {
                        ty = words.next();
                    }
                    if let Some(var) = words.next() {
                        let var = var.split([';', '[', ',']).next().unwrap();
                        let mut aliases = Vec::new();
                        for key in ["material", "label"] {
                            if let Some(alias) = meta[key].as_str()
                                && !aliases.iter().any(|a| a == alias)
                            {
                                aliases.push(alias.to_owned());
                            }
                        }
                        sentinel = Some(annotations.len());
                        annotations.push(Annotation {
                            name: var.into(),
                            default: value_from_json(&meta["default"]),
                            aliases,
                            metadata: meta,
                        });
                    }
                }
            }
            charge_expansion(expanded_bytes, line.len() + 1)?;
            out.push_str(line);
            out.push('\n');
            if let Some(index) = sentinel {
                let marker = format!("_WPE_META_{index}\n");
                charge_expansion(expanded_bytes, marker.len())?;
                out.push_str(&marker);
            }
        }
        charge_expansion(expanded_bytes, 8)?;
        out.push_str("\n#endif\n");
        stack.remove(name);
        Ok(out)
    }
    fn finish(
        &self,
        name: &str,
        stage: ShaderStage,
        prepared: Prepared,
        varyings: &BTreeMap<String, u32>,
    ) -> Result<CompiledShader> {
        let glsl = adapt(&prepared.tokens, stage, varyings)?;
        let mut parser = naga::front::glsl::Frontend::default();
        let module = parser
            .parse(&naga::front::glsl::Options::from(stage.naga()), &glsl)
            .map_err(|e| Error::parse(0, format!("{name}: {}", e.emit_to_string(&glsl))))?;
        compile_module(name, stage, module, glsl, prepared.metadata, prepared.names)
    }
}
impl ShaderStage {
    fn naga(self) -> naga::ShaderStage {
        match self {
            Self::Vertex => naga::ShaderStage::Vertex,
            Self::Fragment => naga::ShaderStage::Fragment,
            Self::Compute => naga::ShaderStage::Compute,
        }
    }
}
pub fn compile_glsl(
    name: &str,
    stage: ShaderStage,
    source: &str,
    defines: &BTreeMap<String, i32>,
) -> Result<CompiledShader> {
    let mut library = SourceLibrary::new();
    library.insert(name, source);
    ShaderCompiler::new(library).compile(name, stage, defines)
}
pub fn compile_wgsl(name: &str, stage: ShaderStage, source: &str) -> Result<CompiledShader> {
    let module = naga::front::wgsl::parse_str(source)
        .map_err(|e| Error::parse(0, e.emit_to_string(source)))?;
    compile_module(
        name,
        stage,
        module,
        source.into(),
        ShaderMetadata::default(),
        vec![name.into()],
    )
}
fn compile_module(
    name: &str,
    stage: ShaderStage,
    module: naga::Module,
    source: String,
    metadata: ShaderMetadata,
    names: Vec<String>,
) -> Result<CompiledShader> {
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|e| {
        Error::parse(
            0,
            format!("{name}: shader validation: {}", e.emit_to_string(&source)),
        )
    })?;
    let mut layouter = naga::proc::Layouter::default();
    layouter
        .update(module.to_ctx())
        .map_err(|e| Error::parse(0, format!("shader layout: {e}")))?;
    let mut bindings = Vec::new();
    let mut next_sampler = 0u8;
    let mut uniforms = Vec::new();
    let mut uniform_size = 0;
    let mut resources = naga::back::msl::EntryPointResources::default();
    let (entry_index, entry) = module
        .entry_points
        .iter()
        .enumerate()
        .find(|(_, e)| e.stage == stage.naga())
        .ok_or_else(|| Error::parse(0, "requested stage entry not found"))?;
    let entry_usage = info.get_entry_point(entry_index);
    for (handle, global) in module.global_variables.iter() {
        // Declarations unused by this entry point do not consume physical slots
        // or become fake required material inputs. Metadata-only reflection is separate.
        if entry_usage[handle].is_empty() {
            continue;
        }
        if let Some(binding) = &global.binding {
            let mut target = naga::back::msl::BindTarget::default();
            let mut entry = ShaderBinding {
                name: global.name.clone().unwrap_or_default(),
                group: binding.group,
                binding: binding.binding,
                resource_slot: None,
                texture_slot: None,
                sampler_slot: None,
                buffer_slot: None,
                dimension: None,
                depth: false,
                comparison: false,
                arrayed: false,
            };
            match module.types[global.ty].inner {
                naga::TypeInner::Image {
                    dim,
                    class,
                    arrayed,
                } => {
                    let slot = if binding.group == 0 {
                        binding.binding / 2
                    } else {
                        binding.binding
                    };
                    let slot = u8::try_from(slot)
                        .map_err(|_| Error::limit(0, "MSL texture slot exceeds255"))?;
                    target.texture = Some(slot);
                    entry.texture_slot = Some(slot as u32);
                    entry.resource_slot = Some(slot as u32);
                    entry.dimension = Some(match dim {
                        naga::ImageDimension::D1 => TextureDimension::D1,
                        naga::ImageDimension::D2 => TextureDimension::D2,
                        naga::ImageDimension::D3 => TextureDimension::D3,
                        naga::ImageDimension::Cube => TextureDimension::Cube,
                    });
                    entry.depth = matches!(class, naga::ImageClass::Depth { .. });
                    entry.arrayed = arrayed;
                }
                naga::TypeInner::Sampler { comparison } => {
                    let logical_slot = if binding.group == 0 {
                        binding.binding / 2
                    } else {
                        binding.binding
                    };
                    if next_sampler >= 16 {
                        return Err(Error::unsupported(
                            0,
                            "Metal stage requires more than 16 physical samplers",
                        ));
                    }
                    let slot = next_sampler;
                    next_sampler += 1;
                    entry.resource_slot = Some(logical_slot);
                    target.sampler = Some(naga::back::msl::BindSamplerTarget::Resource(slot));
                    entry.sampler_slot = Some(slot as u32);
                    entry.comparison = comparison;
                }
                _ => {
                    if global.space != naga::AddressSpace::Uniform {
                        return Err(Error::unsupported(
                            0,
                            "storage/buffer shader bindings require a separate explicit resource contract",
                        ));
                    }
                    target.buffer = Some(1);
                    entry.buffer_slot = Some(1);
                    if uniform_size != 0 {
                        return Err(Error::unsupported(
                            0,
                            "multiple uniform blocks require explicit buffer remapping",
                        ));
                    }
                    uniform_size = layouter[global.ty].size;
                    if let naga::TypeInner::Struct { ref members, .. } =
                        module.types[global.ty].inner
                    {
                        for m in members {
                            let name = m.name.clone().unwrap_or_default();
                            let aliases = metadata.aliases.get(&name).cloned().unwrap_or_default();
                            uniforms.push(UniformMember {
                                name,
                                offset: m.offset,
                                bytes: layouter[m.ty].size,
                                ty: format!("{:?}", module.types[m.ty].inner),
                                layout: uniform_type(&module, m.ty, &layouter)?,
                                aliases,
                            });
                        }
                    }
                }
            }
            resources.resources.insert(*binding, target);
            bindings.push(entry);
        }
    }
    let mut attributes = Vec::new();
    if stage == ShaderStage::Vertex {
        for arg in &entry.function.arguments {
            collect_attributes(
                &module,
                arg.name.as_deref(),
                arg.ty,
                arg.binding.as_ref(),
                &mut attributes,
            );
        }
    }
    let mut options = naga::back::msl::Options {
        lang_version: (2, 4),
        fake_missing_bindings: false,
        ..Default::default()
    };
    options
        .per_entry_point_map
        .insert(entry.name.clone(), resources);
    let pipeline = naga::back::msl::PipelineOptions {
        entry_point: Some((stage.naga(), entry.name.clone())),
        vertex_pulling_transform: false,
        ..Default::default()
    };
    let (mut msl, translation) = naga::back::msl::write_string(&module, &info, &options, &pipeline)
        .map_err(|e| Error::unsupported(0, format!("{name}: MSL backend: {e}")))?;
    let (strided_msl, padded_arrays) =
        crate::metal_layout::preserve_array_strides(&module, &layouter, &msl)?;
    msl = strided_msl;
    let entry_point = translation
        .entry_point_names
        .into_iter()
        .next()
        .ok_or_else(|| Error::parse(0, "MSL backend returned no entry"))?
        .map_err(|e| Error::unsupported(0, format!("MSL entry mapping: {e:?}")))?;
    let mut profile_notes=vec!["Legacy GLSL dialect: explicit scalar/vector broadcast and truncation; native GPU numerical equivalence is unverified.".into()];
    if padded_arrays != 0 {
        profile_notes.push(format!(
            "Preserved {padded_arrays} padded array strides in Metal storage; host packing retains the validated IR layout."
        ));
    }
    for member in &uniforms {
        if let (Some(UniformValue::Vector(v)), UniformType::Vector { size, .. }) =
            (metadata.defaults.get(&member.name), &member.layout)
            && v.len() < *size as usize
        {
            profile_notes.push(format!("Metadata default {} has {} of {} vector channels; this profile zero-fills missing channels. Raw annotation is retained.",member.name,v.len(),size));
        }
    }
    if source.contains("_wpe_ViewportIndex") {
        msl = msl
            .replace("[[user(locn31), flat]]", "[[viewport_array_index]]")
            .replace("[[user(locn31)]]", "[[viewport_array_index]]")
            .replace("[[user(loc31), flat]]", "[[viewport_array_index]]")
            .replace("[[user(loc31)]]", "[[viewport_array_index]]");
        if !msl.contains("[[viewport_array_index]]") {
            return Err(Error::unsupported(
                0,
                "viewport output could not be mapped to MSL viewport_array_index",
            ));
        }
        profile_notes
            .push("Requires a Metal viewport array matching the emitted viewport indices.".into());
    }
    if names
        .iter()
        .any(|n| n == "@builtin/LightingV1-point-profile")
    {
        profile_notes.push("LightingV1 uses an owned orchestration of shipped PBR point-light equations, at most WPE_MAX_LIGHTS; native light-template/cookie/shadow orchestration is not asserted.".into());
    }
    let mut uniform_aliases: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for member in &uniforms {
        for alias in &member.aliases {
            uniform_aliases
                .entry(alias.clone())
                .or_default()
                .push(member.name.clone());
        }
    }
    let resource_hints = bindings
        .iter()
        .filter_map(|binding| {
            binding.texture_slot.map(|slot| {
                let name = binding
                    .name
                    .strip_prefix("_wpe_tex_")
                    .unwrap_or(&binding.name)
                    .to_owned();
                let raw = metadata
                    .annotations
                    .get(&name)
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                ResourceHint {
                    name,
                    slot,
                    default_texture: raw["default"].as_str().map(str::to_owned),
                    metadata: raw,
                }
            })
        })
        .collect();
    Ok(CompiledShader {
        name: name.into(),
        stage,
        msl,
        entry_point,
        bindings,
        uniforms,
        uniform_size,
        attributes,
        preprocessed_glsl: source,
        default_uniforms: metadata.defaults,
        uniform_aliases,
        resource_hints,
        source_names: names,
        profile_notes,
    })
}
fn uniform_type(
    module: &naga::Module,
    ty: naga::Handle<naga::Type>,
    layout: &naga::proc::Layouter,
) -> Result<UniformType> {
    let kind = |kind| match kind {
        naga::ScalarKind::Sint => ScalarType::Int,
        naga::ScalarKind::Uint => ScalarType::Uint,
        naga::ScalarKind::Bool => ScalarType::Bool,
        _ => ScalarType::Float,
    };
    Ok(match &module.types[ty].inner {
        naga::TypeInner::Scalar(s) => UniformType::Scalar(kind(s.kind)),
        naga::TypeInner::Vector { size, scalar } => UniformType::Vector {
            kind: kind(scalar.kind),
            size: *size as u8,
        },
        naga::TypeInner::Matrix { columns, rows, .. } => UniformType::Matrix {
            columns: *columns as u8,
            rows: *rows as u8,
            column_stride: if *rows == naga::VectorSize::Bi { 8 } else { 16 },
        },
        naga::TypeInner::Array {
            base,
            size: naga::ArraySize::Constant(n),
            stride,
        } => UniformType::Array {
            count: n.get(),
            stride: *stride,
            element: Box::new(uniform_type(module, *base, layout)?),
        },
        naga::TypeInner::Struct { members, span } => UniformType::Struct {
            size: *span,
            members: members
                .iter()
                .map(|m| {
                    Ok(UniformMember {
                        name: m.name.clone().unwrap_or_default(),
                        offset: m.offset,
                        bytes: layout[m.ty].size,
                        ty: format!("{:?}", module.types[m.ty].inner),
                        layout: uniform_type(module, m.ty, layout)?,
                        aliases: Vec::new(),
                    })
                })
                .collect::<Result<_>>()?,
        },
        _ => return Err(Error::unsupported(0, "unsupported uniform member layout")),
    })
}
fn collect_attributes(
    module: &naga::Module,
    name: Option<&str>,
    ty: naga::Handle<naga::Type>,
    binding: Option<&naga::Binding>,
    out: &mut Vec<ShaderAttribute>,
) {
    if let Some(naga::Binding::Location { location, .. }) = binding {
        let (components, integer) = match module.types[ty].inner {
            naga::TypeInner::Vector { size, scalar } => {
                (size as u8, scalar.kind != naga::ScalarKind::Float)
            }
            naga::TypeInner::Scalar(s) => (1, s.kind != naga::ScalarKind::Float),
            _ => (4, false),
        };
        out.push(ShaderAttribute {
            name: name.unwrap_or("").into(),
            location: *location,
            components,
            integer,
        });
    } else if let naga::TypeInner::Struct { ref members, .. } = module.types[ty].inner {
        for m in members {
            collect_attributes(module, m.name.as_deref(), m.ty, m.binding.as_ref(), out);
        }
    }
}

fn varying_locations<'a>(
    prepared: impl IntoIterator<Item = &'a Prepared>,
) -> BTreeMap<String, u32> {
    let mut names = BTreeSet::new();
    for p in prepared {
        for i in 0..p.tokens.len() {
            if p.tokens[i] == "varying" {
                let mut j = i + 1;
                while p.tokens.get(j).is_some_and(|s| {
                    ["highp", "mediump", "lowp", "flat", "smooth", "centroid"].contains(&s.as_str())
                }) {
                    j += 1;
                }
                if let Some(name) = p.tokens.get(j + 1) {
                    names.insert(name.clone());
                }
            }
        }
    }
    let canonical = ["v_TexCoord", "v_Color", "v_Normal", "v_ScreenCoord"];
    let mut next = 4;
    names
        .into_iter()
        .map(|name| {
            let location = if name == "_wpe_ViewportIndex" {
                31
            } else {
                canonical
                    .iter()
                    .position(|n| *n == name)
                    .map(|n| n as u32)
                    .unwrap_or_else(|| {
                        let n = next;
                        next += 1;
                        n
                    })
            };
            (name, location)
        })
        .collect()
}
fn adapt(
    tokens: &[String],
    stage: ShaderStage,
    varyings: &BTreeMap<String, u32>,
) -> Result<String> {
    let natural_arguments = crate::array_arguments::lower(tokens)?;
    let tokens = natural_arguments.as_slice();
    let mut body = Vec::new();
    let mut uniforms = Vec::new();
    let mut structs = Vec::new();
    let mut sampler_declarations = String::new();
    let mut private_inputs = Vec::new();
    let mut i = 0;
    let mut braces = 0i32;
    let mut parens = 0i32;
    let mut next_attribute = 4u32;
    let mut next_texture = 16u32;
    let compared = tokens
        .windows(3)
        .filter(|w| w[0] == "texSample2DCompare" && w[1] == "(")
        .map(|w| w[2].clone())
        .collect::<BTreeSet<_>>();
    while i < tokens.len() {
        let token = &tokens[i];
        if braces == 0 && parens == 0 && token == "struct" {
            let open = (i..tokens.len())
                .find(|j| tokens[*j] == "{")
                .ok_or_else(|| Error::parse(0, "struct has no body"))?;
            let mut depth = 1;
            let mut end = open + 1;
            while end < tokens.len() && depth > 0 {
                if tokens[end] == "{" {
                    depth += 1;
                }
                if tokens[end] == "}" {
                    depth -= 1;
                }
                end += 1;
            }
            if tokens.get(end).is_none_or(|s| s != ";") {
                return Err(Error::unsupported(
                    0,
                    "inline struct variable declarations require a separate declaration",
                ));
            }
            structs.push(tokens[i..=end].join(" "));
            i = end + 1;
            continue;
        }
        if braces == 0
            && parens == 0
            && token == "in"
            && tokens
                .get(i + 2)
                .is_some_and(|s| ["gl_InstanceIndex", "gl_VertexIndex"].contains(&s.as_str()))
            && let Some(end) = (i..tokens.len()).find(|j| tokens[*j] == ";")
        {
            i = end + 1;
            continue;
        }
        if braces == 0
            && parens == 0
            && ["uniform", "attribute", "varying"].contains(&token.as_str())
        {
            let end = (i..tokens.len())
                .find(|j| tokens[*j] == ";")
                .ok_or_else(|| Error::parse(0, "unterminated shader global declaration"))?;
            let mut start = i + 1;
            while start < end
                && ["highp", "mediump", "lowp", "flat", "smooth", "centroid"]
                    .contains(&tokens[start].as_str())
            {
                start += 1;
            }
            if start + 1 >= end {
                return Err(Error::parse(0, "missing shader global type/name"));
            }
            let ty = &tokens[start];
            let name = &tokens[start + 1];
            if token == "uniform" {
                if ty.starts_with("sampler") {
                    if start + 2 != end {
                        return Err(Error::unsupported(
                            0,
                            "sampler arrays/multiple sampler declaration require explicit bindings",
                        ));
                    }
                    let slot = name
                        .strip_prefix("g_Texture")
                        .and_then(|s| s.parse::<u32>().ok())
                        .unwrap_or_else(|| {
                            let slot = next_texture;
                            next_texture += 1;
                            slot
                        });
                    let image = match ty.as_str() {
                        "sampler2D" | "sampler2DBackBuffer" | "sampler2DShadow" => "texture2D",
                        "sampler3D" => "texture3D",
                        "samplerCube" | "samplerCUBE" => "textureCube",
                        _ => return Err(Error::unsupported(0, format!("sampler type {ty}"))),
                    };
                    let shadow = compared.contains(name) || ty == "sampler2DShadow";
                    let constructor = if shadow {
                        "sampler2DShadow"
                    } else {
                        match ty.as_str() {
                            "samplerCUBE" => "samplerCube",
                            "sampler2DBackBuffer" => "sampler2D",
                            _ => ty,
                        }
                    };
                    let sampler = if shadow { "samplerShadow" } else { "sampler" };
                    sampler_declarations.push_str(&format!("layout(set=0,binding={}) uniform {image} _wpe_tex_{name};\nlayout(set=0,binding={}) uniform {sampler} _wpe_smp_{name};\n#define {name} {constructor}(_wpe_tex_{name},_wpe_smp_{name})\n",slot*2,slot*2+1));
                } else {
                    uniforms.push(format!("{} ;", tokens[start..end].join(" ")));
                }
            } else {
                let location = if token == "attribute" {
                    match name.as_str() {
                        "a_Position" => 0,
                        "a_Normal" => 1,
                        "a_TexCoord" | "a_TexCoord0" => 2,
                        "a_Color" => 3,
                        _ => {
                            let n = next_attribute;
                            next_attribute += 1;
                            n
                        }
                    }
                } else {
                    *varyings
                        .get(name)
                        .ok_or_else(|| Error::parse(0, "varying location missing"))?
                };
                let direction = if token == "attribute" || stage == ShaderStage::Fragment {
                    "in"
                } else {
                    "out"
                };
                let flat = if ty == "uint"
                    || ty == "int"
                    || ty.starts_with("ivec")
                    || ty.starts_with("uvec")
                {
                    "flat "
                } else {
                    ""
                };
                if token == "varying" && stage == ShaderStage::Fragment {
                    let input = format!("_wpe_input_{name}");
                    let mut declaration = tokens[start..end].to_vec();
                    declaration[1] = input.clone();
                    body.push(format!(
                        "layout(location={location}) {flat}in {} ; {} ;",
                        declaration.join(" "),
                        tokens[start..end].join(" ")
                    ));
                    private_inputs.push(format!("{name}={input};"));
                } else {
                    body.push(format!(
                        "layout(location={location}) {flat}{direction} {} ;",
                        tokens[start..end].join(" ")
                    ));
                }
            }
            i = end + 1;
            continue;
        }
        if token == "{" {
            braces += 1;
        }
        if token == "}" {
            braces -= 1;
        }
        if token == "(" {
            parens += 1;
        }
        if token == ")" {
            parens -= 1;
        }
        body.push(if token == "gl_FragColor" {
            "_wpe_FragColor".into()
        } else {
            token.clone()
        });
        i += 1;
    }
    let mut source = String::from(
        "#version 450\n#define texSample2DCompare(s,uv,z) vec4(texture(s,vec3((uv).xy,z)))\n",
    );
    for ty in structs {
        source.push_str(&ty);
        source.push('\n');
    }
    source.push_str(&sampler_declarations);
    if !uniforms.is_empty() {
        source.push_str("layout(std140,set=1,binding=0) uniform WpeUniforms {\n");
        for u in uniforms {
            source.push_str(&u);
            source.push('\n');
        }
        source.push_str("};\n");
    }
    if stage == ShaderStage::Fragment {
        source.push_str("layout(location=0) out vec4 _wpe_FragColor;\n");
    }
    let mut body = body.join(" ");
    if stage == ShaderStage::Fragment && !private_inputs.is_empty() {
        body = transform_identifiers(&body, &BTreeMap::from([("main", "_wpe_fragment_main")]));
        body.push_str(&format!(
            "\nvoid main(){{{}_wpe_fragment_main();}}",
            private_inputs.join(" ")
        ));
    }
    source.push_str(&body.replace(';', ";\n").replace('}', "}\n"));
    if source.len() > 4 * 1024 * 1024 {
        return Err(Error::limit(0, "adapted shader exceeds4MiB source policy"));
    }
    Ok(source)
}
fn value_from_json(value: &serde_json::Value) -> Option<UniformValue> {
    if let Some(n) = value.as_f64() {
        let n = n as f32;
        return n.is_finite().then_some(UniformValue::Float(n));
    }
    if let Some(n) = value.as_bool() {
        return Some(UniformValue::Int(i32::from(n)));
    }
    if let Some(s) = value.as_str() {
        let numbers = s
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|v| !v.is_empty())
            .map(|v| v.parse::<f32>())
            .collect::<std::result::Result<Vec<_>, _>>()
            .ok()?;
        if !numbers.is_empty() && numbers.iter().all(|v| v.is_finite()) {
            return Some(UniformValue::Vector(numbers));
        }
    }
    if let Some(values) = value.as_array() {
        if values.iter().all(serde_json::Value::is_number) {
            let values = values
                .iter()
                .map(|v| v.as_f64().unwrap() as f32)
                .collect::<Vec<_>>();
            return values
                .iter()
                .all(|v| v.is_finite())
                .then_some(UniformValue::Vector(values));
        }
        return values
            .iter()
            .map(value_from_json)
            .collect::<Option<Vec<_>>>()
            .map(UniformValue::Array);
    }
    if let Some(object) = value.as_object() {
        if let Some(value) = object.get("value") {
            return value_from_json(value);
        }
        return object
            .iter()
            .map(|(k, v)| Some((k.clone(), value_from_json(v)?)))
            .collect::<Option<BTreeMap<_, _>>>()
            .map(UniformValue::Struct);
    }
    None
}
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ *b as u64).wrapping_mul(0x100000001b3)
    })
}
fn token_text(token: pp_rs::token::TokenValue) -> Option<String> {
    use pp_rs::token::{Punct as P, TokenValue as T};
    Some(match token {
        T::Ident(s) => s,
        T::Integer(n) => format!("{}{}", n.value, if n.signed { "" } else { "u" }),
        T::Float(n) => format!("{:?}", n.value),
        T::Punct(p) => match p {
            P::AddAssign => "+=",
            P::SubAssign => "-=",
            P::MulAssign => "*=",
            P::DivAssign => "/=",
            P::ModAssign => "%=",
            P::LeftShiftAssign => "<<=",
            P::RightShiftAssign => ">>=",
            P::AndAssign => "&=",
            P::XorAssign => "^=",
            P::OrAssign => "|=",
            P::Increment => "++",
            P::Decrement => "--",
            P::LogicalAnd => "&&",
            P::LogicalOr => "||",
            P::LogicalXor => "^^",
            P::LessEqual => "<=",
            P::GreaterEqual => ">=",
            P::EqualEqual => "==",
            P::NotEqual => "!=",
            P::LeftShift => "<<",
            P::RightShift => ">>",
            P::LeftBrace => "{",
            P::RightBrace => "}",
            P::LeftParen => "(",
            P::RightParen => ")",
            P::LeftBracket => "[",
            P::RightBracket => "]",
            P::LeftAngle => "<",
            P::RightAngle => ">",
            P::Semicolon => ";",
            P::Comma => ",",
            P::Colon => ":",
            P::Dot => ".",
            P::Equal => "=",
            P::Bang => "!",
            P::Minus => "-",
            P::Tilde => "~",
            P::Plus => "+",
            P::Star => "*",
            P::Slash => "/",
            P::Percent => "%",
            P::Pipe => "|",
            P::Caret => "^",
            P::Ampersand => "&",
            P::Question => "?",
        }
        .into(),
        T::Version(_) | T::Extension(_) | T::Pragma(_) => return None,
    })
}
const PRELUDE: &str = r#"
mat2 wpe_inverse(mat2 m){
 float d=m[0][0]*m[1][1]-m[1][0]*m[0][1];
 return mat2(m[1][1],-m[0][1],-m[1][0],m[0][0])/d;
}
mat3 wpe_inverse(mat3 m){
 vec3 a=cross(m[1],m[2]),b=cross(m[2],m[0]),c=cross(m[0],m[1]);
 return transpose(mat3(a,b,c))/dot(m[0],a);
}
mat4 wpe_inverse(mat4 m){
 mat4 a=m;mat4 result=mat4(1.0);
 for(int pivot=0;pivot<4;pivot++){
  int best=pivot;for(int row=pivot+1;row<4;row++){if(abs(a[pivot][row])>abs(a[pivot][best]))best=row;}
  if(best!=pivot){for(int column=0;column<4;column++){
   float swap=a[column][pivot];a[column][pivot]=a[column][best];a[column][best]=swap;
   swap=result[column][pivot];result[column][pivot]=result[column][best];result[column][best]=swap;
  }}
  float factor=a[pivot][pivot];for(int column=0;column<4;column++){a[column][pivot]/=factor;result[column][pivot]/=factor;}
  for(int row=0;row<4;row++){if(row!=pivot){float f=a[pivot][row];for(int column=0;column<4;column++){a[column][row]-=a[column][pivot]*f;result[column][row]-=result[column][pivot]*f;}}}
 }return result;
}
#define inverse(m) wpe_inverse(m)
#define GLSL 1
#define gl_InstanceID gl_InstanceIndex
#define gl_VertexID gl_VertexIndex
#define gl_ViewportIndex _wpe_ViewportIndex
#define CASTF(x) float(x)
#define CASTI(x) int(x)
#define CASTU(x) uint(x)
#define CAST2(x) vec2(x)
#define CAST3(x) vec3(x)
#define CAST4(x) vec4(x)
#define CAST2X2(x) mat2(x)
#define CAST3X3(x) mat3(x)
#define CAST4X4(x) mat4(x)
#define mul(a,b) ((b)*(a))
#define saturate(x) clamp((x),0.0,1.0)
#define frac(x) fract(x)
#define lerp(a,b,t) mix(a,b,t)
#define fmod(x,y) ((x)-(y)*trunc((x)/(y)))
#define rsqrt(x) inversesqrt(x)
#define atan2(y,x) atan(y,x)
#define ddx(x) dFdx(x)
#define ddy(x) dFdy(x)
#define clip(x) if((x)<0.0) discard
#define texSample2D(s,uv) texture(s,(uv).xy)
#define texSample2DLod(s,uv,lod) textureLod(s,(uv).xy,lod)
#define texSample2DGrad(s,uv,dx,dy) textureGrad(s,(uv).xy,dx,dy)
#define texSample2DBackBuffer(s,uv,resolution) texture(s,(uv).xy)
#define texLoad2D(s,uv,resolution) texelFetch(s,ivec2((uv).xy*(resolution).xy),0)
#define texSample3D texture
#define texSample3DLod textureLod
#define texSampleCUBE texture
#define texSampleCube texture
#define texture2D texture
#define texture3D texture
#define textureCube texture
#define texture2DLod textureLod
#define float2 vec2
#define float3 vec3
#define float4 vec4
#define float2x2 mat2
#define float3x3 mat3
#define float4x4 mat4
"#;
const LIGHTING_V1: &str = r#"
#if LIGHTING
#ifndef WPE_MAX_LIGHTS
#define WPE_MAX_LIGHTS 16
#endif
uniform int wpe_LightCount;
uniform vec4 wpe_LightPositionRadius[WPE_MAX_LIGHTS];
uniform vec4 wpe_LightColorExponent[WPE_MAX_LIGHTS];
vec3 PerformLighting_V1(vec3 worldPos,vec3 albedo,vec3 normal,vec3 viewDirection,vec3 specularTint,vec3 f0,float roughness,float metallic){
 vec3 result=vec3(0.0);
 for(int i=0;i<min(wpe_LightCount,WPE_MAX_LIGHTS);i++){
  vec3 delta=wpe_LightPositionRadius[i].xyz-worldPos;
  if(length(delta)>0.0&&wpe_LightPositionRadius[i].w>0.0)
   result+=ComputePBRLightShadow(normal,delta,viewDirection,albedo,wpe_LightColorExponent[i].rgb,wpe_LightPositionRadius[i].w,wpe_LightColorExponent[i].w,specularTint,f0,roughness,metallic,1.0);
 }return result;
}
#endif
"#;

/// Recompile already-adapted GLSL450 without running the legacy declaration adapter twice.
pub fn compile_adapted_glsl(
    name: &str,
    stage: ShaderStage,
    source: &str,
    defaults: BTreeMap<String, UniformValue>,
) -> Result<CompiledShader> {
    let module = naga::front::glsl::Frontend::default()
        .parse(&naga::front::glsl::Options::from(stage.naga()), source)
        .map_err(|e| Error::parse(0, e.emit_to_string(source)))?;
    compile_module(
        name,
        stage,
        module,
        source.into(),
        ShaderMetadata {
            defaults,
            ..Default::default()
        },
        vec![name.into()],
    )
}

/// Compile the explicit CPU profiles into a matching canonical GPU shader pair.
pub fn builtin_material(material: &crate::Material) -> Result<crate::Material> {
    use crate::CpuProgram;
    let number = |v: f32| -> Result<String> {
        if !v.is_finite() {
            return Err(Error::parse(0, "nonfinite builtin shader parameter"));
        }
        Ok(format!("{v:?}"))
    };
    let vector = |v: &[f32]| -> Result<String> {
        Ok(format!(
            "vec{}({})",
            v.len(),
            v.iter()
                .map(|v| number(*v))
                .collect::<Result<Vec<_>>>()?
                .join(",")
        ))
    };
    let mut frag =
        String::from("varying vec4 v_TexCoord;varying vec4 v_Color;varying vec3 v_Normal;\n");
    let body = match &material.cpu {
        CpuProgram::Textured {
            texture_slot,
            color,
            alpha_test,
        } => {
            frag.push_str(&format!("uniform sampler2D g_Texture{texture_slot};\n"));
            let mut s = format!(
                "vec4 c=texSample2D(g_Texture{texture_slot},v_TexCoord.xy)*{}*v_Color;",
                vector(color)?
            );
            if let Some(limit) = alpha_test {
                s.push_str(&format!("if(c.a<{})discard;", number(*limit)?));
            }
            s.push_str("gl_FragColor=c;");
            s
        }
        CpuProgram::Solid { color } => format!("gl_FragColor={}*v_Color;", vector(color)?),
        CpuProgram::ShadowDepth {
            texture_slot,
            alpha_test,
        } => {
            let alpha = if let Some(slot) = texture_slot {
                frag.push_str(&format!("uniform sampler2D g_Texture{slot};\n"));
                format!("texture(g_Texture{slot},v_TexCoord.xy).a*v_Color.a")
            } else {
                "v_Color.a".into()
            };
            let cut = if let Some(limit) = alpha_test {
                if !limit.is_finite() {
                    return Err(Error::parse(0, "nonfinite shadow alpha cut"));
                }
                format!("if({alpha}<{limit:?})discard;")
            } else {
                String::new()
            };
            format!("{cut}gl_FragColor=vec4(gl_FragCoord.z,0.0,0.0,1.0);")
        }
        CpuProgram::Volume {
            texture_slot,
            color,
        }
        | CpuProgram::Cube {
            texture_slot,
            color,
        } => {
            frag.push_str(&format!(
                "uniform {} g_Texture{texture_slot};\n",
                if matches!(material.cpu, CpuProgram::Cube { .. }) {
                    "samplerCube"
                } else {
                    "sampler3D"
                }
            ));
            format!(
                "gl_FragColor=texture(g_Texture{texture_slot},v_TexCoord.xyz)*{}*v_Color;",
                vector(color)?
            )
        }
        CpuProgram::Lambert {
            texture_slot,
            light_direction,
            ambient,
            light_color,
        } => {
            let sample = if let Some(slot) = texture_slot {
                frag.push_str(&format!("uniform sampler2D g_Texture{slot};\n"));
                format!("texture(g_Texture{slot},v_TexCoord.xy)")
            } else {
                "vec4(1.0)".into()
            };
            format!(
                "vec3 n=v_Normal;float len=length(n);if(len>0.0)n/=len;vec3 l={};len=length(l);if(len>0.0)l/=len;float d=max(dot(n,l),0.0);vec4 c={sample};gl_FragColor=vec4(c.rgb*({}+{}*d),c.a)*v_Color;",
                vector(light_direction)?,
                vector(ambient)?,
                vector(light_color)?
            )
        }
        CpuProgram::GpuOnly { reason } => {
            return Err(Error::unsupported(
                0,
                format!("cannot synthesize an unknown GPU program: {reason}"),
            ));
        }
    };
    frag.push_str("void main(){");
    frag.push_str(&body);
    frag.push('}');
    let mut library = SourceLibrary::new();
    library.insert("builtin.vert", CANONICAL_VERTEX);
    library.insert("builtin.frag", frag);
    let (v, f) = ShaderCompiler::new(library).compile_pair(
        "builtin.vert",
        "builtin.frag",
        &BTreeMap::new(),
    )?;
    let mut out = material.clone();
    out.vertex = Some(std::sync::Arc::new(v));
    out.fragment = Some(std::sync::Arc::new(f));
    Ok(out)
}

/// Apply shader blend mode only to fragments covered by the original draw geometry.
/// The host snapshots the current destination and binds it as `base_texture_id`.
pub fn with_destination_blend(
    material: &crate::Material,
    base_texture_id: crate::TextureId,
    size: [u32; 2],
) -> Result<crate::Material> {
    let crate::BlendState::Custom(mode) = material.state.blend else {
        return Ok(material.clone());
    };
    if size.contains(&0) {
        return Err(Error::parse(
            0,
            "destination blend dimensions must be nonzero",
        ));
    }
    let mut out = if material.fragment.is_some() {
        material.clone()
    } else {
        builtin_material(material)?
    };
    let original = out.fragment.as_ref().unwrap();
    if !original
        .preprocessed_glsl
        .trim_start()
        .starts_with("#version")
    {
        return Err(Error::unsupported(
            0,
            "destination blend requires GLSL450 source metadata",
        ));
    }
    let used = original
        .bindings
        .iter()
        .filter_map(|b| b.resource_slot)
        .chain(out.textures.iter().map(|b| b.slot))
        .collect::<BTreeSet<_>>();
    let slot = (0..32)
        .find(|s| !used.contains(s))
        .ok_or_else(|| Error::limit(0, "no free destination texture/sampler slot below32"))?;
    let mut source = transform_identifiers(
        &original.preprocessed_glsl,
        &BTreeMap::from([("main", "_wpe_original_main")]),
    );
    let helper = crate::blend::glsl(mode)?;
    let names = [
        "burn",
        "dodge",
        "overlay",
        "vivid",
        "reflectblend",
        "to_hsl",
        "hue_rgb",
        "to_rgb",
        "mode_scalar",
        "wpe_blend",
    ];
    let map = names
        .iter()
        .map(|name| (*name, format!("_wpe_dest_{name}")))
        .collect::<BTreeMap<_, _>>();
    let borrowed = map.iter().map(|(k, v)| (*k, v.as_str())).collect();
    source.push_str(&transform_identifiers(&helper, &borrowed));
    source.push_str(&format!("\nlayout(set=0,binding={}) uniform texture2D _wpe_destination_image;\nlayout(set=0,binding={}) uniform sampler _wpe_destination_sampler;\nvoid main(){{_wpe_original_main();vec4 src=_wpe_FragColor;vec2 uv=gl_FragCoord.xy/vec2({}.0,{}.0);vec4 dst=texture(sampler2D(_wpe_destination_image,_wpe_destination_sampler),uv);_wpe_FragColor=vec4(_wpe_dest_wpe_blend(dst.rgb,src.rgb,src.a),dst.a);}}",slot*2,slot*2+1,size[0],size[1]));
    let mut compiled = compile_adapted_glsl(
        &format!("{}+destination-blend-{mode}", original.name),
        ShaderStage::Fragment,
        &source,
        original.default_uniforms.clone(),
    )?;
    compiled.source_names = original.source_names.clone();
    compiled.uniform_aliases = original.uniform_aliases.clone();
    compiled.resource_hints = original.resource_hints.clone();
    for member in &mut compiled.uniforms {
        if let Some(source) = original.uniforms.iter().find(|m| m.name == member.name) {
            member.aliases = source.aliases.clone();
        }
    }
    compiled.profile_notes.push("Destination snapshot is sampled only inside the original triangle fragments; discard/depth/color mask remain active.".into());
    out.fragment = Some(std::sync::Arc::new(compiled));
    out.textures.push(crate::TextureBinding {
        name: "_wpe_destination_image".into(),
        slot,
        texture: base_texture_id,
    });
    out.state.blend = crate::BlendState::Replace;
    out.samplers.insert(
        slot,
        crate::SamplerState {
            min_filter: crate::FilterMode::Nearest,
            mag_filter: crate::FilterMode::Nearest,
            mip_filter: crate::FilterMode::Nearest,
            lod_max: 0.,
            ..Default::default()
        },
    );
    out.cpu=crate::CpuProgram::GpuOnly{reason:"Use the original Custom-blend frame with the CPU reference; this material is a GPU destination-snapshot lowering.".into()};
    Ok(out)
}
fn transform_identifiers(source: &str, replacements: &BTreeMap<&str, &str>) -> String {
    let mut out = String::new();
    let mut begin = 0;
    let bytes = source.as_bytes();
    while begin < bytes.len() {
        if bytes[begin].is_ascii_alphabetic() || bytes[begin] == b'_' {
            let mut end = begin + 1;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            let word = &source[begin..end];
            out.push_str(replacements.get(word).copied().unwrap_or(word));
            begin = end;
        } else {
            let c = source[begin..].chars().next().unwrap();
            out.push(c);
            begin += c.len_utf8();
        }
    }
    out
}
const CANONICAL_VERTEX: &str = "attribute vec4 a_Position;attribute vec4 a_Normal;attribute vec4 a_TexCoord;attribute vec4 a_Color;varying vec4 v_TexCoord;varying vec4 v_Color;varying vec3 v_Normal;void main(){gl_Position=a_Position;v_TexCoord=a_TexCoord;v_Color=a_Color;v_Normal=a_Normal.xyz;}";

/// Preserve alpha-test/discard and explicit fragment depth while removing color outputs.
/// The host binds a Depth32Float depth attachment and no color attachments.
pub fn depth_only_material(material: &crate::Material) -> Result<crate::Material> {
    let mut out = if material.fragment.is_some() {
        material.clone()
    } else {
        builtin_material(material)?
    };
    let original = out.fragment.as_ref().unwrap();
    let marker = "layout(location=0) out vec4 _wpe_FragColor;";
    if !original.preprocessed_glsl.contains(marker) {
        return Err(Error::unsupported(
            0,
            "depth-only lowering requires an adapted GLSL color0 output",
        ));
    }
    let source = original
        .preprocessed_glsl
        .replace(marker, "vec4 _wpe_FragColor;");
    let mut compiled = compile_adapted_glsl(
        &format!("{}+depth-only", original.name),
        ShaderStage::Fragment,
        &source,
        original.default_uniforms.clone(),
    )?;
    compiled.uniform_aliases = original.uniform_aliases.clone();
    compiled.resource_hints = original.resource_hints.clone();
    compiled.source_names = original.source_names.clone();
    for member in &mut compiled.uniforms {
        if let Some(m) = original.uniforms.iter().find(|m| m.name == member.name) {
            member.aliases = m.aliases.clone();
        }
    }
    compiled.profile_notes.push(
        "Depth-only attachment: color output removed; discard and fragment depth remain active."
            .into(),
    );
    out.fragment = Some(std::sync::Arc::new(compiled));
    out.state.color_write_mask = 0;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logical_high_texture_slots_use_dense_physical_samplers_and_ignore_unused() {
        let mut source = String::new();
        for slot in [0, 1, 2, 24, 25, 31] {
            source.push_str(&format!("uniform sampler2D g_Texture{slot};\n"));
        }
        source.push_str("void main(){gl_FragColor=");
        source.push_str(
            &[0, 1, 2, 24, 25]
                .iter()
                .map(|slot| format!("texture(g_Texture{slot},vec2(0.5))"))
                .collect::<Vec<_>>()
                .join("+"),
        );
        source.push_str(";}");
        let shader = compile_glsl(
            "dense-samplers.frag",
            ShaderStage::Fragment,
            &source,
            &BTreeMap::new(),
        )
        .unwrap();
        let samplers = shader
            .bindings
            .iter()
            .filter_map(|b| {
                b.sampler_slot
                    .map(|physical| (b.resource_slot.unwrap(), physical))
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            samplers,
            BTreeMap::from([(0, 0), (1, 1), (2, 2), (24, 3), (25, 4)])
        );
        assert!(shader.bindings.iter().all(|b| b.resource_slot != Some(31)));
        assert!(shader.msl.contains("[[texture(24)]]"));
        assert!(shader.msl.contains("[[sampler(3)]]"));
        assert!(!shader.msl.contains("[[sampler(24)]]"));
        let mut excess = String::new();
        for slot in 0..17 {
            excess.push_str(&format!("uniform sampler2D g_Texture{slot};\n"));
        }
        excess.push_str("void main(){gl_FragColor=");
        excess.push_str(
            &(0..17)
                .map(|slot| format!("texture(g_Texture{slot},vec2(0.5))"))
                .collect::<Vec<_>>()
                .join("+"),
        );
        excess.push_str(";}");
        assert_eq!(
            compile_glsl(
                "too-many.frag",
                ShaderStage::Fragment,
                &excess,
                &BTreeMap::new()
            )
            .unwrap_err()
            .kind,
            crate::ErrorKind::Unsupported
        );
    }
    #[test]
    fn expansion_and_nonfinite_uniforms_fail_with_bounded_errors() {
        let mut library = SourceLibrary::new();
        library.insert("big.h", format!("//{}\n", "x".repeat(1024 * 1024)));
        library.insert("main.frag","#include \"big.h\"\n#include \"big.h\"\n#include \"big.h\"\n#include \"big.h\"\nvoid main(){gl_FragColor=vec4(1);}");
        let error = ShaderCompiler::new(library)
            .reflect_metadata("main.frag", ShaderStage::Fragment, &BTreeMap::new())
            .unwrap_err();
        assert_eq!(error.kind, crate::ErrorKind::Limit);
        assert!(UniformValue::from_json(&serde_json::json!("NaN 1")).is_err());
        assert!(UniformValue::from_json(&serde_json::json!(1e100)).is_err());
        let shader = compile_glsl(
            "matrix.frag",
            ShaderStage::Fragment,
            "uniform mat3 matrix;void main(){gl_FragColor=vec4(matrix[0],1);}",
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(
            shader
                .pack_uniforms(&BTreeMap::from([(
                    "matrix".into(),
                    UniformValue::Matrix {
                        columns: 3,
                        rows: 3,
                        values: vec![f32::INFINITY; 9]
                    }
                )]))
                .is_err()
        );
    }
    #[test]
    fn actual_glsl_to_msl_with_sampler_uniform_and_metadata() {
        let source = "uniform sampler2D g_Texture0;\nuniform vec4 g_Tint; // {\"default\":\"1 0.5 0.25 1\"}\nvarying vec2 v_TexCoord;void main(){gl_FragColor=texSample2D(g_Texture0,v_TexCoord)*g_Tint;}";
        let c = compile_glsl(
            "owned.frag",
            ShaderStage::Fragment,
            source,
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(c.msl.contains("fragment"));
        assert!(c.bindings.iter().any(|b| b.texture_slot == Some(0)));
        let packed = c.pack_uniforms(&BTreeMap::new()).unwrap();
        assert_eq!(f32::from_le_bytes(packed[..4].try_into().unwrap()), 1.);
    }
    #[test]
    fn uniform_array_function_arguments_keep_values_and_std140_storage() {
        let source = "uniform float leftAudio[16];uniform float rightAudio[16];float audio(float a[16],float b[16]){return a[3]+b[7];}void main(){gl_FragColor=vec4(audio(leftAudio,rightAudio));}";
        let shader = compile_glsl(
            "owned-audio.frag",
            ShaderStage::Fragment,
            source,
            &BTreeMap::new(),
        )
        .unwrap();
        let values = BTreeMap::from([
            (
                "leftAudio".into(),
                UniformValue::Vector((0..16).map(|n| n as f32 / 16.).collect()),
            ),
            (
                "rightAudio".into(),
                UniformValue::Vector((0..16).map(|n| 1. - n as f32 / 16.).collect()),
            ),
        ]);
        let bytes = shader.pack_uniforms(&values).unwrap();
        for (name, expected) in [("leftAudio", 3. / 16.), ("rightAudio", 1. - 7. / 16.)] {
            let field = shader.uniforms.iter().find(|m| m.name == name).unwrap();
            let UniformType::Array { count, stride, .. } = field.layout else {
                panic!()
            };
            assert_eq!(count, 16);
            assert_eq!(stride, 16);
            let index = if name == "leftAudio" { 3 } else { 7 };
            let at = field.offset as usize + index * stride as usize;
            assert_eq!(
                f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()),
                expected
            );
        }
        assert!(shader.msl.contains("fragment"));
        let module = naga::front::glsl::Frontend::default()
            .parse(
                &naga::front::glsl::Options::from(naga::ShaderStage::Fragment),
                &shader.preprocessed_glsl,
            )
            .unwrap();
        let function = module
            .functions
            .iter()
            .find(|(_, f)| f.name.as_deref() == Some("audio"))
            .unwrap()
            .1;
        for arg in &function.arguments {
            let naga::TypeInner::Array { stride, .. } = module.types[arg.ty].inner else {
                panic!()
            };
            assert_eq!(stride, 4, "function arrays retain natural float stride");
        }
    }
    #[test]
    fn array_argument_conversion_preserves_scopes_qualifiers_and_bounds() {
        let valid = "uniform float data[16];float read(float a[16]){return a[0];}float read(float a){return a;}void main(){float data=0.25;gl_FragColor=vec4(read(data));}";
        assert!(
            compile_glsl(
                "shadowed.frag",
                ShaderStage::Fragment,
                valid,
                &BTreeMap::new()
            )
            .is_ok()
        );
        let invalid = "uniform float data[16];void write(out float a[16]){a[0]=1.;}void main(){write(data);gl_FragColor=vec4(1);}";
        assert!(
            compile_glsl(
                "readonly.frag",
                ShaderStage::Fragment,
                invalid,
                &BTreeMap::new()
            )
            .is_err()
        );
        let mismatch = "uniform float data[16];float read(float a[8]){return a[0];}void main(){gl_FragColor=vec4(read(data));}";
        assert!(
            compile_glsl(
                "mismatch.frag",
                ShaderStage::Fragment,
                mismatch,
                &BTreeMap::new()
            )
            .is_err()
        );
        let oversized = "uniform float data[4097];float read(float a[4097]){return a[0];}void main(){gl_FragColor=vec4(read(data));}";
        assert_eq!(
            compile_glsl(
                "oversized.frag",
                ShaderStage::Fragment,
                oversized,
                &BTreeMap::new()
            )
            .unwrap_err()
            .kind,
            crate::ErrorKind::Limit
        );
        let vectors = "uniform vec2 data[2];vec2 read(const vec2 a[2]){return a[1];}void main(){gl_FragColor=vec4(read(data),0,1);}";
        let shader = compile_glsl(
            "vectors.frag",
            ShaderStage::Fragment,
            vectors,
            &BTreeMap::new(),
        )
        .unwrap();
        let packed = shader
            .pack_uniforms(&BTreeMap::from([(
                "data".into(),
                UniformValue::Array(vec![
                    UniformValue::Vector(vec![1., 2.]),
                    UniformValue::Vector(vec![3., 4.]),
                ]),
            )]))
            .unwrap();
        assert_eq!(f32::from_le_bytes(packed[16..20].try_into().unwrap()), 3.);
        assert_eq!(f32::from_le_bytes(packed[20..24].try_into().unwrap()), 4.);
    }
    #[test]
    fn conditional_uniforms_and_include_resolution() {
        let mut lib = SourceLibrary::new();
        lib.insert("main.frag","#include \"helpers.h\"\n#if ACTIVE\nuniform vec4 color;\n#else\nuniform float alpha;\n#endif\nvoid main(){\n#if ACTIVE\ngl_FragColor=pick()*color;\n#else\ngl_FragColor=pick()*alpha;\n#endif\n}");
        lib.insert("helpers.h", "vec4 pick(){return vec4(1);}");
        let c = ShaderCompiler::new(lib)
            .compile(
                "main.frag",
                ShaderStage::Fragment,
                &BTreeMap::from([("ACTIVE".into(), 1)]),
            )
            .unwrap();
        assert_eq!(c.uniforms.len(), 1);
        assert_eq!(c.uniforms[0].name, "color");
    }
    #[test]
    fn reflected_std140_matrix_and_array_packing() {
        let c=compile_glsl("layout.frag",ShaderStage::Fragment,"uniform vec3 tint;uniform float opacity;uniform mat3 transform;uniform vec2 points[2];void main(){gl_FragColor=vec4(tint*opacity+transform*vec3(points[0],1),1);}",&BTreeMap::new()).unwrap();
        let fields = c
            .uniforms
            .iter()
            .map(|m| (m.name.as_str(), m.offset))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(fields["opacity"], 12);
        assert_eq!(fields["transform"], 16);
        assert_eq!(fields["points"], 64);
        let values = BTreeMap::from([
            (
                "transform".into(),
                UniformValue::Matrix {
                    columns: 3,
                    rows: 3,
                    values: (0..9).map(|x| x as f32).collect(),
                },
            ),
            (
                "points".into(),
                UniformValue::Array(vec![
                    UniformValue::Vector(vec![10., 11.]),
                    UniformValue::Vector(vec![12., 13.]),
                ]),
            ),
        ]);
        let b = c.pack_uniforms(&values).unwrap();
        assert_eq!(f32::from_le_bytes(b[32..36].try_into().unwrap()), 3.);
        assert_eq!(f32::from_le_bytes(b[80..84].try_into().unwrap()), 12.);
    }
    #[test]
    fn host_matrices_project_column_major_without_coercing_material_matrices() {
        let shader = compile_glsl(
            "host-matrix.frag",
            ShaderStage::Fragment,
            "uniform mat3 g_ModelMatrix;uniform mat4x3 g_EffectModelMatrix;uniform mat3 userMatrix;void main(){gl_FragColor=vec4(g_ModelMatrix[0]+g_EffectModelMatrix[3]+userMatrix[2],1);}",
            &BTreeMap::new(),
        ).unwrap();
        let host = UniformValue::Matrix {
            columns: 4,
            rows: 4,
            values: (0..16).map(|n| n as f32).collect(),
        };
        let mut values = BTreeMap::from([
            ("g_ModelMatrix".into(), host.clone()),
            ("g_EffectModelMatrix".into(), host.clone()),
        ]);
        let packed = shader.pack_uniforms(&values).unwrap();
        for name in ["g_ModelMatrix", "g_EffectModelMatrix"] {
            let member = shader.uniforms.iter().find(|m| m.name == name).unwrap();
            let UniformType::Matrix {
                columns,
                rows,
                column_stride,
            } = &member.layout
            else {
                panic!()
            };
            for c in 0..*columns as usize {
                for r in 0..*rows as usize {
                    let at = member.offset as usize + c * *column_stride as usize + r * 4;
                    assert_eq!(
                        f32::from_le_bytes(packed[at..at + 4].try_into().unwrap()),
                        (c * 4 + r) as f32
                    );
                }
            }
        }
        values.insert("userMatrix".into(), host);
        assert!(
            shader
                .pack_uniforms(&values)
                .unwrap_err()
                .context
                .contains("shape mismatch")
        );
        values.remove("userMatrix");
        values.insert(
            "g_ModelMatrix".into(),
            UniformValue::Matrix {
                columns: 4,
                rows: 4,
                values: vec![1.; 15],
            },
        );
        assert!(shader.pack_uniforms(&values).is_err());
        values.insert(
            "g_ModelMatrix".into(),
            UniformValue::Matrix {
                columns: 4,
                rows: 4,
                values: vec![f32::INFINITY; 16],
            },
        );
        assert!(shader.pack_uniforms(&values).is_err());
    }
    #[test]
    fn real_destination_blend_and_instance_viewport_lowering() {
        let material = crate::Material {
            cpu: crate::CpuProgram::Solid {
                color: [0.2, 0.4, 0.6, 0.5],
            },
            state: crate::PipelineState {
                blend: crate::BlendState::Custom(5),
                ..Default::default()
            },
            ..Default::default()
        };
        let m = with_destination_blend(&material, 99, [16, 16]).unwrap();
        assert_eq!(m.state.blend, crate::BlendState::Replace);
        assert!(m.fragment.as_ref().unwrap().msl.contains("fragment"));
        assert_eq!(m.textures.last().unwrap().texture, 99);
        let c=compile_glsl("viewport.vert",ShaderStage::Vertex,"in uint gl_InstanceID;varying uint gl_ViewportIndex;attribute vec3 a_Position;void main(){gl_Position=vec4(a_Position,1.0);gl_ViewportIndex=gl_InstanceID;}",&BTreeMap::new()).unwrap();
        assert!(c.msl.contains("[[viewport_array_index]]"));
        assert!(c.msl.contains("instance_id"));
    }
    #[test]
    fn active_metadata_alias_and_texture_defaults() {
        let code = r#"
uniform sampler2D g_Texture0; // {"default":"util/noise","mode":"opacitymask"}
#if HIGH
uniform float g_Power; // {"material":"Power","label":"ui_editor_properties_power","default":4}
#else
uniform float g_Power; // {"material":"Low","label":"lowlabel","default":1}
#endif
void main(){gl_FragColor=texture(g_Texture0,vec2(0.5))*g_Power;}
"#;
        let shader = compile_glsl(
            "aliases.frag",
            ShaderStage::Fragment,
            code,
            &BTreeMap::from([("HIGH".into(), 1)]),
        )
        .unwrap();
        assert_eq!(shader.uniform_aliases["Power"], ["g_Power"]);
        assert!(!shader.uniform_aliases.contains_key("Low"));
        let packed = shader
            .pack_uniforms(&BTreeMap::from([(
                "ui_editor_properties_power".into(),
                UniformValue::Float(2.),
            )]))
            .unwrap();
        assert_eq!(f32::from_le_bytes(packed[..4].try_into().unwrap()), 2.);
        assert_eq!(
            shader.resource_hints[0].default_texture.as_deref(),
            Some("util/noise")
        );
        let mut library = SourceLibrary::new();
        library.insert(
            "metadata.frag",
            format!("{code}\nthis is deliberately not valid shader syntax"),
        );
        let reflected = ShaderCompiler::new(library)
            .reflect_metadata(
                "metadata.frag",
                ShaderStage::Fragment,
                &BTreeMap::from([("HIGH".into(), 1)]),
            )
            .unwrap();
        assert_eq!(
            reflected.defaults["g_Power"].to_json(),
            serde_json::json!(4.0)
        );
        assert_eq!(reflected.uniform_aliases["Power"], ["g_Power"]);
        let short = compile_glsl(
            "short.frag",
            ShaderStage::Fragment,
            "uniform vec4 color; // {\"default\":\"0.5 0.25\"}\nvoid main(){gl_FragColor=color;}",
            &BTreeMap::new(),
        )
        .unwrap();
        let packed = short.pack_uniforms(&BTreeMap::new()).unwrap();
        assert_eq!(&packed[8..16], &[0; 8]);
        assert!(short.profile_notes.iter().any(|s| s.contains("zero-fills")));
    }
    #[test]
    fn actual_cube_comparison_and_depth_only_msl() {
        let cube=compile_glsl("cube.frag",ShaderStage::Fragment,"uniform samplerCube g_Texture0;varying vec3 direction;void main(){gl_FragColor=texture(g_Texture0,direction);}",&BTreeMap::new()).unwrap();
        assert!(cube.msl.contains("texturecube<float"));
        let shadow=compile_glsl("shadow.frag",ShaderStage::Fragment,"uniform sampler2D g_Texture0;varying vec2 uv;void main(){gl_FragColor=texSample2DCompare(g_Texture0,uv,0.5);}",&BTreeMap::new()).unwrap();
        assert!(shadow.bindings.iter().any(|b| b.depth));
        assert!(shadow.bindings.iter().any(|b| b.comparison));
        assert!(shadow.msl.contains("sample_compare"));
        let depth = depth_only_material(&crate::Material {
            cpu: crate::CpuProgram::Solid { color: [1.; 4] },
            ..Default::default()
        })
        .unwrap();
        let source = &depth.fragment.unwrap().msl;
        assert!(!source.contains("[[color(0)]]"));
    }
}

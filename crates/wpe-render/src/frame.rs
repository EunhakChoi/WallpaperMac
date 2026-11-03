use crate::{Error, Result, RgbaImage};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

pub type TextureId = u64;

/// Four float4 fields, 64 bytes, 16-byte alignment. No implicit float3 padding.
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GpuVertex {
    /// Builtin/canonical VS: homogeneous clip position (0 <= z <= w).
    /// An original compiled VS instead consumes its raw local a_Position;
    /// the caller supplies that VS's model/view/projection uniforms and uses
    /// CpuProgram::GpuOnly unless it also supplies an equivalent CPU program.
    pub position: [f32; 4],
    pub normal: [f32; 4],
    /// xy = 2D UV; xyz = volume UV. Top-down image rows.
    pub uv: [f32; 4],
    pub color: [f32; 4],
}
impl Default for GpuVertex {
    fn default() -> Self {
        Self {
            position: [0., 0., 0., 1.],
            normal: [0., 0., 1., 0.],
            uv: [0.; 4],
            color: [1.; 4],
        }
    }
}
pub type GPUVertex = GpuVertex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AddressMode {
    #[default]
    Clamp,
    Repeat,
    Mirror,
    Border,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FilterMode {
    Nearest,
    #[default]
    Linear,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SamplerState {
    pub address_u: AddressMode,
    pub address_v: AddressMode,
    pub address_w: AddressMode,
    pub min_filter: FilterMode,
    pub mag_filter: FilterMode,
    pub mip_filter: FilterMode,
    pub border_color: [f32; 4],
    pub lod_min: f32,
    pub lod_max: f32,
    pub compare: Option<CompareFunction>,
}
impl Default for SamplerState {
    fn default() -> Self {
        Self {
            address_u: AddressMode::Clamp,
            address_v: AddressMode::Clamp,
            address_w: AddressMode::Clamp,
            min_filter: FilterMode::Linear,
            mag_filter: FilterMode::Linear,
            mip_filter: FilterMode::Linear,
            border_color: [0.; 4],
            lod_min: 0.,
            lod_max: 32.,
            compare: None,
        }
    }
}
#[derive(Clone, Debug)]
pub enum TextureData {
    Image2D(Arc<RgbaImage>),
    /// Top-down, linear numeric RGBA32F components. Storage format remains explicit.
    Float2D {
        width: u32,
        height: u32,
        rgba: Arc<Vec<f32>>,
    },
    Volume3D {
        width: u32,
        height: u32,
        depth: u32,
        rgba: Arc<Vec<u8>>,
    },
    /// Face order +X,-X,+Y,-Y,+Z,-Z; every face is square, top-down RGBA8.
    CubeMap {
        faces: [Arc<RgbaImage>; 6],
    },
    Depth2D {
        width: u32,
        height: u32,
        depth: Arc<Vec<f32>>,
    },
    RenderTarget {
        width: u32,
        height: u32,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TextureFormat {
    #[default]
    Rgba8Unorm,
    Bgra8Unorm,
    Rgba16Float,
    Rgba32Float,
    Rg16Float,
    R16Float,
    R32Float,
    R8Unorm,
    Depth32Float,
}
impl TextureData {
    pub fn dimensions(&self) -> [u32; 3] {
        match self {
            Self::Image2D(i) => [i.width, i.height, 1],
            Self::Float2D { width, height, .. } => [*width, *height, 1],
            Self::Volume3D {
                width,
                height,
                depth,
                ..
            } => [*width, *height, *depth],
            Self::CubeMap { faces } => [faces[0].width, faces[0].height, 1],
            Self::Depth2D { width, height, .. } | Self::RenderTarget { width, height } => {
                [*width, *height, 1]
            }
        }
    }
}
#[derive(Clone, Debug)]
pub struct Texture {
    pub name: String,
    pub data: TextureData,
    pub mips: Vec<TextureData>,
    pub sampler: SamplerState,
    pub format: TextureFormat,
}
impl Texture {
    pub fn float2d(
        name: impl Into<String>,
        width: u32,
        height: u32,
        rgba: Arc<Vec<f32>>,
        format: TextureFormat,
    ) -> Result<Self> {
        let texture = Self {
            name: name.into(),
            data: TextureData::Float2D {
                width,
                height,
                rgba,
            },
            mips: Vec::new(),
            sampler: SamplerState::default(),
            format,
        };
        texture.validate()?;
        Ok(texture)
    }
    pub fn rgba2d(name: impl Into<String>, image: Arc<RgbaImage>) -> Self {
        Self {
            name: name.into(),
            data: TextureData::Image2D(image),
            mips: Vec::new(),
            sampler: SamplerState::default(),
            format: TextureFormat::Rgba8Unorm,
        }
    }
    pub fn target(name: impl Into<String>, width: u32, height: u32) -> Self {
        Self::target_with_format(name, width, height, TextureFormat::Rgba8Unorm)
    }
    pub fn target_with_format(
        name: impl Into<String>,
        width: u32,
        height: u32,
        format: TextureFormat,
    ) -> Self {
        Self {
            name: name.into(),
            data: TextureData::RenderTarget { width, height },
            mips: Vec::new(),
            sampler: SamplerState::default(),
            format,
        }
    }
    pub fn cube(name: impl Into<String>, faces: [Arc<RgbaImage>; 6]) -> Result<Self> {
        let texture = Self {
            name: name.into(),
            data: TextureData::CubeMap { faces },
            mips: Vec::new(),
            sampler: SamplerState::default(),
            format: TextureFormat::Rgba8Unorm,
        };
        texture.validate()?;
        Ok(texture)
    }
    pub fn depth(
        name: impl Into<String>,
        width: u32,
        height: u32,
        depth: Arc<Vec<f32>>,
    ) -> Result<Self> {
        let texture = Self {
            name: name.into(),
            data: TextureData::Depth2D {
                width,
                height,
                depth,
            },
            mips: Vec::new(),
            sampler: SamplerState::default(),
            format: TextureFormat::Depth32Float,
        };
        texture.validate()?;
        Ok(texture)
    }
    pub fn validate(&self) -> Result<()> {
        self.sampler.validate()?;
        if self.mips.len() > 32 {
            return Err(Error::limit(0, "texture mip count exceeds 32"));
        }
        let mut expected = self.data.dimensions();
        let mut total_bytes = 0usize;
        for (level, data) in std::iter::once(&self.data)
            .chain(self.mips.iter())
            .enumerate()
        {
            let [w, h, d] = data.dimensions();
            if level > 0 {
                if expected == [1; 3] {
                    return Err(Error::parse(0, "mip chain continues beyond 1x1x1"));
                }
                expected = expected.map(|n| (n / 2).max(1));
                if [w, h, d] != expected
                    || std::mem::discriminant(data) != std::mem::discriminant(&self.data)
                {
                    return Err(Error::parse(
                        0,
                        "mip dimensions/type must form a halved chain",
                    ));
                }
            }
            if level > 0
                && self.format == TextureFormat::Depth32Float
                && matches!(data, TextureData::RenderTarget { .. })
            {
                return Err(Error::unsupported(
                    0,
                    "automatic depth-target mip generation is undefined",
                ));
            }
            let bytes = (w as usize)
                .checked_mul(h as usize)
                .and_then(|n| n.checked_mul(d as usize))
                .and_then(|n| n.checked_mul(4))
                .ok_or_else(|| Error::limit(0, "texture dimensions overflow"))?;
            total_bytes = total_bytes
                .checked_add(
                    bytes
                        .checked_mul(if matches!(data, TextureData::Float2D { .. }) {
                            4
                        } else if matches!(data, TextureData::CubeMap { .. }) {
                            6
                        } else {
                            1
                        })
                        .ok_or_else(|| Error::limit(0, "cube bytes overflow"))?,
                )
                .ok_or_else(|| Error::limit(0, "texture mip bytes overflow"))?;
            if total_bytes > wpe_formats::MAX_ALLOCATION {
                return Err(Error::limit(0, "texture mip allocation exceeds policy"));
            }
            if w == 0 || h == 0 || d == 0 || bytes > wpe_formats::MAX_ALLOCATION {
                return Err(Error::limit(
                    0,
                    "texture dimensions are zero or exceed allocation policy",
                ));
            }
            match data {
                TextureData::Image2D(i) => i.validate()?,
                TextureData::Float2D { rgba, .. } => {
                    if !matches!(
                        self.format,
                        TextureFormat::R16Float
                            | TextureFormat::Rg16Float
                            | TextureFormat::Rgba16Float
                            | TextureFormat::R32Float
                            | TextureFormat::Rgba32Float
                    ) {
                        return Err(Error::parse(
                            0,
                            "Float2D requires explicit floating storage format",
                        ));
                    }
                    if rgba.len() != bytes || rgba.iter().any(|v| !v.is_finite()) {
                        return Err(Error::parse(
                            0,
                            "Float2D component count/nonfinite mismatch",
                        ));
                    }
                }
                TextureData::Volume3D { rgba, .. } if rgba.len() != bytes => {
                    return Err(Error::parse(0, "volume RGBA size mismatch"));
                }
                TextureData::CubeMap { faces } => {
                    if bytes > wpe_formats::MAX_ALLOCATION / 6 {
                        return Err(Error::limit(0, "cube faces exceed allocation policy"));
                    }
                    for face in faces {
                        face.validate()?;
                        if face.width != w || face.height != w {
                            return Err(Error::parse(0, "cube faces must share square dimensions"));
                        }
                    }
                }
                TextureData::Depth2D { depth, .. }
                    if (self.format != TextureFormat::Depth32Float
                        || depth.len() != bytes / 4
                        || depth.iter().any(|d| !d.is_finite())) =>
                {
                    return Err(Error::parse(
                        0,
                        "depth texture dimensions/format/values mismatch",
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BlendState {
    Replace,
    #[default]
    Alpha,
    Premultiplied,
    Additive,
    Multiply,
    Screen,
    Custom(u8),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CompareFunction {
    Never,
    Less,
    Equal,
    #[default]
    LessEqual,
    Greater,
    NotEqual,
    GreaterEqual,
    Always,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CullMode {
    #[default]
    None,
    Front,
    Back,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PipelineState {
    pub blend: BlendState,
    pub depth_test: bool,
    pub depth_write: bool,
    pub depth_compare: CompareFunction,
    pub cull: CullMode,
    pub color_write_mask: u8,
}
impl Default for PipelineState {
    fn default() -> Self {
        Self {
            blend: BlendState::Alpha,
            depth_test: false,
            depth_write: false,
            depth_compare: CompareFunction::LessEqual,
            cull: CullMode::None,
            color_write_mask: 15,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextureBinding {
    pub name: String,
    pub slot: u32,
    pub texture: TextureId,
}
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct UniformBlock {
    pub bytes: Vec<u8>,
    pub values: BTreeMap<String, crate::shader::UniformValue>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CpuProgram {
    Textured {
        texture_slot: u32,
        color: [f32; 4],
        alpha_test: Option<f32>,
    },
    Solid {
        color: [f32; 4],
    },
    Volume {
        texture_slot: u32,
        color: [f32; 4],
    },
    Cube {
        texture_slot: u32,
        color: [f32; 4],
    },
    /// R32 shadow atlas: alpha cut first, then store post-projection depth in red.
    ShadowDepth {
        texture_slot: Option<u32>,
        alpha_test: Option<f32>,
    },
    Lambert {
        texture_slot: Option<u32>,
        light_direction: [f32; 3],
        ambient: [f32; 3],
        light_color: [f32; 3],
    },
    /// An arbitrary compiled GPU program has no invented CPU substitute.
    GpuOnly {
        reason: String,
    },
}
impl Default for CpuProgram {
    fn default() -> Self {
        Self::Textured {
            texture_slot: 0,
            color: [1.; 4],
            alpha_test: None,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Material {
    pub name: String,
    pub vertex: Option<Arc<crate::shader::CompiledShader>>,
    pub fragment: Option<Arc<crate::shader::CompiledShader>>,
    pub uniforms: UniformBlock,
    pub textures: Vec<TextureBinding>,
    pub state: PipelineState,
    pub cpu: CpuProgram,
    /// Per-binding override; two draws may use the same texture with different addressing.
    pub samplers: BTreeMap<u32, SamplerState>,
}
impl Default for Material {
    fn default() -> Self {
        Self {
            name: "unlit".into(),
            vertex: None,
            fragment: None,
            uniforms: UniformBlock::default(),
            textures: Vec::new(),
            state: PipelineState::default(),
            cpu: CpuProgram::default(),
            samplers: BTreeMap::new(),
        }
    }
}
#[derive(Clone, Debug)]
pub struct DrawCall {
    pub name: String,
    pub vertices: Vec<GpuVertex>,
    pub indices: Vec<u32>,
    pub material: Material,
    pub instances: u32,
    pub first_instance: u32,
    /// Additional/overriding original attributes, keyed by the exact shader input name.
    pub vertex_streams: BTreeMap<String, VertexStream>,
    pub instance_streams: BTreeMap<String, VertexStream>,
}
impl Default for DrawCall {
    fn default() -> Self {
        Self {
            name: String::new(),
            vertices: Vec::new(),
            indices: Vec::new(),
            material: Material::default(),
            instances: 1,
            first_instance: 0,
            vertex_streams: BTreeMap::new(),
            instance_streams: BTreeMap::new(),
        }
    }
}
#[derive(Clone, Debug)]
pub enum VertexStreamData {
    Float32(Vec<f32>),
    Uint32(Vec<u32>),
    Sint32(Vec<i32>),
}
pub type VertexData = VertexStreamData;
#[derive(Clone, Debug)]
pub struct VertexStream {
    pub components: u8,
    pub data: VertexStreamData,
}
impl VertexStream {
    pub fn float32(components: u8, data: Vec<f32>) -> Self {
        Self {
            components,
            data: VertexStreamData::Float32(data),
        }
    }
    pub fn uint32(components: u8, data: Vec<u32>) -> Self {
        Self {
            components,
            data: VertexStreamData::Uint32(data),
        }
    }
    pub fn len(&self) -> usize {
        match &self.data {
            VertexStreamData::Float32(v) => v.len(),
            VertexStreamData::Uint32(v) => v.len(),
            VertexStreamData::Sint32(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Target {
    #[default]
    Screen,
    Texture(TextureId),
}
#[derive(Clone, Debug)]
pub struct RenderPass {
    pub name: String,
    pub target: Target,
    pub clear: Option<[f32; 4]>,
    pub draws: Vec<usize>,
    pub effect: Option<crate::effect::Effect>,
    pub viewports: Vec<Viewport>,
    pub scissor: Option<[u32; 4]>,
}
impl Default for RenderPass {
    fn default() -> Self {
        Self {
            name: String::new(),
            target: Target::Screen,
            clear: None,
            draws: Vec::new(),
            effect: None,
            viewports: Vec::new(),
            scissor: None,
        }
    }
}
impl RenderPass {
    pub fn new(name: impl Into<String>, target: Target) -> Self {
        Self {
            name: name.into(),
            target,
            ..Default::default()
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub z_near: f32,
    pub z_far: f32,
}
impl Viewport {
    pub fn full(width: u32, height: u32) -> Self {
        Self {
            x: 0.,
            y: 0.,
            width: width as f32,
            height: height as f32,
            z_near: 0.,
            z_far: 1.,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapabilityIssue {
    pub code: String,
    pub resource: Option<String>,
    pub message: String,
}
#[derive(Clone, Debug)]
pub struct RenderFrame {
    pub width: u32,
    pub height: u32,
    pub clear: [f32; 4],
    pub time: f32,
    pub textures: BTreeMap<TextureId, Texture>,
    pub draws: Vec<DrawCall>,
    pub passes: Vec<RenderPass>,
    pub issues: Vec<CapabilityIssue>,
}
impl RenderFrame {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            clear: [0., 0., 0., 1.],
            time: 0.,
            textures: BTreeMap::new(),
            draws: Vec::new(),
            passes: Vec::new(),
            issues: Vec::new(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self.clear.iter().any(|v| !v.is_finite()) || !self.time.is_finite() {
            return Err(Error::parse(0, "nonfinite frame clear/time"));
        }
        if self.width == 0
            || self.height == 0
            || self.width as u64 * self.height as u64 > wpe_formats::MAX_ALLOCATION as u64 / 4
        {
            return Err(Error::limit(0, "frame dimensions exceed allocation policy"));
        }
        for texture in self.textures.values() {
            texture.validate()?;
        }
        for draw in &self.draws {
            if draw.instances > 1_000_000
                || draw.first_instance.checked_add(draw.instances).is_none()
            {
                return Err(Error::limit(
                    0,
                    "instance count exceeds policy or overflows",
                ));
            }
            if draw.indices.len() % 3 != 0
                || draw
                    .indices
                    .iter()
                    .any(|i| *i as usize >= draw.vertices.len())
            {
                return Err(Error::parse(
                    0,
                    format!("{}: invalid triangle indices", draw.name),
                ));
            }
            if draw.vertices.iter().any(|v| {
                v.position
                    .iter()
                    .chain(&v.normal)
                    .chain(&v.uv)
                    .chain(&v.color)
                    .any(|f| !f.is_finite())
            }) {
                return Err(Error::parse(0, "nonfinite vertex"));
            }
            for binding in &draw.material.textures {
                if !self.textures.contains_key(&binding.texture) {
                    return Err(Error::parse(
                        0,
                        format!("{}: missing texture {}", draw.name, binding.texture),
                    ));
                }
            }
            for sampler in draw.material.samplers.values() {
                sampler.validate()?;
            }
            if draw
                .vertex_streams
                .keys()
                .any(|name| draw.instance_streams.contains_key(name))
            {
                return Err(Error::parse(
                    0,
                    "an attribute cannot have both vertex and instance streams",
                ));
            }
            for (streams, count) in [
                (&draw.vertex_streams, draw.vertices.len()),
                (
                    &draw.instance_streams,
                    (draw.first_instance + draw.instances) as usize,
                ),
            ] {
                for (name, stream) in streams {
                    if !(1..=4).contains(&stream.components)
                        || count.checked_mul(stream.components as usize) != Some(stream.len())
                    {
                        return Err(Error::parse(
                            0,
                            format!("{}: attribute {name} component/count mismatch", draw.name),
                        ));
                    }
                    if let VertexStreamData::Float32(v) = &stream.data
                        && v.iter().any(|x| !x.is_finite())
                    {
                        return Err(Error::parse(0, "nonfinite vertex stream"));
                    }
                }
            }
        }
        for pass in &self.passes {
            if pass.draws.iter().any(|i| *i >= self.draws.len()) {
                return Err(Error::parse(0, "render pass draw index outside frame"));
            }
            if let Target::Texture(id) = pass.target
                && !matches!(
                    self.textures.get(&id).map(|t| &t.data),
                    Some(TextureData::RenderTarget { .. })
                )
            {
                return Err(Error::parse(
                    0,
                    "pass target must be declared RenderTarget texture",
                ));
            }
            if pass.effect.is_some() && !pass.draws.is_empty() {
                return Err(Error::parse(
                    0,
                    "an effect pass cannot also contain draw calls",
                ));
            }
            if let Some(effect) = &pass.effect {
                effect.validate_inputs(self)?;
                for id in effect.inputs() {
                    if pass.target == Target::Texture(id) {
                        return Err(Error::parse(
                            0,
                            "same-pass effect feedback requires separate ping-pong targets",
                        ));
                    }
                }
            }
            if let Target::Texture(id) = pass.target {
                for draw in &pass.draws {
                    if self.draws[*draw]
                        .material
                        .textures
                        .iter()
                        .any(|b| b.texture == id)
                    {
                        return Err(Error::parse(
                            0,
                            "same-pass draw feedback requires separate ping-pong targets",
                        ));
                    }
                }
            }
            if pass.viewports.len() > 16
                || pass.viewports.iter().any(|v| {
                    [v.x, v.y, v.width, v.height, v.z_near, v.z_far]
                        .iter()
                        .any(|x| !x.is_finite())
                        || v.width <= 0.
                        || v.height <= 0.
                        || v.z_near < 0.
                        || v.z_far > 1.
                        || v.z_near > v.z_far
                })
            {
                return Err(Error::parse(0, "invalid viewport array"));
            }
            if let Some([x, y, w, h]) = pass.scissor {
                let [tw, th, _] = match pass.target {
                    Target::Screen => [self.width, self.height, 1],
                    Target::Texture(id) => self.textures[&id].data.dimensions(),
                };
                if x.checked_add(w).is_none_or(|end| end > tw)
                    || y.checked_add(h).is_none_or(|end| end > th)
                {
                    return Err(Error::parse(0, "scissor rectangle exceeds target"));
                }
            }
        }
        Ok(())
    }
}
impl SamplerState {
    pub fn validate(&self) -> Result<()> {
        if !self.lod_min.is_finite()
            || !self.lod_max.is_finite()
            || self.lod_min > self.lod_max
            || self.border_color.iter().any(|v| !v.is_finite())
        {
            return Err(Error::parse(0, "invalid sampler LOD/border state"));
        }
        Ok(())
    }
}

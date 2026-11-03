use crate::TextureId;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Effect {
    Copy {
        input: TextureId,
    },
    ColorMatrix {
        input: TextureId,
        matrix: [[f32; 4]; 4],
        offset: [f32; 4],
    },
    GaussianBlur {
        input: TextureId,
        sigma: f32,
        radius: u32,
    },
    Bloom {
        input: TextureId,
        threshold: f32,
        intensity: f32,
        sigma: f32,
        radius: u32,
    },
    /// Internal separable primitive. `texel_fetch` uses target-sized integer reads.
    GaussianAxis {
        input: TextureId,
        sigma: f32,
        radius: u32,
        horizontal: bool,
        threshold: Option<f32>,
        texel_fetch: bool,
    },
    BloomComposite {
        base: TextureId,
        blur: TextureId,
        intensity: f32,
    },
    Blend {
        base: TextureId,
        layer: TextureId,
        mode: u8,
        opacity: f32,
    },
    Displacement {
        input: TextureId,
        map: TextureId,
        scale: [f32; 2],
    },
    Lut3D {
        input: TextureId,
        lut: TextureId,
        strength: f32,
    },
}

impl Effect {
    pub fn inputs(&self) -> Vec<TextureId> {
        match *self {
            Self::Copy { input }
            | Self::ColorMatrix { input, .. }
            | Self::GaussianBlur { input, .. }
            | Self::Bloom { input, .. } => vec![input],
            Self::GaussianAxis { input, .. } => vec![input],
            Self::BloomComposite { base, blur, .. } => vec![base, blur],
            Self::Blend { base, layer, .. } => vec![base, layer],
            Self::Displacement { input, map, .. } => vec![input, map],
            Self::Lut3D { input, lut, .. } => vec![input, lut],
        }
    }
    pub(crate) fn validate_inputs(&self, frame: &crate::RenderFrame) -> Result<()> {
        match self {
            Self::ColorMatrix { matrix, offset, .. }
                if matrix
                    .iter()
                    .flatten()
                    .chain(offset)
                    .any(|v| !v.is_finite()) =>
            {
                return Err(Error::parse(0, "nonfinite effect matrix"));
            }
            Self::GaussianBlur { sigma, radius, .. }
            | Self::Bloom { sigma, radius, .. }
            | Self::GaussianAxis { sigma, radius, .. } => {
                gaussian_weights(*sigma, *radius)?;
            }
            Self::Blend { mode, opacity, .. } if *mode > 32 || !opacity.is_finite() => {
                return Err(Error::parse(0, "invalid blend parameters"));
            }
            Self::Displacement { scale, .. } if scale.iter().any(|v| !v.is_finite()) => {
                return Err(Error::parse(0, "nonfinite displacement scale"));
            }
            Self::Lut3D { strength, .. } if !strength.is_finite() => {
                return Err(Error::parse(0, "nonfinite LUT strength"));
            }
            _ => {}
        }
        if matches!(self,Self::Bloom{threshold,intensity,..}if !threshold.is_finite()||!intensity.is_finite())
            || matches!(self,Self::BloomComposite{intensity,..}if !intensity.is_finite())
            || matches!(self,Self::GaussianAxis{threshold:Some(value),..}if !value.is_finite())
        {
            return Err(Error::parse(0, "nonfinite bloom/threshold parameter"));
        }
        for (slot, id) in self.inputs().iter().enumerate() {
            let texture = frame
                .textures
                .get(id)
                .ok_or_else(|| Error::parse(0, "missing effect texture"))?;
            texture.validate()?;
            let valid = if matches!(self, Self::Lut3D { .. }) && slot == 1 {
                matches!(texture.data, crate::TextureData::Volume3D { .. })
            } else {
                matches!(
                    texture.data,
                    crate::TextureData::Image2D(_)
                        | crate::TextureData::Float2D { .. }
                        | crate::TextureData::RenderTarget { .. }
                ) && texture.format != crate::TextureFormat::Depth32Float
            };
            if !valid {
                return Err(Error::unsupported(
                    0,
                    "color effect requires a 2D color input; LUT slot 1 requires a 3D volume",
                ));
            }
        }
        Ok(())
    }
}

use crate::{Error, Result, cpu::Surface};
pub(crate) fn evaluate(
    effect: &Effect,
    width: u32,
    height: u32,
    sample: &dyn Fn(TextureId, [f32; 3]) -> Result<[f32; 4]>,
    dimensions: &dyn Fn(TextureId) -> Option<[u32; 3]>,
) -> Result<Vec<[f32; 4]>> {
    let mut surface = Surface::new(width, height, [0.; 4])?;
    let uv = |x: u32, y: u32| {
        [
            (x as f32 + 0.5) / width as f32,
            (y as f32 + 0.5) / height as f32,
            0.,
        ]
    };
    match effect {
        Effect::GaussianAxis {
            input,
            sigma,
            radius,
            horizontal,
            threshold,
            texel_fetch,
        } => {
            if *texel_fetch && dimensions(*input) != Some([width, height, 1]) {
                return Err(Error::parse(
                    0,
                    "Gaussian texel-fetch input must match target size",
                ));
            }
            if threshold.is_some_and(|v| !v.is_finite()) {
                return Err(Error::parse(0, "nonfinite Gaussian threshold"));
            }
            let weights = gaussian_weights(*sigma, *radius)?;
            for y in 0..height {
                for x in 0..width {
                    let mut out = [0.; 4];
                    for (i, weight) in weights.iter().enumerate() {
                        let delta = i as i64 - *radius as i64;
                        let xx = if *horizontal {
                            (x as i64 + delta).clamp(0, width as i64 - 1) as u32
                        } else {
                            x
                        };
                        let yy = if *horizontal {
                            y
                        } else {
                            (y as i64 + delta).clamp(0, height as i64 - 1) as u32
                        };
                        let mut color = sample(*input, uv(xx, yy))?;
                        if let Some(threshold) = threshold {
                            let l = color[0] * 0.2126 + color[1] * 0.7152 + color[2] * 0.0722;
                            let amount = if l > 0. {
                                ((l - *threshold) / l).max(0.)
                            } else {
                                0.
                            };
                            for c in &mut color[..3] {
                                *c *= amount;
                            }
                            color[3] = 0.;
                        }
                        for c in 0..4 {
                            out[c] += color[c] * *weight;
                        }
                    }
                    surface.pixels[y as usize * width as usize + x as usize] = out;
                }
            }
        }
        Effect::GaussianBlur {
            input,
            sigma,
            radius,
        }
        | Effect::Bloom {
            input,
            sigma,
            radius,
            ..
        } => {
            for y in 0..height {
                for x in 0..width {
                    surface.pixels[y as usize * width as usize + x as usize] =
                        sample(*input, uv(x, y))?;
                }
            }
            let original = surface.pixels.clone();
            if let Effect::Bloom { threshold, .. } = effect {
                for p in &mut surface.pixels {
                    let l = p[0] * 0.2126 + p[1] * 0.7152 + p[2] * 0.0722;
                    let scale = if l > 0. {
                        ((l - *threshold) / l).max(0.)
                    } else {
                        0.
                    };
                    for c in &mut p[..3] {
                        *c *= scale;
                    }
                    p[3] = 0.;
                }
            }
            let weights = gaussian_weights(*sigma, *radius)?;
            let mut temp = surface.pixels.clone();
            for horizontal in [true, false] {
                for y in 0..height {
                    for x in 0..width {
                        let mut out = [0.; 4];
                        for (i, w) in weights.iter().enumerate() {
                            let delta = i as i64 - *radius as i64;
                            let xx = if horizontal {
                                (x as i64 + delta).clamp(0, width as i64 - 1) as u32
                            } else {
                                x
                            };
                            let yy = if horizontal {
                                y
                            } else {
                                (y as i64 + delta).clamp(0, height as i64 - 1) as u32
                            };
                            let p = surface.pixels[yy as usize * width as usize + xx as usize];
                            for c in 0..4 {
                                out[c] += p[c] * *w;
                            }
                        }
                        temp[y as usize * width as usize + x as usize] = out;
                    }
                }
                std::mem::swap(&mut surface.pixels, &mut temp);
            }
            if let Effect::Bloom { intensity, .. } = effect {
                for (p, src) in surface.pixels.iter_mut().zip(original) {
                    for c in 0..3 {
                        p[c] = src[c] + p[c] * *intensity;
                    }
                    p[3] = src[3];
                }
            }
        }
        _ => {
            for y in 0..height {
                for x in 0..width {
                    let texcoord = uv(x, y);
                    let value = match effect {
                        Effect::Copy { input } => sample(*input, texcoord)?,
                        Effect::BloomComposite {
                            base,
                            blur,
                            intensity,
                        } => {
                            if dimensions(*blur) != Some([width, height, 1]) {
                                return Err(Error::parse(
                                    0,
                                    "bloom composite blur must match target size",
                                ));
                            }
                            if !intensity.is_finite() {
                                return Err(Error::parse(0, "nonfinite bloom intensity"));
                            }
                            let a = sample(*base, texcoord)?;
                            let b = sample(*blur, texcoord)?;
                            [
                                a[0] + b[0] * *intensity,
                                a[1] + b[1] * *intensity,
                                a[2] + b[2] * *intensity,
                                a[3],
                            ]
                        }
                        Effect::ColorMatrix {
                            input,
                            matrix,
                            offset,
                        } => {
                            let s = sample(*input, texcoord)?;
                            std::array::from_fn(|row| {
                                (0..4).map(|c| matrix[row][c] * s[c]).sum::<f32>() + offset[row]
                            })
                        }
                        Effect::Blend {
                            base,
                            layer,
                            mode,
                            opacity,
                        } => {
                            let a = sample(*base, texcoord)?;
                            let b = sample(*layer, texcoord)?;
                            let rgb = crate::blend::apply(
                                *mode,
                                [a[0], a[1], a[2]],
                                [b[0], b[1], b[2]],
                                b[3] * *opacity,
                            )?;
                            [rgb[0], rgb[1], rgb[2], a[3]]
                        }
                        Effect::Displacement { input, map, scale } => {
                            let d = sample(*map, texcoord)?;
                            sample(
                                *input,
                                [
                                    texcoord[0] + (d[0] * 2. - 1.) * scale[0],
                                    texcoord[1] + (d[1] * 2. - 1.) * scale[1],
                                    0.,
                                ],
                            )?
                        }
                        Effect::Lut3D {
                            input,
                            lut,
                            strength,
                        } => {
                            let s = sample(*input, texcoord)?;
                            let dims = dimensions(*lut)
                                .ok_or_else(|| Error::parse(0, "missing LUT texture"))?;
                            let coord = std::array::from_fn(|i| {
                                (s[i].clamp(0., 1.) * (dims[i] - 1) as f32 + 0.5) / dims[i] as f32
                            });
                            let l = sample(*lut, coord)?;
                            [
                                s[0] + (l[0] - s[0]) * *strength,
                                s[1] + (l[1] - s[1]) * *strength,
                                s[2] + (l[2] - s[2]) * *strength,
                                s[3],
                            ]
                        }
                        _ => unreachable!(),
                    };
                    surface.pixels[y as usize * width as usize + x as usize] = value;
                }
            }
        }
    }
    Ok(surface.pixels)
}
pub fn gaussian_weights(sigma: f32, radius: u32) -> Result<Vec<f32>> {
    if !sigma.is_finite() || sigma <= 0. || radius > 256 {
        return Err(Error::limit(
            0,
            "Gaussian sigma must be positive/finite and radius <=256",
        ));
    }
    let mut weights = (0..radius * 2 + 1)
        .map(|i| {
            let d = i as f32 - radius as f32;
            let q = d / sigma;
            (-0.5 * q * q).exp()
        })
        .collect::<Vec<_>>();
    let total = weights.iter().sum::<f32>();
    for w in &mut weights {
        *w /= total;
    }
    Ok(weights)
}

/// Compile a fullscreen effect to real MSL, with reflected bindings and an indexed quad.
/// Shader code is cached per thread; changing numeric values updates uniform data.
pub fn draw_call(effect: &Effect, frame: &crate::RenderFrame) -> Result<crate::DrawCall> {
    draw_call_sized(effect, frame, frame.width, frame.height)
}
type Program = (
    std::sync::Arc<crate::shader::CompiledShader>,
    std::sync::Arc<crate::shader::CompiledShader>,
);
thread_local! {static PROGRAMS:std::cell::RefCell<std::collections::BTreeMap<String,Program>>=const{std::cell::RefCell::new(std::collections::BTreeMap::new())};}
fn program(source: String) -> Result<Program> {
    if let Some(program) = PROGRAMS.with(|cache| cache.borrow().get(&source).cloned()) {
        return Ok(program);
    }
    let mut library = crate::shader::SourceLibrary::new();
    library.insert("effect.vert", CANONICAL_VERTEX);
    library.insert("effect.frag", source.clone());
    let (v, f) = crate::shader::ShaderCompiler::new(library).compile_pair(
        "effect.vert",
        "effect.frag",
        &std::collections::BTreeMap::new(),
    )?;
    let result = (std::sync::Arc::new(v), std::sync::Arc::new(f));
    PROGRAMS.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= 128 {
            cache.clear();
        }
        cache.insert(source, result.clone());
    });
    Ok(result)
}
fn kernel_values(
    values: &mut std::collections::BTreeMap<String, crate::shader::UniformValue>,
    sigma: f32,
    radius: u32,
    size: [u32; 2],
) -> Result<()> {
    use crate::shader::UniformValue as U;
    values.insert("g_EffectRadius".into(), U::Int(radius as i32));
    values.insert(
        "g_EffectWeights".into(),
        U::Array(
            gaussian_weights(sigma, radius)?
                .into_iter()
                .map(U::Float)
                .collect(),
        ),
    );
    values.insert(
        "g_EffectSize".into(),
        U::Vector(vec![size[0] as f32, size[1] as f32]),
    );
    Ok(())
}
const KERNEL_UNIFORMS: &str = "uniform int g_EffectRadius;uniform float g_EffectWeights[513];uniform vec2 g_EffectSize;uniform int g_EffectThresholdEnabled;uniform float g_EffectThreshold;\nvec4 wpe_prefilter(vec4 value){if(g_EffectThresholdEnabled!=0){float luminance=dot(value.rgb,vec3(0.2126,0.7152,0.0722));value.rgb*=luminance>0.0?max((luminance-g_EffectThreshold)/luminance,0.0):0.0;value.a=0.0;}return value;}\n";
pub fn draw_call_sized(
    effect: &Effect,
    frame: &crate::RenderFrame,
    target_width: u32,
    target_height: u32,
) -> Result<crate::DrawCall> {
    use crate::shader::UniformValue as U;
    use crate::{
        BlendState, CpuProgram, DrawCall, GpuVertex, Material, PipelineState, TextureBinding,
        UniformBlock,
    };
    use std::collections::BTreeMap;
    effect.validate_inputs(frame)?;
    if target_width == 0 || target_height == 0 {
        return Err(Error::parse(0, "effect target dimensions must be nonzero"));
    }
    let inputs = effect.inputs();
    let mut values = BTreeMap::new();
    let mut source = String::from("uniform sampler2D g_Texture0;varying vec4 v_TexCoord;\n");
    if inputs.len() > 1 {
        source.push_str(if matches!(effect, Effect::Lut3D { .. }) {
            "uniform sampler3D g_Texture1;\n"
        } else {
            "uniform sampler2D g_Texture1;\n"
        });
    }
    let body = match effect {
        Effect::Copy { .. } => "gl_FragColor=textureLod(g_Texture0,v_TexCoord.xy,0.0);".into(),
        Effect::ColorMatrix { matrix, offset, .. } => {
            if matrix
                .iter()
                .flatten()
                .chain(offset)
                .any(|v| !v.is_finite())
            {
                return Err(Error::parse(0, "nonfinite effect matrix"));
            }
            source.push_str("uniform mat4 g_EffectMatrix;uniform vec4 g_EffectOffset;\n");
            values.insert(
                "g_EffectMatrix".into(),
                U::Matrix {
                    columns: 4,
                    rows: 4,
                    values: (0..4)
                        .flat_map(|c| (0..4).map(move |r| matrix[r][c]))
                        .collect(),
                },
            );
            values.insert("g_EffectOffset".into(), U::Vector(offset.to_vec()));
            "gl_FragColor=g_EffectMatrix*textureLod(g_Texture0,v_TexCoord.xy,0.0)+g_EffectOffset;"
                .into()
        }
        Effect::GaussianBlur { sigma, radius, .. } | Effect::Bloom { sigma, radius, .. } => {
            kernel_values(&mut values, *sigma, *radius, [target_width, target_height])?;
            source.push_str(KERNEL_UNIFORMS);
            source.push_str("uniform int g_EffectBloom;uniform float g_EffectIntensity;\n");
            let (threshold, intensity, bloom) = if let Effect::Bloom {
                threshold,
                intensity,
                ..
            } = effect
            {
                (*threshold, *intensity, 1)
            } else {
                (0., 0., 0)
            };
            if !threshold.is_finite() || !intensity.is_finite() {
                return Err(Error::parse(0, "nonfinite bloom parameter"));
            }
            values.insert("g_EffectThresholdEnabled".into(), U::Int(bloom));
            values.insert("g_EffectThreshold".into(), U::Float(threshold));
            values.insert("g_EffectBloom".into(), U::Int(bloom));
            values.insert("g_EffectIntensity".into(), U::Float(intensity));
            "vec4 sum=vec4(0.0);for(int y=-g_EffectRadius;y<=g_EffectRadius;y++){for(int x=-g_EffectRadius;x<=g_EffectRadius;x++){vec2 uv=clamp(v_TexCoord.xy+vec2(x,y)/g_EffectSize,vec2(0.5)/g_EffectSize,vec2(1.0)-vec2(0.5)/g_EffectSize);sum+=wpe_prefilter(textureLod(g_Texture0,uv,0.0))*g_EffectWeights[x+g_EffectRadius]*g_EffectWeights[y+g_EffectRadius];}}if(g_EffectBloom!=0){vec4 base=textureLod(g_Texture0,v_TexCoord.xy,0.0);gl_FragColor=vec4(base.rgb+sum.rgb*g_EffectIntensity,base.a);}else{gl_FragColor=sum;}".into()
        }
        Effect::GaussianAxis {
            input,
            sigma,
            radius,
            horizontal,
            threshold,
            texel_fetch,
        } => {
            kernel_values(&mut values, *sigma, *radius, [target_width, target_height])?;
            if threshold.is_some_and(|v| !v.is_finite()) {
                return Err(Error::parse(0, "nonfinite Gaussian threshold"));
            }
            if *texel_fetch
                && frame.textures[input].data.dimensions() != [target_width, target_height, 1]
            {
                return Err(Error::parse(
                    0,
                    "Gaussian texel-fetch input must match target size",
                ));
            }
            source.push_str(KERNEL_UNIFORMS);
            source.push_str("uniform int g_EffectHorizontal;\n");
            values.insert("g_EffectHorizontal".into(), U::Int(i32::from(*horizontal)));
            values.insert(
                "g_EffectThresholdEnabled".into(),
                U::Int(i32::from(threshold.is_some())),
            );
            values.insert(
                "g_EffectThreshold".into(),
                U::Float(threshold.unwrap_or(0.)),
            );
            let fetch = if *texel_fetch {
                "ivec2 at=clamp(ivec2(gl_FragCoord.xy)+step,ivec2(0),ivec2(g_EffectSize)-ivec2(1));vec4 value=texelFetch(g_Texture0,at,0);"
            } else {
                "vec2 uv=clamp(v_TexCoord.xy+vec2(step)/g_EffectSize,vec2(0.5)/g_EffectSize,vec2(1.0)-vec2(0.5)/g_EffectSize);vec4 value=textureLod(g_Texture0,uv,0.0);"
            };
            format!(
                "vec4 sum=vec4(0.0);for(int d=-g_EffectRadius;d<=g_EffectRadius;d++){{ivec2 step=g_EffectHorizontal!=0?ivec2(d,0):ivec2(0,d);{fetch}sum+=wpe_prefilter(value)*g_EffectWeights[d+g_EffectRadius];}}gl_FragColor=sum;"
            )
        }
        Effect::BloomComposite { intensity, .. } => {
            if frame.textures[&inputs[1]].data.dimensions() != [target_width, target_height, 1] {
                return Err(Error::parse(
                    0,
                    "bloom composite blur must match target size",
                ));
            }
            if !intensity.is_finite() {
                return Err(Error::parse(0, "nonfinite bloom intensity"));
            }
            source.push_str("uniform float g_EffectIntensity;\n");
            values.insert("g_EffectIntensity".into(), U::Float(*intensity));
            "vec4 base=textureLod(g_Texture0,v_TexCoord.xy,0.0);vec4 blur=texelFetch(g_Texture1,ivec2(gl_FragCoord.xy),0);gl_FragColor=vec4(base.rgb+blur.rgb*g_EffectIntensity,base.a);".into()
        }
        Effect::Blend { mode, opacity, .. } => {
            if !opacity.is_finite() {
                return Err(Error::parse(0, "nonfinite blend opacity"));
            }
            source.push_str(&crate::blend::glsl(*mode)?);
            source.push_str("uniform float g_EffectOpacity;\n");
            values.insert("g_EffectOpacity".into(), U::Float(*opacity));
            "vec4 a=textureLod(g_Texture0,v_TexCoord.xy,0.0),b=textureLod(g_Texture1,v_TexCoord.xy,0.0);gl_FragColor=vec4(wpe_blend(a.rgb,b.rgb,b.a*g_EffectOpacity),a.a);".into()
        }
        Effect::Displacement { scale, .. } => {
            if scale.iter().any(|v| !v.is_finite()) {
                return Err(Error::parse(0, "nonfinite displacement scale"));
            }
            source.push_str("uniform vec2 g_EffectScale;\n");
            values.insert("g_EffectScale".into(), U::Vector(scale.to_vec()));
            "vec4 d=textureLod(g_Texture1,v_TexCoord.xy,0.0);gl_FragColor=textureLod(g_Texture0,v_TexCoord.xy+(d.rg*2.0-1.0)*g_EffectScale,0.0);".into()
        }
        Effect::Lut3D { lut, strength, .. } => {
            if !strength.is_finite() {
                return Err(Error::parse(0, "nonfinite LUT strength"));
            }
            let dims = frame.textures[lut].data.dimensions();
            source.push_str("uniform vec3 g_EffectLutSize;uniform float g_EffectStrength;\n");
            values.insert(
                "g_EffectLutSize".into(),
                U::Vector(dims.map(|n| n as f32).to_vec()),
            );
            values.insert("g_EffectStrength".into(), U::Float(*strength));
            "vec4 c=textureLod(g_Texture0,v_TexCoord.xy,0.0);vec3 uv=(clamp(c.rgb,0.0,1.0)*(g_EffectLutSize-1.0)+0.5)/g_EffectLutSize;vec3 l=textureLod(g_Texture1,uv,0.0).rgb;gl_FragColor=vec4(mix(c.rgb,l,g_EffectStrength),c.a);".into()
        }
    };
    source.push_str("void main(){");
    source.push_str(&body);
    source.push('}');
    let (v, f) = program(source)?;
    let vertices = [
        ([-1., 1., 0., 1.], [0., 0., 0., 0.]),
        ([1., 1., 0., 1.], [1., 0., 0., 0.]),
        ([-1., -1., 0., 1.], [0., 1., 0., 0.]),
        ([1., -1., 0., 1.], [1., 1., 0., 0.]),
    ]
    .into_iter()
    .map(|(position, uv)| GpuVertex {
        position,
        uv,
        ..Default::default()
    })
    .collect();
    Ok(DrawCall {
        name: "builtin-effect".into(),
        vertices,
        indices: vec![0, 2, 1, 1, 2, 3],
        material: Material {
            name: "builtin-effect".into(),
            vertex: Some(v),
            fragment: Some(f),
            uniforms: UniformBlock {
                values,
                ..Default::default()
            },
            textures: inputs
                .into_iter()
                .enumerate()
                .map(|(slot, texture)| TextureBinding {
                    name: format!("g_Texture{slot}"),
                    slot: slot as u32,
                    texture,
                })
                .collect(),
            state: PipelineState {
                blend: BlendState::Replace,
                ..Default::default()
            },
            cpu: CpuProgram::GpuOnly {
                reason: "Execute the containing Effect pass on the CPU reference graph.".into(),
            },
            ..Default::default()
        },
        ..Default::default()
    })
}

/// Expand owned full-target Gaussian/bloom primitives into linear-size kernels.
/// Intermediate RGBA32F storage is fetched by integer coordinates (no float filtering).
pub fn lower_separable(input: &crate::RenderFrame) -> Result<crate::RenderFrame> {
    use crate::{FilterMode, RenderPass, Target, Texture, TextureFormat};
    input.validate()?;
    let mut frame = input.clone();
    let mut used = frame
        .textures
        .keys()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let mut next = 0u64;
    let mut allocate = || -> Result<u64> {
        loop {
            next = next
                .checked_add(1)
                .ok_or_else(|| Error::limit(0, "effect texture ID overflow"))?;
            if used.insert(next) {
                return Ok(next);
            }
        }
    };
    let passes = std::mem::take(&mut frame.passes);
    let mut output = Vec::new();
    for (pass_index, mut pass) in passes.into_iter().enumerate() {
        let params = match pass.effect {
            Some(Effect::GaussianBlur {
                input,
                sigma,
                radius,
            }) => Some((input, sigma, radius, None)),
            Some(Effect::Bloom {
                input,
                sigma,
                radius,
                threshold,
                intensity,
            }) => Some((input, sigma, radius, Some((threshold, intensity)))),
            _ => None,
        };
        let Some((source, sigma, radius, bloom)) = params else {
            output.push(pass);
            continue;
        };
        let [w, h, _] = match pass.target {
            Target::Screen => [frame.width, frame.height, 1],
            Target::Texture(id) => frame.textures[&id].data.dimensions(),
        };
        if pass
            .viewports
            .iter()
            .any(|v| v.x != 0. || v.y != 0. || v.width != w as f32 || v.height != h as f32)
        {
            output.push(pass);
            continue;
        }
        gaussian_weights(sigma, radius)?;
        if bloom.is_some_and(|(t, i)| !t.is_finite() || !i.is_finite()) {
            return Err(Error::parse(0, "nonfinite bloom parameters"));
        }
        let first = allocate()?;
        let mut target = Texture::target_with_format(
            format!("builtin-filter-{pass_index}-horizontal"),
            w,
            h,
            TextureFormat::Rgba32Float,
        );
        target.sampler.min_filter = FilterMode::Nearest;
        target.sampler.mag_filter = FilterMode::Nearest;
        frame.textures.insert(first, target.clone());
        output.push(RenderPass {
            name: format!("{}/horizontal", pass.name),
            target: Target::Texture(first),
            effect: Some(Effect::GaussianAxis {
                input: source,
                sigma,
                radius,
                horizontal: true,
                threshold: bloom.map(|x| x.0),
                texel_fetch: false,
            }),
            clear: Some([0.; 4]),
            ..Default::default()
        });
        if let Some((_, intensity)) = bloom {
            let second = allocate()?;
            target.name = format!("builtin-filter-{pass_index}-vertical");
            frame.textures.insert(second, target);
            output.push(RenderPass {
                name: format!("{}/vertical", pass.name),
                target: Target::Texture(second),
                effect: Some(Effect::GaussianAxis {
                    input: first,
                    sigma,
                    radius,
                    horizontal: false,
                    threshold: None,
                    texel_fetch: true,
                }),
                clear: Some([0.; 4]),
                ..Default::default()
            });
            pass.effect = Some(Effect::BloomComposite {
                base: source,
                blur: second,
                intensity,
            });
        } else {
            pass.effect = Some(Effect::GaussianAxis {
                input: first,
                sigma,
                radius,
                horizontal: false,
                threshold: None,
                texel_fetch: true,
            });
        }
        output.push(pass);
    }
    frame.passes = output;
    frame.validate()?;
    Ok(frame)
}

const CANONICAL_VERTEX: &str = "attribute vec4 a_Position;attribute vec4 a_Normal;attribute vec4 a_TexCoord;attribute vec4 a_Color;varying vec4 v_TexCoord;varying vec4 v_Color;void main(){gl_Position=a_Position;v_TexCoord=a_TexCoord;v_Color=a_Color;}";

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_all_blend_effect_msl_and_gaussian() {
        use std::sync::Arc;
        let mut frame = crate::RenderFrame::new(8, 8);
        let image = Arc::new(crate::RgbaImage::new(1, 1, vec![255; 4]).unwrap());
        frame
            .textures
            .insert(1, crate::Texture::rgba2d("base", image.clone()));
        frame
            .textures
            .insert(2, crate::Texture::rgba2d("layer", image));
        for mode in 0..=32 {
            let draw = draw_call(
                &Effect::Blend {
                    base: 1,
                    layer: 2,
                    mode,
                    opacity: 0.5,
                },
                &frame,
            )
            .unwrap();
            assert!(draw.material.fragment.unwrap().msl.contains("fragment"));
        }
        assert!(
            draw_call(
                &Effect::GaussianBlur {
                    input: 1,
                    sigma: 1.,
                    radius: 2
                },
                &frame
            )
            .is_ok()
        );
    }
    #[test]
    fn separable_graph_matches_cpu_hdr_scissor_and_caches_dynamic_uniforms() {
        use crate::*;
        use std::sync::Arc;
        let mut frame = RenderFrame::new(5, 3);
        let rgba = (0..9)
            .flat_map(|i| {
                if i == 4 {
                    [4., 2., 1., 0.5]
                } else {
                    [0.1, 0.2, 0.3, 1.]
                }
            })
            .collect();
        frame.textures.insert(
            9,
            Texture::float2d("HDR", 3, 3, Arc::new(rgba), TextureFormat::Rgba32Float).unwrap(),
        );
        for effect in [
            Effect::GaussianBlur {
                input: 9,
                sigma: 0.8,
                radius: 2,
            },
            Effect::Bloom {
                input: 9,
                threshold: 0.3,
                intensity: 0.7,
                sigma: 0.8,
                radius: 2,
            },
        ] {
            frame.passes = vec![RenderPass {
                effect: Some(effect),
                clear: Some([0.2, 0.1, 0.3, 1.]),
                scissor: Some([1, 0, 3, 3]),
                ..Default::default()
            }];
            let reference = cpu::render(&frame).unwrap();
            let lowered = lower_separable(&frame).unwrap();
            assert!(lowered.passes.len() >= 2);
            let actual = cpu::render(&lowered).unwrap();
            assert!(
                actual
                    .rgba
                    .iter()
                    .zip(reference.rgba)
                    .all(|(a, b)| a.abs_diff(b) <= 1)
            );
            for pass in &lowered.passes {
                let draw = draw_call_sized(pass.effect.as_ref().unwrap(), &lowered, 5, 3).unwrap();
                let shader = draw.material.fragment.as_ref().unwrap();
                assert!(shader.msl.contains("fragment"));
                assert!(shader.pack_uniforms(&draw.material.uniforms.values).is_ok());
            }
        }
        let a = draw_call(
            &Effect::GaussianBlur {
                input: 9,
                sigma: 1.,
                radius: 2,
            },
            &frame,
        )
        .unwrap();
        let b = draw_call(
            &Effect::GaussianBlur {
                input: 9,
                sigma: 2.,
                radius: 5,
            },
            &frame,
        )
        .unwrap();
        assert!(Arc::ptr_eq(
            a.material.fragment.as_ref().unwrap(),
            b.material.fragment.as_ref().unwrap()
        ));
        assert!(
            a.material
                .fragment
                .unwrap()
                .pack_uniforms(&a.material.uniforms.values)
                .unwrap()
                != b.material
                    .fragment
                    .unwrap()
                    .pack_uniforms(&b.material.uniforms.values)
                    .unwrap()
        );
        for sigma in [f32::from_bits(1), f32::MIN_POSITIVE, f32::MAX] {
            let weights = gaussian_weights(sigma, 256).unwrap();
            assert!(weights.iter().all(|w| w.is_finite()));
            assert!((weights.iter().sum::<f32>() - 1.).abs() < 1e-5);
        }
    }
}

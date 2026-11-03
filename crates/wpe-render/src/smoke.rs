//! Owned portable fixtures shared by CPU tests and physical Mac GPU smoke.
use crate::shader::{ShaderCompiler, SourceLibrary, UniformValue};
use crate::*;
use std::{collections::BTreeMap, sync::Arc};
fn quad(rect: [f32; 4], canvas: [u32; 2], z: f32, uv: [f32; 2], material: Material) -> DrawCall {
    let [x, y, w, h] = rect;
    let fw = canvas[0] as f32;
    let fh = canvas[1] as f32;
    let vertices = [
        ([x, y], [0., 0.]),
        ([x + w, y], [uv[0], 0.]),
        ([x, y + h], [0., uv[1]]),
        ([x + w, y + h], uv),
    ]
    .into_iter()
    .map(|(p, uv)| GpuVertex {
        position: [p[0] / fw * 2. - 1., 1. - p[1] / fh * 2., z, 1.],
        uv: [uv[0], uv[1], 0., 0.],
        ..Default::default()
    })
    .collect();
    DrawCall {
        vertices,
        indices: vec![0, 2, 1, 1, 2, 3],
        material,
        ..Default::default()
    }
}
fn solid(color: [f32; 4], blend: BlendState) -> Material {
    Material {
        cpu: CpuProgram::Solid { color },
        state: PipelineState {
            blend,
            ..Default::default()
        },
        ..Default::default()
    }
}
fn textured(id: u64, color: [f32; 4]) -> Material {
    Material {
        textures: vec![TextureBinding {
            name: "g_Texture0".into(),
            slot: 0,
            texture: id,
        }],
        cpu: CpuProgram::Textured {
            texture_slot: 0,
            color,
            alpha_test: None,
        },
        state: PipelineState {
            blend: BlendState::Replace,
            ..Default::default()
        },
        ..Default::default()
    }
}
fn add_pass(
    frame: &mut RenderFrame,
    name: &str,
    target: Target,
    draws: Vec<DrawCall>,
    clear: Option<[f32; 4]>,
) {
    let first = frame.draws.len();
    let count = draws.len();
    frame.draws.extend(draws);
    frame.passes.push(RenderPass {
        name: name.into(),
        target,
        draws: (first..first + count).collect(),
        clear,
        ..Default::default()
    });
}
pub fn indexed_frame() -> std::result::Result<RenderFrame, String> {
    let mut f = RenderFrame::new(160, 112);
    f.clear = [0.05, 0.05, 0.05, 1.];
    let mut checker = Texture::rgba2d(
        "owned checker",
        Arc::new(
            RgbaImage::new(
                2,
                2,
                vec![
                    255, 0, 0, 255, 0, 0, 255, 255, 0, 0, 255, 255, 255, 0, 0, 255,
                ],
            )
            .map_err(|e| e.to_string())?,
        ),
    );
    checker.sampler = SamplerState {
        address_u: AddressMode::Repeat,
        address_v: AddressMode::Repeat,
        min_filter: FilterMode::Nearest,
        mag_filter: FilterMode::Nearest,
        mip_filter: FilterMode::Nearest,
        ..Default::default()
    };
    f.textures.insert(1, checker);
    let mut hdr =
        Texture::target_with_format("owned HDR mip target", 64, 64, TextureFormat::Rgba16Float);
    hdr.mips = (0..6)
        .map(|i| TextureData::RenderTarget {
            width: 32 >> i,
            height: 32 >> i,
        })
        .collect();
    hdr.sampler = SamplerState {
        min_filter: FilterMode::Nearest,
        mag_filter: FilterMode::Nearest,
        mip_filter: FilterMode::Nearest,
        ..Default::default()
    };
    f.textures.insert(2, hdr);
    f.textures.insert(
        3,
        Texture::float2d(
            "owned signed RG16F",
            1,
            1,
            Arc::new(vec![-1., 2., 0., 1.]),
            TextureFormat::Rg16Float,
        )
        .map_err(|e| e.to_string())?,
    );
    add_pass(
        &mut f,
        "generate HDR/checker mips",
        Target::Texture(2),
        vec![quad(
            [0., 0., 64., 64.],
            [64, 64],
            0.,
            [32., 32.],
            textured(1, [2., 1., 1., 1.]),
        )],
        Some([0.; 4]),
    );
    let clear = f.clear;
    add_pass(
        &mut f,
        "sample mip/HDR/signed source",
        Target::Screen,
        vec![
            quad(
                [8., 8., 32., 32.],
                [160, 112],
                0.,
                [1., 1.],
                textured(2, [0.5, 1., 1., 1.]),
            ),
            quad(
                [48., 8., 32., 32.],
                [160, 112],
                0.,
                [1., 1.],
                textured(3, [-0.5, 0.25, 0., 1.]),
            ),
        ],
        Some(clear),
    );
    let mut front = solid([0., 1., 0., 1.], BlendState::Replace);
    front.state.depth_test = true;
    front.state.depth_write = true;
    let mut back = front.clone();
    back.cpu = CpuProgram::Solid {
        color: [1., 0., 0., 1.],
    };
    add_pass(
        &mut f,
        "depth front",
        Target::Screen,
        vec![quad([88., 8., 32., 32.], [160, 112], 0.25, [1., 1.], front)],
        None,
    );
    add_pass(
        &mut f,
        "depth back rejection",
        Target::Screen,
        vec![quad([88., 8., 32., 32.], [160, 112], 0.75, [1., 1.], back)],
        None,
    );
    add_pass(
        &mut f,
        "alpha source over",
        Target::Screen,
        vec![
            quad(
                [8., 56., 32., 32.],
                [160, 112],
                0.,
                [1., 1.],
                solid([1., 0., 0., 0.5], BlendState::Alpha),
            ),
            quad(
                [8., 56., 32., 32.],
                [160, 112],
                0.,
                [1., 1.],
                solid([0., 0., 1., 0.5], BlendState::Alpha),
            ),
        ],
        None,
    );
    add_pass(
        &mut f,
        "destination custom blend",
        Target::Screen,
        vec![
            quad(
                [48., 56., 32., 32.],
                [160, 112],
                0.,
                [1., 1.],
                solid([0.25, 0.5, 0.75, 1.], BlendState::Replace),
            ),
            quad(
                [48., 56., 32., 32.],
                [160, 112],
                0.,
                [1., 1.],
                solid([0.5, 0.2, 0.1, 0.5], BlendState::Custom(32)),
            ),
        ],
        None,
    );
    let mut bones = SourceLibrary::new();
    bones.insert("skin.vert",r#"
        uniform mat4x3 g_Bones[2];uniform float g_BonesAlpha[2];
        attribute vec3 a_Position;attribute uvec4 a_BlendIndices;attribute vec4 a_BlendWeights;
        varying float v_Alpha;
        void main(){mat4x3 identity=g_Bones[a_BlendIndices.x]*a_BlendWeights.x+g_Bones[a_BlendIndices.y]*a_BlendWeights.y;
        gl_Position=vec4(identity*vec4(a_Position,1.),1.);
        v_Alpha=g_BonesAlpha[a_BlendIndices.x]*a_BlendWeights.x+g_BonesAlpha[a_BlendIndices.y]*a_BlendWeights.y;}
    "#);
    bones.insert(
        "skin.frag",
        "varying float v_Alpha;void main(){gl_FragColor=vec4(1.,0.,0.,v_Alpha);}",
    );
    let (bone_vertex, bone_fragment) = ShaderCompiler::new(bones)
        .compile_pair("skin.vert", "skin.frag", &BTreeMap::new())
        .map_err(|error| error.to_string())?;
    let bone_vertex = Arc::new(bone_vertex);
    let bone_fragment = Arc::new(bone_fragment);
    for (y, alpha) in [(8., 0.25), (28., 0.75)] {
        let mut material = solid([1., 0., 0., alpha], BlendState::Alpha);
        material.vertex = Some(bone_vertex.clone());
        material.fragment = Some(bone_fragment.clone());
        let identity = UniformValue::Matrix {
            columns: 4,
            rows: 3,
            values: vec![1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0.],
        };
        material.uniforms.values.insert(
            "g_Bones".into(),
            UniformValue::Array(vec![identity.clone(), identity]),
        );
        material.uniforms.values.insert(
            "g_BonesAlpha".into(),
            UniformValue::Array(vec![UniformValue::Float(0.), UniformValue::Float(1.)]),
        );
        let mut draw = quad([128., y, 24., 12.], [160, 112], 0., [1., 1.], material);
        draw.vertex_streams.insert(
            "a_BlendIndices".into(),
            VertexStream {
                components: 4,
                data: VertexStreamData::Uint32((0..4).flat_map(|_| [0, 1, 0, 0]).collect()),
            },
        );
        draw.vertex_streams.insert(
            "a_BlendWeights".into(),
            VertexStream {
                components: 4,
                data: VertexStreamData::Float32(
                    (0..4).flat_map(|_| [1. - alpha, alpha, 0., 0.]).collect(),
                ),
            },
        );
        add_pass(
            &mut f,
            "compiled identity-bone skin alpha",
            Target::Screen,
            vec![draw],
            None,
        );
    }
    let mut library = SourceLibrary::new();
    library.insert(
        "owned.vert",
        "attribute vec4 a_Position;void main(){gl_Position=a_Position;}",
    );
    library.insert("owned.frag","uniform sampler2D g_Texture0;uniform sampler2D g_Texture1;uniform sampler2D g_Texture2;uniform sampler2D g_Texture24;uniform sampler2D g_Texture25;uniform float gain;uniform float ownedAudioLeft[16];uniform float ownedAudioRight[16];float audioResponse(float l[16],float r[16]){return l[3]+r[7];}void main(){gl_FragColor=(texture(g_Texture0,vec2(0.5))+texture(g_Texture1,vec2(0.5))+texture(g_Texture2,vec2(0.5))+texture(g_Texture24,vec2(1.25,0.5))+texture(g_Texture25,vec2(0.5)))*gain*audioResponse(ownedAudioLeft,ownedAudioRight);}");
    let (v, frag) = ShaderCompiler::new(library)
        .compile_pair("owned.vert", "owned.frag", &BTreeMap::new())
        .map_err(|e| e.to_string())?;
    let mut mixed = solid(
        [25. / 255., 35. / 255., 15. / 255., 1.],
        BlendState::Replace,
    );
    mixed.vertex = Some(Arc::new(v));
    mixed.fragment = Some(Arc::new(frag));
    mixed
        .uniforms
        .values
        .insert("gain".into(), UniformValue::Float(0.2));
    for (name, index, value) in [("ownedAudioLeft", 3, 0.25), ("ownedAudioRight", 7, 0.75)] {
        let mut bins = vec![0.; 16];
        bins[index] = value;
        mixed
            .uniforms
            .values
            .insert(name.into(), UniformValue::Vector(bins));
    }
    for (slot, pixel) in [
        (0, [25, 0, 0, 255]),
        (1, [0, 50, 0, 255]),
        (2, [0, 0, 75, 255]),
        (24, [100, 0, 0, 255]),
        (25, [0, 125, 0, 255]),
    ] {
        let id = 100 + slot as u64;
        let bytes = if slot == 24 {
            [pixel, [0, 0, 0, 255]].concat()
        } else {
            pixel.to_vec()
        };
        let image =
            RgbaImage::new(if slot == 24 { 2 } else { 1 }, 1, bytes).map_err(|e| e.to_string())?;
        let mut texture = Texture::rgba2d(format!("owned logical{slot}"), Arc::new(image));
        texture.sampler.min_filter = FilterMode::Nearest;
        texture.sampler.mag_filter = FilterMode::Nearest;
        f.textures.insert(id, texture);
        mixed.textures.push(TextureBinding {
            name: format!("g_Texture{slot}"),
            slot,
            texture: id,
        });
    }
    mixed.samplers.insert(
        24,
        SamplerState {
            address_u: AddressMode::Repeat,
            min_filter: FilterMode::Nearest,
            mag_filter: FilterMode::Nearest,
            mip_filter: FilterMode::Nearest,
            ..Default::default()
        },
    );
    add_pass(
        &mut f,
        "compiled MSL logical0/1/2/24/25",
        Target::Screen,
        vec![quad([88., 56., 32., 32.], [160, 112], 0., [1., 1.], mixed)],
        None,
    );
    f.validate().map_err(|e| e.to_string())?;
    Ok(f)
}
pub fn bloom_frame() -> std::result::Result<RenderFrame, String> {
    let mut frame = RenderFrame::new(64, 64);
    let pixels = (0..64)
        .flat_map(|i| {
            if i == 28 {
                [2., 0.5, 0.25, 1.]
            } else {
                [0., 0., 0., 1.]
            }
        })
        .collect();
    let mut texture = Texture::float2d(
        "owned bloom impulse",
        8,
        8,
        Arc::new(pixels),
        TextureFormat::Rgba16Float,
    )
    .map_err(|e| e.to_string())?;
    texture.sampler.min_filter = FilterMode::Nearest;
    texture.sampler.mag_filter = FilterMode::Nearest;
    frame.textures.insert(1, texture);
    frame.passes.push(RenderPass {
        name: "owned separable bloom".into(),
        effect: Some(effect::Effect::Bloom {
            input: 1,
            threshold: 0.2,
            intensity: 0.35,
            sigma: 1.3,
            radius: 4,
        }),
        ..Default::default()
    });
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pixel(image: &RgbaImage, x: usize, y: usize) -> [u8; 4] {
        let at = (y * image.width as usize + x) * 4;
        image.rgba[at..at + 4].try_into().unwrap()
    }
    #[test]
    fn indexed_fixture_has_known_hdr_mip_depth_and_binding_regions() {
        let frame = indexed_frame().unwrap();
        let image = cpu::render(&frame).unwrap();
        assert_eq!(pixel(&image, 24, 24), [128, 0, 128, 255]);
        assert_eq!(pixel(&image, 64, 24), [128, 128, 0, 255]);
        assert_eq!(pixel(&image, 104, 24), [0, 255, 0, 255]);
        assert_eq!(pixel(&image, 104, 72), [25, 35, 15, 255]);
        let quarter = pixel(&image, 140, 12);
        let three_quarters = pixel(&image, 140, 32);
        assert!((72..=75).contains(&quarter[0]) && (9..=11).contains(&quarter[1]));
        assert!((193..=196).contains(&three_quarters[0]) && (2..=4).contains(&three_quarters[1]));
        assert_eq!([quarter[3], three_quarters[3]], [255; 2]);
        for draw in frame
            .draws
            .iter()
            .filter(|draw| draw.vertex_streams.contains_key("a_BlendWeights"))
        {
            draw.material
                .vertex
                .as_ref()
                .unwrap()
                .pack_uniforms(&draw.material.uniforms.values)
                .unwrap();
        }
        assert_eq!(pixel(&image, 150, 100), [13, 13, 13, 255]);
        for y in 56..88 {
            for x in 8..40 {
                assert_eq!(
                    pixel(&image, x, y),
                    [67, 4, 131, 255],
                    "shared quad diagonal at {x},{y}"
                );
            }
        }
        let compiled = frame
            .draws
            .last()
            .unwrap()
            .material
            .fragment
            .as_ref()
            .unwrap();
        assert_eq!(
            compiled
                .bindings
                .iter()
                .filter_map(|b| b.sampler_slot)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );
        assert_eq!(
            compiled
                .bindings
                .iter()
                .filter(|b| b.texture_slot.is_some())
                .filter_map(|b| b.resource_slot)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 24, 25]
        );
    }
    #[test]
    fn bloom_fixture_lowering_preserves_cpu_pixels() {
        let frame = bloom_frame().unwrap();
        let lowered = effect::lower_separable(&frame).unwrap();
        assert_eq!(lowered.passes.len(), 3);
        let reference = cpu::render(&frame).unwrap();
        let actual = cpu::render(&lowered).unwrap();
        assert!(
            reference
                .rgba
                .iter()
                .zip(&actual.rgba)
                .all(|(a, b)| a.abs_diff(*b) <= 1)
        );
        assert!(reference.rgba.chunks_exact(4).any(|p| p[0] > 0));
    }
}

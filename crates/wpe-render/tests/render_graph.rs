use std::sync::Arc;
use wpe_render::*;

fn quad(color: [f32; 4], z: f32) -> DrawCall {
    let vertices = [
        ([-1., 1., z, 1.], [0., 0., 0., 0.]),
        ([1., 1., z, 1.], [1., 0., 0., 0.]),
        ([-1., -1., z, 1.], [0., 1., 0., 0.]),
        ([1., -1., z, 1.], [1., 1., 0., 0.]),
    ]
    .into_iter()
    .map(|(position, uv)| GpuVertex {
        position,
        uv,
        ..Default::default()
    })
    .collect();
    DrawCall {
        vertices,
        indices: vec![0, 2, 1, 1, 2, 3],
        material: Material {
            cpu: CpuProgram::Solid { color },
            ..Default::default()
        },
        ..Default::default()
    }
}
fn pixel(image: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
    let at = (y as usize * image.width as usize + x as usize) * 4;
    image.rgba[at..at + 4].try_into().unwrap()
}

#[test]
fn indexed_quad_has_no_diagonal_double_blending() {
    let mut frame = RenderFrame::new(8, 8);
    frame.draws.push(quad([1., 0., 0., 0.5], 0.));
    let image = cpu::render(&frame).unwrap();
    for p in image.rgba.chunks_exact(4) {
        assert_eq!(p, [128, 0, 0, 255]);
    }
}
#[test]
fn depth_persists_across_passes_until_clear() {
    let mut frame = RenderFrame::new(4, 4);
    let mut front = quad([0., 1., 0., 1.], 0.2);
    front.material.state.depth_test = true;
    front.material.state.depth_write = true;
    let mut back = quad([1., 0., 0., 1.], 0.8);
    back.material.state = front.material.state.clone();
    frame.draws = vec![front, back];
    frame.passes = vec![
        RenderPass {
            draws: vec![0],
            ..Default::default()
        },
        RenderPass {
            draws: vec![1],
            ..Default::default()
        },
    ];
    assert_eq!(pixel(&cpu::render(&frame).unwrap(), 1, 1), [0, 255, 0, 255]);
    frame.passes[1].clear = Some([0., 0., 0., 1.]);
    assert_eq!(pixel(&cpu::render(&frame).unwrap(), 1, 1), [255, 0, 0, 255]);
}
#[test]
fn perspective_correct_uv_uses_reciprocal_w() {
    let mut frame = RenderFrame::new(4, 4);
    let image = Arc::new(
        RgbaImage::new(
            1,
            4,
            vec![0, 0, 0, 255, 64, 0, 0, 255, 128, 0, 0, 255, 255, 0, 0, 255],
        )
        .unwrap(),
    );
    let mut texture = Texture::rgba2d("gradient", image);
    texture.sampler.min_filter = FilterMode::Nearest;
    texture.sampler.mag_filter = FilterMode::Nearest;
    frame.textures.insert(1, texture);
    let vertices = [
        ([-1., 1., 0.5, 1.], [0., 0., 0., 0.]),
        ([1., 1., 0.5, 1.], [0., 0., 0., 0.]),
        ([-2., -2., 1., 2.], [0., 1., 0., 0.]),
    ]
    .into_iter()
    .map(|(position, uv)| GpuVertex {
        position,
        uv,
        ..Default::default()
    })
    .collect();
    frame.draws.push(DrawCall {
        vertices,
        indices: vec![0, 2, 1],
        material: Material {
            textures: vec![TextureBinding {
                name: "g_Texture0".into(),
                slot: 0,
                texture: 1,
            }],
            ..Default::default()
        },
        ..Default::default()
    });
    assert_eq!(pixel(&cpu::render(&frame).unwrap(), 0, 2)[0], 64);
}
#[test]
fn homogeneous_near_plane_clips_geometry() {
    let mut frame = RenderFrame::new(8, 8);
    let mut draw = quad([1., 0., 0., 1.], 0.5);
    draw.vertices[0].position[2] = -0.5;
    draw.vertices[1].position[2] = -0.5;
    frame.draws.push(draw);
    let image = cpu::render(&frame).unwrap();
    assert_eq!(pixel(&image, 4, 0), [0, 0, 0, 255]);
    assert_eq!(pixel(&image, 4, 7), [255, 0, 0, 255]);
}
#[test]
fn sampler_address_filter_and_volume_interpolation() {
    let mut frame = RenderFrame::new(1, 1);
    let mut texture = Texture::rgba2d(
        "rg",
        Arc::new(RgbaImage::new(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap()),
    );
    texture.sampler.mag_filter = FilterMode::Nearest;
    texture.sampler.address_u = AddressMode::Repeat;
    frame.textures.insert(1, texture);
    assert_eq!(
        cpu::sample_texture(&frame, 1, [1.1, 0.5, 0.], 0.).unwrap(),
        [1., 0., 0., 1.]
    );
    frame.textures.get_mut(&1).unwrap().sampler.address_u = AddressMode::Mirror;
    assert_eq!(
        cpu::sample_texture(&frame, 1, [1.1, 0.5, 0.], 0.).unwrap(),
        [0., 0., 1., 1.]
    );
    let sampler = &mut frame.textures.get_mut(&1).unwrap().sampler;
    sampler.address_u = AddressMode::Border;
    sampler.border_color = [0.1, 0.2, 0.3, 0.4];
    assert_eq!(
        cpu::sample_texture(&frame, 1, [1.1, 0.5, 0.], 0.).unwrap(),
        [0.1, 0.2, 0.3, 0.4]
    );
    let sampler = &mut frame.textures.get_mut(&1).unwrap().sampler;
    sampler.address_u = AddressMode::Clamp;
    sampler.mag_filter = FilterMode::Linear;
    assert_eq!(
        cpu::sample_texture(&frame, 1, [0.5, 0.5, 0.], 0.).unwrap(),
        [0.5, 0., 0.5, 1.]
    );
    let rgba = (0..2)
        .flat_map(|z| {
            (0..2).flat_map(move |y| (0..2).flat_map(move |x| [x * 255, y * 255, z * 255, 255]))
        })
        .collect();
    frame.textures.insert(
        2,
        Texture {
            name: "volume".into(),
            data: TextureData::Volume3D {
                width: 2,
                height: 2,
                depth: 2,
                rgba: Arc::new(rgba),
            },
            mips: Vec::new(),
            sampler: SamplerState::default(),
            format: TextureFormat::Rgba8Unorm,
        },
    );
    assert_eq!(
        cpu::sample_texture(&frame, 2, [0.5; 3], 0.).unwrap(),
        [0.5, 0.5, 0.5, 1.]
    );
}

#[test]
fn minification_uses_min_filter_even_when_texture_has_no_mips() {
    let mut frame = RenderFrame::new(1, 1);
    let mut texture = Texture::rgba2d(
        "min/mag distinction",
        Arc::new(RgbaImage::new(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap()),
    );
    texture.sampler.min_filter = FilterMode::Linear;
    texture.sampler.mag_filter = FilterMode::Nearest;
    frame.textures.insert(1, texture);
    assert_eq!(
        cpu::sample_texture(&frame, 1, [0.5, 0.5, 0.], 0.).unwrap(),
        [0., 0., 1., 1.]
    );
    assert_eq!(
        cpu::sample_texture(&frame, 1, [0.5, 0.5, 0.], 4.).unwrap(),
        [0.5, 0., 0.5, 1.]
    );
}
#[test]
fn render_target_pass_does_not_erase_an_existing_screen() {
    let mut frame = RenderFrame::new(4, 4);
    frame.textures.insert(9, Texture::target("offscreen", 4, 4));
    frame.draws = vec![
        quad([1., 0., 0., 1.], 0.),
        quad([0., 0., 1., 1.], 0.),
        quad([0., 1., 0., 0.5], 0.),
    ];
    frame.passes = vec![
        RenderPass {
            draws: vec![0],
            ..Default::default()
        },
        RenderPass {
            target: Target::Texture(9),
            draws: vec![1],
            ..Default::default()
        },
        RenderPass {
            draws: vec![2],
            ..Default::default()
        },
    ];
    let mut renderer = cpu::Renderer::new();
    assert_eq!(
        pixel(&renderer.render(&frame).unwrap(), 2, 2),
        [128, 128, 0, 255]
    );
    assert_eq!(
        pixel(&renderer.target_rgba(9).unwrap(), 2, 2),
        [0, 0, 255, 255]
    );
}
#[test]
fn viewport_and_scissor_intersect() {
    let mut frame = RenderFrame::new(8, 8);
    frame.draws.push(quad([0., 1., 0., 1.], 0.));
    frame.passes.push(RenderPass {
        draws: vec![0],
        viewports: vec![Viewport {
            x: 2.,
            y: 2.,
            width: 4.,
            height: 4.,
            z_near: 0.,
            z_far: 1.,
        }],
        scissor: Some([3, 0, 2, 8]),
        ..Default::default()
    });
    let image = cpu::render(&frame).unwrap();
    for y in 0..8 {
        for x in 0..8 {
            assert_eq!(
                pixel(&image, x, y)[1],
                if (3..5).contains(&x) && (2..6).contains(&y) {
                    255
                } else {
                    0
                }
            );
        }
    }
}
#[test]
fn graph_feedback_and_unknown_cpu_shader_fail_explicitly() {
    let mut frame = RenderFrame::new(1, 1);
    frame.textures.insert(1, Texture::target("rt", 1, 1));
    frame.passes.push(RenderPass {
        target: Target::Texture(1),
        effect: Some(effect::Effect::Copy { input: 1 }),
        ..Default::default()
    });
    assert!(frame.validate().is_err());
    frame.passes.clear();
    let mut draw = quad([1.; 4], 0.);
    draw.material.cpu = CpuProgram::GpuOnly {
        reason: "custom".into(),
    };
    frame.draws.push(draw);
    assert_eq!(
        cpu::render(&frame).unwrap_err().kind,
        ErrorKind::Unsupported
    );
}
#[test]
fn float_target_preserves_hdr_and_unorm_target_clamps() {
    for (format, expected) in [
        (TextureFormat::Rgba16Float, 128),
        (TextureFormat::Rgba8Unorm, 64),
    ] {
        let mut frame = RenderFrame::new(1, 1);
        frame.textures.insert(
            1,
            Texture::rgba2d(
                "white",
                Arc::new(RgbaImage::new(1, 1, vec![255; 4]).unwrap()),
            ),
        );
        frame
            .textures
            .insert(2, Texture::target_with_format("hdr", 1, 1, format));
        frame.passes = vec![
            RenderPass {
                target: Target::Texture(2),
                effect: Some(effect::Effect::ColorMatrix {
                    input: 1,
                    matrix: [
                        [2., 0., 0., 0.],
                        [0., 1., 0., 0.],
                        [0., 0., 1., 0.],
                        [0., 0., 0., 1.],
                    ],
                    offset: [0.; 4],
                }),
                ..Default::default()
            },
            RenderPass {
                effect: Some(effect::Effect::ColorMatrix {
                    input: 2,
                    matrix: [
                        [0.25, 0., 0., 0.],
                        [0., 1., 0., 0.],
                        [0., 0., 1., 0.],
                        [0., 0., 0., 1.],
                    ],
                    offset: [0.; 4],
                }),
                ..Default::default()
            },
        ];
        assert_eq!(pixel(&cpu::render(&frame).unwrap(), 0, 0)[0], expected);
    }
}
#[test]
fn per_instance_attribute_streams_are_numerical_values() {
    let mut frame = RenderFrame::new(2, 2);
    let mut draw = quad([1.; 4], 0.);
    draw.instances = 2;
    draw.instance_streams.insert(
        "a_Color".into(),
        VertexStream::float32(4, vec![1., 0., 0., 1., 0., 0., 1., 0.5]),
    );
    frame.draws.push(draw);
    assert_eq!(
        pixel(&cpu::render(&frame).unwrap(), 0, 0),
        [128, 0, 128, 255]
    );
}
#[test]
fn cube_faces_and_depth_comparison_are_real_samples() {
    let mut frame = RenderFrame::new(2, 2);
    let faces = std::array::from_fn(|i| {
        Arc::new(RgbaImage::new(1, 1, vec![(i as u8) * 40, 0, 0, 255]).unwrap())
    });
    frame
        .textures
        .insert(1, Texture::cube("cube", faces).unwrap());
    for (i, direction) in [
        [1., 0., 0.],
        [-1., 0., 0.],
        [0., 1., 0.],
        [0., -1., 0.],
        [0., 0., 1.],
        [0., 0., -1.],
    ]
    .into_iter()
    .enumerate()
    {
        assert!(
            (cpu::sample_texture(&frame, 1, direction, 0.).unwrap()[0] - i as f32 * 40. / 255.)
                .abs()
                < 1e-6
        );
    }
    frame.textures.insert(
        2,
        Texture::depth("depth", 2, 2, Arc::new(vec![0.25, 0.75, 0.25, 0.75])).unwrap(),
    );
    assert_eq!(
        cpu::sample_compare(&frame, 2, [0.5, 0.5], 0.5, 0).unwrap(),
        0.5
    );
    frame.textures.get_mut(&2).unwrap().sampler.mag_filter = FilterMode::Nearest;
    assert_eq!(
        cpu::sample_compare(&frame, 2, [0.25, 0.5], 0.5, 0).unwrap(),
        0.
    );
    assert_eq!(
        cpu::sample_compare(&frame, 2, [0.75, 0.5], 0.5, 0).unwrap(),
        1.
    );
    let mut depth = Texture::target_with_format("depth-target", 2, 2, TextureFormat::Depth32Float);
    depth.sampler.compare = Some(CompareFunction::LessEqual);
    frame.textures.insert(3, depth);
    let mut draw = quad([1.; 4], 0.4);
    draw.material.state.depth_test = true;
    draw.material.state.depth_write = true;
    frame.draws.push(draw);
    frame.passes.push(RenderPass {
        target: Target::Texture(3),
        clear: Some([1., 0., 0., 1.]),
        draws: vec![0],
        ..Default::default()
    });
    let mut renderer = cpu::Renderer::new();
    renderer.render(&frame).unwrap();
    assert_eq!(
        renderer
            .sample_compare(&frame, 3, [0.5, 0.5], 0.5, 0)
            .unwrap(),
        0.
    );
    assert_eq!(
        renderer
            .sample_compare(&frame, 3, [0.5, 0.5], 0.3, 0)
            .unwrap(),
        1.
    );
}
#[test]
fn draw_sampler_override_and_render_target_mips_are_observable() {
    let mut frame = RenderFrame::new(2, 2);
    frame.textures.insert(
        1,
        Texture::rgba2d(
            "red-blue",
            Arc::new(RgbaImage::new(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap()),
        ),
    );
    let mut draw = quad([1.; 4], 0.);
    for vertex in &mut draw.vertices {
        vertex.uv = [1.1, 0.5, 0., 0.];
    }
    draw.material.cpu = CpuProgram::Textured {
        texture_slot: 0,
        color: [1.; 4],
        alpha_test: None,
    };
    draw.material.textures.push(TextureBinding {
        name: "g_Texture0".into(),
        slot: 0,
        texture: 1,
    });
    draw.material.samplers.insert(
        0,
        SamplerState {
            address_u: AddressMode::Repeat,
            min_filter: FilterMode::Nearest,
            mag_filter: FilterMode::Nearest,
            ..Default::default()
        },
    );
    frame.draws.push(draw);
    assert_eq!(pixel(&cpu::render(&frame).unwrap(), 0, 0), [255, 0, 0, 255]);
    frame.draws[0]
        .material
        .samplers
        .get_mut(&0)
        .unwrap()
        .address_u = AddressMode::Clamp;
    assert_eq!(pixel(&cpu::render(&frame).unwrap(), 0, 0), [0, 0, 255, 255]);
    let mut rt = Texture::target("mipped", 2, 2);
    rt.mips.push(TextureData::RenderTarget {
        width: 1,
        height: 1,
    });
    frame.textures.insert(2, rt);
    for vertex in &mut frame.draws[0].vertices {
        vertex.uv = if vertex.position[0] < 0. {
            [0., 0., 0., 0.]
        } else {
            [1., 0., 0., 0.]
        };
    }
    frame.passes = vec![RenderPass {
        target: Target::Texture(2),
        draws: vec![0],
        ..Default::default()
    }];
    let mut renderer = cpu::Renderer::new();
    renderer.render(&frame).unwrap();
    let mip = renderer.sample(&frame, 2, [0.5, 0.5, 0.], 1.).unwrap();
    assert!((mip[0] - 128. / 255.).abs() < 1e-6);
    assert!((mip[2] - 128. / 255.).abs() < 1e-6);
    frame.passes[0].effect = Some(effect::Effect::Copy { input: 3 });
    frame.passes[0].draws.clear();
    let faces = std::array::from_fn(|_| Arc::new(RgbaImage::new(1, 1, vec![255; 4]).unwrap()));
    frame
        .textures
        .insert(3, Texture::cube("cube", faces).unwrap());
    assert_eq!(frame.validate().unwrap_err().kind, ErrorKind::Unsupported);
}
#[test]
fn alpha_cut_shadow_pass_writes_float_depth_and_half_target_rounds() {
    let mut frame = RenderFrame::new(2, 2);
    frame.textures.insert(
        1,
        Texture::rgba2d(
            "cutout",
            Arc::new(RgbaImage::new(2, 1, vec![255, 255, 255, 0, 255, 255, 255, 255]).unwrap()),
        ),
    );
    frame.textures.insert(
        2,
        Texture::target_with_format("shadow", 2, 2, TextureFormat::R32Float),
    );
    let mut draw = quad([1.; 4], 0.375);
    draw.material.cpu = CpuProgram::ShadowDepth {
        texture_slot: Some(0),
        alpha_test: Some(0.5),
    };
    draw.material.state = PipelineState {
        blend: BlendState::Replace,
        depth_test: true,
        depth_write: true,
        ..Default::default()
    };
    draw.material.textures.push(TextureBinding {
        name: "g_Texture0".into(),
        slot: 0,
        texture: 1,
    });
    let material = shader::builtin_material(&draw.material).unwrap();
    assert!(material.fragment.unwrap().msl.contains("discard_fragment"));
    frame.draws.push(draw);
    frame.passes.push(RenderPass {
        target: Target::Texture(2),
        clear: Some([1., 0., 0., 1.]),
        draws: vec![0],
        ..Default::default()
    });
    let mut renderer = cpu::Renderer::new();
    renderer.render(&frame).unwrap();
    assert_eq!(
        renderer.sample(&frame, 2, [0.25, 0.5, 0.], 0.).unwrap()[0],
        1.
    );
    assert_eq!(
        renderer.sample(&frame, 2, [0.75, 0.5, 0.], 0.).unwrap()[0],
        0.375
    );
    let rounded = cpu::quantize_pixel(TextureFormat::R16Float, [1. / 3., 2., 3., 4.]);
    assert_eq!(rounded, [0.33325195, 0., 0., 1.]);
}
#[test]
fn floating_source_preserves_negative_hdr_components_and_mips() {
    let mut frame = RenderFrame::new(1, 1);
    frame.textures.insert(
        1,
        Texture::float2d(
            "HDR RG",
            1,
            1,
            Arc::new(vec![2., -1., 0., 1.]),
            TextureFormat::Rg16Float,
        )
        .unwrap(),
    );
    assert_eq!(
        cpu::sample_texture(&frame, 1, [0.5, 0.5, 0.], 0.).unwrap(),
        [2., -1., 0., 1.]
    );
    frame.passes.push(RenderPass {
        effect: Some(effect::Effect::ColorMatrix {
            input: 1,
            matrix: [
                [0.25, 0., 0., 0.],
                [0., -0.5, 0., 0.],
                [0., 0., 0., 0.],
                [0., 0., 0., 1.],
            ],
            offset: [0.; 4],
        }),
        ..Default::default()
    });
    assert_eq!(
        pixel(&cpu::render(&frame).unwrap(), 0, 0),
        [128, 128, 0, 255]
    );
    let mut texture = Texture::float2d(
        "float mip",
        2,
        2,
        Arc::new([2., -1., 0., 1.].repeat(4)),
        TextureFormat::Rgba32Float,
    )
    .unwrap();
    texture.mips.push(TextureData::Float2D {
        width: 1,
        height: 1,
        rgba: Arc::new(vec![4., -2., 0., 1.]),
    });
    frame.textures.insert(2, texture);
    assert_eq!(
        cpu::sample_texture(&frame, 2, [0.5, 0.5, 0.], 0.5).unwrap(),
        [3., -1.5, 0., 1.]
    );
    assert!(
        Texture::float2d(
            "invalid unorm",
            1,
            1,
            Arc::new(vec![2.; 4]),
            TextureFormat::Rgba8Unorm
        )
        .is_err()
    );
    assert!(
        Texture::float2d(
            "invalid NaN",
            1,
            1,
            Arc::new(vec![f32::NAN; 4]),
            TextureFormat::Rgba32Float
        )
        .is_err()
    );
    assert!(
        Texture::float2d(
            "invalid size",
            u32::MAX,
            u32::MAX,
            Arc::new(Vec::new()),
            TextureFormat::Rgba32Float
        )
        .is_err()
    );
}

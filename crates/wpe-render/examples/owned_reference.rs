//! An entirely owned procedural scene, exercising the actual CPU frame graph.
use std::{path::PathBuf, sync::Arc};
use wpe_render::*;
fn image_draw(name: &str, id: u64, left: f32, top: f32, right: f32, bottom: f32) -> DrawCall {
    let vertices = [
        ([left, top, 0.4, 1.], [0., 0., 0.4, 0.]),
        ([right, top, 0.4, 1.], [1., 0., 0.4, 0.]),
        ([left, bottom, 0.4, 1.], [0., 1., 0.4, 0.]),
        ([right, bottom, 0.4, 1.], [1., 1., 0.4, 0.]),
    ]
    .into_iter()
    .map(|(position, uv)| GpuVertex {
        position,
        uv,
        ..Default::default()
    })
    .collect();
    DrawCall {
        name: name.into(),
        vertices,
        indices: vec![0, 2, 1, 1, 2, 3],
        material: Material {
            textures: vec![TextureBinding {
                name: "g_Texture0".into(),
                slot: 0,
                texture: id,
            }],
            ..Default::default()
        },
        ..Default::default()
    }
}
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let output = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("usage: owned_reference OUTPUT.png FONT.ttf")?,
    );
    let font = PathBuf::from(
        std::env::args_os()
            .nth(2)
            .ok_or("supply an external font file; fonts are not bundled")?,
    );
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut frame = RenderFrame::new(640, 360);
    frame.clear = [0.025, 0.04, 0.08, 1.];
    let rgba = (0u32..128)
        .flat_map(|y| {
            (0u32..128).flat_map(move |x| {
                let checker = if (x / 16 + y / 16) % 2 == 0 { 30 } else { 70 };
                [checker, (x * 2) as u8, (y * 2) as u8, 255]
            })
        })
        .collect();
    frame.textures.insert(
        1,
        Texture::rgba2d("owned-checker", Arc::new(RgbaImage::new(128, 128, rgba)?)),
    );
    frame.textures.insert(
        2,
        Texture::target_with_format("scene-hdr", 640, 360, TextureFormat::Rgba16Float),
    );
    frame.textures.insert(
        3,
        Texture::target_with_format("bloom-hdr", 640, 360, TextureFormat::Rgba16Float),
    );
    frame
        .draws
        .push(image_draw("checker", 1, -0.93, 0.88, 0.12, -0.65));
    let mut triangle = DrawCall {
        name: "lit-indexed-triangle".into(),
        vertices: vec![
            GpuVertex {
                position: [-0.1, 0.55, 0.2, 1.],
                normal: [0., 0., 1., 0.],
                color: [0.9, 0.3, 0.08, 1.],
                ..Default::default()
            },
            GpuVertex {
                position: [0.9, 0.15, 0.2, 1.],
                normal: [0., 0.6, 0.8, 0.],
                color: [0.1, 0.9, 0.6, 1.],
                ..Default::default()
            },
            GpuVertex {
                position: [0.25, -0.7, 0.2, 1.],
                normal: [0.6, 0., 0.8, 0.],
                color: [0.3, 0.2, 1., 1.],
                ..Default::default()
            },
        ],
        indices: vec![0, 1, 2],
        ..Default::default()
    };
    triangle.material.cpu = CpuProgram::Lambert {
        texture_slot: None,
        light_direction: [0.2, 0.4, 1.],
        ambient: [0.18; 3],
        light_color: [1.2; 3],
    };
    triangle.material.state.depth_test = true;
    triangle.material.state.depth_write = true;
    frame.draws.push(triangle);
    let volume = (0..4)
        .flat_map(|z| {
            (0..4).flat_map(move |y| (0..4).flat_map(move |x| [x * 85, y * 85, z * 85, 255]))
        })
        .collect();
    frame.textures.insert(
        5,
        Texture {
            name: "owned-volume".into(),
            data: TextureData::Volume3D {
                width: 4,
                height: 4,
                depth: 4,
                rgba: Arc::new(volume),
            },
            mips: Vec::new(),
            sampler: SamplerState::default(),
            format: TextureFormat::Rgba8Unorm,
        },
    );
    let mut volume = image_draw("volume-slice", 5, 0.35, 0.88, 0.92, 0.5);
    volume.material.cpu = CpuProgram::Volume {
        texture_slot: 0,
        color: [1.; 4],
    };
    frame.draws.push(volume);
    let text = text::TextLayouter::new(std::fs::read(&font)?)?.layout(
        "Wallpaper Lab   سلام\nIndexed meshes · HDR · volume textures",
        &text::TextStyle {
            font_size: 22.,
            outline_width: 1.,
            shadow_offset: [1., 2.],
            shadow_color: [0., 0., 0., 0.7],
            shadow_blur: 1.,
            padding: 3,
            ..Default::default()
        },
    )?;
    let glyphs = text.glyphs.len();
    let text_issues = text.issues.clone();
    let text_width = text.image.width as f32 / 640. * 2.;
    let text_height = text.image.height as f32 / 360. * 2.;
    frame
        .textures
        .insert(4, Texture::rgba2d("owned-text", Arc::new(text.image)));
    frame.draws.push(image_draw(
        "text",
        4,
        -0.92,
        -0.65,
        -0.92 + text_width,
        -0.65 - text_height,
    ));
    frame.passes = vec![
        RenderPass {
            name: "scene".into(),
            target: Target::Texture(2),
            clear: Some(frame.clear),
            draws: (0..frame.draws.len()).collect(),
            ..Default::default()
        },
        RenderPass {
            name: "bloom".into(),
            target: Target::Texture(3),
            effect: Some(effect::Effect::Bloom {
                input: 2,
                threshold: 0.55,
                intensity: 0.25,
                sigma: 1.2,
                radius: 3,
            }),
            ..Default::default()
        },
        RenderPass {
            name: "present".into(),
            effect: Some(effect::Effect::Copy { input: 3 }),
            ..Default::default()
        },
    ];
    let image = cpu::render(&frame)?;
    image::save_buffer(
        &output,
        &image.rgba,
        image.width,
        image.height,
        image::ColorType::Rgba8,
    )?;
    let effect =
        effect::draw_call_sized(frame.passes[1].effect.as_ref().unwrap(), &frame, 640, 360)?;
    let report = serde_json::json!({"width":image.width,"height":image.height,"draws":frame.draws.len(),"passes":frame.passes.len(),"text_glyphs":glyphs,"text_issues":text_issues,"effect_msl_bytes":effect.material.fragment.as_ref().unwrap().msl.len(),"all_input_artwork":"Owned procedural pixels/geometry/text; external font read only, not bundled.","original_program_or_gpu_executed":false});
    std::fs::write(
        output.with_extension("json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    println!("{}", report);
    Ok(())
}

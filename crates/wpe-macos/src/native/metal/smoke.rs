//! Actual on-device Metal smoke test. No window, desktop/config mutation or original executable.
use super::{offscreen::OffscreenRenderer, *};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use wpe_render::shader::{ShaderCompiler, SourceLibrary};

fn save(path: &Path, image: &RgbaImage) -> Result<(), String> {
    image::RgbaImage::from_raw(image.width, image.height, image.rgba.clone())
        .ok_or("PNG extent mismatch")?
        .save(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}
fn compare(
    renderer: &mut OffscreenRenderer,
    name: &str,
    frame: RenderFrame,
    output: &Path,
) -> Result<Value, String> {
    let cpu = cpu::render(&frame).map_err(|e| e.to_string())?;
    save(&output.join(format!("{name}-cpu.png")), &cpu)?;
    let start = Instant::now();
    let ticket = renderer.submit(frame)?;
    let actual_passes = renderer.actual_pass_count();
    let gpu = loop {
        if let Some(image) = ticket.poll()? {
            break image;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    if (gpu.width, gpu.height, gpu.rgba.len()) != (cpu.width, cpu.height, cpu.rgba.len()) {
        return Err(format!(
            "{name}: GPU readback extent differs from CPU reference"
        ));
    }
    let mut diff = Vec::with_capacity(cpu.rgba.len());
    let mut maximum = 0u8;
    let mut mismatches = 0usize;
    let mut total = 0u64;
    for (a, b) in cpu.rgba.chunks_exact(4).zip(gpu.rgba.chunks_exact(4)) {
        let mut pixel = [0; 4];
        for c in 0..4 {
            let d = a[c].abs_diff(b[c]);
            maximum = maximum.max(d);
            total += d as u64;
            pixel[c] = d.saturating_mul(16);
            if d > 3 {
                mismatches += 1;
            }
        }
        pixel[3] = 255;
        diff.extend(pixel);
    }
    save(&output.join(format!("{name}-gpu.png")), &gpu)?;
    save(
        &output.join(format!("{name}-diff-x16.png")),
        &RgbaImage::new(cpu.width, cpu.height, diff).map_err(|e| e.to_string())?,
    )?;
    let regions = if name == "indexed-depth-blend-mip-float" {
        vec![
            ("HDR mip", [8, 8, 32, 32]),
            ("signed float", [48, 8, 32, 32]),
            ("depth", [88, 8, 32, 32]),
            ("alpha blend", [8, 56, 32, 32]),
            ("custom blend", [48, 56, 32, 32]),
            ("skin alpha 0.25", [128, 8, 24, 12]),
            ("skin alpha 0.75", [128, 28, 24, 12]),
            ("audio arrays and logical textures", [88, 56, 32, 32]),
        ]
    } else {
        vec![("full image", [0, 0, cpu.width, cpu.height])]
    };
    let regions = regions
        .into_iter()
        .map(|(name, [x, y, width, height])| {
            let mut over = 0;
            let mut max = 0;
            for yy in y..y + height {
                for xx in x..x + width {
                    let at = (yy as usize * cpu.width as usize + xx as usize) * 4;
                    for channel in 0..4 {
                        let delta = cpu.rgba[at + channel].abs_diff(gpu.rgba[at + channel]);
                        over += usize::from(delta > 3);
                        max = max.max(delta);
                    }
                }
            }
            json!({"name":name,"rect":[x,y,width,height],"channels_over_tolerance":over,"max_channel_difference":max})
        })
        .collect::<Vec<_>>();
    Ok(
        json!({"name":name,"passed":mismatches==0,"width":gpu.width,"height":gpu.height,"actual_passes":actual_passes,"max_channel_difference":maximum,"channels_over_tolerance":mismatches,"mean_channel_difference":total as f64/cpu.rgba.len()as f64,"tolerance":3,"regions":regions,"elapsed_seconds":start.elapsed().as_secs_f64(),"alpha":"opaque fixture; literal attachment RGBA equals straight RGBA","artifacts":[format!("{name}-cpu.png"),format!("{name}-gpu.png"),format!("{name}-diff-x16.png")]}),
    )
}
fn retained(renderer: &mut OffscreenRenderer, root: &Path) -> Result<Value, String> {
    let library = SourceLibrary::from_directory(root).map_err(|e| e.to_string())?;
    let vertex = [
        "assets/shaders/genericimage2.vert",
        "shaders/genericimage2.vert",
        "genericimage2.vert",
    ]
    .into_iter()
    .find(|name| library.get(name).is_some())
    .ok_or("Optional shader root has no genericimage2.vert")?;
    let fragment = vertex.strip_suffix(".vert").unwrap().to_owned() + ".frag";
    let (v, f) = ShaderCompiler::new(library)
        .compile_pair(
            vertex,
            &fragment,
            &BTreeMap::from([("TEX0FORMAT".into(), 0), ("GS_ENABLED".into(), 0)]),
        )
        .map_err(|e| e.to_string())?;
    let report = json!({"attempted":true,"passed":true,"vertex":v.name,"fragment":f.name,"vertex_msl_bytes":v.msl.len(),"fragment_msl_bytes":f.msl.len(),"meaning":"Actual Metal library+render-pipeline creation, not original engine render parity"});
    let mut material = Material {
        cpu: CpuProgram::Solid { color: [1.; 4] },
        state: PipelineState {
            blend: BlendState::Replace,
            ..Default::default()
        },
        ..Default::default()
    };
    material.vertex = Some(Arc::new(v));
    material.fragment = Some(Arc::new(f));
    renderer.compile_pipeline(&DrawCall {
        material,
        ..Default::default()
    })?;
    Ok(report)
}

/// Run on a physical Mac. Writes report.json and CPU/GPU/difference PNGs.
pub fn run(
    output: &Path,
    shader_root: Option<&Path>,
    mtm: MainThreadMarker,
) -> Result<Value, String> {
    std::fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let result = (|| {
        let mut renderer = OffscreenRenderer::new(160, 112, mtm)?;
        let device = renderer.device_name();
        let indexed = compare(
            &mut renderer,
            "indexed-depth-blend-mip-float",
            wpe_render::smoke::indexed_frame()?,
            output,
        )?;
        let mut bloom = OffscreenRenderer::new(64, 64, mtm)?;
        let bloom_result = compare(
            &mut bloom,
            "separable-bloom",
            wpe_render::smoke::bloom_frame()?,
            output,
        )?;
        let optional = if let Some(root) = shader_root {
            retained(&mut renderer, root)?
        } else {
            json!({"attempted":false,"reason":"Pass a retained shader root as the second argument to compile its actual Metal pipeline"})
        };
        let passed = indexed["passed"] == true
            && bloom_result["passed"] == true
            && bloom_result["actual_passes"]
                .as_u64()
                .is_some_and(|n| n >= 3);
        Ok::<Value, String>(
            json!({"passed":passed,"platform":std::env::consts::ARCH,"device":device,"execution":"Actual Apple Metal compile/pipeline/ordered submission/GPU completion/readback","cases":[indexed,bloom_result],"retained_pipeline":optional,"not_claimed":"Native Wallpaper Engine pixel/lighting/numeric equivalence; this is an owned renderer smoke test"}),
        )
    })();
    let report = match &result {
        Ok(report) => report.clone(),
        Err(error) => {
            json!({"passed":false,"error":error,"execution":"Mac Metal smoke attempt failed; partial artifacts may exist"})
        }
    };
    std::fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    match result {
        Ok(report) if report["passed"] == true => Ok(report),
        Ok(_) => Err(format!(
            "Metal comparison failed; inspect {}",
            output.join("report.json").display()
        )),
        Err(error) => Err(error),
    }
}

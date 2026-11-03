//! Generate the exact CPU references used by the physical Metal smoke test.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::path::PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("usage: cpu_smoke OUTPUT_DIRECTORY")?,
    );
    std::fs::create_dir_all(&output)?;
    let mut cases = Vec::new();
    for (name, frame) in [
        (
            "indexed-depth-blend-mip-float",
            wpe_render::smoke::indexed_frame()?,
        ),
        ("separable-bloom", wpe_render::smoke::bloom_frame()?),
    ] {
        let image = wpe_render::cpu::render(&frame)?;
        let filename = format!("{name}-cpu.png");
        image::RgbaImage::from_raw(image.width, image.height, image.rgba)
            .ok_or("image extent mismatch")?
            .save(output.join(&filename))?;
        cases.push(serde_json::json!({"name":name,"width":frame.width,"height":frame.height,"artifact":filename,"cpu_rendered":true,"metal_executed":false}));
    }
    std::fs::write(
        output.join("cpu-reference.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"passed":true,"cases":cases,"meaning":"Owned portable CPU fixture generation; no GPU execution"}),
        )?,
    )?;
    Ok(())
}

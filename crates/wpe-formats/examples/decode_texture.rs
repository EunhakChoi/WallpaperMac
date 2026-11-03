//! Read-only TEX decoder harness. Output bytes are explicitly RGBA8 or little-endian RGBA32F.
use std::{fs, path::PathBuf};
use wpe_formats::tex::{DecodedImage, Texture};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let input = PathBuf::from(
        args.next()
            .ok_or("decode_texture INPUT OUTPUT [REPORT_JSON]")?,
    );
    let output = PathBuf::from(args.next().ok_or("OUTPUT missing")?);
    let texture = Texture::parse(&fs::read(&input)?)?;
    let image = texture.decode_image(0, 0)?;
    let dimensions = image.dimensions();
    let (kind, bytes) = match image {
        DecodedImage::Rgba8(image) => ("rgba8", image.rgba),
        DecodedImage::Rgba32Float(image) => (
            "rgba32f-le",
            image.rgba.into_iter().flat_map(f32::to_le_bytes).collect(),
        ),
    };
    fs::write(&output, &bytes)?;
    let result = serde_json::json!({"input":input,"output":output,"version":texture.version,"container_version":texture.container_version,"format":texture.header.format,"dimensions":dimensions,"decoded_type":kind,"output_bytes":bytes.len()});
    if let Some(path) = args.next() {
        fs::write(path, serde_json::to_vec_pretty(&result)?)?;
    }
    println!("{result}");
    Ok(())
}

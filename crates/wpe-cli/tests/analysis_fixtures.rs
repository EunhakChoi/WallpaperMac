//! Optional original-compiler oracles; no original artwork or binary is bundled.
use std::path::PathBuf;
use wpe_formats::tex::Texture;

fn analysis() -> PathBuf {
    let root = std::env::var_os("WPE_ANALYSIS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../../investigations/wallpaper-engine")
        });
    assert!(
        root.join("runtime/runs/oracles-v4").is_dir(),
        "Set WPE_ANALYSIS_DIR to the retained analysis directory"
    );
    root
}

#[test]
#[ignore = "requires retained original compiler output fixtures; run explicitly with --ignored"]
fn original_compiler_lossless_and_lz4_output_decode_to_the_input_pixels() {
    let root = analysis().join("runtime/runs/oracles-v4");
    for version in ["2.7.3", "2.8.42"] {
        for (name, width, height) in [("lossless", 16, 16), ("solid-lz4", 64, 64)] {
            let base = root.join(version).join("materials");
            let texture =
                Texture::parse(&std::fs::read(base.join(format!("{name}.tex"))).unwrap()).unwrap();
            let image = texture.decode_rgba8(0, 0).unwrap();
            assert_eq!(
                (image.width, image.height),
                (width, height),
                "{version}/{name}"
            );
            let expected = std::fs::read(base.join(format!("{name}.rgba"))).unwrap();
            assert_eq!(image.rgba.len(), expected.len());
            let mismatch = image.rgba.iter().zip(&expected).position(|(a, b)| a != b);
            assert!(
                mismatch.is_none(),
                "{version}/{name}: first RGBA byte mismatch {mismatch:?}"
            );
        }
    }
}

#[test]
#[ignore = "requires retained owned OBJ and original compiler MDL output; run explicitly with --ignored"]
fn original_compiler_triangle_preserves_positions_indices_and_flips_obj_v() {
    let root = analysis().join("runtime/runs/oracles-v4");
    for version in ["2.7.3", "2.8.42"] {
        let file = root.join(version).join("models/triangle.mdl");
        let model = wpe_formats::mdl::Model::parse(&std::fs::read(file).unwrap()).unwrap();
        assert_eq!(model.meshes.len(), 1);
        assert!(model.unsupported_sections.is_empty());
        let mesh = &model.meshes[0];
        assert_eq!(mesh.vertex_format, 9);
        assert_eq!(mesh.vertex_stride, 20);
        assert_eq!(mesh.indices, [0, 1, 2]);
        for (index, expected) in [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]]
            .iter()
            .enumerate()
        {
            assert_eq!(
                mesh.position(index).unwrap(),
                *expected,
                "{version}/{index}"
            );
        }
        for (index, expected) in [[0., 1.], [1., 1.], [0., 0.]].iter().enumerate() {
            let uv = &mesh.vertices[index * 20 + 12..index * 20 + 20];
            let actual = [
                f32::from_le_bytes(uv[0..4].try_into().unwrap()),
                f32::from_le_bytes(uv[4..8].try_into().unwrap()),
            ];
            assert_eq!(actual, *expected, "{version}/{index} UV");
        }
    }
}

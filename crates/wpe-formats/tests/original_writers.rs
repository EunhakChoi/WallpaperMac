//! External owned-input original compiler controls; no original code execution.
use std::{fs, path::PathBuf};
use wpe_formats::{
    mdl::Model,
    tex::{self, Texture},
};

#[test]
#[ignore = "requires WPE_CORPUS_ROOT pointing to the external frozen investigation"]
fn original_rgba_lz4_and_triangle_controls() {
    let root = PathBuf::from(std::env::var_os("WPE_CORPUS_ROOT").expect("WPE_CORPUS_ROOT"));
    for version in ["2.7.3", "2.8.42"] {
        let base = root.join("resources/runtime-native-v4-files").join(version);
        for name in ["lossless", "solid-lz4"] {
            let bytes = fs::read(base.join("materials").join(format!("{name}.tex"))).unwrap();
            let expected = fs::read(base.join("materials").join(format!("{name}.rgba"))).unwrap();
            let texture = Texture::parse(&bytes).unwrap();
            let decoded = texture.decode_rgba8(0, 0).unwrap();
            assert_eq!(decoded.rgba, expected, "{version}/{name}");
            assert_eq!(texture.images[0].mips[0].lz4, name == "solid-lz4");
            let roundtrip = Texture::parse(&tex::encode_rgba8(&decoded).unwrap())
                .unwrap()
                .decode_rgba8(0, 0)
                .unwrap();
            assert_eq!(roundtrip, decoded);
        }
        let obj = fs::read_to_string(base.join("models/triangle.obj")).unwrap();
        let mut positions = Vec::new();
        let mut uvs = Vec::new();
        for line in obj.lines() {
            let words = line.split_whitespace().collect::<Vec<_>>();
            if words.first() == Some(&"v") {
                positions.push(
                    words[1..4]
                        .iter()
                        .map(|s| s.parse::<f32>().unwrap())
                        .collect::<Vec<_>>(),
                );
            }
            if words.first() == Some(&"vt") {
                uvs.push(
                    words[1..3]
                        .iter()
                        .map(|s| s.parse::<f32>().unwrap())
                        .collect::<Vec<_>>(),
                );
            }
        }
        let model = Model::parse(&fs::read(base.join("models/triangle.mdl")).unwrap()).unwrap();
        assert_eq!(model.version, 23);
        assert_eq!(model.meshes.len(), 1);
        let mesh = &model.meshes[0];
        assert_eq!(mesh.vertex_format, 9);
        assert_eq!(mesh.vertex_stride, 20);
        assert_eq!(mesh.indices, [0, 1, 2]);
        assert_eq!(mesh.vertex_count, positions.len());
        for i in 0..mesh.vertex_count {
            assert_eq!(mesh.position(i).unwrap().as_slice(), positions[i]);
            let bytes = &mesh.vertices[i * mesh.vertex_stride + 12..];
            let uv = [
                f32::from_le_bytes(bytes[..4].try_into().unwrap()),
                f32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            ];
            assert_eq!(
                uv,
                [uvs[i][0], 1. - uvs[i][1]],
                "original writer flips OBJ V"
            );
        }
    }
}

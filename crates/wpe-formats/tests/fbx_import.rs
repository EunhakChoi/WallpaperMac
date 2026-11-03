use serde_json::Value;
use wpe_formats::{ErrorKind, fbx::decode_to_editable};
const ASCII: &[u8] = include_bytes!("fixtures/owned-skinned-animation-ascii.fbx");
const BINARY: &[u8] = include_bytes!("fixtures/owned-skinned-animation-binary.fbx");
#[test]
fn owned_ascii_and_compressed_binary_preserve_geometry_skin_and_moving_keys() {
    let ascii = decode_to_editable(ASCII).unwrap();
    let binary = decode_to_editable(BINARY).unwrap();
    assert_eq!(ascii, binary);
    assert_eq!(ascii["shapes"].as_array().unwrap().len(), 1);
    assert_eq!(
        ascii["shapes"][0]["indexBuffer"],
        serde_json::json!([0, 1, 2])
    );
    assert_eq!(
        ascii["shapes"][0]["weights"],
        serde_json::json!([[0., 1.], [0., 1.], [0., 1.]])
    );
    let points = ascii["shapes"][0]["vertexBuffer"].as_array().unwrap();
    assert_eq!(points.len(), 36);
    assert_eq!(points[0], -2.);
    assert_eq!(points[1], -2.);
    assert_eq!(points[6], 0.);
    assert_eq!(points[7], 1.);
    assert_eq!(ascii["bones"][1]["name"], "owned-root");
    let clip = &ascii["clips"][0];
    assert_eq!(clip["fps"], 2.);
    assert_eq!(clip["frames"], 2);
    let keys = clip["boneMatrices"][1].as_array().unwrap();
    assert_eq!(keys.len(), 3);
    for (key, x) in keys.iter().zip([0., 1., 2.]) {
        assert!((key[12].as_f64().unwrap() - x).abs() < 1e-6);
    }
}
#[test]
fn malformed_input_nonfinite_geometry_hierarchy_and_array_bomb_fail_bounded() {
    for bytes in [
        b"".as_slice(),
        b"FBXHeaderExtension: { FBXVersion: 7400",
        &BINARY[..40],
    ] {
        assert!(decode_to_editable(bytes).is_err());
    }
    let source = std::str::from_utf8(ASCII).unwrap();
    let nonfinite = source.replacen("-2.0, -2.0, 0.0", "NaN, -2.0, 0.0", 1);
    assert!(decode_to_editable(nonfinite.as_bytes()).is_err());
    let cyclic = source.replace(
        "C: \"OO\", 100, 0",
        "C: \"OO\", 100, 200\n\tC: \"OO\", 200, 100",
    );
    assert!(decode_to_editable(cyclic.as_bytes()).is_err());
    let mut bomb = BINARY.to_vec();
    let at = bomb
        .windows(b"Vertices".len())
        .position(|s| s == b"Vertices")
        .unwrap()
        + b"Vertices".len();
    assert_eq!(bomb[at], b'd');
    bomb[at + 1..at + 5].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        decode_to_editable(&bomb).unwrap_err().kind,
        ErrorKind::Limit
    );
}
#[test]
fn curve_flags_and_unavailable_layer_policies_fail_instead_of_fake_animation() {
    let source = std::str::from_utf8(ASCII).unwrap();
    let cubic_auto = source.replace("a: 4\n", "a: 264\n");
    assert!(
        decode_to_editable(cubic_auto.as_bytes())
            .unwrap_err()
            .to_string()
            .contains("stored user tangents")
    );
    let source_no_animation = source.replace("C: \"OP\", 530, 510, \"d|X\"", "C: \"OO\", 530, 0");
    let document: Value = decode_to_editable(source_no_animation.as_bytes()).unwrap();
    assert_eq!(document["clips"].as_array().unwrap().len(), 0);
}
#[test]
fn stored_user_cubic_and_constant_next_are_sampled_at_actual_times() {
    let source = std::str::from_utf8(ASCII).unwrap();
    let source = source.replace(
        "\"CustomFrameRate\", \"double\", \"\", \"A\", 2.0",
        "\"CustomFrameRate\", \"double\", \"\", \"A\", 4.0",
    );
    let cubic = decode_to_editable(source.replace("a: 4\n", "a: 1032\n").as_bytes()).unwrap();
    let keys = cubic["clips"][0]["boneMatrices"][1].as_array().unwrap();
    assert_eq!(keys.len(), 5);
    for (key, expected) in keys.iter().zip([0., 0.3125, 1., 1.6875, 2.]) {
        assert!((key[12].as_f64().unwrap() - expected).abs() < 1e-6, "{key}");
    }
    let weighted =
        decode_to_editable(source.replace("a: 4\n", "a: 50332680\n").as_bytes()).unwrap();
    // Zero time-weights make both control coordinates collinear (y=2*x).
    assert!(
        (weighted["clips"][0]["boneMatrices"][1][1][12]
            .as_f64()
            .unwrap()
            - 0.5)
            .abs()
            < 1e-6
    );
    let constant = decode_to_editable(source.replace("a: 4\n", "a: 258\n").as_bytes()).unwrap();
    assert_eq!(constant["clips"][0]["boneMatrices"][1][0][12], 0.);
    assert_eq!(constant["clips"][0]["boneMatrices"][1][1][12], 2.);
}
#[test]
fn binary75_wide_node_headers_decode_the_same_control_scene() {
    let document = decode_to_editable(include_bytes!(
        "fixtures/owned-skinned-animation-wide-binary.fbx"
    ))
    .unwrap();
    let reference = decode_to_editable(BINARY).unwrap();
    for key in ["shapes", "bones", "clips"] {
        assert_eq!(document[key], reference[key]);
    }
    assert_eq!(document["nativeMetadata"]["fbxVersion"], 7500);
}
#[test]
fn nonstandard_inheritance_rejects_animated_or_scaled_ancestors() {
    let source = std::str::from_utf8(ASCII).unwrap();
    let link = "C: \"OO\", 200, 400";
    let animated = source
        .replace(link, &format!("{link}\n\tC: \"OO\", 200, 100"))
        .replace(
            "C: \"OP\", 510, 200, \"Lcl Translation\"",
            "C: \"OP\", 510, 100, \"Lcl Scaling\"",
        );
    assert!(
        decode_to_editable(animated.as_bytes())
            .unwrap_err()
            .to_string()
            .contains("animated ancestor scale")
    );
    let ancestor=source.replace(link,&format!("{link}\n\tC: \"OO\", 200, 250\n\tC: \"OO\", 250, 100"))
        .replacen("\"Lcl Scaling\", \"Lcl Scaling\", \"\", \"A\", 1.0, 1.0, 1.0","\"Lcl Scaling\", \"Lcl Scaling\", \"\", \"A\", 2.0, 1.0, 1.0",1)
        .replace("Deformer: 300", "Model: 250, \"Model::middle\", \"Null\" {\n Properties70: {\n P: \"InheritType\", \"enum\", \"\", \"A\", 1\n }\n }\n Deformer: 300");
    assert!(
        decode_to_editable(ancestor.as_bytes())
            .unwrap_err()
            .to_string()
            .contains("scaled parent")
    );
}
fn replace_array(source: &str, name: &str, values: impl IntoIterator<Item = String>) -> String {
    let start = source.find(&format!("{name}: *")).unwrap();
    let begin = start + source[start..].find("a: ").unwrap() + 3;
    let end = begin + source[begin..].find('\n').unwrap();
    let mut result = source.to_owned();
    result.replace_range(
        begin..end,
        &values.into_iter().collect::<Vec<_>>().join(", "),
    );
    result
}
#[test]
fn extreme_i64_curve_interval_is_sampled_inside_a_short_real_trim_without_overflow() {
    let source = std::str::from_utf8(ASCII).unwrap();
    // Existing LocalStart=0/LocalStop=one second stays intact. Thus decoding
    // reaches sample(time=0..one-second), inside this enormous signed interval.
    let source = replace_array(
        source,
        "KeyTime",
        [i64::MIN.to_string(), i64::MAX.to_string()],
    );
    let document = decode_to_editable(source.as_bytes()).unwrap();
    let keys = document["clips"][0]["boneMatrices"][1].as_array().unwrap();
    assert_eq!(keys.len(), 3);
    for key in keys {
        assert!((key[12].as_f64().unwrap() - 1.).abs() < 1e-6);
    }
    let rounded = replace_array(
        std::str::from_utf8(ASCII).unwrap(),
        "KeyTime",
        [(i64::MAX - 1).to_string(), i64::MAX.to_string()],
    );
    assert!(decode_to_editable(rounded.as_bytes()).is_err());
}
#[test]
fn affine_global_bind_shear_is_retained_until_local_trs_recovery_and_projective_is_rejected() {
    let original = std::str::from_utf8(ASCII).unwrap();
    let mut projective = [
        1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
    ];
    projective[3] = 0.125;
    let invalid = replace_array(original, "Transform", projective.map(|n| n.to_string()));
    assert!(
        decode_to_editable(invalid.as_bytes())
            .unwrap_err()
            .to_string()
            .contains("projective")
    );
    let mut shear = projective;
    shear[3] = 0.;
    shear[4] = 0.25;
    let invalid = replace_array(original, "TransformLink", shear.map(|n| n.to_string()));
    assert!(
        decode_to_editable(invalid.as_bytes())
            .unwrap_err()
            .to_string()
            .contains("local bind")
    );
    let mut valid = original
        .replace(
            "C: \"OO\", 200, 400",
            "C: \"OO\", 200, 400\n\tC: \"OO\", 200, 100",
        )
        .replacen(
            "\"Lcl Scaling\", \"Lcl Scaling\", \"\", \"A\", 1.0, 1.0, 1.0",
            "\"Lcl Scaling\", \"Lcl Scaling\", \"\", \"A\", 2.0, 1.0, 1.0",
            1,
        );
    let begin = valid.find("Model: 200,").unwrap();
    let end = begin + valid[begin..].find("Deformer: 300").unwrap();
    let mut child = valid[begin..end].replace(
        "\"Lcl Rotation\", \"Lcl Rotation\", \"\", \"A\", 0.0, 0.0, 0.0",
        "\"Lcl Rotation\", \"Lcl Rotation\", \"\", \"A\", 0.0, 0.0, 45.0",
    );
    let property_table = child.find("Properties70:").unwrap();
    let opening = property_table + child[property_table..].find('{').unwrap() + 1;
    child.insert_str(opening, "\n P: \"InheritType\", \"enum\", \"\", \"A\", 1");
    valid.replace_range(begin..end, &child);
    let c = std::f64::consts::FRAC_1_SQRT_2;
    let world = [
        2. * c,
        c,
        0.,
        0.,
        -2. * c,
        c,
        0.,
        0.,
        0.,
        0.,
        1.,
        0.,
        0.,
        0.,
        0.,
        1.,
    ];
    valid = replace_array(&valid, "TransformLink", world.map(|n| n.to_string()));
    let scale = [
        2., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
    ];
    valid = replace_array(&valid, "Transform", scale.map(|n| n.to_string()));
    let document = decode_to_editable(valid.as_bytes()).unwrap();
    for (index, expected) in [(0, c), (1, c), (4, -c), (5, c)] {
        assert!(
            (document["bones"][1]["transform"][index].as_f64().unwrap() - expected).abs() < 1e-6
        );
    }
    assert_eq!(document["shapes"][0]["vertexBuffer"][0], -4.);
}

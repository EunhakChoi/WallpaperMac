use wpe_render::text::{TextLayouter, TextStyle};
#[test]
#[ignore = "requires WPE_TEST_FONT pointing to an external TrueType/OpenType font"]
fn actual_font_shaping_wrapping_and_effects() {
    let bytes = std::fs::read(std::env::var_os("WPE_TEST_FONT").expect("WPE_TEST_FONT")).unwrap();
    assert_eq!(
        wpe_render::text::collection_index(&bytes, "DejaVuSans").unwrap(),
        0
    );
    assert!(wpe_render::text::collection_index(&bytes, "MissingPostscriptFace").is_err());
    let layouter = TextLayouter::new(bytes).unwrap();
    let simple = layouter.layout("office AV", &TextStyle::default()).unwrap();
    assert!(
        simple.glyphs.len() < 9,
        "default Latin ligatures should shape"
    );
    assert!(simple.image.rgba.chunks_exact(4).any(|p| p[3] > 0));
    let wrapped = layouter
        .layout(
            "alpha beta gamma delta",
            &TextStyle {
                wrap_width: Some(110.),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(wrapped.lines > 1);
    let rtl = layouter
        .layout("abc سلام 123", &TextStyle::default())
        .unwrap();
    assert!(rtl.glyphs.iter().any(|g| g.rtl));
    assert!(
        rtl.issues.is_empty(),
        "font must cover Arabic for this test"
    );
    let effects = layouter
        .layout(
            "Effects",
            &TextStyle {
                outline_width: 2.,
                shadow_offset: [3., 4.],
                shadow_blur: 2.,
                shadow_color: [0., 0., 0., 0.7],
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        effects
            .image
            .rgba
            .chunks_exact(4)
            .any(|p| p[3] > 0 && p[3] < 255)
    );
    let limited = layouter
        .layout(
            "alpha beta gamma delta",
            &TextStyle {
                wrap_width: Some(100.),
                max_lines: Some(1),
                ellipsis: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(limited.lines, 1);
    assert!(limited.truncated);
    assert!(limited.logical_width <= 100.1);
    assert!(limited.origin[0] >= 0.);
    let invalid = TextStyle {
        font_size: f32::INFINITY,
        ..Default::default()
    };
    assert!(layouter.layout("x", &invalid).is_err());
}

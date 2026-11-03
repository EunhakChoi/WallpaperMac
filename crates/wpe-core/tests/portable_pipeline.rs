//! Owned fixtures only. These are implementation conformance checks, not an
//! original Windows/Mac GPU oracle and not copies of shipped artwork.
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use wpe_core::{
    Project, Quad, RenderPlan, RgbaImage, Vertex, WallpaperKind, load_project, load_scene,
    render_software,
    vfs::{ResourceOrigin, VirtualFileSystem},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "wpe-core-owned-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self {
            root: fs::canonicalize(root).unwrap(),
        }
    }
    fn write(&self, name: &str, bytes: impl AsRef<[u8]>) {
        let p = self.root.join(name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }
    fn json(&self, name: &str, value: &Value) {
        self.write(name, serde_json::to_vec(value).unwrap());
    }
    fn read_json(&self, name: &str) -> Value {
        serde_json::from_slice(&fs::read(self.root.join(name)).unwrap()).unwrap()
    }
    fn ordinary() -> Self {
        let f = Self::new();
        f.json(
            "project.json",
            &json!({"type":"scene","file":"scene.json","title":"Owned fixture"}),
        );
        f.json("scene.json",&json!({"general":{"orthogonalprojection":{"width":4,"height":4},"clearcolor":"0 0 0"},"objects":[{"id":1,"name":"quadrants","image":"models/image.json","origin":"2 2 0","scale":"1 1 1","angles":"0 0 0","size":"4 4"}]}));
        f.json(
            "models/image.json",
            &json!({"width":4,"height":4,"nopadding":true,"material":"materials/image.json"}),
        );
        f.json("materials/image.json",&json!({"passes":[{"shader":"genericimage2","blending":"translucent","textures":["owned.quadrants"]}]}));
        let image = RgbaImage {
            width: 2,
            height: 2,
            rgba: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ],
        };
        f.write(
            "materials/owned.quadrants.tex",
            wpe_formats::tex::encode_rgba8(&image).unwrap(),
        );
        f
    }
    fn project(&self) -> Project {
        load_project(&self.root, None).unwrap()
    }
    fn plan(&self) -> RenderPlan {
        load_scene(&self.project()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn pixel(image: &RgbaImage, x: u32, y: u32) -> &[u8] {
    let at = (y as usize * image.width as usize + x as usize) * 4;
    &image.rgba[at..at + 4]
}
fn near(a: f32, b: f32) {
    assert!((a - b).abs() < 1e-5, "{a} differs from {b}");
}
fn package(rows: &[(&str, &[u8])]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend(8u32.to_le_bytes());
    bytes.extend(b"PKGV0001");
    bytes.extend((rows.len() as u32).to_le_bytes());
    let mut offset = 0u32;
    for (name, data) in rows {
        bytes.extend((name.len() as u32).to_le_bytes());
        bytes.extend(name.as_bytes());
        bytes.extend(offset.to_le_bytes());
        bytes.extend((data.len() as u32).to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in rows {
        bytes.extend(*data)
    }
    bytes
}

#[test]
fn actual_descriptor_material_dotted_tex_chain_renders_top_down_and_png() {
    let f = Fixture::ordinary();
    let plan = f.plan();
    assert!(plan.issues.is_empty(), "{:?}", plan.issues);
    assert_eq!(plan.quads.len(), 1);
    let image = render_software(&plan, 4, 4).unwrap();
    assert_eq!(pixel(&image, 0, 0), [255, 0, 0, 255]);
    assert_eq!(pixel(&image, 3, 0), [0, 255, 0, 255]);
    assert_eq!(pixel(&image, 0, 3), [0, 0, 255, 255]);
    assert_eq!(pixel(&image, 3, 3), [255, 255, 255, 255]);
    let out = f.root.join("reference.png");
    image::save_buffer_with_format(
        &out,
        &image.rgba,
        4,
        4,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .unwrap();
    assert_eq!(image::open(&out).unwrap().to_rgba8().as_raw(), &image.rgba);
    if let Some(path) = std::env::var_os("WPE_CORE_REFERENCE_PNG") {
        fs::copy(out, path).unwrap();
    }
}

#[test]
fn legacy_metadata_scene_json_automatically_mounts_the_adjacent_scene_package() {
    let f = Fixture::ordinary();
    let scene = fs::read(f.root.join("scene.json")).unwrap();
    f.write("scene.pkg", package(&[("scene.json", &scene)]));
    fs::remove_file(f.root.join("scene.json")).unwrap();
    f.json(
        "project.json",
        &json!({"file":"scene.json","title":"Observed old packed metadata shape"}),
    );
    let project = f.project();
    assert_eq!(project.kind, WallpaperKind::Scene);
    assert!(project.entry.ends_with("scene.pkg"));
    assert_eq!(project.vfs.mounted_package_names(), ["scene.json"]);
    assert_eq!(project.read_resource("scene.json").unwrap().bytes, scene);
    assert_eq!(load_scene(&project).unwrap().quads.len(), 1);
}

#[test]
fn autosize_overwrites_explicit_size_using_logical_dimensions_and_storage_uv() {
    let f = Fixture::ordinary();
    let image = RgbaImage {
        width: 4,
        height: 4,
        rgba: vec![255; 64],
    };
    let mut bytes = wpe_formats::tex::encode_rgba8(&image).unwrap();
    // The owned writer's TEXI header stores logical W/H at bytes34/38.
    bytes[34..38].copy_from_slice(&2u32.to_le_bytes());
    bytes[38..42].copy_from_slice(&2u32.to_le_bytes());
    f.write("materials/owned.quadrants.tex", bytes);
    f.json(
        "models/image.json",
        &json!({"width":99,"height":99,"autosize":true,"material":"materials/image.json"}),
    );
    let mut scene = f.read_json("scene.json");
    scene["objects"][0]["size"] = json!("50 50");
    f.json("scene.json", &scene);
    let plan = f.plan();
    assert_eq!(plan.quads.len(), 1);
    let q = &plan.quads[0];
    near(q.vertices[0].position[0], -0.5);
    near(q.vertices[1].position[0], 0.5);
    near(q.vertices[2].uv[0], 0.15 / 4.0);
    near(q.vertices[1].uv[0], 0.5 - 0.15 / 4.0);
    assert_eq!(q.image.width, 4);
    assert_eq!(q.image.height, 4);
}

#[test]
fn ordinary_size_truncation_and_nopadding_policy() {
    let f = Fixture::ordinary();
    let mut scene = f.read_json("scene.json");
    scene["objects"][0]["size"] = json!("3.9 2.9");
    f.json("scene.json", &scene);
    let plan = f.plan();
    let q = &plan.quads[0];
    near(q.vertices[0].position[0], -0.75);
    near(q.vertices[2].position[1], 0.5);
    assert_eq!(q.vertices[2].uv, [0.0, 0.0]);
    assert_eq!(q.vertices[1].uv, [1.0, 1.0]);
}

#[test]
fn radians_scaled_anchor_and_literal_binding_warning() {
    let f = Fixture::ordinary();
    let mut scene = f.read_json("scene.json");
    scene["objects"][0]["size"] = json!("4 2");
    scene["objects"][0]["scale"] = json!({"value":"2 1 1","type":"vec3"});
    scene["objects"][0]["angles"] =
        json!({"value":format!("0 0 {}",std::f32::consts::FRAC_PI_2),"script":"not executed"});
    scene["objects"][0]["alignment"] = json!("topleft");
    f.json("scene.json", &scene);
    let plan = f.plan();
    let q = &plan.quads[0];
    near(q.vertices[0].position[0], 1.0);
    near(q.vertices[0].position[1], 0.0);
    assert!(plan.issues.iter().any(|i| i.code == "dynamic-binding"));
}

#[test]
fn unsupported_effect_parent_shader_blend_and_nonimage_are_explicit() {
    let f = Fixture::ordinary();
    let mut scene = f.read_json("scene.json");
    scene["objects"][0]["effects"] = json!([{"file":"effect.json"}]);
    scene["objects"][0]["parent"] = json!(9);
    scene["objects"][0]["colorBlendMode"] = json!(31);
    scene["objects"]
        .as_array_mut()
        .unwrap()
        .push(json!({"model":"thing.mdl","name":"model"}));
    f.json("scene.json", &scene);
    f.json("materials/image.json",&json!({"passes":[{"shader":"custom","blending":"additive","combos":{"BLENDMODE":23},"textures":["owned.quadrants"]}]}));
    let plan = f.plan();
    assert_eq!(plan.quads.len(), 1);
    for code in [
        "object-feature",
        "object-blend-mode",
        "material-shader",
        "material-blending",
        "material-feature",
        "unsupported-object",
    ] {
        assert!(plan.issues.iter().any(|i| i.code == code), "missing {code}");
    }
    assert!(plan.issues.iter().any(|i| i.message.contains("model")));
}

#[test]
fn special_descriptor_geometry_policies_are_omitted_with_issues() {
    let f = Fixture::ordinary();
    for key in [
        "fullscreen",
        "passthrough",
        "solidlayer",
        "projectlayer",
        "instanced",
        "puppet",
    ] {
        let mut descriptor = json!({"width":4,"height":4,"material":"materials/image.json"});
        descriptor[key] = json!(true);
        f.json("models/image.json", &descriptor);
        let plan = f.plan();
        assert!(plan.quads.is_empty(), "{key}");
        assert!(
            plan.issues
                .iter()
                .any(|i| i.code == "descriptor-feature" && i.message.contains(key))
        );
    }
}

#[test]
fn invisible_objects_skip_and_invalid_object_does_not_hide_other_images() {
    let f = Fixture::ordinary();
    let mut scene = f.read_json("scene.json");
    let first = scene["objects"][0].clone();
    scene["objects"]
        .as_array_mut()
        .unwrap()
        .push(json!({"image":"missing.json","name":"bad"}));
    scene["objects"].as_array_mut().unwrap().push(first);
    scene["objects"][0]["visible"] = json!(false);
    f.json("scene.json", &scene);
    let plan = f.plan();
    assert_eq!(plan.quads.len(), 1);
    assert!(
        plan.issues
            .iter()
            .any(|i| i.object.as_deref() == Some("bad") && i.code == "object-resource-or-property")
    );
}

#[test]
fn package_lowercases_mount_and_query_keys_and_last_duplicate_wins() {
    let f = Fixture::new();
    f.write(
        "scene.pkg",
        package(&[
            ("ScEnE.JSON", b"first"),
            ("scene.json", b"second"),
            ("scene.json", b"last"),
        ]),
    );
    let mut vfs = VirtualFileSystem::new();
    vfs.mount_package(&f.root.join("scene.pkg")).unwrap();
    let r = vfs.read("SCENE.Json").unwrap();
    assert_eq!(r.bytes, b"last");
    assert!(matches!(r.origin, ResourceOrigin::Package { .. }));
}

#[test]
fn positive_archive_precedes_direct_and_zero_archive_falls_back() {
    let f = Fixture::new();
    let direct = f.root.join("collision.txt");
    f.write("collision.txt", b"direct");
    let key = direct.to_str().unwrap();
    f.write("positive.pkg", package(&[(key, b"archive")]));
    f.write("zero.pkg", package(&[(key, b"")]));
    let mut vfs = VirtualFileSystem::new();
    vfs.mount_package(&f.root.join("positive.pkg")).unwrap();
    assert_eq!(vfs.read(key).unwrap().bytes, b"archive");
    vfs.mount_package(&f.root.join("zero.pkg")).unwrap();
    let r = vfs.read(key).unwrap();
    assert_eq!(r.bytes, b"direct");
    assert!(matches!(r.origin, ResourceOrigin::Direct(_)));
}

#[test]
fn failed_package_remount_clears_the_previous_overlay() {
    let f = Fixture::new();
    f.write("fallback", b"loose");
    f.write("valid.pkg", package(&[("fallback", b"old overlay")]));
    f.write("broken.pkg", b"not a package");
    let mut vfs = VirtualFileSystem::new();
    vfs.mount_directory_front(&f.root);
    vfs.mount_package(&f.root.join("valid.pkg")).unwrap();
    assert_eq!(vfs.read("fallback").unwrap().bytes, b"old overlay");
    assert!(vfs.mount_package(&f.root.join("broken.pkg")).is_err());
    assert_eq!(vfs.read("fallback").unwrap().bytes, b"loose");
}

#[test]
fn directory_front_priority_and_zero_last_duplicate_fallback() {
    let f = Fixture::new();
    f.write("assets/shared", b"assets");
    f.write("project/shared", b"project");
    f.write(
        "x.pkg",
        package(&[("shared", b"positive"), ("SHARED", b"")]),
    );
    let mut vfs = VirtualFileSystem::new();
    vfs.mount_directory_front(f.root.join("assets"));
    vfs.mount_directory_front(f.root.join("project"));
    vfs.mount_package(&f.root.join("x.pkg")).unwrap();
    let r = vfs.read("shared").unwrap();
    assert_eq!(r.bytes, b"project");
    assert!(matches!(r.origin, ResourceOrigin::Directory { .. }));
}

#[test]
fn complete_packaged_scene_chain_beats_conflicting_loose_files() {
    let f = Fixture::ordinary();
    let names = [
        "scene.json",
        "models/image.json",
        "materials/image.json",
        "materials/owned.quadrants.tex",
    ];
    let data = names.map(|n| fs::read(f.root.join(n)).unwrap());
    let rows = names
        .iter()
        .zip(data.iter())
        .map(|(n, b)| (*n, b.as_slice()))
        .collect::<Vec<_>>();
    f.write("scene.pkg", package(&rows));
    f.json(
        "project.json",
        &json!({"type":"scene","file":"scene.pkg","title":"Packaged"}),
    );
    f.write("scene.json", b"not valid json");
    f.write("materials/owned.quadrants.tex", b"broken loose texture");
    let project = f.project();
    assert!(project.entry.ends_with("scene.pkg"));
    let plan = load_scene(&project).unwrap();
    assert_eq!(plan.quads.len(), 1);
    assert_eq!(
        pixel(&render_software(&plan, 4, 4).unwrap(), 0, 0),
        [255, 0, 0, 255]
    );
}

#[test]
fn project_kind_and_physical_web_entry_are_preserved_without_execution() {
    let f = Fixture::new();
    f.json(
        "project.json",
        &json!({"type":"web","file":"nested/index.html","title":"Web"}),
    );
    f.write("nested/index.html", b"<p>owned</p>");
    let p = f.project();
    assert_eq!(p.kind, WallpaperKind::Web);
    assert_eq!(p.entry, f.root.join("nested/index.html"));
    assert!(load_scene(&p).is_err());
    f.json(
        "project.json",
        &json!({"type":"application","file":"never-start.exe"}),
    );
    assert_eq!(f.project().kind, WallpaperKind::Application);
}

#[cfg(unix)]
#[test]
fn physical_web_entry_is_canonical_when_project_directory_is_a_symlink() {
    let f = Fixture::new();
    f.json(
        "real/project.json",
        &json!({"type":"web","file":"nested/index.html","title":"Symlink"}),
    );
    f.write("real/nested/index.html", b"<p>owned</p>");
    std::os::unix::fs::symlink(f.root.join("real"), f.root.join("alias")).unwrap();
    let p = load_project(&f.root.join("alias"), None).unwrap();
    assert_eq!(p.root, f.root.join("real"));
    assert_eq!(p.entry, f.root.join("real/nested/index.html"));
    assert_eq!(p.kind, WallpaperKind::Web);
}

fn full_quad(image: Arc<RgbaImage>) -> Quad {
    Quad {
        name: "test".into(),
        vertices: [
            Vertex {
                position: [-1.0, -1.0],
                uv: [0.0, 1.0],
            },
            Vertex {
                position: [1.0, -1.0],
                uv: [1.0, 1.0],
            },
            Vertex {
                position: [-1.0, 1.0],
                uv: [0.0, 0.0],
            },
            Vertex {
                position: [1.0, 1.0],
                uv: [1.0, 0.0],
            },
        ],
        image,
        color: [1.0; 4],
    }
}
fn transparent_plan(quads: Vec<Quad>) -> RenderPlan {
    RenderPlan {
        width: 2.0,
        height: 2.0,
        clear_color: [0.0; 4],
        quads,
        issues: Vec::new(),
    }
}

#[test]
fn source_over_alpha_and_diagonal_are_composited_once() {
    let image = Arc::new(RgbaImage {
        width: 1,
        height: 1,
        rgba: vec![255, 0, 0, 128],
    });
    let single = render_software(&transparent_plan(vec![full_quad(image.clone())]), 2, 2).unwrap();
    for p in single.rgba.chunks_exact(4) {
        assert_eq!(p, [255, 0, 0, 128]);
    }
    let double = render_software(
        &transparent_plan(vec![full_quad(image.clone()), full_quad(image)]),
        2,
        2,
    )
    .unwrap();
    for p in double.rgba.chunks_exact(4) {
        assert_eq!(p, [255, 0, 0, 192]);
    }
}

#[test]
fn source_over_blue_clear_and_color_modulation() {
    let mut q = full_quad(Arc::new(RgbaImage {
        width: 1,
        height: 1,
        rgba: vec![255, 0, 0, 128],
    }));
    q.color = [0.5, 1.0, 1.0, 1.0];
    let mut p = transparent_plan(vec![q]);
    p.clear_color = [0.0, 0.0, 1.0, 1.0];
    assert_eq!(
        pixel(&render_software(&p, 1, 1).unwrap(), 0, 0),
        [64, 0, 127, 255]
    );
}

#[test]
fn negative_scale_mirrors_pixels() {
    let f = Fixture::ordinary();
    let mut scene = f.read_json("scene.json");
    scene["objects"][0]["scale"] = json!("-1 1 1");
    f.json("scene.json", &scene);
    assert_eq!(
        pixel(&render_software(&f.plan(), 4, 4).unwrap(), 0, 0),
        [0, 255, 0, 255]
    );
}

#[test]
fn degenerate_quad_is_empty_and_nonaffine_nonfinite_or_bad_rgba_reject() {
    let mut q = full_quad(Arc::new(RgbaImage {
        width: 1,
        height: 1,
        rgba: vec![255; 4],
    }));
    q.vertices[1] = q.vertices[0];
    q.vertices[3] = q.vertices[2];
    assert!(
        render_software(&transparent_plan(vec![q]), 1, 1)
            .unwrap()
            .rgba
            .iter()
            .all(|b| *b == 0)
    );
    let mut q = full_quad(Arc::new(RgbaImage {
        width: 1,
        height: 1,
        rgba: vec![255; 4],
    }));
    q.vertices[3].position[0] = 0.0;
    assert!(render_software(&transparent_plan(vec![q.clone()]), 1, 1).is_err());
    q.vertices[3].position[0] = f32::NAN;
    assert!(render_software(&transparent_plan(vec![q]), 1, 1).is_err());
    let q = full_quad(Arc::new(RgbaImage {
        width: 2,
        height: 2,
        rgba: vec![0; 4],
    }));
    assert!(render_software(&transparent_plan(vec![q]), 1, 1).is_err());
    assert!(render_software(&transparent_plan(vec![]), 0, 1).is_err());
    assert!(render_software(&transparent_plan(vec![]), u32::MAX, u32::MAX).is_err());
}

#[test]
fn direct_png_convenience_profile_is_not_silent_native_scene_support() {
    let f = Fixture::new();
    let path = f.root.join("input.png");
    image::save_buffer(&path, &[7, 11, 13, 255], 1, 1, image::ColorType::Rgba8).unwrap();
    let p = load_project(Path::new(&path), None).unwrap();
    let plan = load_scene(&p).unwrap();
    assert!(
        plan.issues
            .iter()
            .any(|i| i.code == "standalone-image-profile")
    );
    assert_eq!(render_software(&plan, 1, 1).unwrap().rgba, [7, 11, 13, 255]);
}

#[test]
fn finite_transform_or_color_overflow_excludes_only_bad_objects() {
    let f = Fixture::ordinary();
    let mut scene = f.read_json("scene.json");
    let mut healthy = scene["objects"][0].clone();
    healthy["name"] = json!("healthy");
    let mut bad_transform = healthy.clone();
    bad_transform["name"] = json!("bad-transform");
    bad_transform["size"] = json!("10000000 10000000");
    bad_transform["scale"] = json!("1e38 1e38 1");
    let mut bad_color = healthy.clone();
    bad_color["name"] = json!("bad-color");
    bad_color["color"] = json!("1e38 1e38 1e38");
    bad_color["brightness"] = json!(1e38);
    scene["objects"] = json!([bad_transform, bad_color, healthy]);
    f.json("scene.json", &scene);
    let plan = f.plan();
    assert_eq!(plan.quads.len(), 1);
    assert_eq!(plan.quads[0].name, "healthy");
    assert!(plan.quads.iter().all(|q| {
        q.color.iter().all(|v| v.is_finite())
            && q.vertices
                .iter()
                .all(|v| v.position.iter().chain(v.uv.iter()).all(|n| n.is_finite()))
    }));
    for object in ["bad-transform", "bad-color"] {
        assert!(
            plan.issues
                .iter()
                .any(|i| i.object.as_deref() == Some(object) && i.message.contains("overflow")),
            "missing overflow issue for {object}"
        );
    }
    let image = render_software(&plan, 4, 4).unwrap();
    assert_eq!(pixel(&image, 0, 0), [255, 0, 0, 255]);
    assert_eq!(pixel(&image, 3, 3), [255, 255, 255, 255]);
}

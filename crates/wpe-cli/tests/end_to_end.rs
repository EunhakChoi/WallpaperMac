use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static SERIAL: AtomicU64 = AtomicU64::new(0);

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "wpe-cli-{}-{stamp}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        if self.0.exists() {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
}

fn run(arguments: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wpe"))
        .args(arguments)
        .output()
        .expect("run CLI")
}

fn successful(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("structured output")
}

#[test]
fn generated_native_resource_chain_renders_a_composite() {
    let directory = Directory::new();
    let root = directory.0.as_os_str();
    successful(run(&["demo".as_ref(), root]));
    let inspect = successful(run(&["inspect".as_ref(), root]));
    assert_eq!(inspect["scene"]["quads"].as_array().unwrap().len(), 2);
    // The dotted texture basename must survive native suffix appending.
    assert!(directory.0.join("materials/demo.gradient.tex").is_file());
    let image_path = directory.0.join("preview.png");
    let report = successful(run(&[
        "render".as_ref(),
        root,
        "--output".as_ref(),
        image_path.as_os_str(),
        "--width".as_ref(),
        "480".as_ref(),
        "--height".as_ref(),
        "270".as_ref(),
    ]));
    assert_eq!(report["quads"], 2);
    let image = image::open(image_path).unwrap().to_rgba8();
    assert_eq!(image.dimensions(), (480, 270));
    let center = image.get_pixel(240, 135).0;
    let edge = image.get_pixel(8, 8).0;
    assert!(
        center[0] > edge[0].saturating_add(40),
        "foreground card was not composited: {center:?} / {edge:?}"
    );
    assert!(
        image.pixels().all(|p| p.0[3] == 255),
        "opaque background should leave opaque output"
    );
    let corpus = successful(run(&["corpus".as_ref(), root]));
    assert_eq!(corpus["texture_occurrences"], 11);
    assert_eq!(corpus["passed"], true);
}

#[test]
fn generated_dynamic_scene_changes_pixels_and_effects_compile_through_the_cli() {
    let directory = Directory::new();
    successful(run(&["demo".as_ref(), directory.0.as_os_str()]));
    let animated = directory.0.join("animated");
    let zero = directory.0.join("zero.png");
    let later = directory.0.join("later.png");
    let first = successful(run(&[
        "frame".as_ref(),
        animated.as_os_str(),
        "--output".as_ref(),
        zero.as_os_str(),
    ]));
    let second = successful(run(&[
        "frame".as_ref(),
        animated.as_os_str(),
        "--time".as_ref(),
        "0.25".as_ref(),
        "--output".as_ref(),
        later.as_os_str(),
    ]));
    assert_eq!(first["frames"][0]["issues"], serde_json::json!([]));
    assert_eq!(second["frames"][0]["issues"], serde_json::json!([]));
    let a = image::open(zero).unwrap().to_rgba8();
    let b = image::open(later).unwrap().to_rgba8();
    let changes = a.pixels().zip(b.pixels()).filter(|(a, b)| a != b).count();
    assert!(
        changes > 1000,
        "script/particle motion did not alter pixels: {changes}"
    );
    let output = directory.0.join("effects.json");
    let simulated = run(&[
        "simulate".as_ref(),
        directory.0.join("effects").as_os_str(),
        "--output".as_ref(),
        output.as_os_str(),
    ]);
    assert!(
        simulated.status.success(),
        "{}",
        String::from_utf8_lossy(&simulated.stderr)
    );
    let effects: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(effects["frames"][0]["issues"], serde_json::json!([]));
    assert!(
        effects["frames"][0]["translated_shader_draws"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(effects["frames"][0]["passes"].as_u64().unwrap() >= 5);
}

#[test]
fn an_unsupported_layer_remains_visible_in_the_compatibility_report() {
    let directory = Directory::new();
    let root = directory.0.as_os_str();
    successful(run(&["demo".as_ref(), root]));
    let scene_path = directory.0.join("scene.json");
    let mut scene: Value = serde_json::from_slice(&std::fs::read(&scene_path).unwrap()).unwrap();
    scene["objects"].as_array_mut().unwrap().push(serde_json::json!({"id":99,"name":"Additional layer","particle":"particles/not-implemented.json","origin":"0 0 0"}));
    std::fs::write(scene_path, serde_json::to_vec(&scene).unwrap()).unwrap();
    let report = successful(run(&["inspect".as_ref(), root]));
    assert_eq!(report["scene"]["quads"].as_array().unwrap().len(), 2);
    let issues = report["scene"]["issues"]
        .as_array()
        .expect("compatibility issue list");
    assert!(
        issues
            .iter()
            .any(|v| v["code"] == "unsupported-object" && v["object"] == "Additional layer"),
        "missing explicit unsupported layer: {issues:?}"
    );
}

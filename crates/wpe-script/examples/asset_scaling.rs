//! Diagnostic for the real multi-realm path. VmRSS is observational, not a portable assertion.
use serde_json::json;
use std::collections::BTreeMap;
use wpe_script::{FrameInput, RuntimeOptions, SceneRuntime, collect_bindings};
fn rss() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find(|l| l.starts_with("VmRSS:"))?
        .split_whitespace()
        .nth(1)?
        .parse::<u64>()
        .ok()
        .map(|k| k * 1024)
}
fn main() {
    let mut options = RuntimeOptions::default();
    let filler = "x".repeat(16 * 1024);
    for n in 0..1024 {
        options.assets.insert(
            format!("library/{n}.json"),
            json!({"name":format!("asset{n}"),"payload":filler}),
        );
    }
    let library_bytes = serde_json::to_vec(&options.assets).unwrap().len();
    let objects=(1..=64).map(|id|json!({"id":id,"alpha":{"value":1,"script":"export function init(v){return v}export function update(v){return v+1}"}})).collect::<Vec<_>>();
    let scene = json!({"objects":objects});
    let before = rss();
    let start = std::time::Instant::now();
    let bindings = collect_bindings(&scene);
    let mut runtime = SceneRuntime::new(scene, BTreeMap::new(), options).unwrap();
    for binding in bindings {
        runtime.attach(binding).unwrap();
    }
    let initialized = runtime.initialize().unwrap();
    let mut issues = initialized.diagnostics.len();
    for _ in 0..3 {
        issues += runtime
            .step(FrameInput::default())
            .unwrap()
            .diagnostics
            .len();
    }
    let after = rss();
    println!("{}",serde_json::to_string_pretty(&json!({"kind":"our-VM-shared-asset-multi-realm-observation","asset_count":1024,"serialized_library_bytes":library_bytes,"bindings":64,"frames":3,"diagnostics":issues,"rss_before_bytes":before,"rss_after_bytes":after,"elapsed_seconds":start.elapsed().as_secs_f64(),"asset_strategy":"one Rust Arc library + lazy per-request JSON conversion; no complete per-realm library"})).unwrap());
}

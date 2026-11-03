//! Read-only, bounded execution of supplied inline scripts in our VM only.
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};
use wpe_script::{FrameInput, RuntimeOptions, SceneRuntime, collect_bindings};
fn read_json(path: &str) -> Option<Value> {
    wpe_formats::jsonc::parse(&fs::read(path).ok()?).ok()
}
fn run_case(case: &Value) -> Value {
    let name = case["id"].as_str().unwrap();
    let mut modules = BTreeMap::new();
    let mut assets = BTreeMap::new();
    let mut resource_names = BTreeSet::new();
    for (module, path) in case["modules"].as_object().into_iter().flatten() {
        if let Ok(source) = fs::read_to_string(path.as_str().unwrap()) {
            modules.insert(module.clone(), source);
        }
    }
    for (module, path) in case["assets"].as_object().into_iter().flatten() {
        if let Some(value) = read_json(path.as_str().unwrap()) {
            assets.insert(module.to_ascii_lowercase(), value);
        }
    }
    let mut package_scene = None;
    if let Some(path) = case["package"].as_str() {
        let package = match fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| wpe_formats::pkg::Package::parse(bytes).map_err(|e| e.to_string()))
        {
            Ok(package) => package,
            Err(error) => return json!({"id":name,"stage":"package","error":error}),
        };
        for (index, member) in package.members().iter().enumerate() {
            resource_names.insert(member.name.replace('\\', "/").to_ascii_lowercase());
            let bytes = package.member_data(index).unwrap();
            if bytes.is_empty() {
                continue;
            }
            let key = member.name.replace('\\', "/").to_ascii_lowercase();
            if key.ends_with(".json") {
                if let Ok(value) = wpe_formats::jsonc::parse(bytes) {
                    if key == "scene.json" {
                        package_scene = Some(value.clone())
                    }
                    assets.insert(key, value);
                }
            } else if key.ends_with(".js")
                && let Ok(source) = std::str::from_utf8(bytes)
            {
                modules.insert(key, source.into());
            }
        }
    }
    let scene = if let Some(scene) = package_scene {
        scene
    } else if let Some(path) = case["scene"].as_str() {
        match read_json(path) {
            Some(scene) => scene,
            None => return json!({"id":name,"stage":"scene","error":"missing or invalid scene"}),
        }
    } else {
        return json!({"id":name,"stage":"scene","error":"package has no scene.json"});
    };
    let bindings = collect_bindings(&scene);
    if bindings.is_empty() {
        return json!({"id":name,"bindings":0,"scope":"no inline scripts"});
    }
    let options = RuntimeOptions {
        assets,
        resource_names,
        user_property_definitions: case["property_definitions"]
            .as_object()
            .map(|v| v.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default(),
        loop_iterations: 20_000,
        max_jobs: 2_000,
        ..Default::default()
    };
    let mut runtime = match SceneRuntime::new(scene, modules, options) {
        Ok(runtime) => runtime,
        Err(error) => return json!({"id":name,"stage":"runtime","error":error.to_string()}),
    };
    let mut input = FrameInput {
        delta_seconds: 1.0 / 60.0,
        ..Default::default()
    };
    if let Some(properties) = case.get("properties") {
        input.user_properties = properties.clone()
    }
    runtime.set_initial_input(input.clone()).unwrap();
    let mut attached = 0;
    let mut failures = Vec::new();
    for binding in bindings.iter().take(512) {
        let id = binding.id.clone();
        match runtime.attach(binding.clone()) {
            Ok(()) => attached += 1,
            Err(error) => {
                failures.push(json!({"binding":id,"stage":"attach","error":error.to_string()}))
            }
        }
    }
    let mut diagnostics = Vec::new();
    let mut commands = 0;
    match runtime.initialize() {
        Ok(output) => {
            commands += output.commands.len();
            diagnostics.extend(output.diagnostics)
        }
        Err(error) => failures.push(json!({"stage":"initialize","error":error.to_string()})),
    }
    for frame in 0..3 {
        input.runtime_seconds = frame as f64 / 60.0;
        match runtime.step(input.clone()) {
            Ok(output) => {
                commands += output.commands.len();
                diagnostics.extend(output.diagnostics)
            }
            Err(error) => {
                failures.push(json!({"stage":"frame","error":error.to_string()}));
                break;
            }
        }
    }
    if let Err(error) = runtime.destroy() {
        failures.push(json!({"stage":"destroy","error":error.to_string()}))
    }
    json!({"id":name,"bindings":bindings.len(),"attached":attached,"failures":failures,"diagnostics":diagnostics,"host_commands":commands})
}
fn main() {
    let manifest = std::env::args().nth(1).expect("corpus manifest JSON");
    let manifest: Value = serde_json::from_slice(&fs::read(manifest).unwrap()).unwrap();
    let reports = manifest["cases"]
        .as_array()
        .unwrap()
        .iter()
        .take(256)
        .map(run_case)
        .collect::<Vec<_>>();
    println!("{}",serde_json::to_string_pretty(&json!({"kind":"our-VM-original-inline-script-probe","original_native_executed":false,"frames_per_scene":3,"reports":reports})).unwrap());
}

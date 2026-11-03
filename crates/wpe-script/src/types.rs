use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeOptions {
    pub max_module_bytes: usize,
    pub max_modules: usize,
    pub loop_iterations: u64,
    pub recursion_limit: usize,
    pub max_jobs: usize,
    pub max_output_bytes: usize,
    pub is_editor: bool,
    pub is_desktop: bool,
    pub is_wallpaper: bool,
    #[serde(default)]
    pub assets: BTreeMap<String, Value>,
    /// Supplied VFS names, including binary descriptors. Shared by Rust realms;
    /// never serialized into each JavaScript host state.
    #[serde(default, skip_serializing)]
    pub resource_names: BTreeSet<String>,
    #[serde(default)]
    pub user_property_definitions: BTreeMap<String, Value>,
}
impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            max_module_bytes: 8 * 1024 * 1024,
            max_modules: 512,
            loop_iterations: 100_000,
            recursion_limit: 128,
            max_jobs: 10_000,
            max_output_bytes: 16 * 1024 * 1024,
            is_editor: false,
            is_desktop: true,
            is_wallpaper: true,
            assets: BTreeMap::new(),
            resource_names: BTreeSet::new(),
            user_property_definitions: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScriptBinding {
    pub id: String,
    pub module: String,
    pub object_id: Value,
    /// JSON pointer to the bound property, preserving an optional `value` wrapper.
    pub path: String,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrameInput {
    pub delta_seconds: f32,
    pub runtime_seconds: f64,
    pub time_of_day: f32,
    pub screen_resolution: [f32; 2],
    pub canvas_size: [f32; 2],
    pub cursor_screen: [f32; 2],
    pub cursor_world: [f32; 3],
    pub cursor_left_down: bool,
    pub audio_left: Vec<f32>,
    pub audio_right: Vec<f32>,
    #[serde(default)]
    pub user_properties: Value,
    #[serde(default)]
    pub general_settings: Value,
    #[serde(default)]
    pub media: Value,
}
impl Default for FrameInput {
    fn default() -> Self {
        Self {
            delta_seconds: 0.0,
            runtime_seconds: 0.0,
            time_of_day: 0.0,
            screen_resolution: [1920.0, 1080.0],
            canvas_size: [1920.0, 1080.0],
            cursor_screen: [0.0; 2],
            cursor_world: [0.0; 3],
            cursor_left_down: false,
            audio_left: vec![0.0; 64],
            audio_right: vec![0.0; 64],
            user_properties: json!({}),
            general_settings: json!({}),
            media: json!({}),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StatePatch {
    pub path: String,
    pub value: Value,
    #[serde(default)]
    pub remove: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostCommand {
    pub kind: String,
    pub object_id: Value,
    pub arguments: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    pub binding: String,
    pub hook: String,
    pub message: String,
    pub level: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HookResult {
    pub binding: String,
    pub object_id: Value,
    pub hook: String,
    pub value: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrameOutput {
    pub scene: Value,
    pub patches: Vec<StatePatch>,
    pub commands: Vec<HostCommand>,
    pub diagnostics: Vec<Diagnostic>,
    pub storage: Value,
    pub hook_results: Vec<HookResult>,
}

/// Discover inline property scripts without executing them. Paths include nested
/// effect/material/particle properties; thisObject addresses their owning object.
pub fn collect_bindings(scene: &Value) -> Vec<ScriptBinding> {
    fn walk(v: &Value, path: &str, id: &Value, out: &mut Vec<ScriptBinding>) {
        match v {
            Value::Object(map) => {
                if let Some(source) = map.get("script").and_then(Value::as_str) {
                    let n = out.len();
                    out.push(ScriptBinding {
                        id: format!("binding-{n}"),
                        module: format!("scripts/__binding_{n}.js"),
                        object_id: id.clone(),
                        path: path.into(),
                        source: Some(source.into()),
                    });
                }
                for (k, v) in map {
                    if k != "script" {
                        walk(
                            v,
                            &format!("{path}/{}", k.replace('~', "~0").replace('/', "~1")),
                            id,
                            out,
                        )
                    }
                }
            }
            Value::Array(a) => {
                for (i, v) in a.iter().enumerate() {
                    walk(v, &format!("{path}/{i}"), id, out)
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    if let Some(objects) = scene.get("objects").and_then(Value::as_array) {
        for (i, v) in objects.iter().enumerate() {
            walk(
                v,
                &format!("/objects/{i}"),
                v.get("id").unwrap_or(&Value::Null),
                &mut out,
            )
        }
    }
    out
}

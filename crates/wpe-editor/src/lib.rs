//! Project document editing and host-service requests. Original editor code is never executed.
mod actions;
mod commands;
mod geometry;
mod image_tools;
pub mod model_tools;
mod resources;
pub mod surface;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

#[derive(Debug)]
pub struct Error {
    pub code: String,
    pub message: String,
}
pub type Result<T> = std::result::Result<T, Error>;
impl Error {
    pub(crate) fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EditorEvent {
    pub kind: String,
    pub payload: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EditorReply {
    pub value: Value,
    pub events: Vec<EditorEvent>,
    pub revision: u64,
    pub dirty: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EditorSnapshot {
    pub project: Value,
    pub scene: Value,
    pub revision: u64,
    pub dirty: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub resources: BTreeMap<String, Value>,
    pub session: Value,
}
#[derive(Clone)]
struct State {
    project: Value,
    scene: Value,
    resources: BTreeMap<String, Option<Value>>,
    files: BTreeMap<String, Option<Arc<Vec<u8>>>>,
    dirty: bool,
    pending: f64,
}
#[derive(Clone, Debug)]
struct Edit {
    document: String,
    path: String,
    before: Option<Value>,
    after: Option<Value>,
}
#[derive(Clone, Debug)]
struct FileEdit {
    path: String,
    before: Option<Arc<Vec<u8>>>,
    after: Option<Arc<Vec<u8>>>,
}
#[derive(Clone, Debug)]
struct Command {
    edits: Vec<Edit>,
    files: Vec<FileEdit>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostRequest {
    pub request_id: u64,
    pub method: String,
    pub args: Value,
}
/// All writable resources stay beneath `root`. Additional mounts are read-only;
/// editing one creates a project-local copy. No absolute import is implicit.
pub struct Editor {
    root: PathBuf,
    initial_scene: Value,
    state: State,
    saved_state: State,
    original_resources: BTreeMap<String, Option<Value>>,
    original_files: BTreeMap<String, Option<Arc<Vec<u8>>>>,
    asset_roots: Vec<PathBuf>,
    revision: u64,
    undo: Vec<Command>,
    redo: Vec<Command>,
    transaction: Option<State>,
    session: Value,
    events: Vec<EditorEvent>,
    next_request: u64,
    pending_requests: BTreeMap<u64, HostRequest>,
}
impl Editor {
    pub fn new(root: PathBuf, project: Value, scene: Value) -> Result<Self> {
        if !project.is_object()
            || !scene.is_object()
            || !scene.get("objects").is_none_or(Value::is_array)
        {
            return Err(Error::new(
                "invalid-document",
                "object documents and scene.objects array required",
            ));
        }
        validate_documents(&project, &scene)?;
        let state = State {
            project,
            scene,
            resources: BTreeMap::new(),
            files: BTreeMap::new(),
            dirty: false,
            pending: 0.0,
        };
        Ok(Self {
            root,
            initial_scene: state.scene.clone(),
            saved_state: state.clone(),
            state,
            original_resources: BTreeMap::new(),
            original_files: BTreeMap::new(),
            asset_roots: Vec::new(),
            revision: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            transaction: None,
            session: json!({"selection":[],"view":{},"tools":{},"particle":null,"model":null}),
            events: Vec::new(),
            next_request: 1,
            pending_requests: BTreeMap::new(),
        })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn mount_asset_root(&mut self, path: PathBuf) -> Result<()> {
        if !path.is_dir() {
            return Err(Error::new("path", "asset mount is not a directory"));
        }
        self.asset_roots.insert(0, path);
        Ok(())
    }
    pub fn snapshot(&self) -> EditorSnapshot {
        EditorSnapshot {
            project: self.state.project.clone(),
            scene: self.state.scene.clone(),
            revision: self.revision,
            dirty: self.state.dirty,
            can_undo: !self.undo.is_empty(),
            can_redo: !self.redo.is_empty(),
            resources: self
                .state
                .resources
                .iter()
                .filter_map(|(k, v)| v.clone().map(|v| (k.clone(), v)))
                .collect(),
            session: self.session.clone(),
        }
    }
    /// Apply transient editor preview controls to a copy; saves/history retain the
    /// authored scene. Feed this scene to the actual ScenePlayer preview.
    pub fn preview_scene(&self) -> Value {
        let mut scene = self.state.scene.clone();
        for method in ["setAnimationPreviewFrame", "setAnimationPreviewPlayback"] {
            let Some(args) = self.session["tools"][method].as_array() else {
                continue;
            };
            let configs = if let Some(array) = args.first().and_then(Value::as_array) {
                array.clone()
            } else {
                args.clone()
            };
            for config in configs {
                let Some(id) = config.get("id") else { continue };
                let Some(object) = scene
                    .get_mut("objects")
                    .and_then(Value::as_array_mut)
                    .and_then(|objects| objects.iter_mut().find(|v| equal_id(&v["id"], id)))
                else {
                    continue;
                };
                let key = config.get("key").and_then(Value::as_str).unwrap_or("");
                let value = if key.starts_with('/') {
                    object.pointer_mut(key)
                } else {
                    object.get_mut(key)
                };
                let Some(animation) = value
                    .and_then(|v| v.get_mut("animation"))
                    .and_then(Value::as_object_mut)
                else {
                    continue;
                };
                if let Some(frame) = config
                    .get("frame")
                    .and_then(Value::as_f64)
                    .filter(|n| n.is_finite())
                {
                    animation.insert("__previewFrame".into(), json!(frame));
                    animation.insert("__previewPlaying".into(), json!(false));
                }
                if let Some(play) = config.get("play").and_then(Value::as_bool) {
                    animation.insert("__previewPlaying".into(), json!(play));
                }
            }
        }
        scene
    }
    pub fn execute(&mut self, method: &str, args: Value) -> Result<EditorReply> {
        let args = args.as_array().cloned().unwrap_or_else(|| vec![args]);
        let before = self.state.clone();
        let previous_session = self.session.clone();
        let old_revision = self.revision;
        let event_start = self.events.len();
        let result = self.route(method, &args);
        let result = result.and_then(|value| {
            validate_documents(&self.state.project, &self.state.scene)?;
            Ok(value)
        });
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                self.state = before;
                self.session = previous_session;
                self.events.truncate(event_start);
                return Err(error);
            }
        };
        if !matches!(
            method,
            "undo"
                | "redo"
                | "performUndoRedo"
                | "applyPaintHistory"
                | "beginTransaction"
                | "commitTransaction"
                | "rollbackTransaction"
                | "forceSave"
                | "saveAs"
                | "createNewProject"
                | "openProject"
                | "closeProject"
        ) && self.changed(&before)
        {
            self.revision += 1;
            self.mark_dirty();
            if self.transaction.is_none() {
                self.record(&before)
            }
        }
        if self.revision != old_revision {
            self.events.push(EditorEvent {
                kind: "documentChanged".into(),
                payload: json!({"revision":self.revision,"dirty":self.state.dirty}),
            });
        }
        Ok(self.reply(value))
    }
    fn reply(&mut self, value: Value) -> EditorReply {
        EditorReply {
            value,
            events: std::mem::take(&mut self.events),
            revision: self.revision,
            dirty: self.state.dirty,
        }
    }
    pub fn pending_host_requests(&self) -> Vec<HostRequest> {
        self.pending_requests.values().cloned().collect()
    }
    pub fn complete_host_request(
        &mut self,
        request_id: u64,
        result: std::result::Result<Value, String>,
    ) -> Result<EditorReply> {
        let request = self
            .pending_requests
            .remove(&request_id)
            .ok_or_else(|| Error::new("host-request", "unknown or already completed request"))?;
        let payload = match result {
            Ok(value) => {
                json!({"request_id":request_id,"method":request.method,"ok":true,"value":value})
            }
            Err(message) => {
                json!({"request_id":request_id,"method":request.method,"ok":false,"error":{"code":"host-service","message":message}})
            }
        };
        self.events.push(EditorEvent {
            kind: "hostCompleted".into(),
            payload: payload.clone(),
        });
        Ok(self.reply(payload))
    }
    fn request_host(&mut self, method: &str, args: &[Value]) -> Value {
        let request_id = self.next_request;
        self.next_request += 1;
        let request = HostRequest {
            request_id,
            method: method.into(),
            args: json!(args),
        };
        self.events.push(EditorEvent {
            kind: "hostCommand".into(),
            payload: serde_json::to_value(&request).unwrap(),
        });
        self.pending_requests.insert(request_id, request);
        json!({"pending":true,"request_id":request_id})
    }
    pub fn begin_transaction(&mut self) -> Result<()> {
        if self.transaction.is_some() {
            return Err(Error::new("transaction", "already active"));
        }
        self.transaction = Some(self.state.clone());
        Ok(())
    }
    pub fn commit(&mut self) -> Result<()> {
        let previous = self
            .transaction
            .take()
            .ok_or_else(|| Error::new("transaction", "none active"))?;
        self.record(&previous);
        Ok(())
    }
    pub fn rollback(&mut self) -> Result<()> {
        self.state = self
            .transaction
            .take()
            .ok_or_else(|| Error::new("transaction", "none active"))?;
        self.revision += 1;
        self.sanitize_selection();
        Ok(())
    }
    fn mark_dirty(&mut self) {
        self.state.dirty = self.changed(&self.saved_state);
        self.state.pending = if self.state.dirty { 10.0 } else { 0.0 };
    }
    fn sanitize_selection(&mut self) {
        let selected = self.session["selection"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        self.session["selection"] = json!(
            selected
                .into_iter()
                .filter(|id| self.index(id).is_ok() || self.resource_path(id).is_some())
                .collect::<Vec<_>>()
        );
    }
    pub fn undo(&mut self) -> Result<bool> {
        if self.transaction.is_some() {
            return Err(Error::new("transaction", "commit or rollback first"));
        }
        let Some(command) = self.undo.pop() else {
            return Ok(false);
        };
        self.apply_command(&command, false)?;
        self.redo.push(command);
        self.revision += 1;
        self.mark_dirty();
        self.sanitize_selection();
        Ok(true)
    }
    pub fn redo(&mut self) -> Result<bool> {
        if self.transaction.is_some() {
            return Err(Error::new("transaction", "commit or rollback first"));
        }
        let Some(command) = self.redo.pop() else {
            return Ok(false);
        };
        self.apply_command(&command, true)?;
        self.undo.push(command);
        self.revision += 1;
        self.mark_dirty();
        self.sanitize_selection();
        Ok(true)
    }
    fn apply_command(&mut self, command: &Command, forward: bool) -> Result<()> {
        let edits: Box<dyn Iterator<Item = &Edit>> = if forward {
            Box::new(command.edits.iter())
        } else {
            Box::new(command.edits.iter().rev())
        };
        for edit in edits {
            let value = if forward {
                edit.after.clone()
            } else {
                edit.before.clone()
            };
            if edit.document == "project" {
                assign(&mut self.state.project, &edit.path, value)?
            } else if edit.document == "scene" {
                assign(&mut self.state.scene, &edit.path, value)?
            } else if edit.path.is_empty() {
                self.state.resources.insert(edit.document.clone(), value);
            } else {
                let doc = self
                    .state
                    .resources
                    .entry(edit.document.clone())
                    .or_insert_with(|| Some(json!({})))
                    .as_mut()
                    .ok_or_else(|| Error::new("history", "missing resource"))?;
                assign(doc, &edit.path, value)?
            }
        }
        for edit in &command.files {
            self.state.files.insert(
                edit.path.clone(),
                if forward {
                    edit.after.clone()
                } else {
                    edit.before.clone()
                },
            );
        }
        Ok(())
    }
    fn changed(&self, before: &State) -> bool {
        !self.edits(before).0.is_empty() || !self.edits(before).1.is_empty()
    }
    fn edits(&self, before: &State) -> (Vec<Edit>, Vec<FileEdit>) {
        let mut edits = Vec::new();
        diff(
            "project",
            "",
            Some(&before.project),
            Some(&self.state.project),
            &mut edits,
        );
        diff(
            "scene",
            "",
            Some(&before.scene),
            Some(&self.state.scene),
            &mut edits,
        );
        for key in before
            .resources
            .keys()
            .chain(self.state.resources.keys())
            .collect::<BTreeSet<_>>()
        {
            let a = before
                .resources
                .get(key)
                .or_else(|| self.original_resources.get(key))
                .and_then(Option::as_ref);
            let b = self.state.resources.get(key).and_then(Option::as_ref);
            diff(key, "", a, b, &mut edits);
        }
        let mut files = Vec::new();
        for key in before
            .files
            .keys()
            .chain(self.state.files.keys())
            .collect::<BTreeSet<_>>()
        {
            let a = before
                .files
                .get(key)
                .or_else(|| self.original_files.get(key))
                .cloned()
                .flatten();
            let b = self.state.files.get(key).cloned().flatten();
            if a != b {
                files.push(FileEdit {
                    path: key.clone(),
                    before: a,
                    after: b,
                });
            }
        }
        (edits, files)
    }
    fn record(&mut self, before: &State) {
        let (edits, files) = self.edits(before);
        if !edits.is_empty() || !files.is_empty() {
            if self.undo.len() > 100 {
                self.undo.remove(0);
            }
            self.undo.push(Command { edits, files });
            self.redo.clear();
        }
    }
    pub fn autosave(&mut self, elapsed: f64) -> Result<bool> {
        if !elapsed.is_finite() || elapsed < 0.0 {
            return Err(Error::new("clock", "finite nonnegative elapsed required"));
        }
        if !self.state.dirty || self.transaction.is_some() {
            return Ok(false);
        }
        self.state.pending -= elapsed;
        if self.state.pending <= 0.0 {
            self.save()?;
            return Ok(true);
        }
        Ok(false)
    }
    pub fn save(&mut self) -> Result<()> {
        if self.transaction.is_some() {
            return Err(Error::new("transaction", "cannot save uncommitted changes"));
        }
        let file = self
            .state
            .project
            .get("file")
            .and_then(Value::as_str)
            .unwrap_or("scene.json");
        if file.ends_with(".pkg") {
            return Err(Error::new(
                "packed-project",
                "unpack a working copy before editing",
            ));
        }
        let mut writes = BTreeMap::new();
        writes.insert(
            file.to_owned(),
            Some(serde_json::to_vec_pretty(&self.state.scene).map_err(json_error)?),
        );
        writes.insert(
            "project.json".into(),
            Some(serde_json::to_vec_pretty(&self.state.project).map_err(json_error)?),
        );
        for (name, value) in &self.state.resources {
            writes.insert(
                name.clone(),
                value
                    .as_ref()
                    .map(serde_json::to_vec_pretty)
                    .transpose()
                    .map_err(json_error)?,
            );
        }
        for (name, data) in &self.state.files {
            writes.insert(name.clone(), data.as_ref().map(|d| d.as_ref().clone()));
        }
        write_batch(&self.root, &writes, self.revision)?;
        self.state.dirty = false;
        self.state.pending = 0.0;
        self.saved_state = self.state.clone();
        self.events.push(EditorEvent {
            kind: "saved".into(),
            payload: json!({"revision":self.revision,"root":self.root}),
        });
        Ok(())
    }
    fn objects(&self) -> Result<&Vec<Value>> {
        self.state
            .scene
            .get("objects")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::new("scene", "objects array required"))
    }
    fn objects_mut(&mut self) -> Result<&mut Vec<Value>> {
        if self.state.scene.get("objects").is_none() {
            self.state.scene["objects"] = json!([])
        }
        self.state.scene["objects"]
            .as_array_mut()
            .ok_or_else(|| Error::new("scene", "objects array required"))
    }
    fn index(&self, id: &Value) -> Result<usize> {
        self.objects()?
            .iter()
            .position(|v| equal_id(&v["id"], id))
            .ok_or_else(|| Error::new("layer", format!("layer {id} not found")))
    }
    fn layer_mut(&mut self, id: &Value) -> Result<&mut Value> {
        let index = self.index(id)?;
        Ok(&mut self.objects_mut()?[index])
    }
    fn resource_path(&self, id: &Value) -> Option<String> {
        id.as_str().and_then(|s| {
            self.session
                .get("resourceHandles")
                .and_then(|v| v.get(s))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| s.contains('/').then(|| s.to_owned()))
        })
    }
    fn resource_handle(&mut self, path: &str) -> Value {
        if !self.session["resourceHandles"].is_object() {
            self.session["resourceHandles"] = json!({})
        }
        if let Some((id, _)) = self.session["resourceHandles"]
            .as_object()
            .unwrap()
            .iter()
            .find(|(_, value)| value.as_str() == Some(path))
        {
            return json!(id);
        }
        let id = format!(
            "resource-{}",
            self.session["resourceHandles"].as_object().unwrap().len() + 1
        );
        self.session["resourceHandles"][&id] = json!(path);
        json!(id)
    }
    fn selected(&self) -> Result<Value> {
        self.session["selection"]
            .as_array()
            .and_then(|v| v.first())
            .cloned()
            .ok_or_else(|| Error::new("selection", "select a layer"))
    }
    fn add_layer(&mut self, mut config: Value, parent: Option<&Value>) -> Result<Value> {
        if !config.is_object() {
            return Err(Error::new("layer", "layer object required"));
        }
        let id = self
            .objects()?
            .iter()
            .filter_map(|v| v["id"].as_u64())
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| Error::new("layer", "id overflow"))?;
        config["id"] = json!(id);
        if let Some(parent) = parent.filter(|v| !v.is_null()) {
            self.index(parent)?;
            config["parent"] = parent.clone();
        }
        self.objects_mut()?.push(config);
        self.session["selection"] = json!([id]);
        Ok(json!(id))
    }
}
pub(crate) fn parse_argument(v: Option<&Value>) -> Result<Value> {
    let value = v
        .cloned()
        .ok_or_else(|| Error::new("argument", "missing JSON argument"))?;
    if let Some(s) = value.as_str() {
        wpe_formats::jsonc::parse(s.as_bytes()).map_err(|e| Error::new("json", e.to_string()))
    } else {
        Ok(value)
    }
}
pub(crate) fn arg(args: &[Value], index: usize) -> Result<&Value> {
    args.get(index)
        .ok_or_else(|| Error::new("argument", format!("argument {index} required")))
}
pub(crate) fn string(args: &[Value], index: usize) -> Result<&str> {
    arg(args, index)?
        .as_str()
        .ok_or_else(|| Error::new("argument", format!("argument {index} must be string")))
}
pub(crate) fn number(args: &[Value], index: usize) -> Result<f64> {
    let n = arg(args, index)?
        .as_f64()
        .ok_or_else(|| Error::new("argument", format!("argument {index} must be numeric")))?;
    if !n.is_finite() {
        return Err(Error::new("argument", "finite value required"));
    }
    Ok(n)
}
pub(crate) fn equal_id(a: &Value, b: &Value) -> bool {
    a == b
        || a.as_u64()
            .zip(b.as_str().and_then(|s| s.parse::<u64>().ok()))
            .is_some_and(|(a, b)| a == b)
        || b.as_u64()
            .zip(a.as_str().and_then(|s| s.parse::<u64>().ok()))
            .is_some_and(|(a, b)| a == b)
}
pub(crate) fn json_error(e: serde_json::Error) -> Error {
    Error::new("json", e.to_string())
}
pub(crate) fn io_error(e: std::io::Error) -> Error {
    Error::new("io", e.to_string())
}
pub(crate) fn diff(
    doc: &str,
    path: &str,
    before: Option<&Value>,
    after: Option<&Value>,
    out: &mut Vec<Edit>,
) {
    if before == after {
        return;
    }
    if let (Some(Value::Object(a)), Some(Value::Object(b))) = (before, after) {
        for key in a.keys().chain(b.keys()).collect::<BTreeSet<_>>() {
            diff(
                doc,
                &format!("{path}/{}", key.replace('~', "~0").replace('/', "~1")),
                a.get(key),
                b.get(key),
                out,
            )
        }
    } else {
        out.push(Edit {
            document: doc.into(),
            path: path.into(),
            before: before.cloned(),
            after: after.cloned(),
        })
    }
}
pub(crate) fn assign(doc: &mut Value, path: &str, value: Option<Value>) -> Result<()> {
    if path.is_empty() {
        *doc = value.unwrap_or(Value::Null);
        return Ok(());
    }
    if !path.starts_with('/') {
        return Err(Error::new("pointer", "JSON pointer must start with /"));
    }
    let mut parts = path
        .split('/')
        .skip(1)
        .map(|p| p.replace("~1", "/").replace("~0", "~"))
        .collect::<Vec<_>>();
    let key = parts.pop().unwrap();
    let mut node = doc;
    for part in parts {
        if node.is_object() {
            if node.get(&part).is_none() {
                node[&part] = json!({})
            }
            node = node.get_mut(&part).unwrap()
        } else if let Some(a) = node.as_array_mut() {
            node = a
                .get_mut(
                    part.parse::<usize>()
                        .map_err(|_| Error::new("pointer", "array index"))?,
                )
                .ok_or_else(|| Error::new("pointer", "array index out of bounds"))?
        } else {
            return Err(Error::new("pointer", "parent is not container"));
        }
    }
    if let Some(map) = node.as_object_mut() {
        if let Some(v) = value {
            map.insert(key, v);
        } else {
            map.remove(&key);
        }
        Ok(())
    } else if let Some(a) = node.as_array_mut() {
        let i = key
            .parse::<usize>()
            .map_err(|_| Error::new("pointer", "array index"))?;
        if let Some(v) = value {
            if i == a.len() {
                a.push(v)
            } else {
                *a.get_mut(i)
                    .ok_or_else(|| Error::new("pointer", "array index out of bounds"))? = v
            }
        } else if i < a.len() {
            a.remove(i);
        } else {
            return Err(Error::new("pointer", "array index out of bounds"));
        }
        Ok(())
    } else {
        Err(Error::new("pointer", "parent is not container"))
    }
}
pub(crate) fn bounded(root: &Path, name: &str) -> Result<PathBuf> {
    if name.is_empty() || name.contains(['\\', ':', '\0']) {
        return Err(Error::new(
            "path",
            "nonempty forward-slash relative path required",
        ));
    }
    let p = Path::new(name);
    if p.is_absolute()
        || p.components()
            .any(|p| !matches!(p, Component::Normal(_) | Component::CurDir))
    {
        return Err(Error::new("path", "relative project path required"));
    }
    let joined = root.join(p);
    fn future_path(path: &Path) -> Result<PathBuf> {
        if fs::symlink_metadata(path).is_ok() {
            return path.canonicalize().map_err(io_error);
        }
        let parent = path
            .parent()
            .ok_or_else(|| Error::new("path", "no ancestor"))?;
        let name = path
            .file_name()
            .ok_or_else(|| Error::new("path", "filename required"))?;
        Ok(future_path(parent)?.join(name))
    }
    let canonical_root = future_path(root)?;
    if future_path(&joined)?.strip_prefix(&canonical_root).is_err() {
        return Err(Error::new("path", "symlink escapes project root"));
    }
    Ok(joined)
}
fn write_batch(
    root: &Path,
    writes: &BTreeMap<String, Option<Vec<u8>>>,
    revision: u64,
) -> Result<()> {
    fs::create_dir_all(root).map_err(io_error)?;
    struct TemporaryFiles(Vec<PathBuf>);
    impl Drop for TemporaryFiles {
        fn drop(&mut self) {
            for path in &self.0 {
                let _ = fs::remove_file(path);
            }
        }
    }
    let mut temporary_files = TemporaryFiles(Vec::new());
    let mut prepared = Vec::new();
    for (index, (name, data)) in writes.iter().enumerate() {
        let path = bounded(root, name)?;
        let parent = path
            .parent()
            .ok_or_else(|| Error::new("path", "parent required"))?;
        fs::create_dir_all(parent).map_err(io_error)?;
        let old = if path.exists() {
            Some(fs::read(&path).map_err(io_error)?)
        } else {
            None
        };
        let temporary = parent.join(format!(
            ".wpe-editor-{}-{revision}-{index}.tmp",
            std::process::id()
        ));
        temporary_files.0.push(temporary.clone());
        if let Some(bytes) = data {
            fs::write(&temporary, bytes).map_err(io_error)?;
        }
        prepared.push((path, temporary, old, data.is_some()));
    }
    for (committed, (path, temporary, _, write)) in prepared.iter().enumerate() {
        let result = if *write {
            fs::rename(temporary, path)
        } else if path.exists() {
            fs::remove_file(path)
        } else {
            Ok(())
        };
        if let Err(error) = result {
            for (path, _, old, _) in prepared[..committed].iter().rev() {
                if let Some(old) = old {
                    let _ = fs::write(path, old);
                } else {
                    let _ = fs::remove_file(path);
                }
            }
            for (_, temporary, _, _) in &prepared {
                let _ = fs::remove_file(temporary);
            }
            return Err(io_error(error));
        }
    }
    Ok(())
}

fn validate_documents(project: &Value, scene: &Value) -> Result<()> {
    if !project.is_object() || !scene.is_object() {
        return Err(Error::new("document", "project/scene object required"));
    }
    let objects = scene
        .get("objects")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::new("scene", "objects array required"))?;
    let mut ids = BTreeSet::new();
    for object in objects {
        if !object.is_object() {
            return Err(Error::new("layer", "layer object required"));
        }
        let id = object
            .get("id")
            .filter(|id| id.is_string() || id.is_u64())
            .ok_or_else(|| Error::new("layer", "string or nonnegative integer id required"))?;
        let key = id
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| id.to_string());
        if !ids.insert(key) {
            return Err(Error::new("layer", "duplicate layer id"));
        }
        for key in ["effects", "animationlayers"] {
            if let Some(values) = object.get(key)
                && values
                    .as_array()
                    .is_none_or(|v| v.iter().any(|v| !v.is_object()))
            {
                return Err(Error::new("layer", format!("{key} must contain objects")));
            }
        }
    }
    Ok(())
}

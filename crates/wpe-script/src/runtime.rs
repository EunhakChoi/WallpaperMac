use crate::{
    Diagnostic, Error, FrameInput, FrameOutput, HookResult, HostCommand, Result, RuntimeOptions,
    ScriptBinding, StatePatch,
};
use boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, Module, NativeFunction, Source,
    builtins::promise::PromiseState,
    job::{Job, JobExecutor, SimpleJobExecutor},
    js_string,
    module::{ModuleLoader, ModuleRequest, Referrer, resolve_module_specifier},
    object::{JsObject, ObjectInitializer},
    property::Attribute,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path},
    rc::Rc,
    sync::{Arc, atomic::Ordering},
};

const HOOKS: &[&str] = &[
    "init",
    "update",
    "destroy",
    "resizeScreen",
    "applyUserProperties",
    "applyGeneralSettings",
    "animationEvent",
    "cursorHitTest",
    "cursorEnter",
    "cursorLeave",
    "cursorMove",
    "cursorClick",
    "cursorDown",
    "cursorUp",
    "mediaStatusChanged",
    "mediaPlaybackChanged",
    "mediaPropertiesChanged",
    "mediaThumbnailChanged",
    "mediaTimelineChanged",
];

fn canonical(name: &str) -> Result<String> {
    if name.contains(['\0', ':', '\\']) || name.starts_with('/') {
        return Err(Error::new(
            "module path",
            "bounded forward-slash relative path required",
        ));
    }
    let mut parts = Vec::new();
    for part in Path::new(name).components() {
        match part {
            Component::Normal(s) => parts.push(
                s.to_str()
                    .ok_or_else(|| Error::new("module path", "UTF-8 required"))?,
            ),
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(Error::new("module path", "root traversal"));
                }
            }
            _ => return Err(Error::new("module path", "relative path required")),
        }
    }
    Ok(parts.join("/").to_ascii_lowercase())
}
fn scoped_resource(key: &str, namespace: Option<&str>) -> Option<String> {
    let namespace = namespace?;
    let (root, rest) = key.split_once('/')?;
    if rest.starts_with("workshop/") {
        return None;
    }
    Some(format!("{root}/{namespace}/{rest}"))
}
fn binding_namespace(scene: &Value, binding: &ScriptBinding) -> Option<String> {
    let mut pointer = binding.path.clone();
    loop {
        if let Some(value) = scene.pointer(&pointer) {
            for key in ["file", "image", "model", "particle", "sprite", "sound"] {
                let Some(path) = value
                    .get(key)
                    .map(|v| v.get("value").unwrap_or(v))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                let Ok(path) = canonical(path) else { continue };
                let parts = path.split('/').collect::<Vec<_>>();
                if let Some(index) = parts.iter().position(|part| *part == "workshop")
                    && let Some(id) = parts.get(index + 1).filter(|id| !id.is_empty())
                {
                    return Some(format!("workshop/{id}"));
                }
            }
        }
        if let Some(index) = pointer.rfind('/') {
            pointer.truncate(index);
        } else {
            break;
        }
        if pointer.is_empty() {
            break;
        }
    }
    scene
        .get("objects")
        .and_then(Value::as_array)?
        .iter()
        .find(|object| object.get("id") == Some(&binding.object_id))
        .and_then(|object| {
            for key in ["image", "model", "particle", "sprite", "sound"] {
                let Some(path) = object
                    .get(key)
                    .map(|v| v.get("value").unwrap_or(v))
                    .and_then(Value::as_str)
                else {
                    continue;
                };
                let Ok(path) = canonical(path) else { continue };
                let parts = path.split('/').collect::<Vec<_>>();
                if let Some(index) = parts.iter().position(|part| *part == "workshop")
                    && let Some(id) = parts.get(index + 1).filter(|id| !id.is_empty())
                {
                    return Some(format!("workshop/{id}"));
                }
            }
            None
        })
}

struct BoundedLoader {
    sources: Arc<BTreeMap<String, String>>,
    modules: RefCell<BTreeMap<String, Module>>,
    namespace: Option<String>,
}
impl ModuleLoader for BoundedLoader {
    fn load_imported_module(
        self: Rc<Self>,
        referrer: Referrer,
        request: ModuleRequest,
        context: &RefCell<&mut Context>,
    ) -> impl std::future::Future<Output = JsResult<Module>> {
        let result = (|| {
            let resolved = resolve_module_specifier(
                None,
                request.specifier(),
                referrer.path(),
                &mut context.borrow_mut(),
            )?;
            let mut key = canonical(&resolved.to_string_lossy())
                .map_err(|e| JsNativeError::typ().with_message(e.to_string()))?;
            if !self.sources.contains_key(&key)
                && let Some(scoped) = scoped_resource(&key, self.namespace.as_deref())
                && self.sources.contains_key(&scoped)
            {
                key = scoped;
            }
            if !self.sources.contains_key(&key) {
                let spec = request.specifier().to_std_string_escaped();
                if !spec.starts_with('.') && !spec.contains(':') && !spec.starts_with('/') {
                    let suffix = if spec.to_ascii_lowercase().ends_with(".js") {
                        spec
                    } else {
                        format!("{spec}.js")
                    };
                    let native = canonical(&format!("scripts/jsmodules/{suffix}"))
                        .map_err(|e| JsNativeError::typ().with_message(e.to_string()))?;
                    if self.sources.contains_key(&native) {
                        key = native;
                    }
                }
            }
            if let Some(module) = self.modules.borrow().get(&key).cloned() {
                return Ok(module);
            }
            let source = self.sources.get(&key).ok_or_else(|| {
                JsNativeError::typ().with_message(format!("module not supplied: {key}"))
            })?;
            let module = Module::parse(
                Source::from_bytes(source).with_path(Path::new(&key)),
                None,
                &mut context.borrow_mut(),
            )?;
            self.modules.borrow_mut().insert(key, module.clone());
            Ok(module)
        })();
        std::future::ready(result)
    }
}

struct BoundedJobs {
    inner: RefCell<Rc<SimpleJobExecutor>>,
    count: Cell<usize>,
    exhausted: Cell<bool>,
    maximum: usize,
}
impl BoundedJobs {
    fn new(maximum: usize) -> Self {
        Self {
            inner: RefCell::new(Rc::new(SimpleJobExecutor::new())),
            count: Cell::new(0),
            exhausted: Cell::new(false),
            maximum,
        }
    }
    fn begin(&self) {
        if self.exhausted.replace(false) {
            *self.inner.borrow_mut() = Rc::new(SimpleJobExecutor::new())
        }
        self.count.set(0)
    }
}
impl JobExecutor for BoundedJobs {
    fn enqueue_job(self: Rc<Self>, job: Job, context: &mut Context) {
        if self.count.get() >= self.maximum {
            self.exhausted.set(true);
            self.inner
                .borrow()
                .get_cancellation_token()
                .store(true, Ordering::Relaxed);
            return;
        }
        self.count.set(self.count.get() + 1);
        self.inner.borrow().clone().enqueue_job(job, context)
    }
    fn run_jobs(self: Rc<Self>, context: &mut Context) -> JsResult<()> {
        self.inner.borrow().clone().run_jobs(context)?;
        if self.exhausted.get() {
            return Err(JsNativeError::range()
                .with_message("promise/module job budget exceeded")
                .into());
        }
        Ok(())
    }
}

#[derive(Clone)]
struct NativeResources {
    assets: Arc<BTreeMap<String, Value>>,
    initial_scene: Arc<Value>,
    resource_names: Arc<BTreeSet<String>>,
    namespace: Option<String>,
    module: Option<Module>,
}
struct Instance {
    binding: ScriptBinding,
    context: Context,
    module: Module,
    jobs: Rc<BoundedJobs>,
    failed: BTreeSet<String>,
    retired: bool,
    initialized: bool,
    hooks: BTreeSet<String>,
}
#[derive(Deserialize)]
struct HostState {
    scene: Value,
    patches: Vec<StatePatch>,
    commands: Vec<HostCommand>,
    logs: Vec<HostLog>,
    storage: Value,
    shared: Value,
}
#[derive(Deserialize)]
struct HostLog {
    level: String,
    message: String,
}

/// A scene has per-binding realms and module caches. This type intentionally stays
/// on its creating thread (Boa Context is !Send), exchanging only JSON with hosts.
pub struct SceneRuntime {
    scene: Value,
    initial_scene: Arc<Value>,
    assets: Arc<BTreeMap<String, Value>>,
    resource_names: Arc<BTreeSet<String>>,
    modules: Arc<BTreeMap<String, String>>,
    options: RuntimeOptions,
    input: FrameInput,
    storage: Value,
    shared: Value,
    shared_host: Option<JsObject>,
    instances: Vec<Instance>,
    patches: Vec<StatePatch>,
    commands: Vec<HostCommand>,
    diagnostics: Vec<Diagnostic>,
    hook_results: Vec<HookResult>,
    events: Vec<(Option<Value>, String, Value)>,
    initialized: bool,
    destroyed: bool,
    next_dynamic: u64,
    failed_bindings: BTreeSet<(String, String, String)>,
}
impl SceneRuntime {
    pub fn new(
        mut scene: Value,
        modules: BTreeMap<String, String>,
        mut options: RuntimeOptions,
    ) -> Result<Self> {
        if !scene.is_object() || !scene.get("objects").is_some_and(Value::is_array) {
            return Err(Error::new("scene", "objects array required"));
        }
        if options.max_modules == 0
            || options.max_jobs == 0
            || options.loop_iterations == 0
            || options.recursion_limit == 0
        {
            return Err(Error::new("limits", "limits must be positive"));
        }
        let mut supplied = BTreeMap::new();
        let mut bytes = 0usize;
        for (name, source) in modules {
            bytes = bytes
                .checked_add(source.len())
                .ok_or_else(|| Error::new("modules", "size overflow"))?;
            let key = canonical(&name)?;
            if supplied.insert(key.clone(), source).is_some() {
                return Err(Error::new(
                    "modules",
                    format!("case-folded duplicate {key}"),
                ));
            }
        }
        if bytes > options.max_module_bytes || supplied.len() > options.max_modules {
            return Err(Error::new("modules", "source/count limit exceeded"));
        }
        for (name, source) in builtin_modules() {
            supplied.entry(name).or_insert(source);
        }
        crate::advance_animation_clocks(&mut scene, 0.0)?;
        let assets = Arc::new(
            std::mem::take(&mut options.assets)
                .into_iter()
                .map(|(name, value)| canonical(&name).map(|name| (name, value)))
                .collect::<Result<BTreeMap<_, _>>>()?,
        );
        let mut resource_names = std::mem::take(&mut options.resource_names)
            .into_iter()
            .map(|name| canonical(&name))
            .collect::<Result<BTreeSet<_>>>()?;
        resource_names.extend(assets.keys().cloned());
        resource_names.extend(supplied.keys().cloned());
        if resource_names.len() > 100_000 {
            return Err(Error::new(
                "resources",
                "supplied resource name limit exceeded",
            ));
        }
        Ok(Self {
            resource_names: Arc::new(resource_names),
            initial_scene: Arc::new(scene.clone()),
            assets,
            scene,
            modules: Arc::new(supplied),
            options,
            input: FrameInput::default(),
            storage: json!({"global":{},"screen":{}}),
            shared: json!({}),
            shared_host: None,
            instances: Vec::new(),
            patches: Vec::new(),
            commands: Vec::new(),
            diagnostics: Vec::new(),
            hook_results: Vec::new(),
            events: Vec::new(),
            initialized: false,
            destroyed: false,
            next_dynamic: 0,
            failed_bindings: BTreeSet::new(),
        })
    }
    pub fn set_initial_input(&mut self, input: FrameInput) -> Result<()> {
        if !self.instances.is_empty() || self.initialized {
            return Err(Error::new(
                "initial input",
                "set input before attaching/evaluating any module",
            ));
        }
        validate_input(&input)?;
        self.input = input;
        Ok(())
    }
    pub fn enqueue_event(
        &mut self,
        object_id: Option<Value>,
        hook: &str,
        payload: Value,
    ) -> Result<()> {
        if !HOOKS.contains(&hook) {
            return Err(Error::new("event", format!("unknown hook {hook}")));
        }
        if self.events.len() >= 4096 {
            return Err(Error::new("event", "queue limit exceeded"));
        }
        self.events.push((object_id, hook.into(), payload));
        Ok(())
    }
    pub fn attach(&mut self, mut binding: ScriptBinding) -> Result<()> {
        if self.destroyed {
            return Err(Error::new("lifecycle", "runtime destroyed"));
        }
        if self.instances.iter().filter(|v| !v.retired).count() >= self.options.max_modules {
            return Err(Error::new("binding", "active binding limit exceeded"));
        }
        if self.instances.iter().any(|v| v.binding.id == binding.id) {
            return Err(Error::new("binding", "duplicate binding id"));
        }
        binding.module = canonical(&binding.module)?;
        if self.scene.pointer(&binding.path).is_none() {
            return Err(Error::new(
                "binding",
                format!("missing property {}", binding.path),
            ));
        }
        let source = binding
            .source
            .as_ref()
            .or_else(|| self.modules.get(&binding.module))
            .ok_or_else(|| Error::new("binding", "module source not supplied"))?;
        if source.len() > self.options.max_module_bytes {
            return Err(Error::new("binding", "inline source limit exceeded"));
        }
        let namespace = binding_namespace(&self.scene, &binding);
        let loader = Rc::new(BoundedLoader {
            sources: self.modules.clone(),
            modules: RefCell::new(BTreeMap::new()),
            namespace: namespace.clone(),
        });
        let jobs = Rc::new(BoundedJobs::new(self.options.max_jobs));
        let mut context = Context::builder()
            .module_loader(loader.clone())
            .job_executor(jobs.clone())
            .build()
            .map_err(|e| Error::new("VM build", e.to_string()))?;
        context
            .runtime_limits_mut()
            .set_loop_iteration_limit(self.options.loop_iterations);
        context
            .runtime_limits_mut()
            .set_recursion_limit(self.options.recursion_limit);
        context.runtime_limits_mut().set_stack_size_limit(16_384);
        context.runtime_limits_mut().set_backtrace_limit(32);
        context.insert_data(NativeResources {
            assets: self.assets.clone(),
            initial_scene: self.initial_scene.clone(),
            resource_names: self.resource_names.clone(),
            namespace,
            module: None,
        });
        let shared_host = self
            .shared_host
            .clone()
            .unwrap_or_else(|| ObjectInitializer::new(&mut context).build());
        self.shared_host = Some(shared_host.clone());
        context
            .register_global_property(
                js_string!("__wpeSharedHost"),
                shared_host,
                Attribute::empty(),
            )
            .map_err(|e| Error::new("shared host", e.to_string()))?;
        context
            .register_global_builtin_callable(
                js_string!("__resolveAsset"),
                1,
                NativeFunction::from_fn_ptr(resolve_asset),
            )
            .map_err(|e| Error::new("native assets", e.to_string()))?;
        context
            .register_global_builtin_callable(
                js_string!("__resolveResource"),
                1,
                NativeFunction::from_fn_ptr(resolve_resource),
            )
            .map_err(|e| Error::new("native resource names", e.to_string()))?;
        context
            .register_global_builtin_callable(
                js_string!("__initialLayerConfig"),
                1,
                NativeFunction::from_fn_ptr(initial_layer_config),
            )
            .map_err(|e| Error::new("native initial config", e.to_string()))?;
        context
            .eval(Source::from_bytes(include_str!("math.js")))
            .map_err(|e| Error::new("math bootstrap", e.to_string()))?;
        context
            .eval(Source::from_bytes(include_str!("host.js")))
            .map_err(|e| Error::new("host bootstrap", e.to_string()))?;
        let mut bootstrap = self.state_for(&binding);
        bootstrap["options"] = serde_json::to_value(&self.options)
            .map_err(|e| Error::new("bootstrap", e.to_string()))?;
        load_state(&mut context, &bootstrap)?;
        jobs.begin();
        let module = Module::parse(
            Source::from_bytes(source).with_path(Path::new(&binding.module)),
            None,
            &mut context,
        )
        .map_err(|e| Error::new(&binding.module, e.to_string()))?;
        let mut resources = context.get_data::<NativeResources>().cloned().unwrap();
        resources.module = Some(module.clone());
        context.insert_data(resources);
        loader
            .modules
            .borrow_mut()
            .insert(binding.module.clone(), module.clone());
        let promise = module.load_link_evaluate(&mut context);
        context
            .run_jobs()
            .map_err(|e| Error::new(&binding.module, e.to_string()))?;
        match promise.state() {
            PromiseState::Fulfilled(_) => {}
            PromiseState::Rejected(e) => {
                return Err(Error::new(
                    &binding.module,
                    e.to_string(&mut context)
                        .map(|s| s.to_std_string_escaped())
                        .unwrap_or_else(|_| "module rejection".into()),
                ));
            }
            PromiseState::Pending => {
                return Err(Error::new(
                    &binding.module,
                    "module remains pending without an allowed async resource",
                ));
            }
        }
        apply_script_properties(&mut context, &module)?;
        let state = export_state(&mut context, self.options.max_output_bytes)?;
        self.apply_state(&binding.id, "global", state);
        context
            .eval(Source::from_bytes("__setPhase('event')"))
            .map_err(|e| Error::new("phase", e.to_string()))?;
        let mut hooks = BTreeSet::new();
        for hook in HOOKS {
            if !module
                .namespace(&mut context)
                .get(JsString::from(*hook), &mut context)
                .map_err(|e| Error::new("exports", e.to_string()))?
                .is_undefined()
            {
                hooks.insert((*hook).to_owned());
            }
        }
        self.instances.push(Instance {
            binding,
            context,
            module,
            jobs,
            failed: BTreeSet::new(),
            retired: false,
            initialized: false,
            hooks,
        });
        Ok(())
    }
    fn initialize_instances(&mut self) -> Result<()> {
        if self.destroyed {
            return Err(Error::new("lifecycle", "runtime destroyed"));
        }
        for i in 0..self.instances.len() {
            if !self.instances[i].initialized && !self.instances[i].retired {
                self.invoke(i, "init", None, true)?;
                self.invoke(
                    i,
                    "applyUserProperties",
                    Some(self.input.user_properties.clone()),
                    false,
                )?;
                self.invoke(
                    i,
                    "applyGeneralSettings",
                    Some(self.input.general_settings.clone()),
                    false,
                )?;
                self.instances[i].initialized = true;
            }
        }
        self.initialized = true;
        Ok(())
    }
    /// Reconcile scripts created, removed or replaced in the scene. New bindings
    /// initialize at the next frame boundary; surviving module state stays intact.
    pub fn reconcile_bindings(&mut self) -> Result<()> {
        let discovered = crate::collect_bindings(&self.scene);
        for i in 0..self.instances.len() {
            if self.instances[i].retired || self.instances[i].binding.source.is_none() {
                continue;
            }
            let binding = &self.instances[i].binding;
            let suffix = binding
                .path
                .splitn(4, '/')
                .skip(3)
                .collect::<Vec<_>>()
                .join("/");
            let same = discovered.iter().any(|v| {
                v.object_id == binding.object_id
                    && v.path.splitn(4, '/').skip(3).collect::<Vec<_>>().join("/") == suffix
                    && v.source == binding.source
            });
            if !same {
                self.retire(i)?;
            }
        }
        self.instances.retain(|instance| !instance.retired);
        for mut binding in discovered {
            let suffix = binding
                .path
                .splitn(4, '/')
                .skip(3)
                .collect::<Vec<_>>()
                .join("/");
            let exists = self.instances.iter().any(|v| {
                !v.retired
                    && v.binding.object_id == binding.object_id
                    && v.binding
                        .path
                        .splitn(4, '/')
                        .skip(3)
                        .collect::<Vec<_>>()
                        .join("/")
                        == suffix
                    && v.binding.source == binding.source
            });
            if !exists {
                let failed_key = (
                    binding.object_id.to_string(),
                    suffix,
                    binding
                        .source
                        .clone()
                        .unwrap_or_else(|| binding.module.clone()),
                );
                if self.failed_bindings.contains(&failed_key) {
                    continue;
                }
                binding.id = format!("dynamic-{}", self.next_dynamic);
                binding.module = format!("scripts/__dynamic_{}.js", self.next_dynamic);
                self.next_dynamic += 1;
                let id = binding.id.clone();
                if let Err(error) = self.attach(binding) {
                    self.failed_bindings.insert(failed_key);
                    self.diagnostics.push(Diagnostic {
                        binding: id,
                        hook: "module".into(),
                        message: error.to_string(),
                        level: "error".into(),
                    });
                }
            }
        }
        Ok(())
    }
    pub fn initialize(&mut self) -> Result<FrameOutput> {
        self.initialize_instances()?;
        Ok(self.output())
    }
    pub fn initialize_with_input(&mut self, input: FrameInput) -> Result<FrameOutput> {
        validate_input(&input)?;
        self.input = input;
        self.initialize()
    }
    pub fn step(&mut self, input: FrameInput) -> Result<FrameOutput> {
        validate_input(&input)?;
        if self.destroyed {
            return Err(Error::new("lifecycle", "runtime destroyed"));
        }
        let prior = self.input.clone();
        self.input = input;
        self.reconcile_bindings()?;
        let initialized = self.initialized;
        self.initialize_instances()?;
        let users = if initialized {
            changed(&prior.user_properties, &self.input.user_properties)
        } else {
            json!({})
        };
        let settings = if initialized {
            changed(&prior.general_settings, &self.input.general_settings)
        } else {
            json!({})
        };
        self.commands.extend(crate::advance_animation_clocks(
            &mut self.scene,
            self.input.delta_seconds,
        )?);
        let events = std::mem::take(&mut self.events);
        for i in 0..self.instances.len() {
            if self.instances[i].retired {
                continue;
            }
            let id = self.instances[i].binding.object_id.clone();
            if !self.scene["objects"]
                .as_array()
                .unwrap()
                .iter()
                .any(|o| o.get("id") == Some(&id))
            {
                self.retire(i)?;
                continue;
            }
            for (target, hook, payload) in &events {
                if target.as_ref().is_none_or(|t| t == &id) {
                    self.invoke(i, hook, Some(payload.clone()), false)?;
                }
            }
            self.tick(i, false)?;
            let events = self
                .commands
                .iter()
                .filter(|c| {
                    c.kind == "animation.event" && (c.object_id.is_null() || c.object_id == id)
                })
                .map(|c| c.arguments.clone())
                .collect::<Vec<_>>();
            for event in events {
                self.invoke(i, "animationEvent", Some(event), false)?;
            }
            if !users.as_object().is_none_or(|o| o.is_empty()) {
                self.invoke(i, "applyUserProperties", Some(users.clone()), false)?
            }
            if !settings.as_object().is_none_or(|o| o.is_empty()) {
                self.invoke(i, "applyGeneralSettings", Some(settings.clone()), false)?
            }
            if self.input.screen_resolution != prior.screen_resolution {
                self.invoke(
                    i,
                    "resizeScreen",
                    Some(json!(self.input.screen_resolution)),
                    false,
                )?
            }
            for (field, hook) in [
                ("status", "mediaStatusChanged"),
                ("playback", "mediaPlaybackChanged"),
                ("properties", "mediaPropertiesChanged"),
                ("thumbnail", "mediaThumbnailChanged"),
                ("timeline", "mediaTimelineChanged"),
            ] {
                if self.input.media.get(field) != prior.media.get(field)
                    && let Some(payload) = self.input.media.get(field).cloned()
                {
                    self.invoke(i, hook, Some(payload), false)?
                }
            }
            self.invoke(i, "update", None, true)?;
        }
        Ok(self.output())
    }
    pub fn dispatch(&mut self, hook: &str, payload: Value) -> Result<FrameOutput> {
        self.dispatch_to(None, hook, payload)
    }
    pub fn probe_with_input(
        &mut self,
        object_id: Option<&Value>,
        hook: &str,
        payload: Value,
        input: FrameInput,
    ) -> Result<FrameOutput> {
        validate_input(&input)?;
        let previous = std::mem::replace(&mut self.input, input);
        let result = self.dispatch_to(object_id, hook, payload);
        self.input = previous;
        result
    }
    pub fn dispatch_to(
        &mut self,
        object_id: Option<&Value>,
        hook: &str,
        payload: Value,
    ) -> Result<FrameOutput> {
        if !HOOKS.contains(&hook) {
            return Err(Error::new("event", format!("unknown hook {hook}")));
        }
        for i in 0..self.instances.len() {
            if object_id.is_none_or(|id| id == &self.instances[i].binding.object_id) {
                self.invoke(i, hook, Some(payload.clone()), false)?
            }
        }
        Ok(self.output())
    }
    pub fn replace_scene(&mut self, scene: Value) -> Result<()> {
        if !scene.get("objects").is_some_and(Value::is_array) {
            return Err(Error::new("scene", "objects array required"));
        }
        self.scene = scene;
        Ok(())
    }
    pub fn scene(&self) -> &Value {
        &self.scene
    }
    pub fn storage(&self) -> &Value {
        &self.storage
    }
    pub fn set_storage(&mut self, storage: Value) -> Result<()> {
        if !storage.is_object() {
            return Err(Error::new("storage", "object required"));
        }
        self.storage = storage;
        Ok(())
    }
    pub fn destroy(&mut self) -> Result<FrameOutput> {
        for i in 0..self.instances.len() {
            self.retire(i)?;
        }
        self.instances.clear();
        self.shared = json!({});
        self.events.clear();
        self.destroyed = true;
        Ok(self.output())
    }
    pub fn evaluate(&mut self, binding_id: &str, expression: &str) -> Result<Value> {
        let i = self
            .instances
            .iter()
            .position(|i| i.binding.id == binding_id)
            .ok_or_else(|| Error::new("binding", "not found"))?;
        let instance = &mut self.instances[i];
        instance.jobs.begin();
        let value = instance
            .context
            .eval(Source::from_bytes(expression))
            .map_err(|e| Error::new("eval", e.to_string()))?;
        instance
            .context
            .run_jobs()
            .map_err(|e| Error::new("jobs", e.to_string()))?;
        value
            .to_json(&mut instance.context)
            .map_err(|e| Error::new("eval JSON", e.to_string()))
            .map(|v| v.unwrap_or(Value::Null))
    }
    fn state_for(&self, binding: &ScriptBinding) -> Value {
        json!({"scene":self.scene,"binding":binding,"input":self.input,"storage":self.storage,"shared":self.shared})
    }
    fn invoke(&mut self, i: usize, hook: &str, payload: Option<Value>, bound: bool) -> Result<()> {
        if self.instances[i].retired
            || self.instances[i].failed.contains(hook)
            || !self.instances[i].hooks.contains(hook)
        {
            return Ok(());
        }
        let state = self.state_for(&self.instances[i].binding);
        let instance = &mut self.instances[i];
        load_state(&mut instance.context, &state)?;
        instance.jobs.begin();
        let outcome = (|| -> JsResult<()> {
            apply_script_properties_js(&mut instance.context, &instance.module)?;
            let value = instance
                .module
                .namespace(&mut instance.context)
                .get(JsString::from(hook), &mut instance.context)?;
            if value.is_undefined() {
                return Ok(());
            }
            let callable = value.as_callable().ok_or_else(|| {
                JsNativeError::typ().with_message(format!("export {hook} is not callable"))
            })?;
            let args = if bound {
                vec![
                    instance
                        .context
                        .eval(Source::from_bytes("__boundValue()"))?,
                ]
            } else if let Some(payload) = payload {
                let value = JsValue::from_json(&payload, &mut instance.context)?;
                vec![event_payload(&mut instance.context, hook, value)?]
            } else {
                vec![]
            };
            global_function(&mut instance.context, "__setPhase")?.call(
                &JsValue::undefined(),
                &[JsString::from(hook).into()],
                &mut instance.context,
            )?;
            global_function(&mut instance.context, "__refreshAudio")?.call(
                &JsValue::undefined(),
                &[],
                &mut instance.context,
            )?;
            let result = callable.call(&JsValue::undefined(), &args, &mut instance.context)?;
            if hook == "cursorHitTest" {
                self.hook_results.push(HookResult {
                    binding: instance.binding.id.clone(),
                    object_id: instance.binding.object_id.clone(),
                    hook: hook.into(),
                    value: Value::Bool(result.to_boolean()),
                });
            }
            if bound && !result.is_undefined() {
                let apply = global_function(&mut instance.context, "__applyReturn")?;
                apply.call(&JsValue::undefined(), &[result], &mut instance.context)?;
            }
            instance.context.run_jobs()?;
            Ok(())
        })();
        if let Err(error) = outcome {
            instance.failed.insert(hook.into());
            self.diagnostics.push(Diagnostic {
                binding: instance.binding.id.clone(),
                hook: hook.into(),
                message: error.to_string(),
                level: "error".into(),
            });
        }
        let id = instance.binding.id.clone();
        let exported = export_state(&mut instance.context, self.options.max_output_bytes)?;
        self.apply_state(&id, hook, exported);
        Ok(())
    }
    fn tick(&mut self, i: usize, advance_animations: bool) -> Result<()> {
        let state = self.state_for(&self.instances[i].binding);
        let instance = &mut self.instances[i];
        if !advance_animations
            && !instance
                .context
                .eval(Source::from_bytes("__hasFrameWork()"))
                .map_err(|e| Error::new("frame work", e.to_string()))?
                .to_boolean()
        {
            return Ok(());
        }
        load_state(&mut instance.context, &state)?;
        instance.jobs.begin();
        let code = if advance_animations {
            "__tick(true)"
        } else {
            "__tick(false)"
        };
        if let Err(error) = instance
            .context
            .eval(Source::from_bytes(code))
            .and_then(|_| instance.context.run_jobs())
        {
            self.diagnostics.push(Diagnostic {
                binding: instance.binding.id.clone(),
                hook: "timer".into(),
                message: error.to_string(),
                level: "error".into(),
            });
        }
        let id = instance.binding.id.clone();
        let exported = export_state(&mut instance.context, self.options.max_output_bytes)?;
        self.apply_state(&id, "timer", exported);
        Ok(())
    }
    fn retire(&mut self, i: usize) -> Result<()> {
        if self.instances[i].retired {
            return Ok(());
        }
        let state = self.state_for(&self.instances[i].binding);
        let instance = &mut self.instances[i];
        load_state(&mut instance.context, &state)?;
        instance.jobs.begin();
        let result = (|| -> JsResult<()> {
            let value = instance
                .module
                .namespace(&mut instance.context)
                .get(js_string!("destroy"), &mut instance.context)?;
            if let Some(f) = value.as_callable() {
                f.call(&JsValue::undefined(), &[], &mut instance.context)?;
            }
            instance.context.run_jobs()?;
            Ok(())
        })();
        if let Err(e) = result {
            self.diagnostics.push(Diagnostic {
                binding: instance.binding.id.clone(),
                hook: "destroy".into(),
                message: e.to_string(),
                level: "error".into(),
            });
        }
        let cleanup = instance.context.eval(Source::from_bytes("__destroyHost()"));
        if let Err(e) = cleanup {
            self.diagnostics.push(Diagnostic {
                binding: instance.binding.id.clone(),
                hook: "destroy cleanup".into(),
                message: e.to_string(),
                level: "error".into(),
            });
        }
        instance.retired = true;
        let id = instance.binding.id.clone();
        let exported = export_state(&mut instance.context, self.options.max_output_bytes)?;
        self.apply_state(&id, "destroy", exported);
        Ok(())
    }
    fn apply_state(&mut self, id: &str, hook: &str, state: HostState) {
        self.scene = state.scene;
        self.storage = state.storage;
        self.shared = state.shared;
        self.patches.extend(state.patches);
        self.commands.extend(state.commands);
        self.diagnostics
            .extend(state.logs.into_iter().map(|l| Diagnostic {
                binding: id.into(),
                hook: hook.into(),
                message: l.message,
                level: l.level,
            }));
    }
    fn output(&mut self) -> FrameOutput {
        FrameOutput {
            scene: self.scene.clone(),
            patches: std::mem::take(&mut self.patches),
            commands: std::mem::take(&mut self.commands),
            diagnostics: std::mem::take(&mut self.diagnostics),
            storage: self.storage.clone(),
            hook_results: std::mem::take(&mut self.hook_results),
        }
    }
}

fn load_state(context: &mut Context, value: &Value) -> Result<()> {
    let value =
        JsValue::from_json(value, context).map_err(|e| Error::new("host state", e.to_string()))?;
    context
        .register_global_property(js_string!("__bridgeState"), value, Attribute::all())
        .map_err(|e| Error::new("host state", e.to_string()))?;
    context
        .eval(Source::from_bytes("__loadState(__bridgeState)"))
        .map_err(|e| Error::new("host state", e.to_string()))?;
    Ok(())
}
fn global_function(context: &mut Context, name: &str) -> JsResult<JsObject> {
    context
        .global_object()
        .get(JsString::from(name), context)?
        .as_callable()
        .ok_or_else(|| {
            JsNativeError::typ()
                .with_message(format!("host function missing: {name}"))
                .into()
        })
}
fn export_state(context: &mut Context, maximum: usize) -> Result<HostState> {
    let value = context
        .eval(Source::from_bytes("JSON.stringify(__exportState())"))
        .map_err(|e| Error::new("host export", e.to_string()))?;
    let text = value
        .to_string(context)
        .map_err(|e| Error::new("host export", e.to_string()))?
        .to_std_string_escaped();
    if text.len() > maximum {
        return Err(Error::new("host export", "output size limit exceeded"));
    }
    serde_json::from_str(&text).map_err(|e| Error::new("host export", e.to_string()))
}
fn event_payload(context: &mut Context, hook: &str, value: JsValue) -> JsResult<JsValue> {
    context.register_global_property(js_string!("__eventValue"), value, Attribute::all())?;
    let code = if hook == "applyUserProperties" {
        "__userProperties(__eventValue)"
    } else if hook == "resizeScreen" {
        "new Vec2(...__eventValue)"
    } else if hook.starts_with("cursor") {
        "(()=>{const e=__eventValue;for(const k of ['worldPosition','localPosition'])if(e[k]!==undefined)e[k]=Array.isArray(e[k])?new Vec3(...e[k]):new Vec3(e[k]);return e})()"
    } else if hook == "mediaPlaybackChanged" {
        "new MediaPlaybackEvent(__eventValue)"
    } else if hook == "mediaThumbnailChanged" {
        "(()=>{const e=__eventValue;for(const k of ['primaryColor','secondaryColor','tertiaryColor','textColor','highContrastColor'])if(e[k]!==undefined)e[k]=Array.isArray(e[k])?new Vec3(...e[k]):new Vec3(e[k]);return e})()"
    } else {
        "__eventValue"
    };
    context.eval(Source::from_bytes(code))
}
fn changed(old: &Value, new: &Value) -> Value {
    let mut map = serde_json::Map::new();
    if let Some(new) = new.as_object() {
        for (k, v) in new {
            if old.get(k) != Some(v) {
                map.insert(k.clone(), v.clone());
            }
        }
    }
    Value::Object(map)
}
fn validate_input(i: &FrameInput) -> Result<()> {
    if !i.delta_seconds.is_finite()
        || i.delta_seconds < 0.0
        || !i.runtime_seconds.is_finite()
        || !i.time_of_day.is_finite()
        || !i
            .screen_resolution
            .iter()
            .chain(i.canvas_size.iter())
            .all(|v| v.is_finite() && *v > 0.0)
        || !i
            .cursor_screen
            .iter()
            .chain(i.cursor_world.iter())
            .all(|v| v.is_finite())
        || i.audio_left.len() != 64
        || i.audio_right.len() != 64
        || !i
            .audio_left
            .iter()
            .chain(i.audio_right.iter())
            .all(|v| v.is_finite())
    {
        return Err(Error::new(
            "frame input",
            "finite clocks/vectors and64stereo bins required",
        ));
    }
    Ok(())
}
fn builtin_modules() -> BTreeMap<String, String> {
    [
 ("wemath","export const smoothStep=WEMath.smoothStep,mix=WEMath.mix,deg2rad=WEMath.deg2rad,rad2deg=WEMath.rad2deg;"),
 ("wevector","export const angleVector2=WEVector.angleVector2,vectorAngle2=WEVector.vectorAngle2;"),
 ("wecolor","export const rgb2hsv=WEColor.rgb2hsv,hsv2rgb=WEColor.hsv2rgb,normalizeColor=WEColor.normalizeColor,expandColor=WEColor.expandColor;")
 ].into_iter().map(|(k,v)|(k.into(),v.into())).collect()
}

fn apply_script_properties_js(context: &mut Context, module: &Module) -> JsResult<()> {
    let properties = module
        .namespace(context)
        .get(js_string!("scriptProperties"), context)?;
    if !properties.is_undefined() {
        global_function(context, "__applyScriptProperties")?.call(
            &JsValue::undefined(),
            &[properties],
            context,
        )?;
    }
    Ok(())
}
fn apply_script_properties(context: &mut Context, module: &Module) -> Result<()> {
    apply_script_properties_js(context, module)
        .map_err(|e| Error::new("script properties", e.to_string()))
}

fn resolve_asset(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = args
        .first()
        .and_then(JsValue::as_string)
        .ok_or_else(|| JsNativeError::typ().with_message("asset string required"))?
        .to_std_string_escaped();
    let key = canonical(&name).map_err(|e| JsNativeError::typ().with_message(e.to_string()))?;
    let resources = context
        .get_data::<NativeResources>()
        .cloned()
        .ok_or_else(|| JsNativeError::typ().with_message("resource library missing"))?;
    let namespace = resource_namespace(&resources, context);
    let key = if resources.resource_names.contains(&key) {
        key
    } else {
        scoped_resource(&key, namespace.as_deref())
            .filter(|key| resources.resource_names.contains(key))
            .unwrap_or(key)
    };
    match resources.assets.get(&key) {
        Some(value) => JsValue::from_json(&json!({"path":key,"config":value}), context),
        None => Ok(JsValue::undefined()),
    }
}
fn resolve_resource(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = args
        .first()
        .and_then(JsValue::as_string)
        .ok_or_else(|| JsNativeError::typ().with_message("resource string required"))?
        .to_std_string_escaped();
    let key = canonical(&name).map_err(|e| JsNativeError::typ().with_message(e.to_string()))?;
    let resources = context
        .get_data::<NativeResources>()
        .cloned()
        .ok_or_else(|| JsNativeError::typ().with_message("resource library missing"))?;
    let namespace = resource_namespace(&resources, context);
    let key = if resources.resource_names.contains(&key) {
        key
    } else {
        scoped_resource(&key, namespace.as_deref())
            .filter(|key| resources.resource_names.contains(key))
            .unwrap_or(key)
    };
    if resources.resource_names.contains(&key) {
        Ok(JsString::from(key).into())
    } else {
        Ok(JsValue::undefined())
    }
}
fn resource_namespace(resources: &NativeResources, context: &mut Context) -> Option<String> {
    if let Some(module) = &resources.module
        && let Ok(value) = module
            .namespace(context)
            .get(js_string!("__workshopId"), context)
        && let Some(id) = value.as_string()
    {
        let id = id.to_std_string_escaped();
        if id.is_empty() {
            return None;
        }
        if id.len() <= 128
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Some(format!("workshop/{id}"));
        }
        return None;
    }
    resources.namespace.clone()
}
fn initial_layer_config(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let id = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .to_json(context)?
        .unwrap_or(Value::Null);
    let resources = context
        .get_data::<NativeResources>()
        .cloned()
        .ok_or_else(|| JsNativeError::typ().with_message("initial scene missing"))?;
    let result = resources
        .initial_scene
        .get("objects")
        .and_then(Value::as_array)
        .and_then(|objects| objects.iter().find(|o| o.get("id") == Some(&id)));
    match result {
        Some(value) => JsValue::from_json(value, context),
        None => Ok(JsValue::null()),
    }
}

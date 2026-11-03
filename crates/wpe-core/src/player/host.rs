use super::*;
use wpe_formats::mdl::SectionData;
use wpe_sim::model::{AnimationLayer, PlaybackMode, Transform};
impl ScenePlayer {
    pub(super) fn initialize_scripts(&mut self) -> Result<()> {
        let bindings = wpe_script::collect_bindings(&self.scene);
        let mut modules = BTreeMap::new();
        let mut assets = BTreeMap::new();
        let mut total = 0usize;
        let resource_names = self.project.vfs.resource_names(&[])?;
        for name in resource_names
            .iter()
            .filter(|name| {
                std::path::Path::new(name)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        extension.eq_ignore_ascii_case("js")
                            || extension.eq_ignore_ascii_case("json")
                    })
            })
            .cloned()
        {
            let Ok(data) = self.project.read_resource(&name) else {
                continue;
            };
            if data.bytes.len() > 8 * 1024 * 1024 {
                continue;
            }
            if name.ends_with(".js") {
                if let Ok(source) = String::from_utf8(data.bytes) {
                    total += source.len();
                    modules.insert(name, source);
                }
            } else if assets.len() < 8192
                && let Ok(value) = wpe_formats::jsonc::parse(&data.bytes)
            {
                assets.insert(name, value);
            }
        }
        let options = wpe_script::RuntimeOptions {
            max_modules: modules.len().max(512),
            max_module_bytes: total.max(8 * 1024 * 1024),
            assets,
            resource_names: resource_names.into_iter().collect(),
            user_property_definitions: self
                .project
                .metadata
                .pointer("/general/properties")
                .and_then(Value::as_object)
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default(),
            is_editor: self.options.is_editor,
            is_wallpaper: self.options.is_wallpaper,
            is_desktop: self.options.is_desktop,
            ..Default::default()
        };
        let mut vm = wpe_script::SceneRuntime::new(self.scene.clone(), modules, options)
            .map_err(script_error)?;
        vm.set_initial_input(self.script_input(0., &self.last_input))
            .map_err(script_error)?;
        vm.set_storage(self.storage.clone()).map_err(script_error)?;
        for binding in bindings {
            if let Err(error) = vm.attach(binding.clone()) {
                self.issue("script-module", Some(&binding.id), error.to_string());
            }
        }
        let out = vm.initialize().map_err(script_error)?;
        self.vm = Some(vm);
        self.apply_script_output(out)
    }
    pub(super) fn initialize_entities(&mut self) -> Result<()> {
        let mut objects = self
            .scene
            .get("objects")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let ids = objects
            .iter()
            .enumerate()
            .map(|(i, o)| object_key(o, i))
            .collect::<Vec<_>>();
        self.particles.retain(|k, _| ids.contains(k));
        self.children.retain(|k, _| ids.contains(k));
        self.models.retain(|k, _| ids.contains(k));
        self.entity_sources.retain(|k, _| ids.contains(k));
        for (at, object) in objects.iter_mut().enumerate() {
            let key = object_key(object, at);
            let editable = self.editable_model_data(object)?;
            let source = ["image", "particle", "model", "sound", "text"]
                .iter()
                .find_map(|k| object.get(*k).map(|v| format!("{k}:{v}")))
                .or_else(|| {
                    editable
                        .as_ref()
                        .and_then(|v| v.get("sourceModel"))
                        .map(|v| format!("editable-model:{v}"))
                })
                .unwrap_or_default();
            if self.entity_sources.get(&key) == Some(&source) {
                continue;
            }
            self.particles.remove(&key);
            self.models.remove(&key);
            self.children.remove(&key);
            self.entity_sources.insert(key.clone(), source);
            if let Some(file) = object
                .get("particle")
                .and_then(Value::as_str)
                .map(str::to_owned)
            {
                let result = (|| -> Result<_> {
                    let definition = self.assets.json(&self.project, &file)?;
                    let seed = number(object.get("seed"), 5489.) as u32;
                    let mut system = ParticleSystem::from_json_mode(
                        &definition,
                        seed,
                        self.scene
                            .pointer("/general/orthogonalprojection")
                            .is_none(),
                    )
                    .map_err(|e| Error::invalid(&file, e.to_string()))?;
                    if let Some(overrides) = object.get("instanceoverride") {
                        system.instance = overrides.clone();
                    }
                    if let Some(dep) = object
                        .get("dependencies")
                        .and_then(Value::as_array)
                        .and_then(|a| {
                            a.iter().find(|d| {
                                d.get("type").and_then(Value::as_str) == Some("emitterimage")
                            })
                        })
                        && let Some(source) = self
                            .scene
                            .get("objects")
                            .and_then(Value::as_array)
                            .and_then(|a| a.iter().find(|o| o.get("id") == dep.get("id")))
                        && let Some(file) = source.get("image").and_then(Value::as_str)
                    {
                        let image = self.assets.json(&self.project, file)?;
                        if let Some(material) = image.get("material").and_then(Value::as_str) {
                            let material = self.assets.json(&self.project, material)?;
                            if let Some(name) = material
                                .pointer("/passes/0/textures/0")
                                .and_then(Value::as_str)
                            {
                                let asset = self.assets.texture_asset(&self.project, name)?;
                                if let wpe_render::TextureData::Image2D(image) = &asset.texture.data
                                {
                                    system.emission_image = Some(image.clone());
                                }
                            }
                        }
                    }
                    Ok((definition, system, seed))
                })();
                match result {
                    Ok((definition, system, seed)) => {
                        let mut library = BTreeMap::new();
                        self.load_child_library(&definition, 0, &mut library)?;
                        let children = wpe_sim::children::ParticleChildren::new(
                            &definition,
                            &library,
                            seed,
                            self.scene
                                .pointer("/general/orthogonalprojection")
                                .is_none(),
                        )
                        .map_err(|e| Error::invalid("particle children", e.to_string()))?;
                        self.children.insert(key.clone(), children);
                        self.particles.insert(key.clone(), system);
                        object["__playing"] = json!(boolean(object.get("__playing"), true));
                    }
                    Err(e) => self.issue("particle-init", Some(&key), e.to_string()),
                }
            }
            let model_file = if let Some(file) = object.get("model").and_then(Value::as_str) {
                Some(file.into())
            } else if let Some(file) = object.get("image").and_then(Value::as_str) {
                self.assets
                    .json(&self.project, file)
                    .ok()
                    .and_then(|d| d.get("puppet").and_then(Value::as_str).map(str::to_owned))
            } else {
                editable
                    .as_ref()
                    .and_then(|v| v.get("sourceModel"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            };
            let editable_animator = editable.as_ref().filter(|value| {
                value
                    .get("sourceModel")
                    .and_then(Value::as_str)
                    .is_some_and(|source| !source.to_ascii_lowercase().ends_with(".mdl"))
                    || (value.get("sourceModel").is_none() && value.get("bones").is_some())
            });
            if model_file.is_some() || editable_animator.is_some() {
                let result = (|| -> Result<_> {
                    if let Some(value) = editable_animator {
                        return ModelAnimator::from_editable(value)
                            .map_err(|error| Error::invalid("editable model", error.to_string()));
                    }
                    let file = model_file
                        .as_deref()
                        .ok_or_else(|| Error::invalid("model", "model source required"))?;
                    let source = if file.to_ascii_lowercase().ends_with(".mdl") {
                        file.to_owned()
                    } else {
                        let descriptor = self.assets.json(&self.project, file)?;
                        descriptor
                            .get("model")
                            .or_else(|| descriptor.get("file"))
                            .and_then(Value::as_str)
                            .ok_or_else(|| Error::invalid(file, "model descriptor missing model"))?
                            .into()
                    };
                    let model = wpe_formats::mdl::Model::parse(
                        &self.project.read_resource(&source)?.bytes,
                    )?;
                    ModelAnimator::new(model).map_err(|e| Error::invalid(source, e.to_string()))
                })();
                match result {
                    Ok(model) => {
                        if object.get("blendshapes").is_none() {
                            object["blendshapes"] = json!(model.morph_names.iter().enumerate()
                                .map(|(index, name)| json!({"name":name,"index":index,"weight":0.}))
                                .collect::<Vec<_>>());
                        }
                        for message in &model.issues {
                            self.issue("model-profile", Some(&key), message.clone());
                        }
                        object["__animationClips"]=json!(model.clips.iter().enumerate().map(|(index,clip)|json!({"name":clip.name,"animation":clip.id,"clip":index,"fps":clip.fps,"frameCount":clip.frames,"duration":clip.frames as f32/clip.fps,"loop":clip.mode!="single","events":clip.events.iter().map(|event|{let value=serde_json::from_str::<Value>(&event.text).unwrap_or_else(|_|json!({"name":event.text}));let mut value=if value.is_object(){value}else{json!({"name":value})};value["frame"]=json!(f32::from_bits(event.frame_word)*clip.fps);value}).collect::<Vec<_>>()})).collect::<Vec<_>>());
                        if object.get("bones").is_none() {
                            object["bones"]=json!(model.bones.iter().map(|b|json!({"name":b.name,"parent":b.parent.map(|i|i as i32).unwrap_or(-1),"transform":b.bind.matrix().0})).collect::<Vec<_>>());
                        }
                        if object.get("animationlayers").is_none() {
                            object["animationlayers"]=json!(model.clips.iter().enumerate().take(1).map(|(i,c)|json!({"name":c.name,"clip":i,"fps":c.fps,"frameCount":c.frames,"duration":c.frames as f32/c.fps,"__time":0,"__playing":true,"rate":1,"blend":1,"loop":true})).collect::<Vec<_>>());
                        }
                        if let Some(layers) = object
                            .get_mut("animationlayers")
                            .and_then(Value::as_array_mut)
                        {
                            for layer in layers {
                                let clip = layer
                                    .get("animation")
                                    .and_then(Value::as_u64)
                                    .and_then(|id| model.clips.iter().position(|c| c.id == id))
                                    .or_else(|| {
                                        layer
                                            .get("clip")
                                            .and_then(Value::as_u64)
                                            .map(|v| v as usize)
                                    })
                                    .or_else(|| {
                                        model.clips.iter().position(|c| {
                                            Some(c.name.as_str())
                                                == layer.get("name").and_then(Value::as_str)
                                        })
                                    });
                                if let Some(index) = clip
                                    && let Some(clip) = model.clips.get(index)
                                {
                                    layer["clip"] = json!(index);
                                    layer["fps"] = json!(clip.fps);
                                    layer["frameCount"] = json!(clip.frames);
                                    layer["duration"] = json!(clip.frames as f32 / clip.fps);
                                    if layer.get("__time").is_none() {
                                        layer["__time"] = json!(0);
                                    }
                                    if layer.get("__playing").is_none() {
                                        layer["__playing"] = json!(true);
                                    }
                                    if layer.get("loop").is_none() {
                                        layer["loop"] = json!(clip.mode != "single");
                                    }
                                }
                            }
                        }
                        let attachments = model
                            .model
                            .sections
                            .iter()
                            .filter_map(|s| {
                                if let SectionData::Attachments(a) = &s.data {
                                    Some(a)
                                } else {
                                    None
                                }
                            })
                            .flatten()
                            .map(|a| json!({"name":a.name,"bone":a.bone,"transform":a.matrix}))
                            .collect::<Vec<_>>();
                        if !attachments.is_empty() && object.get("attachments").is_none() {
                            object["attachments"] = json!(attachments);
                        }
                        self.models.insert(key.clone(), model);
                    }
                    Err(e) => self.issue("model-init", Some(&key), e.to_string()),
                }
            }
            if let Some(file) = object
                .get("image")
                .and_then(Value::as_str)
                .map(str::to_owned)
                && let Err(e) = self.enrich_image(object, &file)
            {
                self.issue("image-init", Some(&key), e.to_string());
            }
            if object.get("sound").is_some() {
                if object.get("__playing").is_none() {
                    object["__playing"] = json!(!boolean(object.get("startsilent"), false));
                }
                self.host_commands.push(wpe_script::HostCommand {
                    kind: "sound.configure".into(),
                    object_id: object.get("id").cloned().unwrap_or(Value::Null),
                    arguments: object.clone(),
                });
            }
            if let Some(effects) = object.get_mut("effects").and_then(Value::as_array_mut) {
                for (effect_index, effect) in effects.iter_mut().enumerate() {
                    effect["__wpe_index"] = json!(effect_index);
                    if let Some(file) = effect.get("file").and_then(Value::as_str)
                        && let Ok(config) = self.assets.json(&self.project, file)
                    {
                        let base = config
                            .get("passes")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        let overrides = effect
                            .get("passes")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        let mut merged = Vec::new();
                        for (i, route) in base.into_iter().enumerate() {
                            let mut pass = if let Some(material) =
                                route.get("material").and_then(Value::as_str)
                            {
                                self.assets
                                    .json_scoped(
                                        &self.project,
                                        material,
                                        file.rsplit_once('/').map(|p| p.0).unwrap_or(""),
                                    )
                                    .ok()
                                    .and_then(|m| m.pointer("/passes/0").cloned())
                                    .unwrap_or_else(|| json!({}))
                            } else {
                                json!({})
                            };
                            merge(&mut pass, &route);
                            if let Some(over) = overrides.get(i) {
                                super::draw::merge_pass(&mut pass, over);
                            }
                            if let Err(error) = self.assets.hydrate_material(
                                &self.project,
                                &mut pass,
                                file.rsplit_once('/').map(|p| p.0).unwrap_or(""),
                            ) {
                                self.issue("material-metadata", Some(&key), error.to_string());
                            }
                            merged.push(pass);
                        }
                        effect["passes"] = json!(merged);
                    }
                }
            }
        }
        self.scene["objects"] = json!(objects);
        if let Some(vm) = &mut self.vm {
            vm.replace_scene(self.scene.clone()).map_err(script_error)?;
        }
        Ok(())
    }
    fn enrich_image(&mut self, object: &mut Value, file: &str) -> Result<()> {
        let descriptor = self.assets.json(&self.project, file)?;
        let material = descriptor.get("material").and_then(Value::as_str);
        let first = if let Some(material) = material {
            let material = self.assets.json(&self.project, material)?;
            material
                .pointer("/passes/0/textures/0")
                .and_then(Value::as_str)
                .map(str::to_owned)
        } else {
            None
        };
        let Some(first) = first.filter(|s| !s.starts_with("_rt_")) else {
            return Ok(());
        };
        let asset = match self.assets.texture_asset(&self.project, &first) {
            Ok(asset) => asset,
            Err(error) => {
                if self.assets.video_requested.contains(&first) {
                    object["__videoTexture"] =
                        json!({"resource":first,"__playing":true,"__time":0,"loop":true,"rate":1});
                    return Ok(());
                }
                return Err(error);
            }
        };
        let size = asset.logical_frame(0);
        if boolean(descriptor.get("autosize"), false) || object.get("size").is_none() {
            object["size"] = json!(format!("{} {}", size[0], size[1]));
        }
        if !asset.frames.is_empty() || asset.images.len() > 1 {
            let duration = if asset.frames.is_empty() {
                asset.images.len() as f32 / 30.
            } else {
                asset.frames.iter().map(|f| f.frame_time).sum()
            };
            let count = asset.frames.len().max(asset.images.len());
            object["__textureAnimation"] = json!({"frameCount":count,"duration":duration,"fps":count as f32/duration.max(1e-6),"__time":0,"__playing":true,"rate":1,"loop":true});
        }
        Ok(())
    }
    fn load_child_library(
        &mut self,
        definition: &Value,
        depth: usize,
        library: &mut BTreeMap<String, Value>,
    ) -> Result<()> {
        if depth >= 8 {
            return Err(Error::invalid("particle children", "nesting exceeds8"));
        }
        for child in definition
            .get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(file) = child
                .get("name")
                .or_else(|| child.get("file"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            if library.contains_key(file) {
                continue;
            }
            let config = self.assets.json(&self.project, file)?;
            library.insert(file.into(), config.clone());
            self.load_child_library(&config, depth + 1, library)?;
        }
        Ok(())
    }
    pub(super) fn apply_script_output(&mut self, out: wpe_script::FrameOutput) -> Result<()> {
        self.scene = out.scene;
        self.storage = out.storage;
        for diagnostic in out.diagnostics {
            self.issue(
                &format!("script-{}", diagnostic.level),
                Some(&diagnostic.binding),
                diagnostic.message,
            );
        }
        self.initialize_entities()?;
        for command in out.commands {
            let key = command.object_id.to_string();
            let args = &command.arguments;
            let kind = command.kind.as_str();
            let object = self
                .scene
                .get("objects")
                .and_then(Value::as_array)
                .and_then(|a| a.iter().find(|o| o.get("id") == Some(&command.object_id)));
            match kind {
                "layer.play" | "layer.pause" | "layer.stop" => {
                    if let Some(system) = self.particles.get_mut(&key) {
                        system.emission_enabled = kind == "layer.play";
                        system.paused = kind == "layer.pause";
                        if kind == "layer.stop" {
                            system.reset(number(object.and_then(|o| o.get("seed")), 5489.) as u32);
                            system.emission_enabled = false;
                        }
                    } else {
                        self.host_commands.push(command.clone());
                    }
                }
                "particle.emit" => {
                    if let Some(system) = self.particles.get_mut(&key) {
                        system.emit(number(args.get("count"), 1.).max(0.) as usize, 0.);
                    }
                }
                "particle.instance" => {
                    if let Some(system) = self.particles.get_mut(&key) {
                        let name = args.get("property").and_then(Value::as_str).unwrap_or("");
                        let value = args.get("value").cloned().unwrap_or(Value::Null);
                        if let Some(index) = name
                            .strip_prefix("controlpoint")
                            .and_then(|s| s.parse::<usize>().ok())
                        {
                            let position = vec3(Some(&value), Vec3::ZERO);
                            let _ = system.set_control_point(
                                index,
                                Mat4::trs(position, Quat::IDENTITY, Vec3::ONE),
                            );
                        } else {
                            system.instance[name] = value;
                        }
                    }
                }
                "bone.physicsImpulse" => {
                    if let Some(model) = self.models.get_mut(&key) {
                        let index = number(args.get("bone"), 0.) as usize;
                        model
                            .physics_impulse(
                                index,
                                vec3(args.get("directionalImpulse"), Vec3::ZERO),
                                vec3(args.get("angularImpulse"), Vec3::ZERO),
                            )
                            .map_err(|e| Error::invalid("bone physics", e.to_string()))?;
                    }
                }
                "bone.physicsReset" => {
                    if let Some(model) = self.models.get_mut(&key) {
                        let index = args.get("bone").filter(|v| !v.is_null()).and_then(|v| {
                            v.as_u64().map(|v| v as usize).or_else(|| {
                                model
                                    .bones
                                    .iter()
                                    .position(|b| Some(b.name.as_str()) == v.as_str())
                            })
                        });
                        model
                            .reset_physics(index)
                            .map_err(|e| Error::invalid("bone physics", e.to_string()))?;
                    }
                }
                "bone.transform"
                | "bone.localTransform"
                | "bone.localAngles"
                | "bone.localOrigin" => {
                    if let Some(model) = self.models.get_mut(&key) {
                        let index = number(args.get("bone"), 0.) as usize;
                        if let Some(bone) = object
                            .and_then(|o| o.get("bones"))
                            .and_then(|a| a.get(index))
                            && let Some(matrix) = matrix(bone.get("transform"))
                        {
                            model
                                .bone_overrides
                                .insert(index, Transform::from_matrix(matrix));
                        }
                    }
                }
                "blendShape.weight" => {
                    if let Some(model) = self.models.get_mut(&key) {
                        model.morph_weights.insert(
                            number(args.get("index"), 0.) as usize,
                            number(args.get("weight"), 0.),
                        );
                    }
                }
                "effect.materialFunction" => {
                    let path = args.get("path").and_then(Value::as_str).unwrap_or("");
                    let name = args.get("name").and_then(Value::as_str).unwrap_or("");
                    let effect = self.scene.pointer(path).cloned();
                    if let Some(effect) = effect
                        && let Some(file) = effect.get("file").and_then(Value::as_str)
                    {
                        let config = self.assets.json(&self.project, file)?;
                        if let Some(function) = config.get("functions").and_then(|f| f.get(name)) {
                            match function.get("action").and_then(Value::as_str) {
                                Some("clear") => {
                                    let index = effect
                                        .get("__wpe_index")
                                        .and_then(Value::as_u64)
                                        .unwrap_or(0);
                                    for fbo in function
                                        .get("fbos")
                                        .and_then(Value::as_array)
                                        .into_iter()
                                        .flatten()
                                        .filter_map(Value::as_str)
                                    {
                                        let clear = config
                                            .get("fbos")
                                            .and_then(Value::as_array)
                                            .and_then(|a| {
                                                a.iter().find(|f| {
                                                    f.get("name").and_then(Value::as_str)
                                                        == Some(fbo)
                                                })
                                            })
                                            .and_then(|f| f.get("clear"))
                                            .map(super::draw::color4)
                                            .unwrap_or([0.; 4]);
                                        self.assets
                                            .pending_clears
                                            .insert(format!("{key}:{index}:{fbo}"), clear);
                                    }
                                }
                                Some(action) => self.issue(
                                    "material-action",
                                    Some(&key),
                                    format!("Unknown material action {action}"),
                                ),
                                None => self.issue(
                                    "material-action",
                                    Some(&key),
                                    "Material action missing",
                                ),
                            }
                        } else {
                            self.issue(
                                "material-action",
                                Some(&key),
                                format!("No material function {name} in {file}"),
                            );
                        }
                    }
                }
                "scene.createLayer"
                | "scene.destroyLayer"
                | "scene.camera"
                | "modelData.create"
                | "modelData.apply"
                | "modelData.replace"
                | "modelData.destroy"
                | "modelData.release"
                | "effect.materialProperty"
                | "animation.event" => {}
                "engine.openUserShortcut" => self.host_commands.push(command.clone()),
                "textureAnimation.join" => {
                    if let Some(path) = args.get("path").and_then(Value::as_str)
                        && let Some(clock) = self.scene.pointer_mut(path)
                    {
                        let duration = number(clock.get("duration"), 0.);
                        let time = self.time as f32 * number(clock.get("rate"), 1.);
                        clock["__time"] = json!(if duration > 0. {
                            time.rem_euclid(duration)
                        } else {
                            time
                        });
                        clock["__playing"] = json!(true);
                        clock["__joined"] = json!(true);
                    }
                }
                _ if kind.starts_with("animationLayer.")
                    || kind.starts_with("sceneAnimation.")
                    || kind.starts_with("propertyAnimation.")
                    || kind.starts_with("textureAnimation.") => {}
                _ if kind.starts_with("videoTexture.") => self.host_commands.push(command.clone()),
                _ => self.issue(
                    "script-host-command",
                    Some(&key),
                    format!("Unknown host command {kind}"),
                ),
            }
        }
        if let Some(vm) = &mut self.vm {
            vm.replace_scene(self.scene.clone()).map_err(script_error)?;
        }
        Ok(())
    }
    pub(super) fn sync_model(model: &mut ModelAnimator, object: &Value) {
        model.root_motion_enabled = boolean(object.get("rootmotion"), true);
        model.material_variant = number(object.get("skin"), 0.).max(0.) as usize;
        if let Some(shapes) = object.get("blendshapes").and_then(Value::as_array) {
            for (index, shape) in shapes.iter().enumerate() {
                model
                    .morph_weights
                    .insert(index, number(shape.get("weight"), 0.));
            }
        }
        if let Some(layers) = object.get("animationlayers").and_then(Value::as_array) {
            let previous = std::mem::take(&mut model.layers);
            model.layers = layers
                .iter()
                .enumerate()
                .filter_map(|(index, config)| {
                    let clip = config
                        .get("clip")
                        .and_then(Value::as_u64)
                        .map(|n| n as usize)
                        .or_else(|| {
                            config
                                .get("animation")
                                .and_then(Value::as_u64)
                                .and_then(|id| model.clips.iter().position(|c| c.id == id))
                        })
                        .or_else(|| {
                            model.clips.iter().position(|c| {
                                Some(c.name.as_str()) == config.get("name").and_then(Value::as_str)
                            })
                        })?;
                    if clip >= model.clips.len() {
                        return None;
                    }
                    let mut layer = previous
                        .get(index)
                        .filter(|layer| layer.clip == clip)
                        .cloned()
                        .unwrap_or_else(|| AnimationLayer::new(clip));
                    layer
                        .set_external_time(number(config.get("__time"), 0.))
                        .ok()?;
                    layer.playing = boolean(config.get("__playing"), true);
                    layer.visible = boolean(config.get("visible"), true);
                    layer.rate = number(config.get("rate"), 1.);
                    layer.weight = if boolean(config.get("visible"), true) {
                        number(config.get("blend"), 1.)
                    } else {
                        0.
                    };
                    layer.target_weight = layer.weight;
                    layer.additive = boolean(config.get("additive"), false);
                    layer.mode = if boolean(config.get("loop"), true) {
                        PlaybackMode::Loop
                    } else {
                        PlaybackMode::Once
                    };
                    Some(layer)
                })
                .collect();
        }
    }
    pub(super) fn editable_model_data(&mut self, object: &Value) -> Result<Option<Value>> {
        let Some(data) = object.get("modeldata") else {
            return Ok(None);
        };
        if let Some(name) = data.as_str() {
            if let Some(data) = self
                .scene
                .get("__modelData")
                .and_then(Value::as_array)
                .and_then(|models| {
                    models
                        .iter()
                        .find(|v| v.get("id").and_then(Value::as_str) == Some(name))
                })
            {
                return Ok(Some(data.clone()));
            }
            return self.assets.json(&self.project, name).map(Some);
        }
        Ok(Some(data.clone()))
    }
}
fn merge(target: &mut Value, source: &Value) {
    if let (Some(a), Some(b)) = (target.as_object_mut(), source.as_object()) {
        for (k, v) in b {
            if let Some(old) = a.get_mut(k)
                && old.is_object()
                && v.is_object()
            {
                merge(old, v)
            } else {
                a.insert(k.clone(), v.clone());
            }
        }
    } else {
        *target = source.clone();
    }
}

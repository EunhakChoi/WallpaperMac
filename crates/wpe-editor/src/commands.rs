use crate::resources::{base64, decode_image, safe_basename};
use crate::*;
impl Editor {
    pub(crate) fn route(&mut self, method: &str, args: &[Value]) -> Result<Value> {
        if args.is_empty()
            && [
                "importFile",
                "importModel",
                "importImage",
                "importSound",
                "importFontFile",
                "replaceFile",
                "replaceTexture",
                "importTextureFile",
                "confirmUnsavedChanges",
            ]
            .contains(&method)
        {
            return Ok(self.request_host(method, args));
        }
        match method {
            "snapshot" => serde_json::to_value(self.snapshot()).map_err(json_error),
            "beginTransaction" => {
                self.begin_transaction()?;
                Ok(Value::Null)
            }
            "commitTransaction" => {
                self.commit()?;
                Ok(Value::Null)
            }
            "rollbackTransaction" => {
                self.rollback()?;
                Ok(Value::Null)
            }
            "undo" => Ok(json!(self.undo()?)),
            "redo" => Ok(json!(self.redo()?)),
            "performUndoRedo" => Ok(json!(
                if args.first().and_then(Value::as_bool).unwrap_or(false) {
                    self.redo()?
                } else {
                    self.undo()?
                }
            )),
            "forceSave" => {
                self.save()?;
                Ok(Value::Null)
            }
            "checkUnsavedChanges" => Ok(json!(self.state.dirty)),
            "confirmUnsavedChanges" => {
                match args.first().and_then(Value::as_str).unwrap_or("cancel") {
                    "save" => self.save()?,
                    "discard" => {
                        self.state.dirty = false;
                    }
                    "cancel" => return Ok(json!(false)),
                    _ => return Err(Error::new("argument", "save,discard,cancel required")),
                }
                Ok(json!(true))
            }
            "saveProjectData" => {
                let value = parse_argument(args.first())?;
                if !value.is_object() {
                    return Err(Error::new("project", "object required"));
                }
                self.state.project = value;
                Ok(Value::Null)
            }
            "saveSceneGeneralData" => {
                let value = parse_argument(args.first())?;
                if !value.is_object() {
                    return Err(Error::new("scene", "general object required"));
                }
                self.state.scene["general"] = value;
                Ok(Value::Null)
            }
            "applyTemporaryProjectData" => {
                self.session["temporaryProject"] = parse_argument(args.first())?;
                self.events.push(EditorEvent {
                    kind: "previewProjectData".into(),
                    payload: self.session["temporaryProject"].clone(),
                });
                Ok(Value::Null)
            }
            "writeDocumentProperty" => {
                let document = string(args, 0)?;
                let path = string(args, 1)?;
                let value = args.get(2).cloned();
                match document {
                    "project" => assign(&mut self.state.project, path, value)?,
                    "scene" => assign(&mut self.state.scene, path, value)?,
                    name => {
                        let mut doc = self.resource(name)?;
                        assign(&mut doc, path, value)?;
                        self.put_resource(name, Some(doc))?;
                    }
                }
                Ok(Value::Null)
            }
            "createNewProject" => {
                let config = parse_argument(args.first())?;
                self.state.project=config.get("project").cloned().unwrap_or_else(||json!({"title":config.get("title").and_then(Value::as_str).unwrap_or("Untitled"),"type":"scene","file":"scene.json"}));
                self.state.scene = config
                    .get("scene")
                    .cloned()
                    .unwrap_or_else(|| json!({"general":{"clearcolor":"0 0 0"},"objects":[]}));
                self.initial_scene = self.state.scene.clone();
                self.state.resources.clear();
                self.state.files.clear();
                self.undo.clear();
                self.redo.clear();
                self.revision += 1;
                self.state.dirty = true;
                self.state.pending = 10.0;
                Ok(json!(true))
            }
            "openProject" => {
                if self.state.dirty {
                    return Err(Error::new(
                        "unsaved-changes",
                        "save or discard before opening",
                    ));
                }
                let name = args
                    .first()
                    .and_then(Value::as_str)
                    .unwrap_or("project.json");
                if name != "project.json" {
                    return Ok(self.request_host(method, args));
                }
                let project = wpe_formats::jsonc::parse(&self.read_bytes(name)?)
                    .map_err(|e| Error::new("json", e.to_string()))?;
                let file = project
                    .get("file")
                    .and_then(Value::as_str)
                    .unwrap_or("scene.json");
                let scene = wpe_formats::jsonc::parse(&self.read_bytes(file)?)
                    .map_err(|e| Error::new("json", e.to_string()))?;
                if !scene["objects"].is_array() {
                    return Err(Error::new("scene", "objects required"));
                }
                self.state.project = project;
                self.state.scene = scene;
                self.saved_state = self.state.clone();
                self.initial_scene = self.state.scene.clone();
                self.state.resources.clear();
                self.state.files.clear();
                self.undo.clear();
                self.redo.clear();
                self.revision += 1;
                Ok(json!(true))
            }
            "closeProject" => {
                if self.state.dirty {
                    return Err(Error::new(
                        "unsaved-changes",
                        "save or discard before closing",
                    ));
                }
                self.session["closed"] = json!(true);
                Ok(json!(true))
            }
            "readJsonFile" => self.resource(string(args, 0)?),
            "readProjectFile" => {
                let name = string(args, 0)?;
                let bytes = self.read_bytes(name)?;
                Ok(json!({"file":name,"text":String::from_utf8(bytes.clone()).ok(),"bytes":bytes}))
            }
            "writeProjectFile" => {
                let name = string(args, 0)?;
                let value = arg(args, 1)?;
                if name == "project.json"
                    || Some(name) == self.state.project.get("file").and_then(Value::as_str)
                {
                    let doc = if value.is_object() && value.get("bytes").is_none() {
                        value.clone()
                    } else {
                        parse_argument(Some(value))?
                    };
                    if !doc.is_object() {
                        return Err(Error::new("document", "object JSON document required"));
                    }
                    if name == "project.json" {
                        self.state.project = doc
                    } else {
                        self.state.scene = doc
                    }
                    return Ok(json!(true));
                }
                if let Some(array) = value.get("bytes").and_then(Value::as_array) {
                    let bytes = array
                        .iter()
                        .map(|b| {
                            b.as_u64()
                                .filter(|n| *n <= 255)
                                .map(|n| n as u8)
                                .ok_or_else(|| Error::new("bytes", "values0..255required"))
                        })
                        .collect::<Result<Vec<_>>>()?;
                    self.put_bytes(name, Some(bytes))?
                } else if let Some(text) = value.as_str() {
                    self.put_bytes(name, Some(text.as_bytes().to_vec()))?
                } else {
                    self.put_resource(name, Some(value.clone()))?
                }
                Ok(json!(true))
            }
            "getWallpaperDetails" => Ok(self.state.project.clone()),
            "getImportFileList" => Ok(json!(self.file_list()?)),
            "getProjects"
            | "getTemplates"
            | "getShaderPresets"
            | "listAvailableEffects"
            | "listAvailablePresets"
            | "getAvailableTextures"
            | "getAvailableParticleElements" => {
                let prefix = match method {
                    "listAvailableEffects" => "effects/",
                    "listAvailablePresets" => "presets/",
                    "getAvailableTextures" => "materials/",
                    "getAvailableParticleElements" => "particles/",
                    "getTemplates" => "templates/",
                    "getShaderPresets" => "shaders/",
                    _ => "",
                };
                Ok(json!(
                    self.file_list()?
                        .into_iter()
                        .filter(|p| p.starts_with(prefix))
                        .collect::<Vec<_>>()
                ))
            }
            "importFile" | "replaceFile" | "importAssetCopyFiles" | "replaceTextureCopyFiles" => {
                let (name, bytes) = self.import_source(args, 0)?;
                let destination = args
                    .get(1)
                    .and_then(Value::as_str)
                    .unwrap_or(&name)
                    .to_owned();
                self.put_bytes(&destination, Some(bytes))?;
                Ok(json!({"file":destination}))
            }
            "importImage" => {
                let (name, bytes) = self.import_source(args, 0)?;
                self.import_image(&name, &bytes, true)
            }
            "importSound" | "addSound" => {
                let (name, bytes) = self.import_source(args, 0)?;
                let filename = Path::new(&name)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| Error::new("path", "sound filename"))?;
                let destination = format!("sounds/{filename}");
                self.put_bytes(&destination, Some(bytes))?;
                self.add_layer(json!({"name":name,"sound":[destination],"playbackmode":"loop","volume":1.0,"startsilent":false}),None)
            }
            "importModel" | "importModelAddToScene" => {
                let (name, bytes) = self.import_source(args, 0)?;
                let result = if name.to_ascii_lowercase().ends_with(".obj") {
                    self.import_obj(&name, &bytes)?
                } else if name.to_ascii_lowercase().ends_with(".fbx") {
                    self.import_fbx_model(&name, bytes)?
                } else if name.to_ascii_lowercase().ends_with(".mdl") {
                    let model = wpe_formats::mdl::Model::parse(&bytes)
                        .map_err(|e| Error::new("model", e.to_string()))?;
                    let file = format!("models/{}.mdl", safe_basename(&name)?);
                    self.put_bytes(&file, Some(bytes))?;
                    json!({"file":file,"model":model.summary(),"format":"mdl"})
                } else {
                    return Ok(self.request_host(method, args));
                };
                if method == "importModelAddToScene" {
                    let config = if let Some(model) = result.get("modeldata") {
                        json!({"name":name,"modeldata":model})
                    } else {
                        json!({"name":name,"model":result["file"]})
                    };
                    let id = self.add_layer(config, None)?;
                    Ok(json!({"layer":id,"resource":result}))
                } else {
                    Ok(result)
                }
            }
            "importFontFile" => {
                let (name, bytes) = self.import_source(args, 0)?;
                let filename = Path::new(&name)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| Error::new("path", "font filename"))?;
                let file = format!("fonts/{filename}");
                self.put_bytes(&file, Some(bytes))?;
                Ok(json!({"file":file}))
            }
            "getTextureSize"
            | "getResizeRecommendationsFromFile"
            | "getDominantColorFromFile"
            | "checkBadGIF" => {
                let (_, bytes) = self.import_source(args, 0)?;
                let image = if bytes.starts_with(b"TEXV") {
                    let texture = wpe_formats::tex::Texture::parse(&bytes)
                        .map_err(|e| Error::new("texture", e.to_string()))?;
                    let decoded = texture
                        .decode_rgba8(0, 0)
                        .map_err(|e| Error::new("texture", e.to_string()))?;
                    image::RgbaImage::from_raw(decoded.width, decoded.height, decoded.rgba)
                        .ok_or_else(|| Error::new("texture", "invalid RGBA"))?
                } else {
                    decode_image(&bytes)?
                };
                if method == "getDominantColorFromFile" {
                    let mut sums = [0u64; 3];
                    let mut alpha = 0u64;
                    for p in image.pixels() {
                        for k in 0..3 {
                            sums[k] += p[k] as u64 * p[3] as u64;
                        }
                        alpha += p[3] as u64;
                    }
                    Ok(json!(sums.map(|s| if alpha == 0 {
                        0.0
                    } else {
                        s as f64 / alpha as f64 / 255.0
                    })))
                } else if method == "checkBadGIF" {
                    Ok(json!({"decodable":true,"width":image.width(),"height":image.height()}))
                } else {
                    Ok(
                        json!({"width":image.width(),"height":image.height(),"size":[image.width(),image.height()]}),
                    )
                }
            }
            "getImageDataUrl" | "getTexturePreview" => {
                let name = string(args, 0)?;
                let bytes = self.read_bytes(name)?;
                let image = if bytes.starts_with(b"TEXV") {
                    let texture = wpe_formats::tex::Texture::parse(&bytes)
                        .map_err(|e| Error::new("texture", e.to_string()))?;
                    let d = texture
                        .decode_rgba8(0, 0)
                        .map_err(|e| Error::new("texture", e.to_string()))?;
                    image::RgbaImage::from_raw(d.width, d.height, d.rgba)
                        .ok_or_else(|| Error::new("image", "RGBA size"))?
                } else {
                    decode_image(&bytes)?
                };
                let mut png = std::io::Cursor::new(Vec::new());
                image::DynamicImage::ImageRgba8(image)
                    .write_to(&mut png, image::ImageFormat::Png)
                    .map_err(|e| Error::new("image", e.to_string()))?;
                Ok(json!(format!(
                    "data:image/png;base64,{}",
                    base64(png.get_ref())
                )))
            }
            "duplicateTextureFile" => {
                let source = string(args, 0)?;
                let destination = string(args, 1)?;
                self.put_bytes(destination, Some(self.read_bytes(source)?))?;
                Ok(json!(destination))
            }
            "importTextureFile" | "replaceTexture" => {
                let (name, bytes) = self.import_source(args, 0)?;
                self.import_image(&name, &bytes, false)
            }
            "exportTextureFile" => {
                let source = string(args, 0)?;
                let destination = string(args, 1)?;
                let texture = wpe_formats::tex::Texture::parse(&self.read_bytes(source)?)
                    .map_err(|e| Error::new("texture", e.to_string()))?;
                let decoded = texture
                    .decode_rgba8(0, 0)
                    .map_err(|e| Error::new("texture", e.to_string()))?;
                let image = image::RgbaImage::from_raw(decoded.width, decoded.height, decoded.rgba)
                    .ok_or_else(|| Error::new("texture", "invalid RGBA"))?;
                let mut png = std::io::Cursor::new(Vec::new());
                image::DynamicImage::ImageRgba8(image)
                    .write_to(&mut png, image::ImageFormat::Png)
                    .map_err(|e| Error::new("image", e.to_string()))?;
                self.put_bytes(destination, Some(png.into_inner()))?;
                Ok(json!(destination))
            }
            "addAssetToScene" | "addPreset" => {
                let mut config = parse_argument(args.first())?;
                if let Some(file) = config.as_str() {
                    config = self.resource(file)?
                }
                self.add_layer(config, args.get(2))
            }
            "addLight"
            | "addCamera"
            | "addTextLayer"
            | "addCompositionLayer"
            | "addSolidLayer"
            | "addTransformLayer" => {
                let mut config = if args.is_empty() {
                    json!({})
                } else {
                    parse_argument(args.first())?
                };
                if !config.is_object() {
                    return Err(Error::new("layer", "configuration object required"));
                }
                let object = config.as_object_mut().unwrap();
                match method {
                    "addLight" => {
                        object
                            .entry("light")
                            .or_insert(json!({"color":"1 1 1","intensity":1}));
                    }
                    "addCamera" => {
                        object.entry("camera").or_insert(json!({"fov":60,"zoom":1}));
                    }
                    "addTextLayer" => {
                        object.entry("text").or_insert(json!("Text"));
                    }
                    "addCompositionLayer" => {
                        object.entry("projectlayer").or_insert(json!(true));
                    }
                    "addSolidLayer" => {
                        object.entry("solidlayer").or_insert(json!(true));
                    }
                    _ => {
                        object.entry("transform").or_insert(json!(true));
                    }
                }
                object.entry("name").or_insert(json!(method));
                object.entry("origin").or_insert(json!("0 0 0"));
                self.add_layer(config, None)
            }
            "removeFromScene" => {
                let ids = if let Some(array) = args.first().and_then(Value::as_array) {
                    array.clone()
                } else if let Some(id) = args.first() {
                    vec![id.clone()]
                } else {
                    self.session["selection"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                };
                for id in &ids {
                    self.index(id)?;
                }
                let objects = self.objects_mut()?;
                objects.retain(|o| !ids.iter().any(|id| equal_id(id, &o["id"])));
                for object in objects {
                    if ids.iter().any(|id| equal_id(id, &object["parent"])) {
                        object.as_object_mut().unwrap().remove("parent");
                    }
                }
                self.session["selection"] = json!([]);
                Ok(json!(ids))
            }
            "duplicateRenderable" => {
                let id = args.first().cloned().map_or_else(|| self.selected(), Ok)?;
                let mut config = self.objects()?[self.index(&id)?].clone();
                config.as_object_mut().unwrap().remove("id");
                if let Some(name) = config["name"].as_str() {
                    config["name"] = json!(format!("{name} copy"))
                }
                self.add_layer(config, None)
            }
            "moveRenderable" | "centerRenderable" | "rotateRenderable" => {
                let id = arg(args, 0)?.clone();
                let origin = if method == "moveRenderable" && args.len() == 2 {
                    let refid = arg(args, 1)?;
                    self.objects()?[self.index(refid)?]
                        .get("origin")
                        .cloned()
                        .unwrap_or(json!("0 0 0"))
                } else if method == "moveRenderable" {
                    json!(format!(
                        "{} {} {}",
                        number(args, 1)?,
                        number(args, 2)?,
                        number(args, 3)?
                    ))
                } else if method == "centerRenderable" {
                    let size = self.state.scene["general"]["orthogonalprojection"].clone();
                    json!(format!(
                        "{} {} 0",
                        size["width"].as_f64().unwrap_or(1920.0) / 2.0,
                        size["height"].as_f64().unwrap_or(1080.0) / 2.0
                    ))
                } else {
                    json!(format!("0 0 {}", number(args, 1)?.to_radians()))
                };
                self.layer_mut(&id)?[if method == "rotateRenderable" {
                    "angles"
                } else {
                    "origin"
                }] = origin;
                Ok(id)
            }
            "sendAssetPropertyChange" => {
                let config = parse_argument(args.first())?;
                let id = config
                    .get("id")
                    .cloned()
                    .map_or_else(|| self.selected(), Ok)?;
                let properties = config
                    .get("properties")
                    .and_then(Value::as_object)
                    .ok_or_else(|| Error::new("properties", "properties object required"))?;
                let key = config.get("key").and_then(Value::as_str);
                let tag = config.get("tag").and_then(Value::as_str).unwrap_or("main");
                if let Some(file) = self.resource_path(&id) {
                    let mut resource = self.resource(&file)?;
                    let target = if tag == "main" {
                        &mut resource
                    } else {
                        let key =
                            key.ok_or_else(|| Error::new("properties", "nested key required"))?;
                        resource
                            .pointer_mut(key)
                            .ok_or_else(|| Error::new("properties", "nested target missing"))?
                    };
                    for (k, v) in properties {
                        target[k] = v.clone();
                    }
                    self.put_resource(&file, Some(resource))?;
                } else {
                    let layer = self.layer_mut(&id)?;
                    let target = if tag == "main" {
                        layer
                    } else {
                        let key =
                            key.ok_or_else(|| Error::new("properties", "nested key required"))?;
                        layer
                            .pointer_mut(key)
                            .ok_or_else(|| Error::new("properties", "nested target missing"))?
                    };
                    for (k, v) in properties {
                        target[k] = v.clone();
                    }
                }
                Ok(json!(true))
            }
            "sendSceneOrderChange" => {
                let order = parse_argument(args.first())?;
                let from = order["from"]
                    .as_u64()
                    .ok_or_else(|| Error::new("order", "from required"))?
                    as usize;
                let to = order["to"]
                    .as_u64()
                    .ok_or_else(|| Error::new("order", "to required"))?
                    as usize;
                let count = order["count"].as_u64().unwrap_or(1) as usize;
                let parent = order.get("parent").cloned();
                let objects = self.objects_mut()?;
                if count == 0
                    || from.checked_add(count).is_none_or(|n| n > objects.len())
                    || to > objects.len() - count
                {
                    return Err(Error::new("order", "range out of bounds"));
                }
                let moved = objects
                    .drain(from..from + count)
                    .map(|mut v| {
                        if let Some(parent) = &parent {
                            v["parent"] = parent.clone();
                        }
                        v
                    })
                    .collect::<Vec<_>>();
                objects.splice(to..to, moved);
                Ok(json!(true))
            }
            "getAssetProperties" => {
                let id = arg(args, 0)?;
                if let Ok(index) = self.index(id) {
                    Ok(self.objects()?[index].clone())
                } else {
                    let name = self
                        .resource_path(id)
                        .ok_or_else(|| Error::new("resource", "resource path required"))?;
                    let mut value = self.resource(&name)?;
                    let id = self.resource_handle(&name);
                    value["id"] = id;
                    value["key"] = json!(name.split('/').next().unwrap_or("resource"));
                    Ok(value)
                }
            }
            "getPropertiesOriginalValues" => {
                let id = arg(args, 0)?;
                self.initial_scene["objects"]
                    .as_array()
                    .and_then(|a| a.iter().find(|v| equal_id(&v["id"], id)))
                    .cloned()
                    .ok_or_else(|| {
                        Error::new(
                            "original-value",
                            "layer was created after this project was opened",
                        )
                    })
            }
            "findRenderableByMaterial" => {
                let material = string(args, 0)?;
                Ok(json!(
                    self.objects()?
                        .iter()
                        .filter(|v| v.to_string().contains(material))
                        .map(|v| v["id"].clone())
                        .collect::<Vec<_>>()
                ))
            }
            "getEffectConfig" => {
                let id = arg(args, 0)?;
                let index = arg(args, 1)?
                    .as_u64()
                    .ok_or_else(|| Error::new("effect", "index required"))?
                    as usize;
                self.objects()?[self.index(id)?]["effects"]
                    .as_array()
                    .and_then(|a| a.get(index))
                    .cloned()
                    .ok_or_else(|| Error::new("effect", "index out of bounds"))
            }
            "addEffect" | "addEffectFromConfig" => {
                let id = arg(args, 0)?.clone();
                let mut config = parse_argument(args.get(1))?;
                if let Some(name) = config.as_str() {
                    config = self.resource(name)?
                }
                let effects = self
                    .layer_mut(&id)?
                    .as_object_mut()
                    .unwrap()
                    .entry("effects")
                    .or_insert(json!([]))
                    .as_array_mut()
                    .ok_or_else(|| Error::new("effect", "effects array required"))?;
                effects.push(config);
                Ok(json!(effects.len() - 1))
            }
            "applyEffectConfig" => {
                let id = arg(args, 0)?.clone();
                let index = arg(args, 1)?
                    .as_u64()
                    .ok_or_else(|| Error::new("effect", "index required"))?
                    as usize;
                let config = parse_argument(args.get(2))?;
                let effects = self.layer_mut(&id)?["effects"]
                    .as_array_mut()
                    .ok_or_else(|| Error::new("effect", "effects required"))?;
                *effects
                    .get_mut(index)
                    .ok_or_else(|| Error::new("effect", "index out of bounds"))? = config;
                Ok(json!(true))
            }
            "checkEffectCompatibility" => {
                let config = parse_argument(args.first())?;
                Ok(
                    json!({"compatible":config.is_object(),"reason":if config.is_object(){Value::Null}else{json!("effect must be an object")}}),
                )
            }
            "addAnimationLayer" => {
                let id = arg(args, 0)?.clone();
                let config = parse_argument(args.get(1))
                    .unwrap_or_else(|_| args.get(1).cloned().unwrap_or(Value::Null));
                let config = if let Some(name) = config.as_str() {
                    self.resource(name)?
                } else {
                    config
                };
                let a = self
                    .layer_mut(&id)?
                    .as_object_mut()
                    .unwrap()
                    .entry("animationlayers")
                    .or_insert(json!([]))
                    .as_array_mut()
                    .ok_or_else(|| Error::new("animation", "array required"))?;
                a.push(config);
                Ok(json!(a.len() - 1))
            }
            "getAvailableAnimations" | "getBoneInfos" | "getAllAttachments" => {
                let id = args.first().cloned().map_or_else(|| self.selected(), Ok)?;
                let layer = &self.objects()?[self.index(&id)?];
                Ok(layer[match method {
                    "getBoneInfos" => "bones",
                    "getAllAttachments" => "attachments",
                    _ => "animations",
                }]
                .clone())
            }
            "removeRenderableItem" | "dragRenderableItem" => {
                let id = arg(args, 0)?.clone();
                let key = if method == "removeRenderableItem" {
                    args.get(2).and_then(Value::as_str).unwrap_or("effects")
                } else {
                    args.get(3).and_then(Value::as_str).unwrap_or("effects")
                };
                let from = arg(args, 1)?
                    .as_u64()
                    .ok_or_else(|| Error::new("index", "index required"))?
                    as usize;
                let a = self.layer_mut(&id)?[key]
                    .as_array_mut()
                    .ok_or_else(|| Error::new("array", "item collection missing"))?;
                if from >= a.len() {
                    return Err(Error::new("index", "out of bounds"));
                }
                let value = a.remove(from);
                if method == "dragRenderableItem" {
                    let to = arg(args, 2)?
                        .as_u64()
                        .ok_or_else(|| Error::new("index", "index required"))?
                        as usize;
                    if to > a.len() {
                        return Err(Error::new("index", "out of bounds"));
                    }
                    a.insert(to, value);
                }
                Ok(json!(true))
            }
            "createAsset" => {
                let name = string(args, 0)?;
                let value = parse_argument(args.get(1))?;
                self.put_resource(name, Some(value))?;
                Ok(json!(name))
            }
            "createParticle" | "addParticle" => {
                let input = parse_argument(args.first())?;
                let config = if let Some(source) = input.get("source").and_then(Value::as_str) {
                    let mut value = self.resource(source)?;
                    if let Some(name) = input.get("name") {
                        value["name"] = name.clone();
                    }
                    value
                } else {
                    input
                };
                let name = config["name"].as_str().unwrap_or("particle");
                let file = config["file"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        format!(
                            "particles/{}.json",
                            safe_basename(name).unwrap_or_else(|_| "particle".into())
                        )
                    });
                if method == "createParticle" || self.read_bytes(&file).is_err() {
                    self.put_resource(&file, Some(config.clone()))?;
                }
                if method == "createParticle" {
                    Ok(json!({"name":name,"file":file}))
                } else {
                    let id = self.add_layer(json!({"name":name,"particle":file}), None)?;
                    Ok(json!({"id":id,"file":file}))
                }
            }
            "deleteParticle" => {
                let name = string(args, 0)?;
                self.put_resource(name, None)?;
                self.objects_mut()?
                    .retain(|v| v["particle"].as_str() != Some(name));
                Ok(json!(true))
            }
            "applyParticle" => {
                let target = arg(args, 0)?.clone();
                if self.index(&target).is_ok()
                    && args
                        .get(1)
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.starts_with('{'))
                {
                    let file = string(args, 1)?;
                    self.resource(file)?;
                    self.layer_mut(&target)?["particle"] = json!(file);
                } else {
                    let name = self
                        .resource_path(&target)
                        .ok_or_else(|| Error::new("particle", "handle/path required"))?;
                    self.put_resource(&name, Some(parse_argument(args.get(1))?))?;
                }
                Ok(json!(true))
            }
            "saveParticle" | "saveModel" => {
                let source = self
                    .resource_path(arg(args, 0)?)
                    .ok_or_else(|| Error::new("resource", "handle/path required"))?;
                let destination = args.get(1).and_then(Value::as_str).unwrap_or(&source);
                let value = self.resource(&source)?;
                self.put_resource(destination, Some(value))?;
                Ok(json!(destination))
            }
            "addParticleElement"
            | "removeParticleElement"
            | "duplicateParticleElement"
            | "orderParticleElements" => self.particle_elements(method, args),
            "getParticleStats" => {
                let name = self
                    .resource_path(arg(args, 0)?)
                    .ok_or_else(|| Error::new("particle", "handle/path required"))?;
                let particle = self.resource(&name)?;
                Ok(
                    json!({"initializers":particle["initializer"].as_array().map_or(0,Vec::len),"operators":particle["operator"].as_array().map_or(0,Vec::len),"emitters":particle["emitter"].as_array().map_or(0,Vec::len),"children":particle["children"].as_array().map_or(0,Vec::len)}),
                )
            }
            "loadParticleInEditor" | "loadModelEditor" => {
                let name = string(args, 0)?;
                let mut value = if name.ends_with(".mdl") {
                    wpe_formats::mdl::Model::parse(&self.read_bytes(name)?)
                        .map_err(|e| Error::new("model", e.to_string()))?
                        .summary()
                } else {
                    self.resource(name)?
                };
                let id = self.resource_handle(name);
                self.session[if method == "loadModelEditor" {
                    "model"
                } else {
                    "particle"
                }] = json!({"id":id,"file":name});
                value["id"] = id;
                value["file"] = json!(name);
                value["preview"] = self.request_host(method, args);
                Ok(value)
            }
            "stopParticleEditorPreview" | "stopModelEditorPreview" => {
                self.session[if method == "stopModelEditorPreview" {
                    "model"
                } else {
                    "particle"
                }] = Value::Null;
                Ok(self.request_host(method, args))
            }
            "reloadParticles" | "reloadModel" => {
                self.events.push(EditorEvent {
                    kind: "resourcesChanged".into(),
                    payload: json!({"method":method}),
                });
                Ok(json!(true))
            }
            "getUsedUserProperties" => {
                let mut names = BTreeSet::new();
                fn scan(v: &Value, out: &mut BTreeSet<String>) {
                    if let Some(name) = v
                        .get("user")
                        .and_then(|v| v.get("name"))
                        .and_then(Value::as_str)
                    {
                        out.insert(name.into());
                    }
                    match v {
                        Value::Object(o) => {
                            for v in o.values() {
                                scan(v, out)
                            }
                        }
                        Value::Array(a) => {
                            for v in a {
                                scan(v, out)
                            }
                        }
                        _ => {}
                    }
                }
                scan(&self.state.scene, &mut names);
                for value in self.state.resources.values().flatten() {
                    scan(value, &mut names);
                }
                Ok(json!(names))
            }
            "checkSceneAnimated" => {
                fn animated(v: &Value) -> bool {
                    match v {
                        Value::Object(o) => {
                            o.contains_key("animation")
                                || o.contains_key("script")
                                || o.contains_key("particle")
                                || o.values().any(animated)
                        }
                        Value::Array(a) => a.iter().any(animated),
                        _ => false,
                    }
                }
                Ok(json!(animated(&self.state.scene)))
            }
            "getUnusedAssets" | "cleanUnusedAssets" => {
                let mut references = self.state.project.to_string() + &self.state.scene.to_string();
                let names = self.file_list()?;
                for name in &names {
                    if name.ends_with(".json")
                        && let Ok(bytes) = self.read_bytes(name)
                        && let Ok(doc) = wpe_formats::jsonc::parse(&bytes)
                    {
                        references.push_str(&doc.to_string());
                    }
                }
                for resource in self.state.resources.values().flatten() {
                    references.push_str(&resource.to_string());
                }
                let unused = names
                    .into_iter()
                    .filter(|name| {
                        !matches!(name.as_str(), "project.json" | "scene.json")
                            && !name.ends_with(".json")
                            && !references.contains(name)
                            && !Path::new(name)
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .is_some_and(|s| references.contains(s))
                            && !name.starts_with(".wpe-")
                    })
                    .collect::<Vec<_>>();
                if method == "cleanUnusedAssets" {
                    let selected = args.first().and_then(Value::as_array).ok_or_else(|| {
                        Error::new("confirmation", "explicit unused file list required")
                    })?;
                    for name in selected {
                        let name = name
                            .as_str()
                            .ok_or_else(|| Error::new("path", "filename required"))?;
                        if !unused.iter().any(|v| v == name) {
                            return Err(Error::new("reference", "file is referenced or unknown"));
                        }
                        self.put_bytes(name, None)?;
                    }
                }
                Ok(json!(unused))
            }
            "deleteCustomEffect" => {
                self.put_resource(string(args, 0)?, None)?;
                Ok(json!(true))
            }
            "duplicateEffectForEditing" => {
                let source = string(args, 0)?;
                let destination = string(args, 1)?;
                let value = self.resource(source)?;
                self.put_resource(destination, Some(value))?;
                Ok(json!(destination))
            }
            "genericSceneAction" => self.action(false, args),
            "modelAction" => self.action(true, args),
            "saveEditorSettings"
            | "setGridVisible"
            | "setSelectionBoxVisible"
            | "setEditorGraphicsFlagsVisible"
            | "setGizmoMode"
            | "setGizmoOrientation"
            | "setCameraMode"
            | "markResourceSelected"
            | "updateParticleEditorSettings"
            | "setAnimationPreviewFrame"
            | "setAnimationPreviewPlayback"
            | "updatePaintOptions"
            | "startPickPosition"
            | "endPickPosition" => {
                if method == "updatePaintOptions" && self.session["paint"].is_object() {
                    self.session["paint"]["options"] = parse_argument(args.first())?;
                }
                self.session["tools"][method] = json!(args);
                Ok(json!(true))
            }
            "setSelectedRenderable" => {
                let selected = args.first().cloned().unwrap_or(Value::Null);
                self.session["selection"] = if selected.is_null() {
                    json!([])
                } else if selected.is_array() {
                    selected
                } else {
                    json!([selected])
                };
                Ok(json!(true))
            }
            "resetView" => {
                self.session["view"] = json!({});
                Ok(json!(true))
            }
            "getViewTransforms" => Ok(self.session["view"].clone()),
            "setViewTransforms" => {
                self.session["view"] = parse_argument(args.first())?;
                Ok(json!(true))
            }
            "beginPaint" => self.begin_paint(args),
            "paintStroke" => self.paint(args, false),
            "clearPaint" => self.paint(args, true),
            "endPaint" => {
                if args.first().and_then(Value::as_bool) == Some(false) {
                    self.rollback()?
                } else {
                    self.commit()?
                }
                self.session["paint"] = Value::Null;
                Ok(json!(true))
            }
            "applyPaintHistory" => Ok(json!(
                if args.first().and_then(Value::as_bool) == Some(true) {
                    self.redo()?
                } else {
                    self.undo()?
                }
            )),
            "getPublishInfoMessages" => Ok(
                json!({"unsaved":self.state.dirty,"empty":self.objects()?.is_empty(),"project_type":self.state.project["type"]}),
            ),
            "performGenerateNormalMap"
            | "performGenerateDepthMap"
            | "performGrabCut"
            | "updateCharacterSeparationMask"
            | "finishCharacterSheetCreation"
            | "cancelForegroundCompiler" => self.image_tool(method, args),
            "previewSizeChanged"
            | "previewToggleVisibility"
            | "restartPreview"
            | "unlockPublishAchievement"
            | "validateVideoFile"
            | "selectModelTexture"
            | "openInExplorer"
            | "saveAs"
            | "deleteProject"
            | "sceneMouseMove"
            | "sceneMouseWheel"
            | "sceneKey"
            | "scenePaintOverrideMove"
            | "startEffectPreview"
            | "stopEffectPreview"
            | "applyWallpaper"
            | "runScriptPreview"
            | "performUserShortcutTest"
            | "performUserTextureTest"
            | "choosePublishMethod"
            | "preparePublishAssets"
            | "queryWorkshop"
            | "queryWorkshopPublishedFiles"
            | "addLayersFromWorkshopAsset"
            | "confirmClose"
            | "deleteFileFromWorkshop"
            | "startVideoExport"
            | "cancelVideoExport"
            | "exportListModalPreviews"
            | "selectExternalApp"
            | "checkExternalApp"
            | "getCropSettings"
            | "isDLCInstalled"
            | "installDLC"
            | "uninstallDLC"
            | "getDLCDownloadProgress"
            | "requestUpdateProjectBackendTags"
            | "navigateToFile"
            | "canRunScriptPreview" => Ok(self.request_host(method, args)),
            _ => Err(Error::new(
                "unknown-command",
                format!("unknown editor method {method}"),
            )),
        }
    }
    fn particle_elements(&mut self, method: &str, args: &[Value]) -> Result<Value> {
        let name = self
            .resource_path(arg(args, 0)?)
            .ok_or_else(|| Error::new("resource", "particle handle/path required"))?;
        let key = string(args, 1)?;
        if ![
            "initializer",
            "operator",
            "emitter",
            "renderer",
            "controlpoint",
            "children",
        ]
        .contains(&key)
        {
            return Err(Error::new("particle", "invalid element collection"));
        }
        let mut resource = self.resource(&name)?;
        let array = resource
            .as_object_mut()
            .ok_or_else(|| Error::new("particle", "object required"))?
            .entry(key)
            .or_insert(json!([]))
            .as_array_mut()
            .ok_or_else(|| Error::new("particle", "array required"))?;
        let result = if method == "addParticleElement" {
            let mut item = parse_argument(args.get(2))?;
            if !item.is_object() {
                return Err(Error::new("particle", "element configuration required"));
            }
            item["id"] = json!(
                array
                    .iter()
                    .filter_map(|v| v["id"].as_u64())
                    .max()
                    .unwrap_or(0)
                    + 1
            );
            array.push(item.clone());
            item
        } else {
            let selected = arg(args, 2)?;
            let index = array
                .iter()
                .position(|v| equal_id(&v["id"], selected))
                .or_else(|| selected.as_u64().map(|v| v as usize))
                .ok_or_else(|| Error::new("index", "index required"))?;
            if index >= array.len() {
                return Err(Error::new("index", "out of bounds"));
            }
            if method == "removeParticleElement" {
                array.remove(index);
                json!(true)
            } else if method == "duplicateParticleElement" {
                let mut value = array[index].clone();
                value["id"] = json!(
                    array
                        .iter()
                        .filter_map(|v| v["id"].as_u64())
                        .max()
                        .unwrap_or(0)
                        + 1
                );
                array.insert(index + 1, value.clone());
                json!({"index":index+1,"item":value})
            } else {
                let to = arg(args, 3)?
                    .as_u64()
                    .ok_or_else(|| Error::new("index", "index required"))?
                    as usize;
                if to >= array.len() {
                    return Err(Error::new("index", "out of bounds"));
                }
                let value = array.remove(index);
                array.insert(to, value);
                json!(true)
            }
        };
        self.put_resource(&name, Some(resource))?;
        Ok(result)
    }
}

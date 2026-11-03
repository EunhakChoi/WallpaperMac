//! Public native providers for editor host requests. Every return value describes
//! performed work or a pending job whose progress can be queried independently.
use super::*;
use wpe_platform::export::ExportSettings;
impl AppDelegate {
    pub(super) fn editor_local_event(&self, event: &NSEvent) -> bool {
        let mut state = self.ivars().borrow_mut();
        let Some(preview) = &state.editor_preview else {
            return false;
        };
        if event.window(self.mtm()).as_deref() != Some(&*preview.window) {
            return false;
        }
        let mut pan = state.editor_pan;
        let mut zoom = state.editor_zoom;
        match event.r#type() {
            NSEventType::ScrollWheel => {
                let delta = event.scrollingDeltaY() as f32;
                if !delta.is_finite() {
                    return false;
                }
                zoom = (zoom * (delta.clamp(-100., 100.) * 0.02).exp()).clamp(0.01, 100.);
            }
            NSEventType::OtherMouseDragged => {
                let canvas = preview.surface.canvas_size().unwrap_or([1920., 1080.]);
                let bounds = preview.surface.view().bounds();
                pan[0] -= event.deltaX() as f32 * canvas[0] / (bounds.size.width as f32).max(1.);
                pan[1] += event.deltaY() as f32 * canvas[1] / (bounds.size.height as f32).max(1.);
            }
            NSEventType::KeyDown => {
                if event
                    .modifierFlags()
                    .intersects(NSEventModifierFlags::Command | NSEventModifierFlags::Control)
                {
                    return false;
                }
                match event.keyCode() {
                    123 => pan[0] -= 10.,
                    124 => pan[0] += 10.,
                    125 => pan[1] -= 10.,
                    126 => pan[1] += 10.,
                    29 => {
                        pan = [0.; 2];
                        zoom = 1.;
                    }
                    _ => return false,
                }
            }
            _ => return false,
        }
        if pan.iter().any(|n| !n.is_finite()) {
            return false;
        }
        let result = preview.surface.set_preview_camera(zoom, pan);
        state.editor_zoom = zoom;
        state.editor_pan = pan;
        if let Err(error) = result {
            append_bounded(&mut state.report, &error);
        }
        true
    }
    pub(super) fn additional_editor_host(
        &self,
        method: &str,
        args: &[Value],
    ) -> Result<Value, String> {
        match method {
            "startVideoExport" => {
                if self
                    .ivars()
                    .borrow()
                    .export
                    .as_ref()
                    .is_some_and(|job| job.is_active())
                {
                    return Err("An export is already running; cancel it first".into());
                }
                let mut settings: ExportSettings =
                    serde_json::from_value(parsed(args.first())?).map_err(|e| e.to_string())?;
                settings.validate().map_err(|e| e.to_string())?;
                if settings.output.is_none() {
                    let panel = NSSavePanel::savePanel(self.mtm());
                    panel.setCanCreateDirectories(true);
                    panel.setNameFieldStringValue(&NSString::from_str(&format!(
                        "Wallpaper.{}",
                        settings.extension()
                    )));
                    if panel.runModal() != NSModalResponseOK {
                        return Ok(json!({"cancelled":true}));
                    }
                    settings.output = Some(PathBuf::from(
                        panel
                            .URL()
                            .and_then(|url| url.path())
                            .ok_or("No output path selected")?
                            .to_string(),
                    ));
                }
                let project = self.preview_project()?;
                let ffmpeg = self
                    .ivars()
                    .borrow()
                    .service
                    .config
                    .external_tools
                    .get("ffmpeg")
                    .cloned();
                let initial_input = {
                    let state = self.ivars().borrow();
                    PlayerInput {
                        audio_left: state.bands.bands64[0].to_vec(),
                        audio_right: state.bands.bands64[1].to_vec(),
                        media: state.media.value(),
                        time_of_day: facts::calendar().1,
                        ..Default::default()
                    }
                };
                let job = export::ExportJob::new(
                    project,
                    settings,
                    ffmpeg.as_deref(),
                    initial_input,
                    self.mtm(),
                )?;
                let status = job.status();
                self.ivars().borrow_mut().export = Some(job);
                Ok(json!({"started":true,"complete":false,"status":status}))
            }
            "cancelVideoExport" => {
                let mut state = self.ivars().borrow_mut();
                if let Some(job) = &mut state.export {
                    job.cancel();
                    return Ok(job.status());
                }
                Ok(json!({"cancelled":false,"idle":true}))
            }
            "performUserTextureTest" | "performUserShortcutTest" => {
                let key = args
                    .first()
                    .and_then(Value::as_str)
                    .ok_or("User property key required")?;
                {
                    let state = self.ivars().borrow();
                    if state
                        .editor
                        .as_ref()
                        .ok_or("Editor closed")?
                        .snapshot()
                        .project
                        .pointer("/general/properties")
                        .and_then(|p| p.get(key))
                        .is_none()
                    {
                        return Err("Unknown user property key".into());
                    }
                }
                let Some(path) = self.choose_path(false) else {
                    return Ok(json!({"cancelled":true}));
                };
                let value = if method == "performUserTextureTest" {
                    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
                    if bytes.len() > 128 * 1024 * 1024 {
                        return Err("Preview texture exceeds128MiB".into());
                    }
                    let filename = path
                        .file_name()
                        .ok_or("No selected filename")?
                        .to_string_lossy();
                    let name = format!("__preview/{filename}");
                    self.ivars()
                        .borrow_mut()
                        .editor_test_resources
                        .insert(name.clone(), std::sync::Arc::new(bytes));
                    json!(name)
                } else {
                    let url = NSURL::fileURLWithPath_isDirectory(
                        &NSString::from_str(&path.to_string_lossy()),
                        path.is_dir(),
                    );
                    if !NSWorkspace::sharedWorkspace().openURL(&url) {
                        return Err("macOS could not open the selected shortcut".into());
                    }
                    json!(path)
                };
                self.ivars()
                    .borrow_mut()
                    .editor_test_properties
                    .insert(key.into(), value.clone());
                self.refresh_editor_preview()?;
                Ok(
                    json!({"property":key,"value":value,"temporary":true,"shortcut_launched":method=="performUserShortcutTest"}),
                )
            }
            "sceneMouseMove" => {
                if self.ivars().borrow().editor_preview.is_none() {
                    self.show_editor_preview(true)?;
                }
                let value = parsed(args.first())?;
                let state = self.ivars().borrow();
                let preview = state.editor_preview.as_ref().ok_or("Preview closed")?;
                let size = preview.surface.view().bounds().size;
                let scale = preview.window.backingScaleFactor() as f32;
                let x = value["x"].as_f64().unwrap_or(0.) as f32;
                let y = value["y"].as_f64().unwrap_or(0.) as f32;
                if !x.is_finite() || !y.is_finite() {
                    return Err("Finite pointer coordinates required".into());
                }
                let width = (size.width as f32 * scale).max(1.);
                let height = (size.height as f32 * scale).max(1.);
                let mode = value["mode"].as_str().unwrap_or("move");
                let buttons = if mode == "down" {
                    value["key"].as_u64().unwrap_or(1) as u32
                } else if mode == "up" {
                    0
                } else {
                    NSEvent::pressedMouseButtons() as u32
                };
                preview.surface.set_pointer(
                    [x / width, 1. - y / height],
                    buttons,
                    mode != "leave",
                )?;
                Ok(json!({"delivered":true,"coordinates":"physical preview pixels, Y up"}))
            }
            "sceneMouseWheel" => {
                let value = parsed(args.first())?;
                let delta = value["delta"].as_f64().unwrap_or(0.) as f32;
                if !delta.is_finite() {
                    return Err("Finite wheel delta required".into());
                }
                let mut state = self.ivars().borrow_mut();
                state.editor_zoom =
                    (state.editor_zoom * (delta.clamp(-100., 100.) * 0.08).exp()).clamp(0.01, 100.);
                if let Some(preview) = &state.editor_preview {
                    preview
                        .surface
                        .set_preview_camera(state.editor_zoom, state.editor_pan)?;
                }
                Ok(json!({"zoom":state.editor_zoom}))
            }
            "sceneKey" => {
                let value = parsed(args.first())?;
                if value["mode"] != "down" {
                    return Ok(json!({"handled":false}));
                }
                let key = value["key"].as_u64().unwrap_or(0);
                let mut state = self.ivars().borrow_mut();
                match key {
                    37 => state.editor_pan[0] -= 10.,
                    39 => state.editor_pan[0] += 10.,
                    38 => state.editor_pan[1] += 10.,
                    40 => state.editor_pan[1] -= 10.,
                    36 | 48 => {
                        state.editor_pan = [0.; 2];
                        state.editor_zoom = 1.;
                    }
                    _ => {
                        return Ok(
                            json!({"handled":false,"reason":"No SceneScript keyboard hook exists"}),
                        );
                    }
                }
                if let Some(preview) = &state.editor_preview {
                    preview
                        .surface
                        .set_preview_camera(state.editor_zoom, state.editor_pan)?;
                }
                Ok(json!({"handled":true,"pan":state.editor_pan,"zoom":state.editor_zoom}))
            }
            "scenePaintOverrideMove" => {
                let value = parsed(args.first())?;
                let mut state = self.ivars().borrow_mut();
                let editor = state.editor.as_mut().ok_or("Editor closed")?;
                let mut options = editor
                    .snapshot()
                    .session
                    .pointer("/paint/options")
                    .cloned()
                    .unwrap_or(json!({}));
                if let Some(radius) = value["radius"].as_f64() {
                    if !radius.is_finite() || radius <= 0. || radius > 8192. {
                        return Err("Brush radius must be finite0..8192".into());
                    }
                    options["radius"] = json!(radius);
                }
                editor
                    .execute("updatePaintOptions", json!([options]))
                    .map_err(|e| e.to_string())
                    .map(|r| r.value)
            }
            "loadModelEditor"
            | "playClipAnimation"
            | "playStaticAnimation"
            | "stopAnimation"
            | "setEditMode" => {
                if method == "loadModelEditor" {
                    let mut state = self.ivars().borrow_mut();
                    let editor = state.editor.as_mut().ok_or("Editor closed")?;
                    let id = editor.snapshot().session["model"]["id"].clone();
                    editor
                        .execute("genericModelAction", json!(["setEditMode", id, {}]))
                        .map_err(|e| e.to_string())?;
                }
                self.ivars().borrow_mut().editor_preview_scene = None;
                self.show_editor_preview(true)?;
                self.refresh_editor_preview()?;
                Ok(
                    json!({"preview":true,"profile":"editable model sidecar with original MDL source"}),
                )
            }
            "loadParticleInEditor" => {
                let file = args
                    .first()
                    .and_then(Value::as_str)
                    .ok_or("Particle resource path required")?;
                let project = self.preview_project()?;
                project.read_resource(file).map_err(|e| e.to_string())?;
                self.ivars().borrow_mut().editor_preview_scene = Some(
                    json!({"general":{"orthogonalprojection":{"width":1024,"height":768}},"objects":[{"id":1,"name":"Particle preview","particle":file,"origin":"512 384 0"}]}),
                );
                self.show_editor_preview(true)?;
                self.refresh_editor_preview()?;
                Ok(json!({"preview":true,"resource":file}))
            }
            "stopModelEditorPreview" | "stopParticleEditorPreview" | "stopEffectPreview" => {
                let mut state = self.ivars().borrow_mut();
                state.editor_preview_scene = None;
                if method == "stopModelEditorPreview"
                    && let Some(editor) = &mut state.editor
                {
                    editor.clear_model_preview();
                }
                drop(state);
                self.refresh_editor_preview()?;
                Ok(json!({"stopped":true}))
            }
            "startEffectPreview" => {
                let path = args
                    .get(1)
                    .or_else(|| args.first())
                    .and_then(Value::as_str)
                    .ok_or("A preview resource path is required")?;
                let project = self.preview_project()?;
                let bytes = project.read_resource(path).map_err(|e| e.to_string())?;
                let value = wpe_formats::jsonc::parse(&bytes.bytes).map_err(|e| e.to_string())?;
                if value.get("objects").is_some() {
                    self.ivars().borrow_mut().editor_preview_scene = Some(value);
                } else if value.get("material").is_some() {
                    self.ivars().borrow_mut().editor_preview_scene = Some(
                        json!({"general":{"orthogonalprojection":{"width":1024,"height":768}},"objects":[{"id":1,"name":"Effect preview","image":path,"origin":"512 384 0","size":"768 576"}]}),
                    );
                } else {
                    return Err("Effect preview must declare an actual scene or image resource; an effect definition alone requires a target image".into());
                }
                self.show_editor_preview(true)?;
                self.refresh_editor_preview()?;
                Ok(json!({"preview":true,"resource":path}))
            }
            "cameraEnterPOV" | "cameraEnterPOVPath" => {
                let id = args.first().ok_or("Camera layer ID required")?;
                let state = self.ivars().borrow();
                let scene = state
                    .editor
                    .as_ref()
                    .ok_or("Editor closed")?
                    .preview_scene();
                let object = find_object(&scene, id)?;
                let camera = object
                    .get("camera")
                    .cloned()
                    .unwrap_or_else(|| object.clone());
                if camera.get("eye").is_none() && camera.get("center").is_none() {
                    return Err("Selected layer has no explicit camera eye/center data".into());
                }
                drop(state);
                let mut state = self.ivars().borrow_mut();
                state.editor_view = Some(camera.clone());
                if let Some(preview) = &state.editor_preview {
                    preview.surface.set_preview_view(Some(camera))?;
                }
                Ok(json!({"camera":id,"transient":true}))
            }
            "cameraExitPOV" | "cameraExitPOVPath" => {
                let mut state = self.ivars().borrow_mut();
                state.editor_view = None;
                if let Some(preview) = &state.editor_preview {
                    preview.surface.set_preview_view(None)?;
                }
                Ok(json!(true))
            }
            "getCropSettings" => {
                let project = self.preview_project()?;
                let scene = scene_document(&project)?;
                let object = find_object(&scene, args.first().ok_or("Layer ID required")?)?;
                let texture = object.get("image").and_then(Value::as_str);
                Ok(
                    json!({"renderable":object["id"],"image":texture,"origin":object["origin"],"size":object["size"],"scale":object["scale"],"rotation":object["angles"],"crop":object.get("crop").cloned().unwrap_or(Value::Null),"profile":"authored bounds; texture generator applies explicit crop options"}),
                )
            }
            "cleanLocalStorage" => {
                let mut state = self.ivars().borrow_mut();
                let root = state
                    .editor
                    .as_ref()
                    .ok_or("Editor closed")?
                    .root()
                    .to_string_lossy()
                    .into_owned();
                let prefix = format!("{root}::");
                let before = state.service.config.scene_storage.len();
                state
                    .service
                    .config
                    .scene_storage
                    .retain(|key, _| !key.starts_with(&prefix));
                let removed = state.service.config.scene_storage.len() != before;
                if let Some(preview) = &state.editor_preview {
                    preview.surface.clear_storage()?;
                }
                state.service.save().map_err(|e| e.to_string())?;
                Ok(json!({"cleared":true,"persisted_key_removed":removed}))
            }
            "runExternalImageEditor" => {
                let project = self.preview_project()?;
                let value = args
                    .get(1)
                    .or_else(|| args.first())
                    .ok_or("Image resource required")?;
                let file = value
                    .as_str()
                    .or_else(|| value["file"].as_str())
                    .ok_or("Relative image filename required")?;
                let bytes = project.read_resource(file).map_err(|e| e.to_string())?;
                let (root, tool) = {
                    let state = self.ivars().borrow();
                    (
                        state
                            .editor
                            .as_ref()
                            .ok_or("Editor closed")?
                            .root()
                            .to_owned(),
                        state
                            .service
                            .config
                            .external_tools
                            .get("image_editor")
                            .cloned(),
                    )
                };
                let output = root.join("external-edit").join(
                    Path::new(file)
                        .file_name()
                        .ok_or("Invalid image filename")?,
                );
                std::fs::create_dir_all(output.parent().unwrap()).map_err(|e| e.to_string())?;
                std::fs::write(&output, &*bytes.bytes).map_err(|e| e.to_string())?;
                if let Some(tool) = tool {
                    launch_tool(&tool, &output)?;
                } else {
                    let url = NSURL::fileURLWithPath_isDirectory(
                        &NSString::from_str(&output.to_string_lossy()),
                        false,
                    );
                    if !NSWorkspace::sharedWorkspace().openURL(&url) {
                        return Err("No image editor accepted this file; configure image_editor or choose an associated application".into());
                    }
                }
                Ok(
                    json!({"launched":true,"file":output,"completion":"Re-import the edited file to commit pixels into the document"}),
                )
            }
            "importPuppetReferenceOverlay"
            | "importPuppetWarpDepth"
            | "exportPuppetWarpDepth"
            | "paintPuppetClippingMask" => self.puppet_host(method, args),
            "requestUpdateProjectBackendTags" => {
                let mut state = self.ivars().borrow_mut();
                let editor = state.editor.as_mut().ok_or("Editor closed")?;
                let mut project = editor.snapshot().project;
                let mut tags = project["tags"].as_array().cloned().unwrap_or_default();
                for kind in [
                    "Scene",
                    if project["general"]["properties"].is_object() {
                        "Customizable"
                    } else {
                        "Scene"
                    },
                ] {
                    if !tags.iter().any(|v| v.as_str() == Some(kind)) {
                        tags.push(json!(kind));
                    }
                }
                project["tags"] = json!(tags);
                let reply = editor
                    .execute("saveProjectData", json!([project]))
                    .map_err(|e| e.to_string())?;
                Ok(
                    json!({"local_tags_updated":true,"tags":editor.snapshot().project["tags"],"revision":reply.revision,"uploaded":false}),
                )
            }
            "queryWorkshopPublishedFiles" => self.steam_command(
                "steam.published",
                args.first()
                    .filter(|v| v.is_object())
                    .cloned()
                    .unwrap_or(json!({})),
            ),
            "deleteFileFromWorkshop" => {
                let id = args.first().ok_or("Workshop item ID required")?;
                let alert = NSAlert::new(self.mtm());
                alert.setMessageText(ns_string!("Delete this published Workshop item?"));
                alert.setInformativeText(&NSString::from_str(&format!(
                    "Item {id}. Steam deletes this server item permanently."
                )));
                alert.addButtonWithTitle(ns_string!("Delete Workshop item"));
                alert.addButtonWithTitle(ns_string!("Cancel"));
                if alert.runModal() != NSAlertFirstButtonReturn {
                    return Ok(json!({"cancelled":true}));
                }
                self.steam_command("steam.delete", json!({"id":id}))
            }
            "addLayersFromWorkshopAsset" => self.import_workshop_asset(args),
            "exportListModalPreviews" => self.export_asset_previews(args),
            "unlockPublishAchievement" => {
                let kind = self
                    .ivars()
                    .borrow()
                    .editor
                    .as_ref()
                    .ok_or("Editor closed")?
                    .snapshot()
                    .project["type"]
                    .as_str()
                    .unwrap_or("scene")
                    .to_owned();
                self.steam_command("steam.achievement.publish", json!({"kind":kind}))
            }
            "addFBXfile" => self.import_fbx_animation(args),
            _ => Err(format!(
                "Unsupported native capability: {method}; no operation was committed"
            )),
        }
    }
    fn import_fbx_animation(&self, args: &[Value]) -> Result<Value, String> {
        let id = args.first().ok_or("Model resource or layer ID required")?;
        let path = match self.choose_path(false) {
            Some(path) => path,
            None => return Ok(json!({"cancelled":true})),
        };
        if !path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("fbx"))
        {
            return Err("Choose a supported binary or ASCII FBX animation file".into());
        }
        if std::fs::metadata(&path)
            .map_err(|error| error.to_string())?
            .len()
            > 32 * 1024 * 1024
        {
            return Err("FBX file exceeds the32MiB import policy".into());
        }
        let bytes = std::sync::Arc::new(std::fs::read(&path).map_err(|error| error.to_string())?);
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or("FBX filename must be UTF-8")?;
        let value = {
            let mut state = self.ivars().borrow_mut();
            state
                .editor
                .as_mut()
                .ok_or("Editor closed")?
                .import_fbx_for_resource(id, name, bytes)
                .map_err(|error| error.to_string())?
        };
        self.refresh_editor_preview()?;
        Ok(value)
    }
    fn puppet_host(&self, method: &str, args: &[Value]) -> Result<Value, String> {
        let id = args.first().ok_or("Layer ID required")?.clone();
        let project = self.preview_project()?;
        let scene = scene_document(&project)?;
        let object = find_object(&scene, &id)?.clone();
        if method == "paintPuppetClippingMask" {
            let image = object["image"].as_str().ok_or("Image layer required")?;
            let descriptor = resource_json(&project, image)?;
            let material = descriptor["material"]
                .as_str()
                .ok_or("Image material required")?;
            let mut state = self.ivars().borrow_mut();
            return state
                .editor
                .as_mut()
                .ok_or("Editor closed")?
                .execute(
                    "beginPaint",
                    json!([id,material,0,3,{"radius":8.,"opacity":1.,"color":[1,1,1,1]}]),
                )
                .map(|r| r.value)
                .map_err(|e| e.to_string());
        }
        if method == "exportPuppetWarpDepth" {
            let shape = object
                .pointer("/modeldata/shapes/0")
                .or_else(|| object.pointer("/modeldata/model/shapes/0"))
                .ok_or("Editable shape is required")?;
            let panel = NSSavePanel::savePanel(self.mtm());
            panel.setNameFieldStringValue(ns_string!("Puppet-depth.json"));
            if panel.runModal() != NSModalResponseOK {
                return Ok(json!({"cancelled":true}));
            }
            let path = PathBuf::from(
                panel
                    .URL()
                    .and_then(|u| u.path())
                    .ok_or("No output URL")?
                    .to_string(),
            );
            let bytes=serde_json::to_vec_pretty(&json!({"profile":"wpe-editable-depth-v1","vertexFormat":shape["vertexFormat"],"vertexBuffer":shape["vertexBuffer"]})).map_err(|e|e.to_string())?;
            std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
            return Ok(json!({"written":path}));
        }
        let path = match self.choose_path(false) {
            Some(p) => p,
            None => return Ok(json!({"cancelled":true})),
        };
        let basename = path
            .file_name()
            .ok_or("No selected filename")?
            .to_string_lossy();
        let destination = format!("imports/{basename}");
        let mut state = self.ivars().borrow_mut();
        let editor = state.editor.as_mut().ok_or("Editor closed")?;
        if method == "importPuppetReferenceOverlay" {
            editor
                .import_external_file(&path, &destination)
                .map_err(|e| e.to_string())?;
            let reply = editor
                .execute(
                    "sendAssetPropertyChange",
                    json!([{"id":id,"properties":{"referenceoverlay":destination}}]),
                )
                .map_err(|e| e.to_string())?;
            return Ok(json!({"imported":destination,"revision":reply.revision}));
        }
        let mut shape = object
            .pointer("/modeldata/shapes/0")
            .cloned()
            .ok_or("Editable modeldata.shapes[0] required")?;
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let depth=wpe_formats::jsonc::parse(&bytes).map_err(|_|"Depth import uses the exported wpe-editable-depth-v1 JSON, preserving exact finite vertex depths")?;
        if depth["vertexFormat"] != shape["vertexFormat"] {
            return Err("Depth vertex format differs from this mesh".into());
        }
        let source = depth["vertexBuffer"]
            .as_array()
            .ok_or("Depth vertexBuffer required")?;
        let target = shape["vertexBuffer"]
            .as_array_mut()
            .ok_or("Mesh vertexBuffer required")?;
        if source.len() != target.len()
            || source
                .iter()
                .any(|v| !v.as_f64().is_some_and(f64::is_finite))
        {
            return Err("Depth vertex count or finite components differ".into());
        }
        let format = shape["vertexFormat"]
            .as_array()
            .cloned()
            .ok_or("Mesh format required")?;
        let mut stride = 0;
        let mut position = None;
        for name in format {
            let size = match name.as_str() {
                Some("position" | "normal") => 3,
                Some("uv") => 2,
                Some("color" | "tangentSigned") => 4,
                _ => return Err("Unsupported editable vertex component".into()),
            };
            if name == "position" {
                position = Some(stride);
            }
            stride += size;
        }
        let offset = position.ok_or("Position component required")? + 2;
        if stride == 0 || !source.len().is_multiple_of(stride) {
            return Err("Depth stride is invalid".into());
        }
        let target = shape["vertexBuffer"].as_array_mut().unwrap();
        for base in (0..source.len()).step_by(stride) {
            target[base + offset] = source[base + offset].clone();
        }
        let mut model = object["modeldata"].clone();
        model["shapes"][0] = shape;
        let reply = editor
            .execute(
                "sendAssetPropertyChange",
                json!([{"id":id,"properties":{"modeldata":model}}]),
            )
            .map_err(|e| e.to_string())?;
        Ok(json!({"depths_imported":source.len()/stride,"revision":reply.revision}))
    }
    fn import_workshop_asset(&self, args: &[Value]) -> Result<Value, String> {
        let request = args
            .first()
            .ok_or("Workshop item ID or installed content path required")?;
        let path = if let Some(path) = request["folder"].as_str() {
            PathBuf::from(path)
        } else {
            let id = request
                .as_u64()
                .or_else(|| request.as_str().and_then(|s| s.parse().ok()))
                .or_else(|| request["id"].as_u64())
                .ok_or("Installed Workshop item ID required")?;
            let state = self.ivars().borrow();
            state
                .steam
                .as_ref()
                .ok_or("Connect Steamworks first")?
                .install_info(id)
                .map_err(|e| e.to_string())?
                .ok_or("Workshop asset is not installed; download it first")?
                .folder
        };
        let assets = self.ivars().borrow().service.config.assets.clone();
        let source = wpe_core::load_project(&path, assets.as_deref()).map_err(|e| e.to_string())?;
        let incoming = source.read_scene_document().map_err(|e| e.to_string())?;
        let objects = incoming["objects"]
            .as_array()
            .ok_or("Workshop asset contains no scene objects")?;
        let current = self.preview_project()?;
        let mut names = std::collections::BTreeSet::<String>::new();
        let mut pending = vec![source.root.clone()];
        let mut total = 0u64;
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                let ty = entry.file_type().map_err(|e| e.to_string())?;
                if ty.is_symlink() {
                    return Err("Workshop asset symlinks are not imported".into());
                }
                if ty.is_dir() {
                    pending.push(entry.path());
                } else if ty.is_file() {
                    let path = entry.path();
                    let name = path
                        .strip_prefix(&source.root)
                        .map_err(|e| e.to_string())?
                        .to_string_lossy()
                        .replace('\\', "/");
                    if name == "project.json"
                        || path
                            .extension()
                            .is_some_and(|e| e.eq_ignore_ascii_case("pkg"))
                    {
                        continue;
                    }
                    total = total
                        .checked_add(entry.metadata().map_err(|e| e.to_string())?.len())
                        .ok_or("Asset size overflow")?;
                    names.insert(name);
                    if names.len() > 100000 || total > 2 * 1024 * 1024 * 1024 {
                        return Err("Workshop import exceeds100000 files or2GiB".into());
                    }
                }
            }
        }
        names.extend(source.vfs.mounted_package_names());
        names.retain(|n| n != "project.json" && Some(n.as_str()) != source.scene_resource_name());
        let mut staged = vec![];
        total = 0;
        for name in names {
            let resource = source.read_resource(&name).map_err(|e| e.to_string())?;
            total += resource.bytes.len() as u64;
            if total > 2 * 1024 * 1024 * 1024 || staged.len() > 100000 {
                return Err("Expanded Workshop asset exceeds bounds".into());
            }
            if let Ok(existing) = current.read_resource(&name) {
                if existing.bytes != resource.bytes {
                    return Err(format!(
                        "Workshop asset resource collision: {name}. Existing project/global data are preserved."
                    ));
                }
                continue;
            }
            staged.push((name, resource.bytes));
        }
        let mut state = self.ivars().borrow_mut();
        let editor = state.editor.as_mut().ok_or("Editor closed")?;
        let mut all = editor.snapshot().scene["objects"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut next = all
            .iter()
            .filter_map(|o| o["id"].as_u64())
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or("Object ID overflow")?;
        let mut remap = BTreeMap::<String, Value>::new();
        for object in objects {
            remap.insert(
                serde_json::to_string(&object["id"]).map_err(|e| e.to_string())?,
                json!(next),
            );
            next = next.checked_add(1).ok_or("Object ID overflow")?;
        }
        for object in objects {
            let mut object = object.clone();
            remap_object_ids(&mut object, &remap);
            all.push(object);
        }
        editor.begin_transaction().map_err(|e| e.to_string())?;
        let result = (|| -> Result<(), String> {
            for (name, bytes) in &mut staged {
                editor
                    .stage_resource_bytes(name, std::sync::Arc::new(std::mem::take(bytes)))
                    .map_err(|e| e.to_string())?;
            }
            editor
                .execute("writeDocumentProperty", json!(["scene", "/objects", all]))
                .map_err(|e| e.to_string())?;
            Ok(())
        })();
        if let Err(error) = result {
            let _ = editor.rollback();
            return Err(error);
        }
        editor.commit().map_err(|e| e.to_string())?;
        Ok(
            json!({"layers_imported":objects.len(),"resources_staged":staged.len(),"bytes":total,"collision_policy":"preserve existing resources; reject differing bytes"}),
        )
    }
    fn export_asset_previews(&self, args: &[Value]) -> Result<Value, String> {
        let items = args
            .first()
            .and_then(Value::as_array)
            .ok_or("Preview entries array required")?;
        if items.len() > 100 {
            return Err("At most100 preview entries per request".into());
        }
        let project = self.preview_project()?;
        let root = self
            .ivars()
            .borrow()
            .service
            .config_path()
            .parent()
            .ok_or("Missing app state directory")?
            .join("AssetPreviews");
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let mut written = vec![];
        for (index, item) in items.iter().enumerate() {
            let path = item["preview"]
                .as_str()
                .ok_or("Each asset needs an actual preview resource path")?;
            let bytes = project.read_resource(path).map_err(|e| e.to_string())?;
            let image = if path.to_ascii_lowercase().ends_with(".tex") {
                wpe_formats::tex::Texture::parse(&bytes.bytes)
                    .and_then(|t| t.decode_rgba8(0, 0))
                    .map_err(|e| e.to_string())?
            } else {
                let decoded = image::load_from_memory(&bytes.bytes)
                    .map_err(|e| e.to_string())?
                    .to_rgba8();
                wpe_formats::RgbaImage {
                    width: decoded.width(),
                    height: decoded.height(),
                    rgba: decoded.into_raw(),
                }
            };
            let output = root.join(format!("asset-{index:03}.png"));
            image::save_buffer_with_format(
                &output,
                &image.rgba,
                image.width,
                image.height,
                image::ColorType::Rgba8,
                image::ImageFormat::Png,
            )
            .map_err(|e| e.to_string())?;
            written.push(json!({"path":item["path"],"preview":output}));
        }
        Ok(json!({"written":written}))
    }
}
fn parsed(value: Option<&Value>) -> Result<Value, String> {
    match value {
        Some(Value::String(s)) => serde_json::from_str(s).map_err(|e| e.to_string()),
        Some(v) => Ok(v.clone()),
        None => Ok(json!({})),
    }
}
fn find_object<'a>(scene: &'a Value, id: &Value) -> Result<&'a Value, String> {
    scene["objects"]
        .as_array()
        .and_then(|a| a.iter().find(|o| o.get("id") == Some(id)))
        .ok_or_else(|| format!("Scene layer {id} not found"))
}
fn launch_tool(tool: &Path, file: &Path) -> Result<(), String> {
    let executable = if tool
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("app"))
    {
        objc2_foundation::NSBundle::bundleWithPath(&NSString::from_str(&tool.to_string_lossy()))
            .and_then(|bundle| bundle.executablePath())
            .map(|path| PathBuf::from(path.to_string()))
            .ok_or("Selected application bundle has no executable")?
    } else {
        tool.to_owned()
    };
    let mut command = std::process::Command::new(executable);
    command
        .arg(file)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    std::thread::Builder::new()
        .name("wallpaper-editor-tool".into())
        .spawn(move || {
            let _ = child.wait();
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn resource_json(project: &wpe_core::Project, name: &str) -> Result<Value, String> {
    wpe_formats::jsonc::parse(
        &project
            .read_resource(name)
            .map_err(|e| e.to_string())?
            .bytes,
    )
    .map_err(|e| e.to_string())
}
fn scene_document(project: &wpe_core::Project) -> Result<Value, String> {
    project.read_scene_document().map_err(|e| e.to_string())
}

fn remap_object_ids(value: &mut Value, ids: &BTreeMap<String, Value>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(key.as_str(), "id" | "parent" | "parentid") {
                    if let Ok(key) = serde_json::to_string(value)
                        && let Some(mapped) = ids.get(&key)
                    {
                        *value = mapped.clone();
                    }
                } else {
                    remap_object_ids(value, ids);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                remap_object_ids(value, ids)
            }
        }
        _ => {}
    }
}

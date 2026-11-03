//! The management WebView alone may call these native editor services.
use super::*;
use embedded::{SurfaceOptions, WallpaperSurface};
use objc2_app_kit::{NSAutoresizingMaskOptions, NSOpenPanel};
pub(super) struct EditorPreview {
    pub window: Retained<NSWindow>,
    pub surface: WallpaperSurface,
}
impl AppDelegate {
    pub(super) fn editor_execute(&self, args: Value) -> Result<Value, String> {
        let method = args["method"].as_str().ok_or("Missing editor command")?;
        let argument = args.get("args").cloned().unwrap_or(Value::Null);
        let reply = {
            let mut state = self.ivars().borrow_mut();
            let editor = state
                .editor
                .as_mut()
                .ok_or("Open a project in the editor first")?;
            if method == "save" {
                editor.save().map_err(|e| e.to_string())?;
                json!({"snapshot":editor.snapshot()})
            } else {
                serde_json::to_value(
                    editor
                        .execute(method, argument)
                        .map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?
            }
        };
        let completions = crate::editor_requests::settle_requests(
            || {
                self.ivars()
                    .borrow()
                    .editor
                    .as_ref()
                    .map(|e| e.pending_host_requests())
                    .ok_or("Editor closed".to_string())
            },
            |request| self.editor_host_request(&request.method, &request.args),
            |id, value| {
                self.ivars()
                    .borrow_mut()
                    .editor
                    .as_mut()
                    .ok_or("Editor closed")?
                    .complete_host_request(id, value)
                    .map_err(|e| e.to_string())
            },
        )?;
        {
            let mut state = self.ivars().borrow_mut();
            if state.editor_close {
                state.editor.take();
                state.editor_preview.take();
                state.editor_close = false;
            }
            if let Some(editor) = state.editor_replacement.take() {
                state.editor = Some(editor);
                state.editor_test_properties.clear();
                state.editor_test_resources.clear();
                state.editor_preview_scene = None;
            }
        }
        if self.ivars().borrow().editor_preview.is_some()
            && !matches!(
                method,
                "sceneMouseMove"
                    | "sceneMouseWheel"
                    | "sceneKey"
                    | "scenePaintOverrideMove"
                    | "startVideoExport"
                    | "cancelVideoExport"
            )
        {
            self.refresh_editor_preview()?;
        }
        Ok(json!({"reply":reply,"host_completions":completions}))
    }
    pub(super) fn show_editor_preview(&self, visible: bool) -> Result<(), String> {
        if !visible {
            if let Some(preview) = &self.ivars().borrow().editor_preview {
                preview.surface.set_paused(true);
                preview.window.orderOut(None);
            }
            return Ok(());
        }
        if let Some(preview) = &self.ivars().borrow().editor_preview {
            preview.window.makeKeyAndOrderFront(None);
            preview.surface.set_paused(false);
            return Ok(());
        }
        let rect = NSRect::new(NSPoint::new(0., 0.), NSSize::new(960., 600.));
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(self.mtm()),
                rect,
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::Resizable
                    | NSWindowStyleMask::Miniaturizable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe {
            window.setReleasedWhenClosed(false);
            window.setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        window.setTitle(ns_string!("Wallpaper — Editor Preview"));
        let surface = self.make_editor_surface(&window, None)?;
        let view = surface.view();
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        window.setContentView(Some(&view));
        surface.set_paused(false);
        window.center();
        window.makeKeyAndOrderFront(None);
        self.ivars().borrow_mut().editor_preview = Some(EditorPreview { window, surface });
        Ok(())
    }
    pub(super) fn preview_project(&self) -> Result<wpe_core::Project, String> {
        let (root, mut metadata, scene, mut resources, assets) = {
            let state = self.ivars().borrow();
            let editor = state.editor.as_ref().ok_or("Editor closed")?;
            let snapshot = editor.snapshot();
            let metadata = snapshot
                .session
                .get("temporaryProject")
                .filter(|v| v.is_object())
                .cloned()
                .unwrap_or(snapshot.project);
            (
                editor.root().to_owned(),
                metadata,
                state
                    .editor_preview_scene
                    .clone()
                    .or_else(|| editor.model_preview_scene().map(frame_model_scene))
                    .unwrap_or_else(|| editor.preview_scene()),
                editor.preview_resources().map_err(|e| e.to_string())?,
                state.service.config.assets.clone(),
            )
        };
        {
            let state = self.ivars().borrow();
            for (key, value) in &state.editor_test_properties {
                if let Some(property) = metadata
                    .pointer_mut("/general/properties")
                    .and_then(Value::as_object_mut)
                    .and_then(|properties| properties.get_mut(key))
                {
                    property["value"] = value.clone();
                }
            }
            for (name, bytes) in &state.editor_test_resources {
                resources.insert(name.clone(), Some(bytes.clone()));
            }
        }
        let mut project = load_project(&root, assets.as_deref()).map_err(|e| e.to_string())?;
        if metadata["type"]
            .as_str()
            .is_some_and(|kind| !kind.eq_ignore_ascii_case("scene"))
        {
            return Err("Native document preview requires a scene project".into());
        }
        let file = metadata["file"].as_str().unwrap_or("scene.json").to_owned();
        resources.insert(
            file.clone(),
            Some(std::sync::Arc::new(
                serde_json::to_vec(&scene).map_err(|e| e.to_string())?,
            )),
        );
        project
            .vfs
            .mount_memory_front(resources)
            .map_err(|e| e.to_string())?;
        project
            .override_scene_entry(&file)
            .map_err(|e| e.to_string())?;
        project.metadata = metadata;
        Ok(project)
    }
    fn make_editor_surface(
        &self,
        window: &NSWindow,
        storage: Option<BTreeMap<String, Value>>,
    ) -> Result<WallpaperSurface, String> {
        let project = self.preview_project()?;
        let (fps, capture_audio, volume) = {
            let state = self.ivars().borrow();
            (
                state.service.config.fps,
                state.service.config.audio.enabled,
                state.service.config.playback_volume,
            )
        };
        let rect = window
            .contentView()
            .map(|v| v.bounds())
            .unwrap_or(NSRect::new(NSPoint::new(0., 0.), NSSize::new(960., 600.)));
        let (_, time_of_day) = facts::calendar();
        let initial_input = PlayerInput {
            screen_resolution: [
                rect.size.width as f32 * window.backingScaleFactor() as f32,
                rect.size.height as f32 * window.backingScaleFactor() as f32,
            ],
            pointer_active: false,
            buttons: NSEvent::pressedMouseButtons() as u32,
            time_of_day,
            general_settings: json!({"fps":fps}),
            ..Default::default()
        };
        let surface = WallpaperSurface::new(
            project,
            rect,
            SurfaceOptions {
                screen_key: "editor-preview".into(),
                is_editor: true,
                is_wallpaper: false,
                paused: true,
                fps,
                capture_audio,
                volume,
                storage: storage.unwrap_or_default(),
                initial_input,
                ..Default::default()
            },
            self.mtm(),
        )?;
        {
            let state = self.ivars().borrow();
            surface.set_preview_camera(state.editor_zoom, state.editor_pan)?;
            surface.set_preview_view(state.editor_view.clone())?;
        }
        surface.tick()?;
        Ok(surface)
    }
    pub(super) fn refresh_editor_preview(&self) -> Result<(), String> {
        let existing = {
            let state = self.ivars().borrow();
            state.editor_preview.as_ref().map(|preview| {
                (
                    preview.window.clone(),
                    preview.surface.storage(),
                    preview.surface.is_paused(),
                )
            })
        };
        if let Some((window, storage, paused)) = existing {
            let surface = self.make_editor_surface(&window, Some(storage))?;
            window.setContentView(Some(&surface.view()));
            surface.set_paused(paused);
            self.ivars().borrow_mut().editor_preview = Some(EditorPreview { window, surface });
        }
        Ok(())
    }
    fn editor_host_request(&self, method: &str, args: &Value) -> Result<Value, String> {
        let argv = args.as_array().cloned().unwrap_or_default();
        match method {
            "openProject" => {
                let path = if let Some(path) = argv.first().and_then(Value::as_str) {
                    let path = PathBuf::from(path);
                    if path.is_absolute() {
                        path
                    } else {
                        self.ivars()
                            .borrow()
                            .editor
                            .as_ref()
                            .ok_or("Editor closed")?
                            .root()
                            .join(path)
                    }
                } else {
                    match self.choose_path(false) {
                        Some(path) => path,
                        None => return Ok(json!({"cancelled":true})),
                    }
                };
                let (assets, workspace) = {
                    let state = self.ivars().borrow();
                    (
                        state.service.config.assets.clone(),
                        state
                            .service
                            .config_path()
                            .parent()
                            .ok_or("Missing app state directory")?
                            .join("EditorProjects"),
                    )
                };
                let (editor, copy) = open_editor(&path, assets.as_deref(), &workspace)?;
                let result = json!({"workspace":copy,"snapshot":editor.snapshot()});
                self.ivars().borrow_mut().editor_replacement = Some(editor);
                Ok(result)
            }
            "saveAs" => {
                let panel = NSSavePanel::savePanel(self.mtm());
                panel.setNameFieldStringValue(&NSString::from_str(
                    argv.first()
                        .and_then(Value::as_str)
                        .unwrap_or("Wallpaper Project"),
                ));
                panel.setCanCreateDirectories(true);
                if panel.runModal() != NSModalResponseOK {
                    return Ok(json!({"cancelled":true}));
                }
                let url = panel.URL().ok_or("No output directory")?;
                let path = PathBuf::from(
                    url.path()
                        .ok_or("Save location is not a file URL")?
                        .to_string(),
                );
                let (root, project, scene, assets) = {
                    let mut state = self.ivars().borrow_mut();
                    let editor = state.editor.as_mut().ok_or("Editor closed")?;
                    editor.save().map_err(|e| e.to_string())?;
                    let snapshot = editor.snapshot();
                    (
                        editor.root().to_owned(),
                        snapshot.project,
                        snapshot.scene,
                        state.service.config.assets.clone(),
                    )
                };
                let output =
                    wpe_platform::library::export_copy(&root, &path).map_err(|e| e.to_string())?;
                let mut editor = wpe_editor::Editor::new(output.clone(), project, scene)
                    .map_err(|e| e.to_string())?;
                if let Some(assets) = assets {
                    editor.mount_asset_root(assets).map_err(|e| e.to_string())?;
                }
                let result = json!({"root":output,"snapshot":editor.snapshot()});
                self.ivars().borrow_mut().editor_replacement = Some(editor);
                Ok(result)
            }
            "deleteProject" => {
                let (root, owned) = {
                    let state = self.ivars().borrow();
                    (
                        state
                            .editor
                            .as_ref()
                            .ok_or("Editor closed")?
                            .root()
                            .canonicalize()
                            .map_err(|e| e.to_string())?,
                        state
                            .service
                            .config_path()
                            .parent()
                            .ok_or("Missing app state directory")?
                            .join("EditorProjects")
                            .canonicalize()
                            .map_err(|e| e.to_string())?,
                    )
                };
                if !root.starts_with(&owned) || !root.join(".wallpaper-mac-workspace").is_file() {
                    return Err("Delete Project only removes app-owned working copies; reveal external projects in Finder instead".into());
                }
                let alert = NSAlert::new(self.mtm());
                alert.setMessageText(ns_string!("Move this editor working copy to Trash?"));
                alert.setInformativeText(ns_string!("The imported source project is preserved."));
                alert.addButtonWithTitle(ns_string!("Move to Trash"));
                alert.addButtonWithTitle(ns_string!("Cancel"));
                if alert.runModal() != NSAlertFirstButtonReturn {
                    return Ok(json!({"cancelled":true}));
                }
                let url = NSURL::fileURLWithPath_isDirectory(
                    &NSString::from_str(&root.to_string_lossy()),
                    true,
                );
                let mut trashed = None;
                objc2_foundation::NSFileManager::defaultManager()
                    .trashItemAtURL_resultingItemURL_error(&url, Some(&mut trashed))
                    .map_err(|e| e.localizedDescription().to_string())?;
                self.ivars().borrow_mut().editor_close = true;
                Ok(
                    json!({"trashed":root,"trash":trashed.and_then(|u|u.path()).map(|s|s.to_string())}),
                )
            }
            "confirmUnsavedChanges" => {
                let alert = NSAlert::new(self.mtm());
                alert.setMessageText(ns_string!("Save editor changes?"));
                alert.addButtonWithTitle(ns_string!("Save"));
                alert.addButtonWithTitle(ns_string!("Discard"));
                alert.addButtonWithTitle(ns_string!("Cancel"));
                let response = alert.runModal();
                let action = if response == NSAlertFirstButtonReturn {
                    "save"
                } else if response == NSAlertSecondButtonReturn {
                    "discard"
                } else {
                    "cancel"
                };
                let mut state = self.ivars().borrow_mut();
                let editor = state.editor.as_mut().ok_or("Editor closed")?;
                Ok(editor
                    .execute("confirmUnsavedChanges", json!([action]))
                    .map_err(|e| e.to_string())?
                    .value)
            }
            "isDLCInstalled" | "getDLCDownloadProgress" | "installDLC" | "uninstallDLC" => {
                let id = argv.first().ok_or("A DLC AppID is required")?;
                let method = match method {
                    "installDLC" => "steam.dlc.install",
                    "uninstallDLC" => "steam.dlc.uninstall",
                    _ => "steam.dlc.status",
                };
                let value = self.steam_command(method, json!({"id":id}))?;
                let info = &value["status"];
                let (done, total) = info["progress"]
                    .as_array()
                    .and_then(|a| Some((a.first()?.as_u64()?, a.get(1)?.as_u64()?)))
                    .unwrap_or((0, 0));
                let installed = info["installed"].as_bool().unwrap_or(false);
                Ok(
                    json!({"installed":installed,"downloading":total>done,"offline":false,"success":true,"downloaded":done,"total":total,"finished":installed&&done>=total,"requested":value["requested"]}),
                )
            }
            "choosePublishMethod" => {
                Ok(json!({"methods":["steamworks"],"status":self.steam_status()}))
            }
            "restartPreview" => {
                self.ivars().borrow_mut().editor_preview.take();
                self.show_editor_preview(true)?;
                Ok(json!({"preview":true}))
            }
            "previewToggleVisibility" => {
                let visible = argv.first().and_then(Value::as_bool).unwrap_or(true);
                self.show_editor_preview(visible)?;
                Ok(json!({"visible":visible}))
            }
            "previewSizeChanged" => {
                self.show_editor_preview(true)?;
                let values = argv.first().filter(|v| v.is_object());
                let width = values
                    .and_then(|v| v["width"].as_f64())
                    .or_else(|| argv.get(2).and_then(Value::as_f64));
                let height = values
                    .and_then(|v| v["height"].as_f64())
                    .or_else(|| argv.get(3).and_then(Value::as_f64));
                if let (Some(w), Some(h)) = (width, height) {
                    if !(1. ..=8192.).contains(&w) || !(1. ..=8192.).contains(&h) {
                        return Err("Preview dimensions must be finite and1..8192".into());
                    }
                    let state = self.ivars().borrow();
                    let preview = state.editor_preview.as_ref().unwrap();
                    preview.window.setContentSize(NSSize::new(w, h));
                    preview
                        .surface
                        .resize(NSRect::new(NSPoint::new(0., 0.), NSSize::new(w, h)));
                }
                Ok(json!({"native_view":true,"shared_dxgi_handle":false}))
            }
            "canRunScriptPreview" => Ok(json!(true)),
            "runScriptPreview" => {
                self.show_editor_preview(true)?;
                self.refresh_editor_preview()?;
                Ok(json!({"preview":true,"vm":"wpe-script"}))
            }
            "importFile"
            | "importFontFile"
            | "replaceFile"
            | "replaceTexture"
            | "importTextureFile"
            | "importModelAddToScene"
            | "importImage"
            | "importSound"
            | "importParticleSystem"
            | "importModel"
            | "selectModelTexture"
            | "selectExternalApp" => {
                if !argv.is_empty() && matches!(method, "importModel" | "importModelAddToScene") {
                    return Err("This model format is unsupported; the native editor imports OBJ, MDL and bounded FBX directly".into());
                }
                let panel = NSOpenPanel::openPanel(self.mtm());
                panel.setCanChooseFiles(true);
                panel.setCanChooseDirectories(method == "selectExternalApp");
                panel.setAllowsMultipleSelection(false);
                if panel.runModal() != NSModalResponseOK {
                    return Ok(json!({"cancelled":true}));
                }
                let url = panel.URL().ok_or("No selected URL")?;
                let path =
                    PathBuf::from(url.path().ok_or("Selected URL is not a file")?.to_string());
                if matches!(method, "selectExternalApp" | "performUserShortcutTest") {
                    return Ok(json!(path));
                }
                if matches!(method, "importModel" | "importModelAddToScene")
                    && !path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
                        s.eq_ignore_ascii_case("obj")
                            || s.eq_ignore_ascii_case("mdl")
                            || s.eq_ignore_ascii_case("fbx")
                    })
                {
                    return Err(
                        "Selected model format is unsupported; choose OBJ, MDL or FBX".into(),
                    );
                }
                let basename = path
                    .file_name()
                    .ok_or("Selected file has no name")?
                    .to_string_lossy();
                let destination = format!("imports/{basename}");
                let mut state = self.ivars().borrow_mut();
                let editor = state.editor.as_mut().ok_or("Editor closed")?;
                let staged = editor
                    .import_external_file(&path, &destination)
                    .map_err(|e| e.to_string())?;
                let result = if matches!(
                    method,
                    "importFile"
                        | "importFontFile"
                        | "replaceFile"
                        | "replaceTexture"
                        | "importTextureFile"
                        | "importModelAddToScene"
                        | "importImage"
                        | "importSound"
                        | "importParticleSystem"
                        | "importModel"
                ) {
                    Some(
                        editor
                            .execute(method, json!([destination]))
                            .map_err(|e| e.to_string())?,
                    )
                } else {
                    None
                };
                Ok(
                    json!({"staged":staged,"import":result,"path":destination,"file":destination,"name":basename}),
                )
            }
            "openInExplorer" | "navigateToFile" => {
                let root = self
                    .ivars()
                    .borrow()
                    .editor
                    .as_ref()
                    .ok_or("Editor closed")?
                    .root()
                    .to_owned();
                let candidate = argv
                    .first()
                    .and_then(Value::as_str)
                    .map(|s| root.join(s))
                    .unwrap_or_else(|| root.clone());
                let path = candidate.canonicalize().map_err(|e| e.to_string())?;
                if !path.starts_with(root.canonicalize().map_err(|e| e.to_string())?) {
                    return Err("Editor reveal path escapes working copy".into());
                }
                let url = NSURL::fileURLWithPath_isDirectory(
                    &NSString::from_str(&path.to_string_lossy()),
                    path.is_dir(),
                );
                NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(
                    &objc2_foundation::NSArray::from_slice(&[&*url]),
                );
                Ok(json!(true))
            }
            "applyWallpaper" => {
                let root = {
                    let mut state = self.ivars().borrow_mut();
                    let editor = state.editor.as_mut().ok_or("Editor closed")?;
                    editor.save().map_err(|e| e.to_string())?;
                    editor.root().to_owned()
                };
                let screen = self.default_screen();
                self.assign_path(&screen, &root)?;
                Ok(json!({"screen":screen,"project":root}))
            }
            "confirmClose" => {
                let dirty = self
                    .ivars()
                    .borrow()
                    .editor
                    .as_ref()
                    .is_some_and(|e| e.snapshot().dirty);
                if dirty {
                    return Err(
                        "Save or undo the document changes before closing the editor".into(),
                    );
                }
                self.ivars().borrow_mut().editor_preview.take();
                Ok(json!(true))
            }
            "queryWorkshop" => self.steam_command(
                "steam.query",
                argv.first()
                    .cloned()
                    .filter(Value::is_object)
                    .unwrap_or(json!({})),
            ),
            "preparePublishAssets" => {
                let mut state = self.ivars().borrow_mut();
                let editor = state.editor.as_mut().ok_or("Editor closed")?;
                editor.save().map_err(|e| e.to_string())?;
                Ok(json!({"content":editor.root(),"saved":true,"published":false}))
            }
            "checkExternalApp" => {
                let path = argv
                    .first()
                    .and_then(Value::as_str)
                    .ok_or("An explicit application path is required")?;
                if !(Path::new(path).is_file()
                    || (Path::new(path).is_dir()
                        && Path::new(path)
                            .extension()
                            .is_some_and(|s| s.eq_ignore_ascii_case("app"))))
                {
                    return Err("External executable or .app does not exist".into());
                }
                Ok(json!(path))
            }
            "validateVideoFile" => {
                let path = argv
                    .first()
                    .and_then(Value::as_str)
                    .ok_or("A relative video path is required")?;
                let root = self
                    .ivars()
                    .borrow()
                    .editor
                    .as_ref()
                    .ok_or("Editor closed")?
                    .root()
                    .to_owned();
                let source = root.join(path).canonicalize().map_err(|e| e.to_string())?;
                if !source.starts_with(root.canonicalize().map_err(|e| e.to_string())?) {
                    return Err("Video path escapes working copy".into());
                }
                let view = VideoView::new(
                    &source,
                    NSRect::new(NSPoint::new(0., 0.), NSSize::new(1., 1.)),
                    false,
                    self.mtm(),
                )?;
                view.set_paused(true);
                if let Some(error) = view.failure() {
                    return Err(error);
                }
                Ok(
                    json!({"asset_created":true,"decoder_status":"asynchronous; consult preview diagnostics","playback_verified":false}),
                )
            }
            _ => self.additional_editor_host(method, &argv),
        }
    }
}

pub(super) fn open_editor(
    path: &Path,
    assets: Option<&Path>,
    workspace: &Path,
) -> Result<(wpe_editor::Editor, PathBuf), String> {
    let source = load_project(path, assets).map_err(|e| e.to_string())?;
    if source.kind != WallpaperKind::Scene {
        return Err("The document editor opens scene projects".into());
    }
    let copy =
        wpe_platform::library::working_copy(&source.root, workspace).map_err(|e| e.to_string())?;
    let mut metadata = source.metadata.clone();
    if source
        .entry
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pkg"))
        && !copy.join(".wallpaper-mac-unpacked").exists()
    {
        let mut count = 0usize;
        let mut bytes = 0usize;
        for name in source.vfs.mounted_package_names() {
            let resource = source.read_resource(&name).map_err(|e| e.to_string())?;
            if matches!(
                resource.origin,
                wpe_core::vfs::ResourceOrigin::Package { .. }
            ) {
                let relative = Path::new(&name);
                if relative.is_absolute()
                    || relative.components().any(|c| {
                        !matches!(
                            c,
                            std::path::Component::Normal(_) | std::path::Component::CurDir
                        )
                    })
                {
                    return Err("Packed editor member has an unsafe path".into());
                }
                count += 1;
                bytes = bytes
                    .checked_add(resource.bytes.len())
                    .ok_or("Unpack size overflow")?;
                if count > 100000 || bytes > 2 * 1024 * 1024 * 1024 {
                    return Err("Editor unpack exceeds100000 files or2GiB".into());
                }
                let destination = copy.join(relative);
                if let Some(parent) = destination.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::write(destination, resource.bytes).map_err(|e| e.to_string())?;
            }
        }
        metadata["file"] = json!("scene.json");
        metadata["type"] = json!("scene");
        std::fs::write(
            copy.join("project.json"),
            serde_json::to_vec_pretty(&metadata).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        std::fs::write(copy.join(".wallpaper-mac-unpacked"), b"1").map_err(|e| e.to_string())?;
    }
    if !copy.join("project.json").exists() {
        metadata["type"] = json!("scene");
        std::fs::write(
            copy.join("project.json"),
            serde_json::to_vec_pretty(&metadata).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    let project = load_project(&copy, assets).map_err(|e| e.to_string())?;
    if project.kind != WallpaperKind::Scene {
        return Err("The document editor opens scene projects".into());
    }
    let file = project.metadata["file"]
        .as_str()
        .filter(|s| !s.ends_with(".pkg"))
        .unwrap_or("scene.json");
    let resource = project.read_resource(file).map_err(|e| e.to_string())?;
    let scene = wpe_formats::jsonc::parse(&resource.bytes).map_err(|e| e.to_string())?;
    let mut editor = wpe_editor::Editor::new(project.root, project.metadata, scene)
        .map_err(|e| e.to_string())?;
    for directory in source.vfs.directories().iter().rev() {
        if directory != &source.root && directory != &copy {
            editor
                .mount_asset_root(directory.clone())
                .map_err(|e| e.to_string())?;
        }
    }
    Ok((editor, copy))
}

fn frame_model_scene(mut scene: Value) -> Value {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    if let Some(shapes) = scene
        .pointer("/objects/0/modeldata/shapes")
        .and_then(Value::as_array)
    {
        for shape in shapes {
            let mut stride = 0;
            let mut position = None;
            if let Some(format) = shape["vertexFormat"].as_array() {
                for field in format {
                    let size = match field.as_str() {
                        Some("position" | "normal") => 3,
                        Some("uv") => 2,
                        Some("color" | "tangentSigned") => 4,
                        _ => 0,
                    };
                    if field == "position" {
                        position = Some(stride);
                    }
                    stride += size;
                }
            }
            if stride == 0 {
                continue;
            }
            if let (Some(offset), Some(vertices)) = (position, shape["vertexBuffer"].as_array()) {
                for vertex in vertices.chunks_exact(stride) {
                    for axis in 0..3 {
                        if let Some(value) =
                            vertex[offset + axis].as_f64().filter(|v| v.is_finite())
                        {
                            min[axis] = min[axis].min(value);
                            max[axis] = max[axis].max(value);
                        }
                    }
                }
            }
        }
    }
    if min.iter().chain(&max).all(|n| n.is_finite()) {
        let center = [
            (min[0] + max[0]) * 0.5,
            (min[1] + max[1]) * 0.5,
            (min[2] + max[2]) * 0.5,
        ];
        let extent = (0..3).map(|i| max[i] - min[i]).fold(1., f64::max);
        scene["general"] = json!({"clearcolor":"0.08 0.09 0.12","zoom":1.});
        scene["camera"] = json!({"eye":[center[0],center[1],center[2]+extent*2.],"center":center,"up":[0,1,0],"fov":50,"nearz":(extent*0.001).max(0.00001),"farz":extent*20.});
    }
    scene
}

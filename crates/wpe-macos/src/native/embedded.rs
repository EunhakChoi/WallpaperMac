//! Reusable native view host for previews and ScreenSaver.framework.
use super::*;
use objc2_app_kit::NSAutoresizingMaskOptions;
use wpe_core::{Project, ScenePlayerOptions};
#[derive(Clone, Debug)]
pub struct SurfaceOptions {
    pub screen_key: String,
    pub is_editor: bool,
    pub is_wallpaper: bool,
    pub is_desktop: bool,
    pub paused: bool,
    pub muted: bool,
    pub volume: f32,
    pub fps: u32,
    pub capture_audio: bool,
    pub storage: BTreeMap<String, Value>,
    pub initial_input: PlayerInput,
    pub application_runtime: Option<wpe_platform::application::ApplicationRuntime>,
}
impl Default for SurfaceOptions {
    fn default() -> Self {
        Self {
            screen_key: "embedded".into(),
            is_editor: false,
            is_wallpaper: true,
            is_desktop: false,
            paused: false,
            muted: false,
            volume: 1.,
            fps: 30,
            capture_audio: false,
            storage: BTreeMap::new(),
            initial_input: PlayerInput::default(),
            application_runtime: None,
        }
    }
}
struct SurfaceState {
    content: Option<Content>,
    options: SurfaceOptions,
    storage: BTreeMap<String, Value>,
    capture: Option<AudioCapture>,
    audio: AudioFrameProcessor,
    bands: AudioBands,
    last: Instant,
    errors: VecDeque<String>,
    dirty: bool,
    pointer: Option<([f32; 2], u32, bool)>,
}
define_class!(
 #[unsafe(super=NSObject)]#[thread_kind=MainThreadOnly]#[ivars=RefCell<SurfaceState>]
 struct SurfaceDelegate;
 unsafe impl NSObjectProtocol for SurfaceDelegate{}
 unsafe impl WKNavigationDelegate for SurfaceDelegate{
  #[unsafe(method(webView:didFinishNavigation:))]fn finished(&self,_view:&WKWebView,_navigation:Option<&WKNavigation>){let state=self.ivars().borrow();if let Some(Content::Web(web))=&state.content{web.refresh_initial();web.set_paused(state.options.paused);web.set_muted(state.options.muted);}}
  #[unsafe(method(webView:didFailNavigation:withError:))]fn failed(&self,_view:&WKWebView,_navigation:Option<&WKNavigation>,error:&NSError){self.error(error.localizedDescription().to_string());}
 }
 unsafe impl WKScriptMessageHandler for SurfaceDelegate{
  #[unsafe(method(userContentController:didReceiveScriptMessage:))]fn message(&self,controller:&WKUserContentController,message:&WKScriptMessage){let current=matches!(&self.ivars().borrow().content,Some(Content::Web(web))if web.owns_controller(controller));if !current{return;}let body=unsafe{message.body()};let text:Retained<NSString>=unsafe{msg_send![&*body,description]};let text=text.to_string();if text.len()<=16384{self.error(text);}}
 }
);
impl SurfaceDelegate {
    fn error(&self, error: String) {
        let mut state = self.ivars().borrow_mut();
        if state.errors.len() >= 64 {
            state.errors.pop_front();
        }
        if state.errors.back() != Some(&error) {
            state.errors.push_back(error);
        }
    }
}
pub struct WallpaperSurface {
    delegate: Retained<SurfaceDelegate>,
}
impl WallpaperSurface {
    pub fn new(
        project: Project,
        frame: NSRect,
        options: SurfaceOptions,
        mtm: MainThreadMarker,
    ) -> Result<Self, String> {
        if options.fps == 0 || options.fps > 240 {
            return Err("Surface frame rate must be1..240".into());
        }
        let storage = options.storage.clone();
        let paused = options.paused;
        let muted = options.muted;
        let volume = options.volume;
        if !volume.is_finite() || !(0. ..=1.).contains(&volume) {
            return Err("Surface playback volume must be0..1".into());
        }
        let delegate = SurfaceDelegate::alloc(mtm).set_ivars(RefCell::new(SurfaceState {
            content: None,
            options,
            storage,
            capture: None,
            audio: AudioFrameProcessor::default(),
            bands: AudioBands::default(),
            last: Instant::now(),
            errors: VecDeque::new(),
            dirty: false,
            pointer: None,
        }));
        let delegate: Retained<SurfaceDelegate> = unsafe { msg_send![super(delegate), init] };
        let mut state = delegate.ivars().borrow_mut();
        let content = match project.kind.clone() {
            WallpaperKind::Scene => {
                let options = ScenePlayerOptions {
                    is_editor: state.options.is_editor,
                    is_wallpaper: state.options.is_wallpaper,
                    is_desktop: state.options.is_desktop,
                };
                let screen = state.options.screen_key.clone();
                let initial = state.options.initial_input.clone();
                let (scene, issues) = LiveScene::new(
                    project,
                    &screen,
                    &mut state.storage,
                    frame,
                    mtm,
                    options,
                    initial,
                )?;
                for issue in issues {
                    state.errors.push_back(issue);
                }
                Content::Scene(Box::new(scene))
            }
            WallpaperKind::Video => {
                Content::Video(VideoView::new(&project.entry, frame, true, mtm)?)
            }
            WallpaperKind::Web => Content::Web(WebView::new(
                &project.entry,
                &project.root,
                &project.metadata,
                frame,
                ProtocolObject::from_ref(&*delegate),
                ProtocolObject::from_ref(&*delegate),
                mtm,
                paused,
                muted,
                volume,
            )?),
            WallpaperKind::Application => {
                let runtime = state.options.application_runtime.clone().ok_or(
                    "An application surface requires an explicitly configured external runtime",
                )?;
                Content::Application(Box::new(application::ApplicationView::new(
                    project, runtime, frame, mtm,
                )?))
            }
            WallpaperKind::Unknown(kind) => return Err(format!("Unknown wallpaper kind{kind}")),
        };
        content.view().setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content.set_volume(volume);
        content.set_muted(muted);
        content.set_paused(paused);
        state.content = Some(content);
        if state.options.capture_audio && !paused {
            state.capture = Some(AudioCapture::start(wpe_platform::config::AudioSettings {
                enabled: true,
                ..Default::default()
            })?);
        }
        drop(state);
        Ok(Self { delegate })
    }
    pub fn view(&self) -> Retained<NSView> {
        match &self.delegate.ivars().borrow().content {
            Some(content) => {
                unsafe { Retained::retain(content.view() as *const NSView as *mut NSView) }
                    .expect("live content view")
            }
            None => unreachable!("surface content exists until destruction"),
        }
    }
    pub fn resize(&self, frame: NSRect) {
        let view = self.view();
        if view.frame() != frame {
            view.setFrame(frame);
            self.delegate.ivars().borrow_mut().dirty = true;
        }
    }
    pub fn set_pointer(&self, cursor: [f32; 2], buttons: u32, active: bool) -> Result<(), String> {
        if cursor.iter().any(|v| !v.is_finite()) {
            return Err("Pointer coordinates must be finite".into());
        }
        self.delegate.ivars().borrow_mut().pointer = Some((cursor, buttons, active));
        Ok(())
    }
    pub fn set_preview_camera(&self, zoom: f32, pan: [f32; 2]) -> Result<(), String> {
        let mut state = self.delegate.ivars().borrow_mut();
        let Some(Content::Scene(scene)) = &state.content else {
            return Err("Preview camera requires a scene".into());
        };
        scene
            .player
            .borrow_mut()
            .set_preview_camera(zoom, pan)
            .map_err(|e| e.to_string())?;
        state.dirty = true;
        Ok(())
    }
    pub fn set_preview_view(&self, camera: Option<Value>) -> Result<(), String> {
        let mut state = self.delegate.ivars().borrow_mut();
        let Some(Content::Scene(scene)) = &state.content else {
            return Err("Preview view requires a scene".into());
        };
        scene
            .player
            .borrow_mut()
            .set_preview_view(camera)
            .map_err(|e| e.to_string())?;
        state.dirty = true;
        Ok(())
    }

    pub fn canvas_size(&self) -> Option<[f32; 2]> {
        let state = self.delegate.ivars().borrow();
        match &state.content {
            Some(Content::Scene(scene)) => Some(scene.renderer.canvas_size()),
            _ => None,
        }
    }
    pub fn is_paused(&self) -> bool {
        self.delegate.ivars().borrow().options.paused
    }
    pub fn set_paused(&self, paused: bool) {
        let mut state = self.delegate.ivars().borrow_mut();
        state.options.paused = paused;
        state.last = Instant::now();
        if let Some(content) = &state.content {
            content.set_paused(paused);
        }
        if paused {
            state.capture.take();
        } else if state.options.capture_audio && state.capture.is_none() {
            match AudioCapture::start(wpe_platform::config::AudioSettings {
                enabled: true,
                ..Default::default()
            }) {
                Ok(c) => state.capture = Some(c),
                Err(e) => state.errors.push_back(e),
            }
        }
    }
    pub fn set_volume(&self, volume: f32) {
        let mut state = self.delegate.ivars().borrow_mut();
        state.options.volume = volume.clamp(0., 1.);
        if let Some(content) = &state.content {
            content.set_volume(state.options.volume);
        }
    }
    pub fn set_muted(&self, muted: bool) {
        let mut state = self.delegate.ivars().borrow_mut();
        state.options.muted = muted;
        if let Some(content) = &state.content {
            content.set_volume(state.options.volume);
            content.set_muted(muted);
        }
    }
    pub fn clear_storage(&self) -> Result<(), String> {
        let mut state = self.delegate.ivars().borrow_mut();
        let prefix = match &state.content {
            Some(Content::Scene(scene)) => Some(format!(
                "{}::",
                scene.player.borrow().project().root.display()
            )),
            _ => None,
        };
        if let Some(prefix) = prefix {
            state.storage.retain(|key, _| !key.starts_with(&prefix));
        }
        if let Some(Content::Scene(scene)) = &state.content {
            scene
                .player
                .borrow_mut()
                .set_storage(json!({"global":{},"screen":{}}))
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub fn tick(&self) -> Result<(), String> {
        let mut state = self.delegate.ivars().borrow_mut();
        let now = Instant::now();
        let dt = now.duration_since(state.last).as_secs_f32().min(0.25);
        state.last = now;
        if state.options.paused && !state.dirty {
            return Ok(());
        }
        let paused = state.options.paused;
        state.dirty = false;
        let raw = state
            .capture
            .as_ref()
            .map(AudioCapture::latest)
            .unwrap_or_default();
        if !paused {
            state.bands = state.audio.process(&raw, dt);
        }
        let bands = state.bands.clone();
        let fps = state.options.fps;
        let (_, time_of_day) = facts::calendar();
        let mut storage = std::mem::take(&mut state.storage);
        let pointer_override = state.pointer.take();
        let result = match &state.content {
            Some(Content::Scene(scene)) => {
                let view = &scene.renderer.view;
                let bounds = view.bounds();

                let (cursor, buttons, pointer_active) = pointer_override
                    .unwrap_or_else(|| facts::pointer_for_view(view, state.options.is_desktop));
                let (cursor, inside_canvas) = crate::render_graph::fit_pointer(
                    cursor,
                    [bounds.size.width as f32, bounds.size.height as f32],
                    scene.renderer.canvas_size(),
                );
                let pointer_active = pointer_active && inside_canvas;
                let input = PlayerInput {
                    cursor,
                    buttons,
                    pointer_active,
                    screen_resolution: [
                        bounds.size.width as f32
                            * view
                                .window()
                                .map(|w| w.backingScaleFactor() as f32)
                                .unwrap_or(1.),
                        bounds.size.height as f32
                            * view
                                .window()
                                .map(|w| w.backingScaleFactor() as f32)
                                .unwrap_or(1.),
                    ],
                    audio_left: bands.bands64[0].to_vec(),
                    audio_right: bands.bands64[1].to_vec(),
                    time_of_day,
                    general_settings: json!({"fps":fps}),
                    ..Default::default()
                };
                if paused {
                    scene.render_current(&input)
                } else {
                    scene.advance(dt, &input, &mut storage).map(|issue| {
                        if let Some(issue) = issue {
                            eprintln!("Embedded wallpaper: {issue}");
                        }
                    })
                }
            }
            Some(Content::Web(web)) => {
                web.general(fps);
                web.audio(&bands.web_stereo());
                web.refresh_initial();
                Ok(())
            }
            Some(Content::Application(app)) => app.tick(),
            Some(Content::Video(video)) => video.failure().map(Err).unwrap_or(Ok(())),
            None => Ok(()),
        };
        state.storage = storage;
        if let Err(error) = &result {
            if state.errors.len() >= 64 {
                state.errors.pop_front();
            }
            state.errors.push_back(error.clone());
        }
        result
    }
    pub fn storage(&self) -> BTreeMap<String, Value> {
        self.delegate.ivars().borrow().storage.clone()
    }
    pub fn diagnostics(&self) -> Vec<String> {
        self.delegate
            .ivars()
            .borrow()
            .errors
            .iter()
            .cloned()
            .collect()
    }
    pub fn replace_scene(&self, scene: Value) -> Result<(), String> {
        let state = self.delegate.ivars().borrow();
        let Some(Content::Scene(content)) = &state.content else {
            return Err("This surface is not a scene".into());
        };
        let mut player = content.player.borrow_mut();
        player.replace_scene(scene).map_err(|e| e.to_string())?;
        player.set_native_text_provider(true);
        *content.text.borrow_mut() = Default::default();
        *content.videos.borrow_mut() = Default::default();
        content.videos.borrow().set_paused(state.options.paused);
        *content.sounds.borrow_mut() = sound::SoundManager::for_editor(state.options.is_editor);
        content.sounds.borrow().set_paused(state.options.paused);
        content.sounds.borrow().set_muted(state.options.muted);
        content.sounds.borrow().set_volume(state.options.volume);
        Ok(())
    }
}
impl Drop for WallpaperSurface {
    fn drop(&mut self) {
        let mut state = self.delegate.ivars().borrow_mut();
        state.capture.take();
        if let Some(content) = state.content.take() {
            content.view().removeFromSuperview();
            content.set_paused(true);
            drop(content);
        }
    }
}

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, Bool};
use objc2::{
    ClassType, DefinedClass, MainThreadOnly, define_class, extern_class, extern_methods, msg_send,
    sel,
};
use objc2_app_kit::*;
use objc2_foundation::{
    NSData, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL,
    NSURLBookmarkCreationOptions, NSURLBookmarkResolutionOptions, ns_string,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};
use wpe_macos::{SurfaceOptions, WallpaperSurface};
#[link(name = "ScreenSaver", kind = "framework")]
unsafe extern "C" {}
extern_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind=MainThreadOnly]
    #[derive(Debug, PartialEq, Eq, Hash)]
    struct ScreenSaverView;
);
impl ScreenSaverView {
    extern_methods!(
        #[unsafe(method(isAnimating))]
        #[unsafe(method_family=none)]
        fn is_animating(&self) -> bool;
        #[unsafe(method(setAnimationTimeInterval:))]
        #[unsafe(method_family=none)]
        fn set_interval(&self, value: f64);
    );
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct Config {
    project: Option<PathBuf>,
    assets: Option<PathBuf>,
    project_bookmark: Option<Vec<u8>>,
    assets_bookmark: Option<Vec<u8>>,
    muted: bool,
    capture_audio: bool,
    fps: u32,
    storage: BTreeMap<String, Value>,
    application_runtime: Option<wpe_platform::application::ApplicationRuntime>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            project: None,
            assets: None,
            project_bookmark: None,
            assets_bookmark: None,
            muted: true,
            capture_audio: false,
            fps: 30,
            storage: BTreeMap::new(),
            application_runtime: None,
        }
    }
}
struct Scope {
    url: Retained<NSURL>,
    active: bool,
}
impl Drop for Scope {
    fn drop(&mut self) {
        if self.active {
            unsafe {
                self.url.stopAccessingSecurityScopedResource();
            }
        }
    }
}
#[derive(Default)]
struct State {
    surface: Option<WallpaperSurface>,
    config: Config,
    preview: bool,
    sheet: Option<Retained<NSPanel>>,
    mute: Option<Retained<NSButton>>,
    capture: Option<Retained<NSButton>>,
    label: Option<Retained<NSTextField>>,
    placeholder: Option<Retained<NSTextField>>,
    _scopes: Vec<Scope>,
    error: Option<String>,
}
define_class!(
 #[unsafe(super=ScreenSaverView)]#[name="WallpaperMacScreenSaver"]#[thread_kind=MainThreadOnly]#[ivars=RefCell<State>]
 struct WallpaperMacScreenSaver;
 unsafe impl NSObjectProtocol for WallpaperMacScreenSaver{}
 impl WallpaperMacScreenSaver{
  #[unsafe(method_id(initWithFrame:isPreview:))]
  fn init(this:Allocated<Self>,frame:NSRect,preview:bool)->Retained<Self>{let loaded=load_config();let config=loaded.as_ref().cloned().unwrap_or_default();let this=this.set_ivars(RefCell::new(State{preview,config,..Default::default()}));let this:Retained<Self>=unsafe{msg_send![super(this),initWithFrame:frame,isPreview:preview]};this.set_interval(1./30.);if let Err(error)=loaded{this.show_error(&error);}this}
  #[unsafe(method(startAnimation))]fn start(&self){unsafe{let _:()=msg_send![super(self),startAnimation];}
  if self.ivars().borrow().surface.is_none(){self.load_surface();}
  if let Some(surface)=&self.ivars().borrow().surface{surface.set_paused(false);}}
  #[unsafe(method(stopAnimation))]fn stop(&self){
    let result={let mut state=self.ivars().borrow_mut();let result=if let Some(surface)=state.surface.take(){surface.set_paused(true);let before=state.config.storage.clone();state.config.storage=surface.storage();drop(surface);save_storage_delta(&before,&state.config.storage)}else{Ok(())};state._scopes.clear();result};
    if let Err(error)=result{self.show_error(&error);}
    unsafe{let _:()=msg_send![super(self),stopAnimation];}
  }
  #[unsafe(method(animateOneFrame))]fn animate(&self){let error={let state=self.ivars().borrow();state.surface.as_ref().and_then(|surface|{surface.resize(self.bounds());surface.tick().err()})};if let Some(error)=error{self.show_error(&error);}}
  #[unsafe(method(hasConfigureSheet))]fn has_sheet(&self)->bool{true}
  #[unsafe(method_id(configureSheet))]fn configure_sheet(&self)->Retained<NSWindow>{self.sheet().into_super()}
  #[unsafe(method(chooseProject:))]fn choose_project(&self,_sender:Option<&AnyObject>){if let Some((path,bookmark))=self.choose(false){let mut state=self.ivars().borrow_mut();state.config.project=Some(path);state.config.project_bookmark=bookmark;state.config.application_runtime=None;drop(state);self.refresh_label();}}
  #[unsafe(method(chooseAssets:))]fn choose_assets(&self,_sender:Option<&AnyObject>){if let Some((path,bookmark))=self.choose(true){let mut state=self.ivars().borrow_mut();state.config.assets=Some(path);state.config.assets_bookmark=bookmark;drop(state);self.refresh_label();}}
  #[unsafe(method(saveConfiguration:))]fn save(&self,_sender:Option<&AnyObject>){let result={let mut state=self.ivars().borrow_mut();state.config.muted=state.mute.as_ref().is_none_or(|b|b.state()!=0);state.config.capture_audio=state.capture.as_ref().is_some_and(|b|b.state()!=0);save_config(&state.config)};if let Err(error)=result{self.show_error(&error);return;}self.load_surface();if let Some(sheet)=self.ivars().borrow().sheet.clone(){if let Some(parent)=sheet.sheetParent(){parent.endSheet(&sheet); }sheet.orderOut(None);}}
 }
);
impl WallpaperMacScreenSaver {
    fn load_surface(&self) {
        let config = self.ivars().borrow().config.clone();
        let result = (|| -> Result<(WallpaperSurface, Vec<Scope>), String> {
            let project = config
                .project
                .as_ref()
                .ok_or("Choose a scene, video or web project in Screen Saver Options.")?;
            let mut scopes = vec![];
            let project = resolve(project, config.project_bookmark.as_deref(), &mut scopes)?;
            let assets = config
                .assets
                .as_ref()
                .map(|p| resolve(p, config.assets_bookmark.as_deref(), &mut scopes))
                .transpose()?;
            let project =
                wpe_core::load_project(&project, assets.as_deref()).map_err(|e| e.to_string())?;
            let preview = self.ivars().borrow().preview;
            let options = SurfaceOptions {
                screen_key: if preview {
                    "screensaver-preview".into()
                } else {
                    format!(
                        "screensaver-{}",
                        self.window()
                            .and_then(|window| window.screen())
                            .map(|screen| wpe_macos::screen_identity(&screen))
                            .unwrap_or_else(|| "unattached".into())
                    )
                },
                paused: !self.is_animating(),
                muted: config.muted,
                capture_audio: config.capture_audio,
                fps: config.fps,
                storage: config.storage.clone(),
                initial_input: wpe_core::PlayerInput {
                    screen_resolution: [
                        self.bounds().size.width as f32
                            * self
                                .window()
                                .map(|w| w.backingScaleFactor() as f32)
                                .unwrap_or(1.),
                        self.bounds().size.height as f32
                            * self
                                .window()
                                .map(|w| w.backingScaleFactor() as f32)
                                .unwrap_or(1.),
                    ],
                    pointer_active: false,
                    time_of_day: wpe_macos::local_time_of_day(),
                    general_settings: serde_json::json!({"fps":config.fps}),
                    ..Default::default()
                },
                application_runtime: config.application_runtime.clone(),
                ..Default::default()
            };
            let surface = WallpaperSurface::new(project, self.bounds(), options, self.mtm())?;
            Ok((surface, scopes))
        })();
        match result {
            Ok((surface, scopes)) => {
                let view = surface.view();
                self.addSubview(&view);
                let mut state = self.ivars().borrow_mut();
                state.surface.take();
                state._scopes = scopes;
                state.surface = Some(surface);
                state.error = None;
                if let Some(placeholder) = state.placeholder.take() {
                    placeholder.removeFromSuperview();
                }
                self.set_interval(1. / config.fps.clamp(1, 240) as f64);
            }
            Err(error) => self.show_error(&error),
        }
    }
    fn show_error(&self, error: &str) {
        let mut state = self.ivars().borrow_mut();
        if state.error.as_deref() == Some(error) {
            return;
        }
        state.error = Some(error.into());
        eprintln!("Wallpaper Screen Saver: {error}");
        if let Some(label) = &state.placeholder {
            label.setStringValue(&NSString::from_str(error));
            return;
        }
        let label = NSTextField::labelWithString(&NSString::from_str(error), self.mtm());
        label.setFrame(NSRect::new(
            NSPoint::new(16., 16.),
            NSSize::new((self.bounds().size.width - 32.).max(40.), 80.),
        ));
        label.setTextColor(Some(&NSColor::whiteColor()));
        self.addSubview(&label);
        state.placeholder = Some(label);
    }
    fn sheet(&self) -> Retained<NSPanel> {
        if let Some(sheet) = &self.ivars().borrow().sheet {
            return sheet.clone();
        }
        let rect = NSRect::new(NSPoint::new(0., 0.), NSSize::new(620., 270.));
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(self.mtm()),
            rect,
            NSWindowStyleMask::Titled,
            NSBackingStoreType::Buffered,
            false,
        );
        unsafe {
            panel.setReleasedWhenClosed(false);
        }
        panel.setTitle(ns_string!("Wallpaper Screen Saver"));
        let content = panel.contentView().expect("panel content view");
        let label =
            NSTextField::labelWithString(ns_string!("Choose a wallpaper project"), self.mtm());
        label.setFrame(NSRect::new(NSPoint::new(20., 196.), NSSize::new(580., 48.)));
        content.addSubview(&label);
        for (title, action, x) in [
            ("Choose Project…", sel!(chooseProject:), 20.),
            ("Choose Assets…", sel!(chooseAssets:), 200.),
            ("Save", sel!(saveConfiguration:), 500.),
        ] {
            let button = NSButton::initWithFrame(
                NSButton::alloc(self.mtm()),
                NSRect::new(
                    NSPoint::new(x, 22.),
                    NSSize::new(if x == 500. { 90. } else { 170. }, 34.),
                ),
            );
            button.setTitle(&NSString::from_str(title));
            unsafe {
                button.setTarget(Some(self));
                button.setAction(Some(action));
            }
            content.addSubview(&button);
        }
        let mute = unsafe {
            NSButton::checkboxWithTitle_target_action(
                ns_string!("Mute sound"),
                Some(self),
                None,
                self.mtm(),
            )
        };
        mute.setFrame(NSRect::new(NSPoint::new(20., 136.), NSSize::new(240., 26.)));
        let capture = unsafe {
            NSButton::checkboxWithTitle_target_action(
                ns_string!("Enable system audio response (requires permission)"),
                Some(self),
                None,
                self.mtm(),
            )
        };
        capture.setFrame(NSRect::new(NSPoint::new(20., 94.), NSSize::new(570., 26.)));
        content.addSubview(&mute);
        content.addSubview(&capture);
        let mut state = self.ivars().borrow_mut();
        mute.setState(i64::from(state.config.muted) as isize);
        capture.setState(i64::from(state.config.capture_audio) as isize);
        state.label = Some(label);
        state.mute = Some(mute);
        state.capture = Some(capture);
        state.sheet = Some(panel.clone());
        drop(state);
        self.refresh_label();
        panel
    }
    fn refresh_label(&self) {
        let state = self.ivars().borrow();
        if let Some(label) = &state.label {
            label.setStringValue(&NSString::from_str(
                &state
                    .config
                    .project
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Choose a wallpaper project folder or project.json".into()),
            ));
        }
    }
    fn choose(&self, directory: bool) -> Option<(PathBuf, Option<Vec<u8>>)> {
        let panel = NSOpenPanel::openPanel(self.mtm());
        panel.setCanChooseFiles(!directory);
        panel.setCanChooseDirectories(true);
        panel.setAllowsMultipleSelection(false);
        if panel.runModal() != NSModalResponseOK {
            return None;
        }
        let url = panel.URL()?;
        let path = PathBuf::from(url.path()?.to_string());
        let bookmark = url
            .bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
                NSURLBookmarkCreationOptions::WithSecurityScope,
                None,
                None,
            )
            .ok()
            .map(|data| data.to_vec());
        Some((path, bookmark))
    }
}
fn path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME unavailable")?;
    Ok(PathBuf::from(home).join("Library/Application Support/WallpaperMac/screensaver.json"))
}
fn load_config() -> Result<Config, String> {
    let path = path()?;
    if !path.exists() {
        let app = wpe_platform::config::ConfigStore::new(path.with_file_name("state.json"))
            .load()
            .map_err(|e| e.to_string())?;
        let project = app.screens.values().next().map(|s| s.project.clone());
        let runtime = project
            .as_ref()
            .and_then(|p| app.applications.get(&p.to_string_lossy().into_owned()))
            .cloned();
        return Ok(Config {
            project,
            assets: app.assets,
            fps: app.fps,
            storage: app.scene_storage,
            application_runtime: runtime,
            ..Config::default()
        });
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("Screen saver configuration exceeds16MiB".into());
    }
    let config: Config = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if config.fps == 0 || config.fps > 240 {
        return Err("Screen saver FPS must be1..240".into());
    }
    Ok(config)
}
struct PreferencesLock(std::fs::File);
impl PreferencesLock {
    fn acquire() -> Result<Self, String> {
        use std::os::fd::AsRawFd;
        let file_path = path()?.with_file_name(".screensaver.lock");
        std::fs::create_dir_all(file_path.parent().ok_or("No preferences directory")?)
            .map_err(|e| e.to_string())?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(file_path)
            .map_err(|e| e.to_string())?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(Self(file))
    }
}
impl Drop for PreferencesLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
fn save_storage_delta(
    before: &BTreeMap<String, Value>,
    after: &BTreeMap<String, Value>,
) -> Result<(), String> {
    let _lock = PreferencesLock::acquire()?;
    let mut current = load_config()?;
    for (key, value) in after {
        if before.get(key) != Some(value) {
            current.storage.insert(key.clone(), value.clone());
        }
    }
    for key in before.keys() {
        if !after.contains_key(key) {
            current.storage.remove(key);
        }
    }
    write_config(&current)
}
fn save_config(config: &Config) -> Result<(), String> {
    let _lock = PreferencesLock::acquire()?;
    let mut config = config.clone();
    let current = load_config()?;
    for (key, value) in current.storage {
        config.storage.entry(key).or_insert(value);
    }
    write_config(&config)
}
fn write_config(config: &Config) -> Result<(), String> {
    let path = path()?;
    let parent = path.parent().ok_or("Missing preferences directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let temporary = parent.join(format!(
        ".screensaver-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let bytes = serde_json::to_vec_pretty(config).map_err(|e| e.to_string())?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("Screen saver state exceeds16MiB".into());
    }
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(|e| e.to_string())
}
fn resolve(
    path: &Path,
    bookmark: Option<&[u8]>,
    scopes: &mut Vec<Scope>,
) -> Result<PathBuf, String> {
    if let Some(bookmark) = bookmark {
        let mut stale = Bool::NO;
        let url = unsafe {
            NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(
                &NSData::with_bytes(bookmark),
                NSURLBookmarkResolutionOptions::WithSecurityScope
                    | NSURLBookmarkResolutionOptions::WithoutUI,
                None,
                &mut stale,
            )
        }
        .map_err(|e| e.localizedDescription().to_string())?;
        let resolved = url.path().ok_or("Bookmark URL has no path")?.to_string();
        let active = unsafe { url.startAccessingSecurityScopedResource() };
        scopes.push(Scope { url, active });
        Ok(PathBuf::from(resolved))
    } else {
        Ok(path.to_owned())
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn wallpaper_screensaver_register_class() {
    let _ = WallpaperMacScreenSaver::class();
}
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static REGISTER: extern "C" fn() = wallpaper_screensaver_register_class;

//! Main-thread AppKit host and isolated management control plane.
mod application;
mod audio;
mod control;
mod editor_host;
mod editor_services;
pub mod embedded;
mod export;
mod export_video;
pub(crate) use facts::screen_identity;
mod facts;
mod login;
mod media;
pub(crate) mod metal;
mod now_playing;
mod scene;
mod sound;
mod steam;
mod text;
mod video_texture;
mod workshop;
use crate::Options;
use audio::AudioCapture;
use control::Management;
use media::{VideoView, WebView};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::*;
use objc2_core_graphics::{CGWindowLevelForKey, CGWindowLevelKey};
use objc2_foundation::{
    MainThreadMarker, NSError, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString, NSTimer, NSURL, ns_string,
};
use objc2_web_kit::{
    WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate,
    WKScriptMessage, WKScriptMessageHandler, WKUserContentController, WKWebView,
};
use scene::LiveScene;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    time::Instant,
};
use wpe_core::{PlayerInput, WallpaperKind, load_project};
use wpe_platform::{
    audio::{AudioBands, AudioFrameProcessor},
    config::Selection,
    service::{HostAction, PlatformService},
};

enum Content {
    Scene(Box<LiveScene>),
    Video(VideoView),
    Web(WebView),
    Application(Box<application::ApplicationView>),
}
impl Content {
    fn view(&self) -> &NSView {
        match self {
            Self::Scene(v) => &v.renderer.view,
            Self::Video(v) => &v.view,
            Self::Web(v) => &v.view,
            Self::Application(v) => &v.renderer.view,
        }
    }
    fn set_paused(&self, paused: bool) {
        match self {
            Self::Scene(v) => {
                v.player.borrow_mut().set_paused(paused);
                v.sounds.borrow().set_paused(paused);
                v.videos.borrow().set_paused(paused);
                if !paused {
                    v.renderer.redraw()
                }
            }
            Self::Video(v) => v.set_paused(paused),
            Self::Web(v) => v.set_paused(paused),
            Self::Application(v) => v.set_paused(paused),
        }
    }
    fn set_volume(&self, volume: f32) {
        match self {
            Self::Scene(v) => v.sounds.borrow().set_volume(volume),
            Self::Video(v) => v.set_volume(volume),
            Self::Web(v) => v.set_volume(volume),
            Self::Application(v) => v.set_volume(volume),
        }
    }
    fn set_muted(&self, muted: bool) {
        match self {
            Self::Scene(v) => v.sounds.borrow().set_muted(muted),
            Self::Video(v) => v.set_muted(muted),
            Self::Web(v) => v.set_muted(muted),
            Self::Application(v) => v.set_muted(muted),
        }
    }
}
struct Slot {
    window: Retained<NSWindow>,
    content: Option<Content>,
    selection: Selection,
    metadata: Value,
    title: String,
    partial: bool,
    paused: bool,
    stopped: bool,
    last_error: Option<String>,
    mask: u32,
    desktop: bool,
    dirty: bool,
}
struct AppState {
    options: Options,
    service: PlatformService,
    slots: BTreeMap<String, Slot>,
    displays: Vec<facts::Display>,
    management: Option<Management>,
    settings_window: Option<Retained<NSWindow>>,
    status: Option<Retained<NSStatusItem>>,
    timer: Option<Retained<NSTimer>>,
    timer_fps: u32,
    pause_items: Vec<Retained<NSMenuItem>>,
    capture: Option<AudioCapture>,
    audio: AudioFrameProcessor,
    bands: AudioBands,
    media: now_playing::MediaMonitor,
    last_tick: Instant,
    last_policy: Instant,
    last_ui: Instant,
    sleeping: bool,
    paused_mask: u32,
    muted: bool,
    stopped: bool,
    report: String,
    runtime_messages: usize,
    editor: Option<wpe_editor::Editor>,
    export: Option<export::ExportJob>,
    export_status: Value,
    editor_replacement: Option<wpe_editor::Editor>,
    editor_close: bool,
    editor_zoom: f32,
    editor_pan: [f32; 2],
    editor_view: Option<Value>,
    editor_preview_scene: Option<Value>,
    editor_test_properties: BTreeMap<String, Value>,
    editor_test_resources: BTreeMap<String, std::sync::Arc<Vec<u8>>>,
    editor_preview: Option<editor_host::EditorPreview>,
    input_monitor: Option<Retained<AnyObject>>,
    keys: VecDeque<(u16, bool)>,
    process_audio: BTreeMap<i32, (String, AudioCapture)>,
    policy_audio: Option<AudioCapture>,
    policy_exclusions: Vec<i32>,
    audio_activity: wpe_platform::playback::AudioActivity,
    audio_scan: Instant,
    started: Instant,
    steam: Option<wpe_platform::steam::SteamProvider>,
    steam_config: Option<wpe_platform::steam::SteamConfig>,
    steam_error: Option<String>,
    steam_events: VecDeque<wpe_platform::steam::SteamEvent>,
    login_requested: bool,
}
define_class!(
 #[unsafe(super=NSObject)]#[thread_kind=MainThreadOnly]#[ivars=RefCell<AppState>]
 struct AppDelegate;
 unsafe impl NSObjectProtocol for AppDelegate{}
 impl AppDelegate{
  #[unsafe(method(openProject:))]fn open_action(&self,_sender:Option<&AnyObject>){let screen=self.default_screen();if let Some(path)=self.choose_path(false)&&let Err(error)=self.assign_path(&screen,&path){self.report_failure("Cannot open project",&error);}}
  #[unsafe(method(showSettings:))]fn settings_action(&self,_sender:Option<&AnyObject>){self.show_settings();}
  #[unsafe(method(togglePause:))]fn pause_action(&self,_sender:Option<&AnyObject>){let result={let mut state=self.ivars().borrow_mut();state.service.execute("playback.pause",json!({}))};if let Err(error)=result{self.record_error(&error.to_string());}self.apply_policy(true);}
  #[unsafe(method(showCompatibility:))]fn compatibility_action(&self,_sender:Option<&AnyObject>){self.show_settings();if let Some(ui)=&self.ivars().borrow().management{unsafe{ui.view.evaluateJavaScript_completionHandler(ns_string!("document.querySelector('[data-tab=diagnostics]').click()"),None);}}}
  #[unsafe(method(quitApplication:))]fn quit_action(&self,_sender:Option<&AnyObject>){NSApplication::sharedApplication(self.mtm()).terminate(None);}
  #[unsafe(method(frameTick:))]fn frame_tick(&self,_timer:&NSTimer){self.tick();}
  #[unsafe(method(willSleep:))]fn will_sleep(&self,_note:&NSNotification){{let mut state=self.ivars().borrow_mut();state.sleeping=true;state.capture.take();state.process_audio.clear();state.policy_audio.take();}self.apply_policy(true);}
  #[unsafe(method(didWake:))]fn did_wake(&self,_note:&NSNotification){let mut state=self.ivars().borrow_mut();state.sleeping=false;state.last_tick=Instant::now();state.last_policy=Instant::now();drop(state);self.restart_audio();self.apply_policy(true);}
 }
 unsafe impl NSApplicationDelegate for AppDelegate{
  #[unsafe(method(applicationDidFinishLaunching:))]fn did_launch(&self,_note:&NSNotification){
   self.install_menus();self.install_input();let center=NSWorkspace::sharedWorkspace().notificationCenter();unsafe{center.addObserver_selector_name_object(self,sel!(willSleep:),Some(NSWorkspaceWillSleepNotification),None);center.addObserver_selector_name_object(self,sel!(didWake:),Some(NSWorkspaceDidWakeNotification),None);}
   self.refresh_displays();self.restart_audio();self.restart_steam(false);if self.ivars().borrow().service.config.start_at_login&&let Err(error)=login::set_enabled(true){self.record_error(&error);}
   let assignments={self.ivars().borrow().service.effective.clone()};for(screen,selection)in assignments{if let Err(error)=self.open_selection(&screen,selection){self.record_error(&format!("{screen}: {error}"));}}
   let initial={self.ivars().borrow().options.project.clone()};if let Some(project)=initial{let screen=self.default_screen();if let Err(error)=self.assign_path(&screen,&project){self.report_failure("Cannot open project",&error);}}
   if self.ivars().borrow().slots.is_empty(){self.show_settings();}
   let fps=self.ivars().borrow().service.config.fps;let timer=unsafe{NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(1.0/fps as f64,self,sel!(frameTick:),None,true)};common_timer(&timer);self.ivars().borrow_mut().timer=Some(timer);
  }
  #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]fn terminate_on_close(&self,_app:&NSApplication)->bool{false}
  #[unsafe(method(applicationDidChangeScreenParameters:))]fn screens_changed(&self,_note:&NSNotification){self.refresh_displays();}
  #[unsafe(method(applicationWillTerminate:))]fn terminate(&self,_note:&NSNotification){
   let mut state=self.ivars().borrow_mut();if let Some(timer)=state.timer.take(){timer.invalidate();}
   if let Some(monitor)=state.input_monitor.take(){unsafe{NSEvent::removeMonitor(&monitor);}}let _=state.service.save();state.capture.take();for slot in state.slots.values_mut(){slot.content.take();slot.window.orderOut(None);}state.slots.clear();state.editor_preview.take();state.export.take();state.steam.take();state.management.take();state.settings_window.take();if let Some(status)=state.status.take(){NSStatusBar::systemStatusBar().removeStatusItem(&status);}drop(state);unsafe{NSWorkspace::sharedWorkspace().notificationCenter().removeObserver(self);}
  }
 }
 unsafe impl NSWindowDelegate for AppDelegate{
  #[unsafe(method(windowDidResize:))]fn window_resized(&self,note:&NSNotification){let Some(object)=note.object()else{return;};let pointer=Retained::as_ptr(&object);if let Ok(mut state)=self.ivars().try_borrow_mut(){for slot in state.slots.values_mut(){if Retained::as_ptr(&slot.window).cast::<AnyObject>()==pointer{slot.dirty=true;}}}}

  #[unsafe(method(windowWillClose:))]fn window_closed(&self,note:&NSNotification){let Some(object)=note.object()else{return;};let window=Retained::as_ptr(&object);let mut state=self.ivars().borrow_mut();for slot in state.slots.values_mut(){if Retained::as_ptr(&slot.window).cast::<AnyObject>()==window{slot.content.take();slot.stopped=true;slot.paused=true;}}
   if state.editor_preview.as_ref().is_some_and(|p|Retained::as_ptr(&p.window).cast::<AnyObject>()==window){state.editor_preview.take();}}
 }
 unsafe impl WKNavigationDelegate for AppDelegate{
  #[unsafe(method(webView:didFinishNavigation:))]fn web_finished(&self,view:&WKWebView,_navigation:Option<&WKNavigation>){let state=self.ivars().borrow();for slot in state.slots.values(){if let Some(Content::Web(web))=&slot.content&&&*web.view==view{web.refresh_initial();web.set_paused(slot.paused);}}}
  #[unsafe(method(webView:didFailNavigation:withError:))]fn web_failed(&self,view:&WKWebView,_navigation:Option<&WKNavigation>,error:&NSError){if self.is_wallpaper_web(view){self.record_error(&error.localizedDescription().to_string());}}
  #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]fn web_failed_initial(&self,view:&WKWebView,_navigation:Option<&WKNavigation>,error:&NSError){if self.is_wallpaper_web(view){self.record_error(&error.localizedDescription().to_string());}}
  #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]fn navigation_policy(&self,view:&WKWebView,action:&WKNavigationAction,decision:&block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>){
   let management=self.ivars().borrow().management.as_ref().is_some_and(|ui|&*ui.view==view);let allowed=if management{unsafe{action.request().URL()}.and_then(|u|u.scheme()).is_none_or(|scheme|scheme.to_string()=="about")}else{true};decision.call((if allowed{WKNavigationActionPolicy::Allow}else{WKNavigationActionPolicy::Cancel},));
  }
 }
 unsafe impl WKScriptMessageHandler for AppDelegate{
  #[unsafe(method(userContentController:didReceiveScriptMessage:))]fn script_message(&self,controller:&WKUserContentController,message:&WKScriptMessage){
   let web=unsafe{message.webView()};let management=self.ivars().borrow().management.as_ref().is_some_and(|ui|ui.owns(controller,web.as_deref()))&&unsafe{message.frameInfo().isMainFrame()};
   let body=unsafe{message.body()};let description:Retained<NSString>=unsafe{msg_send![&*body,description]};let text=description.to_string();if text.len()>2*1024*1024{self.record_error("Web message exceeded2MiB");return;}let Ok(value)=serde_json::from_str::<Value>(&text)else{return;};
   if management{let Some(id)=value["id"].as_u64()else{return;};let Some(method)=value["method"].as_str()else{return;};let args=value.get("args").cloned().unwrap_or(json!({}));if method=="native.workshopDetails"{self.workshop_details(id,args);return;}let result=self.command(method,args);if let Some(ui)=&self.ivars().borrow().management{ui.reply(id,result);}return;}
   // Untrusted wallpaper views may report diagnostics only. They never route to command().
   let current=self.ivars().borrow().slots.values().any(|slot|matches!(&slot.content,Some(Content::Web(web))if web.owns_controller(controller)&&Some(&*web.view)==unsafe{message.webView()}.as_deref()));
   if !current{return;}let error=value["type"].as_str()==Some("error");let text=value["message"].as_str().unwrap_or(&text);if error{self.record_error(text);}else{let mut state=self.ivars().borrow_mut();if state.runtime_messages<32{eprintln!("Web: {}",bounded(text,4096));}state.runtime_messages=state.runtime_messages.saturating_add(1);}
  }
 }
);

impl AppDelegate {
    fn new(options: Options, mtm: MainThreadMarker) -> Result<Retained<Self>, String> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("HOME is unavailable")?;
        let store = home.join("Library/Application Support/WallpaperMac/state.json");
        let mut service = PlatformService::new(store).map_err(|e| e.to_string())?;
        if service.config.library_roots.is_empty() {
            service.config.library_roots = wpe_platform::library::steam_workshop_roots(&home)
                .into_iter()
                .filter(|p| p.is_dir())
                .collect();
            service.refresh();
        }
        if options.assets.is_some() {
            service.config.assets = options.assets.clone();
        }
        if options.desktop {
            service.config.desktop = true;
        }
        let media = now_playing::MediaMonitor::new(service.config.media.clone());
        let timer_fps = service.config.fps;
        let now = Instant::now();
        let this = Self::alloc(mtm).set_ivars(RefCell::new(AppState {
            options,
            service,
            slots: BTreeMap::new(),
            displays: vec![],
            management: None,
            settings_window: None,
            status: None,
            timer: None,
            timer_fps,
            pause_items: vec![],
            capture: None,
            audio: AudioFrameProcessor::default(),
            bands: AudioBands::default(),
            media,
            last_tick: now,
            last_policy: now,
            last_ui: now,
            sleeping: false,
            paused_mask: 0,
            muted: false,
            stopped: false,
            report: String::new(),
            runtime_messages: 0,
            editor: None,
            export: None,
            export_status: Value::Null,
            editor_replacement: None,
            editor_close: false,
            editor_zoom: 1.,
            editor_pan: [0.; 2],
            editor_view: None,
            editor_preview_scene: None,
            editor_test_properties: BTreeMap::new(),
            editor_test_resources: BTreeMap::new(),
            editor_preview: None,
            input_monitor: None,
            keys: VecDeque::new(),
            process_audio: BTreeMap::new(),
            policy_audio: None,
            policy_exclusions: vec![],
            audio_activity: Default::default(),
            audio_scan: now - std::time::Duration::from_secs(6),
            started: now,
            steam: None,
            steam_config: None,
            steam_error: None,
            steam_events: VecDeque::new(),
            login_requested: false,
        }));
        Ok(unsafe { msg_send![super(this), init] })
    }
    fn default_screen(&self) -> String {
        let state = self.ivars().borrow();
        state
            .displays
            .get(state.options.screen)
            .or_else(|| state.displays.first())
            .map(|d| d.id.clone())
            .unwrap_or_else(|| "display-0".into())
    }
    fn alert(&self, title: &str, message: &str) {
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(&NSString::from_str(title));
        alert.setInformativeText(&NSString::from_str(bounded(message, 8192)));
        alert.addButtonWithTitle(ns_string!("OK"));
        alert.runModal();
    }
    fn record_error(&self, error: &str) {
        let mut state = self.ivars().borrow_mut();
        append_bounded(&mut state.report, &format!("\n{error}"));
        if state.runtime_messages < 32 {
            eprintln!("Wallpaper: {}", bounded(error, 4096));
        }
        state.runtime_messages = state.runtime_messages.saturating_add(1);
        for slot in state.slots.values_mut() {
            if !slot.partial {
                slot.partial = true;
                slot.window.setTitle(&NSString::from_str(&format!(
                    "{} — Partial compatibility",
                    slot.title
                )));
            }
        }
    }
    fn report_failure(&self, title: &str, error: &str) {
        self.record_error(&format!("{title}: {error}"));
        self.alert(title, error);
    }
    fn action_menu(&self) -> Retained<NSMenu> {
        let menu = NSMenu::new(self.mtm());
        for (title, action, key) in [
            ("Settings & Library…", sel!(showSettings:), ","),
            ("Open Project…", sel!(openProject:), "o"),
            ("Pause", sel!(togglePause:), "p"),
            ("Compatibility Report…", sel!(showCompatibility:), "i"),
            ("Quit", sel!(quitApplication:), "q"),
        ] {
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    NSMenuItem::alloc(self.mtm()),
                    &NSString::from_str(title),
                    Some(action),
                    &NSString::from_str(key),
                )
            };
            unsafe {
                item.setTarget(Some(self));
            }
            menu.addItem(&item);
            if title == "Pause" {
                self.ivars().borrow_mut().pause_items.push(item);
            }
        }
        menu
    }
    fn install_menus(&self) {
        let main = NSMenu::new(self.mtm());
        let item = NSMenuItem::new(self.mtm());
        item.setTitle(ns_string!("Wallpaper"));
        item.setSubmenu(Some(&self.action_menu()));
        main.addItem(&item);
        NSApplication::sharedApplication(self.mtm()).setMainMenu(Some(&main));
        let status = NSStatusBar::systemStatusBar().statusItemWithLength(-1.);
        if let Some(button) = status.button(self.mtm()) {
            button.setTitle(ns_string!("WP"));
        }
        status.setMenu(Some(&self.action_menu()));
        self.ivars().borrow_mut().status = Some(status);
    }
    fn choose_path(&self, directory: bool) -> Option<PathBuf> {
        #[allow(deprecated)]
        NSApplication::sharedApplication(self.mtm()).activateIgnoringOtherApps(true);
        let panel = NSOpenPanel::openPanel(self.mtm());
        panel.setCanChooseFiles(!directory);
        panel.setCanChooseDirectories(true);
        panel.setAllowsMultipleSelection(false);
        if panel.runModal() != NSModalResponseOK {
            return None;
        }
        panel.URL()?.path().map(|p| PathBuf::from(p.to_string()))
    }
    fn assign_path(&self, screen: &str, path: &Path) -> Result<(), String> {
        let reply = self
            .ivars()
            .borrow_mut()
            .service
            .execute(
                "assign",
                json!({"screen":screen,"selection":{"project":path}}),
            )
            .map_err(|e| e.to_string())?;
        self.actions(reply.actions)?;
        Ok(())
    }
    fn show_settings(&self) {
        let existing = { self.ivars().borrow().settings_window.clone() };
        let window = if let Some(w) = existing {
            w
        } else {
            let rect = NSRect::new(NSPoint::new(160., 140.), NSSize::new(1080., 760.));
            let window = self.new_window(rect, false);
            window.setTitle(ns_string!("Wallpaper — Library & Settings"));
            let frame = NSRect::new(NSPoint::new(0., 0.), rect.size);
            let ui = Management::new(
                frame,
                ProtocolObject::from_ref(self),
                ProtocolObject::from_ref(self),
                self.mtm(),
            );
            window.setContentView(Some(&ui.view));
            let mut state = self.ivars().borrow_mut();
            state.management = Some(ui);
            state.settings_window = Some(window.clone());
            window
        };
        window.makeKeyAndOrderFront(None);
        #[allow(deprecated)]
        NSApplication::sharedApplication(self.mtm()).activateIgnoringOtherApps(true);
    }
    fn new_window(&self, rect: NSRect, desktop: bool) -> Retained<NSWindow> {
        let style = if desktop {
            NSWindowStyleMask::Borderless
        } else {
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable
        };
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(self.mtm()),
                rect,
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe {
            window.setReleasedWhenClosed(false);
        }
        window.setDelegate(Some(ProtocolObject::from_ref(self)));
        window.setBackgroundColor(Some(&NSColor::blackColor()));
        if desktop {
            window.setLevel(
                CGWindowLevelForKey(CGWindowLevelKey::DesktopWindowLevelKey) as isize + 1,
            );
            window.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::Stationary
                    | NSWindowCollectionBehavior::IgnoresCycle
                    | NSWindowCollectionBehavior::FullScreenAuxiliary,
            );
            window.setIgnoresMouseEvents(true);
            window.setHasShadow(false);
        }
        window
    }
    fn refresh_displays(&self) {
        let displays = facts::displays(self.mtm());
        let mut state = self.ivars().borrow_mut();
        state.displays = displays;
        let existing = state.displays.clone();
        let desktop = state.service.config.desktop;
        let paused_mask = state.paused_mask;
        let manually_paused = state.service.paused;
        for (id, slot) in &mut state.slots {
            if let Some(display) = existing.iter().find(|d| &d.id == id) {
                slot.mask = display.mask;
                slot.dirty = true;
                slot.paused = manually_paused || paused_mask & slot.mask != 0;
                if let Some(content) = &slot.content {
                    content.set_paused(slot.paused);
                }
                if desktop {
                    slot.window.setFrame_display(display.rect, true);
                }
                if !slot.stopped {
                    slot.window.orderFrontRegardless();
                }
            } else {
                slot.mask = 0;
                slot.paused = true;
                slot.window.orderOut(None);
                if let Some(content) = &slot.content {
                    content.set_paused(true);
                }
            }
        }
        let missing = state
            .service
            .effective
            .iter()
            .filter(|(id, _)| {
                !state.slots.contains_key(*id) && state.displays.iter().any(|d| &d.id == *id)
            })
            .map(|(id, s)| (id.clone(), s.clone()))
            .collect::<Vec<_>>();
        drop(state);
        for (id, selection) in missing {
            if let Err(error) = self.open_selection(&id, selection) {
                self.record_error(&error);
            }
        }
    }
    fn open_selection(&self, id: &str, selection: Selection) -> Result<(), String> {
        let (assets, desktop, display, properties) = {
            let state = self.ivars().borrow();
            (
                state.service.config.assets.clone(),
                state.service.config.desktop,
                state.displays.iter().find(|d| d.id == id).cloned(),
                state.service.config.selection_properties(&selection),
            )
        };
        let Some(display) = display else {
            return Ok(());
        };
        let mut project =
            load_project(&selection.project, assets.as_deref()).map_err(|e| e.to_string())?;
        for (key, value) in properties {
            if let Some(p) = project.metadata.pointer_mut(&format!(
                "/general/properties/{}",
                key.replace('~', "~0").replace('/', "~1")
            )) {
                p["value"] = value;
            }
        }
        let rect = if desktop {
            display.rect
        } else {
            NSRect::new(
                NSPoint::new(display.rect.origin.x + 40., display.rect.origin.y + 40.),
                NSSize::new(
                    960f64.min(display.rect.size.width * 0.85),
                    600f64.min(display.rect.size.height * 0.85),
                ),
            )
        };
        let frame = NSRect::new(NSPoint::new(0., 0.), rect.size);
        let mut issues = project
            .issues
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let metadata = project.metadata.clone();
        let title = project.title.clone();
        let initial_input = {
            let state = self.ivars().borrow();
            let screen = NSScreen::screens(self.mtm()).objectAtIndex(display.index);
            let scale = screen.backingScaleFactor();
            let (cursor, buttons) = facts::cursor(rect);
            let (_, time_of_day) = facts::calendar();
            PlayerInput {
                cursor,
                buttons,
                pointer_active: false,
                screen_resolution: [
                    rect.size.width as f32 * scale as f32,
                    rect.size.height as f32 * scale as f32,
                ],
                media: state.media.value(),
                audio_left: state.bands.bands64[0].to_vec(),
                audio_right: state.bands.bands64[1].to_vec(),
                time_of_day,
                general_settings: json!({"fps":state.service.config.fps}),
                ..Default::default()
            }
        };
        let initially_paused = {
            let state = self.ivars().borrow();
            state.service.paused || state.stopped || state.paused_mask & display.mask != 0
        };
        let content = match &project.kind.clone() {
            WallpaperKind::Scene => {
                let (scene, reported) = LiveScene::new(
                    project,
                    id,
                    &mut self.ivars().borrow_mut().service.config.scene_storage,
                    frame,
                    self.mtm(),
                    wpe_core::ScenePlayerOptions {
                        is_editor: false,
                        is_wallpaper: true,
                        is_desktop: desktop,
                    },
                    initial_input,
                )?;
                issues.extend(reported);
                Content::Scene(Box::new(scene))
            }
            WallpaperKind::Video => {
                Content::Video(VideoView::new(&project.entry, frame, desktop, self.mtm())?)
            }
            WallpaperKind::Web => Content::Web(WebView::new(
                &project.entry,
                &project.root,
                &project.metadata,
                frame,
                ProtocolObject::from_ref(self),
                ProtocolObject::from_ref(self),
                self.mtm(),
                initially_paused,
                self.ivars().borrow().muted,
                self.ivars().borrow().service.config.playback_volume,
            )?),
            WallpaperKind::Application => {
                let runtime=self.ivars().borrow().service.config.applications.get(&project.root.to_string_lossy().into_owned()).cloned().ok_or("Configure an explicit external application runtime for this project in Application Runtimes settings")?;
                issues.push("External application window capture profile: requires configured runtime and Screen Recording permission; input forwarding and Windows desktop IPC are not emulated; mute pauses the owned process group".into());
                Content::Application(Box::new(application::ApplicationView::new(
                    project,
                    runtime,
                    frame,
                    self.mtm(),
                )?))
            }
            WallpaperKind::Unknown(kind) => return Err(format!("Unknown wallpaper type: {kind}")),
        };
        let window = self.new_window(rect, desktop);
        let partial = !issues.is_empty();
        window.setTitle(&NSString::from_str(&if partial {
            format!("{} — Partial compatibility", title)
        } else {
            title.clone()
        }));
        window.setContentView(Some(content.view()));
        let mut state = self.ivars().borrow_mut();
        let paused = state.service.paused || (state.paused_mask & display.mask != 0);
        content.set_volume(state.service.config.playback_volume);
        content.set_muted(state.muted);
        content.set_paused(paused || state.stopped);
        let stopped = state.stopped;
        let content = if stopped { None } else { Some(content) };
        let old = state.slots.insert(
            id.into(),
            Slot {
                window: window.clone(),
                content,
                selection,
                metadata,
                title,
                partial,
                paused,
                stopped,
                last_error: None,
                mask: display.mask,
                desktop,
                dirty: true,
            },
        );
        for issue in issues {
            append_bounded(&mut state.report, &format!("\n{issue}"));
        }
        drop(state);
        if let Some(old) = old {
            old.window.orderOut(None);
            old.window.setContentView(None);
            old.window.close();
            drop(old);
        }
        if !stopped {
            if desktop {
                window.orderFrontRegardless();
            } else {
                window.makeKeyAndOrderFront(None);
            }
        }
        Ok(())
    }
    fn is_wallpaper_web(&self, view: &WKWebView) -> bool {
        self.ivars()
            .borrow()
            .slots
            .values()
            .any(|slot| matches!(&slot.content,Some(Content::Web(web))if &*web.view==view))
    }
    fn restart_audio(&self) {
        let settings = {
            let mut state = self.ivars().borrow_mut();
            state.capture.take();
            state.process_audio.clear();
            state.policy_audio.take();
            state.policy_exclusions.clear();
            state.audio_activity = Default::default();
            state.audio = AudioFrameProcessor::default();
            state.bands = AudioBands::default();
            state.service.config.audio.clone()
        };
        let need_capture =
            {
                let state = self.ivars().borrow();
                settings.enabled
                    || state.service.config.policy.audio != wpe_platform::playback::Action::Run
                    || state.service.config.rules.iter().any(|rule| {
                        rule.condition == wpe_platform::playback::Condition::PlayingAudio
                    })
            };
        if need_capture {
            match AudioCapture::start(settings) {
                Ok(capture) => self.ivars().borrow_mut().capture = Some(capture),
                Err(error) => self.record_error(&error),
            }
        }
    }
    fn actions(&self, actions: Vec<HostAction>) -> Result<(), String> {
        let mut failures = Vec::<String>::new();
        for action in actions {
            match action {
                HostAction::Assign { screen, selection } => {
                    if let Some(selection) = selection {
                        if let Err(error) = self.update_selection(&screen, selection) {
                            let message = format!("Display {screen}: {error}");
                            self.record_error(&message);
                            failures.push(message);
                        }
                    } else {
                        let removed = { self.ivars().borrow_mut().slots.remove(&screen) };
                        if let Some(slot) = removed {
                            slot.window.orderOut(None);
                            slot.window.setContentView(None);
                            slot.window.close();
                        }
                    }
                }
                HostAction::Playback {
                    paused,
                    muted,
                    stopped,
                } => {
                    let mut rebuild = Vec::new();
                    let mut state = self.ivars().borrow_mut();
                    state.paused_mask = paused;
                    state.muted = muted;
                    state.stopped = stopped;
                    for (id, slot) in &mut state.slots {
                        slot.paused = slot.mask == 0 || paused & slot.mask != 0;
                        if stopped {
                            slot.window.setContentView(None);
                            slot.content.take();
                            slot.stopped = true;
                            slot.window.orderOut(None);
                        } else if slot.stopped {
                            rebuild.push((id.clone(), slot.selection.clone()));
                        } else if let Some(content) = &slot.content {
                            content.set_paused(slot.paused);
                            content.set_muted(muted);
                        }
                    }
                    let is_paused = state.service.paused;
                    for item in &state.pause_items {
                        item.setTitle(if is_paused {
                            ns_string!("Resume")
                        } else {
                            ns_string!("Pause")
                        });
                    }
                    drop(state);
                    for (id, selection) in rebuild {
                        if let Err(error) = self.open_selection(&id, selection) {
                            self.record_error(&error);
                            failures.push(error);
                        }
                    }
                }
                HostAction::AudioChanged => self.restart_audio(),
                HostAction::OpenUri { uri } => {
                    if let Some(url) = NSURL::URLWithString(&NSString::from_str(&uri))
                        && !NSWorkspace::sharedWorkspace().openURL(&url)
                    {
                        self.record_error("Steam client could not open this URI");
                        failures.push("Steam client could not open this URI".into());
                    }
                }
                HostAction::SettingsChanged => self.reconfigure(),
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("\n"))
        }
    }
    fn reconfigure(&self) {
        self.restart_steam(false);
        {
            let state = self.ivars().borrow();
            for slot in state.slots.values() {
                if let Some(content) = &slot.content {
                    content.set_volume(state.service.config.playback_volume);
                }
            }
        }
        let requested = self.ivars().borrow().service.config.start_at_login;
        let previous = {
            let mut state = self.ivars().borrow_mut();
            let previous = state.login_requested;
            state.login_requested = requested;
            previous
        };
        if previous != requested
            && let Err(error) = login::set_enabled(requested)
        {
            self.record_error(&error);
        }
        {
            let mut state = self.ivars().borrow_mut();
            let media = state.service.config.media.clone();
            state.media.configure(media);
        }
        let fps = self.ivars().borrow().service.config.fps;
        let previous_timer = { self.ivars().borrow_mut().timer.take() };
        if let Some(timer) = previous_timer {
            timer.invalidate();
            let timer = unsafe {
                NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                    1. / fps as f64,
                    self,
                    sel!(frameTick:),
                    None,
                    true,
                )
            };
            common_timer(&timer);
            let mut state = self.ivars().borrow_mut();
            state.timer_fps = fps;
            state.timer = Some(timer);
        }
        let rebuild = {
            let state = self.ivars().borrow();
            state
                .slots
                .iter()
                .filter(|(_, slot)| slot.desktop != state.service.config.desktop || matches!(&slot.content,Some(Content::Application(app))if state.service.config.applications.get(&app.config_key()).is_none_or(|runtime|!app.matches(runtime))))
                .map(|(id, slot)| (id.clone(), slot.selection.clone()))
                .collect::<Vec<_>>()
        };
        for (id, selection) in rebuild {
            if let Err(error) = self.open_selection(&id, selection) {
                self.record_error(&error);
            }
        }
        self.refresh_displays();
    }
    fn update_selection(&self, id: &str, selection: Selection) -> Result<(), String> {
        let mut state = self.ivars().borrow_mut();
        let desktop = state.service.config.desktop;
        let properties = state.service.config.selection_properties(&selection);
        if let Some(slot) = state.slots.get_mut(id)
            && slot.selection.project == selection.project
            && slot.desktop == desktop
            && !slot.stopped
        {
            let mut wrapped = serde_json::Map::new();
            for (key, value) in &properties {
                wrapped.insert(key.clone(), json!({"value":value}));
                if let Some(p) = slot.metadata.pointer_mut(&format!(
                    "/general/properties/{}",
                    key.replace('~', "~0").replace('/', "~1")
                )) {
                    p["value"] = value.clone();
                }
            }
            match &slot.content {
                Some(Content::Scene(scene)) => scene
                    .player
                    .borrow_mut()
                    .set_user_properties(&json!(properties))
                    .map_err(|e| e.to_string())?,
                Some(Content::Web(web)) => web.update_properties(&Value::Object(wrapped)),
                _ => {}
            }
            slot.selection = selection;
            slot.dirty = true;
            return Ok(());
        }
        drop(state);
        self.open_selection(id, selection)
    }
    fn apply_policy(&self, force: bool) {
        let mut state = self.ivars().borrow_mut();
        if !force && state.last_policy.elapsed().as_secs_f32() < 0.25 {
            return;
        }
        let dt = state.last_policy.elapsed().as_secs_f64();
        state.last_policy = Instant::now();
        let now = state.started.elapsed().as_secs_f64();
        if let Some(capture) = &state.capture {
            let status = state.policy_audio.as_ref().unwrap_or(capture).status();
            if status.age_seconds < 0.3 {
                state.audio_activity.observe_global(now, status.peak);
            }
        }
        let mut facts = facts::playback(
            &state.displays,
            state.sleeping,
            state.audio_activity.global_active(now),
        );
        if !state.sleeping && state.audio_scan.elapsed().as_secs_f32() >= 5.0 {
            state.audio_scan = Instant::now();
            let needed = facts
                .applications
                .iter()
                .filter(|app| {
                    state.service.config.rules.iter().any(|rule| {
                        rule.condition == wpe_platform::playback::Condition::PlayingAudio
                            && wpe_platform::playback::executable_matches(
                                &rule.executable,
                                &app.executable,
                            )
                    })
                })
                .map(|app| app.executable.clone())
                .collect::<Vec<_>>();
            let workspace = NSWorkspace::sharedWorkspace();
            let matches = workspace
                .runningApplications()
                .iter()
                .filter_map(|app| {
                    let executable = app.executableURL()?.path()?.to_string();
                    if needed.contains(&executable) {
                        Some((app.processIdentifier(), executable))
                    } else {
                        None
                    }
                })
                .take(16)
                .collect::<BTreeMap<_, _>>();
            let mut exclusions = matches
                .iter()
                .filter(|(_, executable)| {
                    state.service.config.rules.iter().any(|rule| {
                        rule.condition == wpe_platform::playback::Condition::PlayingAudio
                            && matches!(rule.action, wpe_platform::playback::RuleAction::Run)
                            && wpe_platform::playback::executable_matches(
                                &rule.executable,
                                executable,
                            )
                    })
                })
                .map(|(pid, _)| *pid)
                .collect::<Vec<_>>();
            exclusions.sort_unstable();
            if exclusions != state.policy_exclusions {
                state.policy_audio.take();
                state.audio_activity = Default::default();
                if !exclusions.is_empty() {
                    match AudioCapture::start_excluding(exclusions.clone()) {
                        Ok(capture) => state.policy_audio = Some(capture),
                        Err(error) => append_bounded(&mut state.report, &error),
                    }
                }
                state.policy_exclusions = exclusions;
            }
            state
                .process_audio
                .retain(|pid, _| matches.contains_key(pid));
            for (pid, executable) in matches {
                if state.process_audio.contains_key(&pid) {
                    continue;
                }
                match AudioCapture::start_for_pid(pid) {
                    Ok(capture) => {
                        state.process_audio.insert(pid, (executable, capture));
                    }
                    Err(error) => {
                        append_bounded(&mut state.report, &format!("\nApplication audio: {error}"));
                    }
                }
            }
        }
        let peaks = state
            .process_audio
            .values()
            .map(|(name, capture)| (name.clone(), capture.status()))
            .collect::<Vec<_>>();
        for (name, status) in peaks {
            if status.age_seconds < 0.3 {
                state
                    .audio_activity
                    .observe_process(now, &name, status.peak);
            }
        }
        for app in &mut facts.applications {
            app.playing_audio = state.audio_activity.process_active(now, &app.executable);
        }
        facts.audio_unattributed = state.audio_activity.global_active(now);
        state.audio_activity.prune(now);
        let (calendar, _) = facts::calendar();
        let actions = state.service.tick(dt, calendar, facts);
        drop(state);
        match actions {
            Ok(actions) => {
                let _ = self.actions(actions);
            }
            Err(error) => self.record_error(&error.to_string()),
        }
    }
    fn tick(&self) {
        let now = Instant::now();
        let mut errors = Vec::new();
        {
            let mut state = self.ivars().borrow_mut();
            let dt = now.duration_since(state.last_tick).as_secs_f32().min(0.25);
            state.last_tick = now;
            if state.sleeping {
                return;
            }
            if let Some(capture) = &state.capture {
                capture.set_analysis_enabled(
                    state.service.config.audio.enabled
                        && (state
                            .slots
                            .values()
                            .any(|slot| !slot.paused && !slot.stopped)
                            || state
                                .export
                                .as_ref()
                                .is_some_and(|job| job.uses_live_inputs())),
                );
            }
            let raw = if state.service.config.audio.enabled {
                state
                    .capture
                    .as_ref()
                    .map(AudioCapture::latest)
                    .unwrap_or_default()
            } else {
                Default::default()
            };
            state.bands = state.audio.process(&raw, dt);
            let bands = state.bands.web_stereo();
            let audio_left = state.bands.bands64[0].to_vec();
            let audio_right = state.bands.bands64[1].to_vec();
            state.media.poll();
            if state.service.config.media.source == wpe_platform::media::MediaSource::Owned {
                let owned = state
                    .slots
                    .values()
                    .find_map(|slot| match &slot.content {
                        Some(Content::Video(video)) => Some(video.media(&slot.title)),
                        Some(Content::Scene(scene)) => scene.sounds.borrow().media(),
                        _ => None,
                    })
                    .unwrap_or_else(|| wpe_platform::media::unavailable(true));
                state.media.set_owned(owned);
            }
            let media = state.media.value();
            let keys = state
                .keys
                .iter()
                .map(|(key, down)| (key.to_string(), *down))
                .collect::<BTreeMap<_, _>>();
            let (_, time_of_day) = facts::calendar();
            let fps = state.service.config.fps;
            let displays = state.displays.clone();
            let mut storage = std::mem::take(&mut state.service.config.scene_storage);
            let mut native_events = Vec::new();
            for (id, slot) in &mut state.slots {
                if slot.stopped || (slot.paused && !slot.dirty) {
                    continue;
                }
                slot.dirty = false;
                let error = match &slot.content {
                    Some(Content::Scene(scene)) => {
                        let rect = slot
                            .window
                            .convertRectToScreen(scene.renderer.view.bounds());
                        let (cursor, buttons, pointer_active) =
                            facts::pointer_for_view(&scene.renderer.view, slot.desktop);
                        let bounds = scene.renderer.view.bounds();
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
                            keys: keys.clone(),
                            screen_resolution: [
                                rect.size.width as f32 * slot.window.backingScaleFactor() as f32,
                                rect.size.height as f32 * slot.window.backingScaleFactor() as f32,
                            ],
                            media: media.clone(),
                            audio_left: audio_left.clone(),
                            audio_right: audio_right.clone(),
                            time_of_day,
                            general_settings: json!({"fps":fps}),
                        };
                        let output = if slot.paused {
                            scene.render_current(&input).map(|_| None)
                        } else {
                            scene.advance(dt, &input, &mut storage)
                        };
                        for event in scene.drain_events() {
                            native_events.push((
                                event,
                                slot.selection.clone(),
                                slot.metadata.clone(),
                            ));
                        }
                        match output {
                            Ok(issue) => issue,
                            Err(error) => Some(error),
                        }
                    }
                    Some(Content::Application(app)) => app.tick().err(),
                    Some(Content::Video(video)) => video.failure(),
                    Some(Content::Web(web)) => {
                        web.media(&media);
                        web.general(fps);
                        web.audio(&bands);
                        if displays.iter().any(|d| &d.id == id) {
                            let (cursor, buttons, active) =
                                facts::pointer_for_view(&web.view, slot.desktop);
                            if slot.desktop {
                                web.input(cursor, buttons, active);
                            }
                        }
                        web.refresh_initial();
                        None
                    }
                    None => None,
                };
                if let Some(error) = error
                    && slot.last_error.as_ref() != Some(&error)
                {
                    slot.last_error = Some(error.clone());
                    errors.push(error);
                }
            }
            state.service.config.scene_storage = storage;
            for (event, selection, metadata) in native_events {
                if event.kind == "engine.openUserShortcut"
                    && let Some(name) = event.arguments["name"].as_str()
                    && metadata
                        .pointer(&format!(
                            "/general/properties/{}/type",
                            name.replace('~', "~0").replace('/', "~1")
                        ))
                        .and_then(Value::as_str)
                        == Some("shortcut")
                    && let Some(value) = selection.properties.get(name).and_then(Value::as_str)
                {
                    let url = NSURL::URLWithString(&NSString::from_str(value)).or_else(|| {
                        if Path::new(value).is_absolute() {
                            Some(NSURL::fileURLWithPath_isDirectory(
                                &NSString::from_str(value),
                                false,
                            ))
                        } else {
                            None
                        }
                    });
                    if let Some(url) = url {
                        NSWorkspace::sharedWorkspace().openURL(&url);
                    }
                }
            }
            if let Some(editor) = &mut state.editor
                && let Err(error) = editor.autosave(dt as f64)
            {
                errors.push(error.to_string());
            }
            if let Some(preview) = &state.editor_preview
                && let Err(error) = preview.surface.tick()
            {
                errors.push(error);
            }
            let export_input = PlayerInput {
                audio_left: state.bands.bands64[0].to_vec(),
                audio_right: state.bands.bands64[1].to_vec(),
                media: state.media.value(),
                time_of_day,
                ..Default::default()
            };
            if let Some(export) = &mut state.export {
                export.tick(&export_input);
            }
            if state.export.as_ref().is_some_and(|job| !job.is_active()) {
                state.export_status = state
                    .export
                    .as_ref()
                    .map(|job| job.status())
                    .unwrap_or(Value::Null);
                state.export.take();
            }
            state.keys.clear();
            if state.last_ui.elapsed().as_secs() > 30 {
                state.last_ui = now;
                if let Err(error) = state.service.save() {
                    errors.push(error.to_string());
                }
            }
        }
        for error in errors {
            self.record_error(&error);
        }
        self.apply_policy(false);
        self.pump_steam();
        self.sync_idle_providers();
    }
    fn sync_idle_providers(&self) {
        let (fps, changed, wants_capture, has_capture) = {
            let state = self.ivars().borrow();
            let active = state.slots.values().any(|slot| {
                !slot.paused
                    && !slot.stopped
                    && matches!(
                        &slot.content,
                        Some(Content::Scene(_) | Content::Web(_) | Content::Application(_))
                    )
            }) || state
                .editor_preview
                .as_ref()
                .is_some_and(|p| !p.surface.is_paused())
                || state.export.as_ref().is_some_and(|job| job.is_active());
            let fps = if active { state.service.config.fps } else { 4 };
            let watch_audio =
                state.service.config.policy.audio != wpe_platform::playback::Action::Run
                    || state.service.config.rules.iter().any(|rule| {
                        rule.condition == wpe_platform::playback::Condition::PlayingAudio
                    });
            (
                fps,
                state.timer.is_some() && state.timer_fps != fps,
                !state.sleeping && ((active && state.service.config.audio.enabled) || watch_audio),
                state.capture.is_some(),
            )
        };
        if changed {
            let old = {
                let mut state = self.ivars().borrow_mut();
                state.timer_fps = fps;
                state.timer.take()
            };
            if let Some(old) = old {
                old.invalidate();
            }
            let timer = unsafe {
                NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                    1. / fps as f64,
                    self,
                    sel!(frameTick:),
                    None,
                    true,
                )
            };
            common_timer(&timer);
            self.ivars().borrow_mut().timer = Some(timer);
        }
        if wants_capture && !has_capture {
            self.restart_audio();
        } else if !wants_capture && has_capture {
            self.ivars().borrow_mut().capture.take();
        }
    }
    fn install_input(&self) {
        let this =
            unsafe { Retained::retain(self as *const Self as *mut Self) }.expect("delegate exists");
        let block =
            block2::RcBlock::new(move |event: std::ptr::NonNull<NSEvent>| -> *mut NSEvent {
                if let Some(event) = unsafe { event.as_ptr().as_ref() } {
                    if this.editor_local_event(event) {
                        return std::ptr::null_mut();
                    }
                    if matches!(event.r#type(), NSEventType::KeyDown | NSEventType::KeyUp) {
                        let mut state = this.ivars().borrow_mut();
                        if state.keys.len() < 512 {
                            state.keys.push_back((
                                event.keyCode(),
                                event.r#type() == NSEventType::KeyDown,
                            ));
                        }
                    }
                }
                event.as_ptr()
            });
        let monitor = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::KeyDown | NSEventMask::KeyUp,
                &block,
            )
        };
        self.ivars().borrow_mut().input_monitor = monitor;
    }
    fn snapshot(&self) -> Value {
        let state = self.ivars().borrow();
        let mut value = state.service.snapshot();
        value["displays"] = json!(
            state
                .displays
                .iter()
                .map(facts::Display::json)
                .collect::<Vec<_>>()
        );
        value["slots"]=json!(state.slots.iter().map(|(id,slot)|json!({"display":id,"title":slot.title,"paused":slot.paused,"stopped":slot.stopped,"partial":slot.partial,"error":slot.last_error,"presentation":match &slot.content{Some(Content::Scene(scene))=>Some(scene.renderer.presentation_profile()),_=>None}})).collect::<Vec<_>>());
        value["report"] = json!(state.report);
        value["media_status"] = state.media.status();
        value["export"] = state
            .export
            .as_ref()
            .map(|job| job.status())
            .unwrap_or_else(|| state.export_status.clone());
        value["login"] = login::status();
        value["steam"] = self.steam_status();
        let mut props = serde_json::Map::new();
        for (id, slot) in &state.slots {
            props.insert(
                id.clone(),
                slot.metadata
                    .pointer("/general/properties")
                    .cloned()
                    .unwrap_or(json!({})),
            );
        }
        value["project_properties"] = Value::Object(props);
        value["audio"]=state.capture.as_ref().map(|c|{let s=c.status();json!({"state":s.state,"blocks":s.received_blocks,"dropped":s.dropped_blocks,"peak":s.peak,"age_seconds":if s.age_seconds.is_finite(){Some(s.age_seconds)}else{None}})}).unwrap_or(json!({"state":"Disabled"}));
        value["capabilities"] = json!({"audio_capture":"ScreenCaptureKit; permission required","application_wallpapers":"Explicit external-runtime process + ScreenCaptureKit window capture; Windows IPC/input not emulated; mute pauses process","global_media":"Opt-in public Music/Spotify AppleEvents or owned AVPlayer metadata; no universal public session API","per_application_audio_rule":"ScreenCaptureKit application-filtered PCM peaks; max16 concurrent application streams; macOS permission required","mac_runtime_verified":false});
        value
    }
    fn command(&self, method: &str, args: Value) -> Result<Value, String> {
        if method.starts_with("steam.") {
            return self.steam_command(method, args);
        }
        match method {
            "state" => Ok(self.snapshot()),
            "login.settings" => {
                login::open_settings();
                Ok(Value::Null)
            }
            "native.open" => {
                let screen = args["screen"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| self.default_screen());
                if let Some(path) = self.choose_path(false) {
                    self.assign_path(&screen, &path)?;
                }
                Ok(Value::Null)
            }
            "native.assetsFolder" => {
                if let Some(path) = self.choose_path(true) {
                    let mut config = self.ivars().borrow().service.config.clone();
                    config.assets = Some(path);
                    let reply = self
                        .ivars()
                        .borrow_mut()
                        .service
                        .execute("config.set", json!({"config":config}))
                        .map_err(|e| e.to_string())?;
                    self.actions(reply.actions)?;
                }
                Ok(Value::Null)
            }
            "native.externalTool" => {
                let name = args["name"].as_str().ok_or("Tool name required")?;
                if !matches!(name, "ffmpeg" | "image_editor" | "assimp") {
                    return Err("Unknown external tool".into());
                }
                if let Some(path) = self.choose_path(false) {
                    let mut config = self.ivars().borrow().service.config.clone();
                    config.external_tools.insert(name.into(), path);
                    let reply = self
                        .ivars()
                        .borrow_mut()
                        .service
                        .execute("config.set", json!({"config":config}))
                        .map_err(|e| e.to_string())?;
                    self.actions(reply.actions)?;
                }
                Ok(Value::Null)
            }
            "native.screenSaverFolder" => {
                let path = PathBuf::from(std::env::var_os("HOME").ok_or("HOME unavailable")?)
                    .join("Library/Screen Savers");
                std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
                let url = NSURL::fileURLWithPath_isDirectory(
                    &NSString::from_str(&path.to_string_lossy()),
                    true,
                );
                if !NSWorkspace::sharedWorkspace().openURL(&url) {
                    return Err("Finder could not open the Screen Savers directory".into());
                }
                Ok(
                    json!({"directory":path,"instruction":"Copy WallpaperMac.saver from the build here, then select it and configure Screen Saver Options in System Settings"}),
                )
            }
            "native.libraryFolder" => {
                if let Some(path) = self.choose_path(true) {
                    let reply = self
                        .ivars()
                        .borrow_mut()
                        .service
                        .execute("library.add", json!({"path":path}))
                        .map_err(|e| e.to_string())?;
                    self.actions(reply.actions)?;
                }
                Ok(Value::Null)
            }
            "export.status" => {
                let state = self.ivars().borrow();
                Ok(state
                    .export
                    .as_ref()
                    .map(|job| job.status())
                    .unwrap_or_else(|| {
                        if state.export_status.is_null() {
                            json!({"idle":true})
                        } else {
                            state.export_status.clone()
                        }
                    }))
            }
            "editor.model" => {
                let mut state = self.ivars().borrow_mut();
                state
                    .editor
                    .as_mut()
                    .ok_or("Editor closed")?
                    .editable_model_resource(args.get("id").ok_or("Model resource ID required")?)
                    .map_err(|e| e.to_string())
            }
            "editor.snapshot" => self
                .ivars()
                .borrow()
                .editor
                .as_ref()
                .map(|editor| json!(editor.snapshot()))
                .ok_or("Open an editor project first".into()),
            "editor.openPicker" => {
                let Some(path) = self.choose_path(false) else {
                    return Ok(json!({"cancelled":true}));
                };
                self.command("editor.open", json!({"path":path}))
            }
            "editor.open" => {
                let (path, assets, workspace) = {
                    let state = self.ivars().borrow();
                    let path = if let Some(path) = args["path"].as_str() {
                        PathBuf::from(path)
                    } else {
                        let screen = args["screen"].as_str().ok_or("Missing screen")?;
                        state
                            .service
                            .config
                            .screens
                            .get(screen)
                            .ok_or("No project assigned")?
                            .project
                            .clone()
                    };
                    (
                        path,
                        state.service.config.assets.clone(),
                        state
                            .service
                            .config_path()
                            .parent()
                            .ok_or("Missing app state directory")?
                            .join("EditorProjects"),
                    )
                };
                let (editor, copy) =
                    editor_host::open_editor(&path, assets.as_deref(), &workspace)?;
                let value =
                    json!({"snapshot":editor.snapshot(),"workspace":copy,"source_preserved":true});
                let mut state = self.ivars().borrow_mut();
                state.editor_preview.take();
                state.editor = Some(editor);
                state.editor_test_properties.clear();
                state.editor_test_resources.clear();
                state.editor_preview_scene = None;
                state.editor_zoom = 1.;
                state.editor_pan = [0.; 2];
                state.editor_view = None;
                Ok(value)
            }
            "editor.execute" => self.editor_execute(args),
            "editor.preview" => {
                self.show_editor_preview(true)?;
                Ok(json!({"preview":true}))
            }

            _ => {
                let reply = self
                    .ivars()
                    .borrow_mut()
                    .service
                    .execute(method, args)
                    .map_err(|e| e.to_string())?;
                self.actions(reply.actions)?;
                self.apply_policy(true);
                Ok(reply.value)
            }
        }
    }
}
fn bounded(value: &str, limit: usize) -> &str {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}
fn append_bounded(report: &mut String, text: &str) {
    if report.len() < 65536 {
        report.push_str(bounded(text, 4096.min(65536 - report.len())));
    }
}
pub(super) fn run(options: Options) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("macOS host must start on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(if options.desktop {
        NSApplicationActivationPolicy::Accessory
    } else {
        NSApplicationActivationPolicy::Regular
    });
    let delegate = AppDelegate::new(options, mtm)?;
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
    app.setDelegate(None);
    Ok(())
}

fn common_timer(timer: &NSTimer) {
    unsafe {
        objc2_foundation::NSRunLoop::mainRunLoop()
            .addTimer_forMode(timer, objc2_foundation::NSRunLoopCommonModes);
    }
}

pub(super) fn local_time_of_day() -> f32 {
    facts::calendar().1
}

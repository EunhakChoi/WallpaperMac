use objc2::{MainThreadOnly, rc::Retained, runtime::ProtocolObject};
use objc2_app_kit::{NSAutoresizingMaskOptions, NSView};
use objc2_av_foundation::{
    AVLayerVideoGravityResizeAspect, AVLayerVideoGravityResizeAspectFill, AVPlayerItem,
    AVPlayerLayer, AVPlayerLooper, AVPlayerLooperStatus, AVQueuePlayer,
};
use objc2_foundation::{MainThreadMarker, NSArray, NSRect, NSString, NSURL};
use objc2_web_kit::{
    WKAudiovisualMediaTypes, WKNavigationDelegate, WKScriptMessageHandler, WKUserContentController,
    WKUserScript, WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration,
};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    path::Path,
    time::{Duration, Instant},
};

fn file_url(path: &Path, directory: bool) -> Result<Retained<NSURL>, String> {
    let text = path.to_str().ok_or_else(|| {
        format!(
            "Path cannot be represented by a macOS URL: {}",
            path.display()
        )
    })?;
    Ok(NSURL::fileURLWithPath_isDirectory(
        &NSString::from_str(text),
        directory,
    ))
}

pub(super) struct VideoView {
    pub view: Retained<NSView>,
    player: Retained<AVQueuePlayer>,
    looper: Retained<AVPlayerLooper>,
    layer: Retained<AVPlayerLayer>,
}

impl VideoView {
    pub fn new(
        path: &Path,
        frame: NSRect,
        fill: bool,
        mtm: MainThreadMarker,
    ) -> Result<Self, String> {
        let url = file_url(path, false)?;
        // AVFoundation owns asynchronous decoding and all replicated loop items. The queue,
        // template/looper and layer are retained for the provider's full lifetime.
        unsafe {
            let item = AVPlayerItem::playerItemWithURL(&url, mtm);
            let player = AVQueuePlayer::queuePlayerWithItems(&NSArray::new(), mtm);
            let looper = AVPlayerLooper::playerLooperWithPlayer_templateItem(&player, &item);
            let layer = AVPlayerLayer::playerLayerWithPlayer(Some(&player));
            let gravity = if fill {
                AVLayerVideoGravityResizeAspectFill
            } else {
                AVLayerVideoGravityResizeAspect
            };
            layer.setVideoGravity(gravity.ok_or("AVFoundation video scaling is unavailable")?);
            let view = NSView::initWithFrame(NSView::alloc(mtm), frame);
            view.setWantsLayer(true);
            view.setLayer(Some(&layer));
            view.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            Ok(Self {
                view,
                player,
                looper,
                layer,
            })
        }
    }

    pub fn set_paused(&self, paused: bool) {
        unsafe {
            if paused {
                self.player.pause()
            } else {
                self.player.play()
            }
        }
    }

    pub fn set_volume(&self, volume: f32) {
        unsafe {
            self.player.setVolume(volume.clamp(0., 1.));
        }
    }
    pub fn set_muted(&self, muted: bool) {
        unsafe {
            self.player.setMuted(muted);
        }
    }

    pub fn media(&self, title: &str) -> Value {
        let mut value = wpe_platform::media::unavailable(true);
        unsafe {
            if let Some(item) = self.player.currentItem() {
                value["playback"] = json!({"state":if self.player.rate()==0.{2}else{1}});
                value["properties"]["title"] = json!(title);
                value["properties"]["contentType"] = json!("video");
                let position = self.player.currentTime().seconds();
                let duration = item.duration().seconds();
                value["timeline"] = json!({"position":if position.is_finite(){position.max(0.)}else{0.},"duration":if duration.is_finite(){duration.max(0.)}else{0.}});
            }
        }
        value
    }
    pub fn failure(&self) -> Option<String> {
        unsafe {
            if self.looper.status() == AVPlayerLooperStatus::Failed {
                return Some(
                    self.looper
                        .error()
                        .map(|e| e.localizedDescription().to_string())
                        .unwrap_or_else(|| "AVFoundation could not loop this video".into()),
                );
            }
            self.player
                .error()
                .map(|e| e.localizedDescription().to_string())
        }
    }
}

impl Drop for VideoView {
    fn drop(&mut self) {
        unsafe {
            self.player.pause();
            self.looper.disableLooping();
            self.layer.setPlayer(None);
            self.player.removeAllItems();
        }
    }
}

pub(super) struct WebView {
    pub view: Retained<WKWebView>,
    controller: Retained<WKUserContentController>,
    last_media: RefCell<Value>,
    last_refresh: RefCell<Instant>,
}

impl WebView {
    #[allow(clippy::too_many_arguments)] // Explicit file, delegates and initial playback policy.
    pub fn new(
        path: &Path,
        root: &Path,
        metadata: &Value,
        frame: NSRect,
        navigation: &ProtocolObject<dyn WKNavigationDelegate>,
        messages: &ProtocolObject<dyn WKScriptMessageHandler>,
        mtm: MainThreadMarker,
        initially_paused: bool,
        initially_muted: bool,
        initial_volume: f32,
    ) -> Result<Self, String> {
        let url = file_url(path, false)?;
        let root_url = file_url(root, true)?;
        let properties = metadata
            .get("general")
            .and_then(|v| v.get("properties"))
            .and_then(Value::as_object)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|(key, property)| {
                        property
                            .get("value")
                            .map(|value| (key.clone(), json!({"value":value})))
                    })
                    .collect::<serde_json::Map<String, Value>>()
            })
            .unwrap_or_default();
        let script = include_str!("web_bridge.js")
            .replace(
                "__INITIAL_PROPERTIES__",
                &Value::Object(properties).to_string(),
            )
            .replace(
                "__INITIAL_PAUSED__",
                if initially_paused { "true" } else { "false" },
            );
        let script = script
            .replace(
                "__INITIAL_MUTED__",
                if initially_muted { "true" } else { "false" },
            )
            .replace(
                "__INITIAL_VOLUME__",
                &initial_volume.clamp(0., 1.).to_string(),
            );
        unsafe {
            let configuration = WKWebViewConfiguration::new(mtm);
            configuration
                .setMediaTypesRequiringUserActionForPlayback(WKAudiovisualMediaTypes::None);
            let controller = WKUserContentController::new(mtm);
            controller.addScriptMessageHandler_name(messages, &NSString::from_str("wpeHost"));
            let user_script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                WKUserScript::alloc(mtm),
                &NSString::from_str(&script),
                WKUserScriptInjectionTime::AtDocumentStart,
                true,
            );
            controller.addUserScript(&user_script);
            configuration.setUserContentController(&controller);
            let view = WKWebView::initWithFrame_configuration(
                WKWebView::alloc(mtm),
                frame,
                &configuration,
            );
            view.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            view.setNavigationDelegate(Some(navigation));
            view.setAllMediaPlaybackSuspended_completionHandler(initially_paused, None);
            if view
                .loadFileURL_allowingReadAccessToURL(&url, &root_url)
                .is_none()
            {
                controller.removeScriptMessageHandlerForName(&NSString::from_str("wpeHost"));
                return Err("WebKit refused the local project URL".into());
            }
            Ok(Self {
                view,
                controller,
                last_media: RefCell::new(Value::Null),
                last_refresh: RefCell::new(Instant::now() - Duration::from_secs(2)),
            })
        }
    }

    pub fn refresh_initial(&self) {
        if self.last_refresh.borrow().elapsed() < Duration::from_secs(1) {
            return;
        }
        *self.last_refresh.borrow_mut() = Instant::now();
        self.evaluate("window.__wpeNativeHost?.applyInitial()");
        let media = self.last_media.borrow();
        if !media.is_null() {
            self.evaluate(&format!("window.__wpeNativeHost?.media({media})"));
        }
    }
    pub fn media(&self, value: &Value) {
        if *self.last_media.borrow() != *value {
            self.evaluate(&format!("window.__wpeNativeHost?.media({value})"));
            *self.last_media.borrow_mut() = value.clone();
        }
    }

    pub fn owns_controller(&self, controller: &WKUserContentController) -> bool {
        &*self.controller == controller
    }

    pub fn general(&self, fps: u32) {
        self.evaluate(&format!(
            "window.__wpeNativeHost?.updateGeneralProperties({{fps:{fps}}})"
        ));
    }
    pub fn set_volume(&self, volume: f32) {
        self.evaluate(&format!(
            "window.__wpeNativeHost?.setVolume({})",
            volume.clamp(0., 1.)
        ));
    }
    pub fn set_muted(&self, muted: bool) {
        self.evaluate(&format!("window.__wpeNativeHost?.setMuted({muted})"));
    }
    pub fn update_properties(&self, properties: &Value) {
        self.evaluate(&format!(
            "window.__wpeNativeHost?.updateProperties({properties})"
        ));
    }
    pub fn audio(&self, bands: &[f32]) {
        if let Ok(bands) = serde_json::to_string(bands) {
            self.evaluate(&format!("window.__wpeNativeHost?.audio({bands})"));
        }
    }
    pub fn input(&self, cursor: [f32; 2], buttons: u32, active: bool) {
        if cursor.iter().all(|v| v.is_finite()) {
            self.evaluate(&format!(
                "window.__wpeNativeHost?.input({},{},{buttons},{active})",
                cursor[0], cursor[1]
            ));
        }
    }
    fn evaluate(&self, code: &str) {
        unsafe {
            self.view
                .evaluateJavaScript_completionHandler(&NSString::from_str(code), None);
        }
    }

    pub fn set_paused(&self, paused: bool) {
        unsafe {
            self.view
                .setAllMediaPlaybackSuspended_completionHandler(paused, None);
            self.view.evaluateJavaScript_completionHandler(
                &NSString::from_str(if paused {
                    "window.__wpeNativeHost?.setPaused(true)"
                } else {
                    "window.__wpeNativeHost?.setPaused(false)"
                }),
                None,
            );
        }
    }
}

impl Drop for WebView {
    fn drop(&mut self) {
        unsafe {
            self.view
                .setAllMediaPlaybackSuspended_completionHandler(true, None);
            self.view.evaluateJavaScript_completionHandler(
                &NSString::from_str("window.__wpeNativeHost?.setPaused(true)"),
                None,
            );
            self.view.setNavigationDelegate(None);
            self.view.stopLoading();
            self.controller
                .removeScriptMessageHandlerForName(&NSString::from_str("wpeHost"));
        }
    }
}

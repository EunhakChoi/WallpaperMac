use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{MainThreadMarker, NSRect, NSString};
use objc2_web_kit::*;
pub(super) struct Management {
    pub view: Retained<WKWebView>,
    controller: Retained<WKUserContentController>,
}
impl Management {
    pub fn new(
        frame: NSRect,
        handler: &ProtocolObject<dyn WKScriptMessageHandler>,
        navigation: &ProtocolObject<dyn WKNavigationDelegate>,
        mtm: MainThreadMarker,
    ) -> Self {
        let controller = unsafe { WKUserContentController::new(mtm) };
        unsafe {
            controller.addScriptMessageHandler_name(handler, &NSString::from_str("wpeControl"));
        }
        let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
        unsafe {
            configuration.setUserContentController(&controller);
        }
        let view = unsafe {
            WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &configuration)
        };
        unsafe {
            view.setNavigationDelegate(Some(navigation));
            view.loadHTMLString_baseURL(&NSString::from_str(include_str!("management.html")), None);
        }
        Self { view, controller }
    }
    pub fn owns(&self, controller: &WKUserContentController, web: Option<&WKWebView>) -> bool {
        &*self.controller == controller && web.is_some_and(|web| &*self.view == web)
    }
    pub fn reply(&self, id: u64, result: Result<serde_json::Value, String>) {
        let (ok, value) = match result {
            Ok(v) => (true, v),
            Err(e) => (false, serde_json::json!({"message":e})),
        };
        if let Ok(value) = serde_json::to_string(&value) {
            let code = format!("window.__nativeReply({id},{ok},{value})");
            unsafe {
                self.view
                    .evaluateJavaScript_completionHandler(&NSString::from_str(&code), None);
            }
        }
    }
}
impl Drop for Management {
    fn drop(&mut self) {
        unsafe {
            self.view.setNavigationDelegate(None);
            self.view.stopLoading();
            self.controller
                .removeScriptMessageHandlerForName(&NSString::from_str("wpeControl"));
        }
    }
}

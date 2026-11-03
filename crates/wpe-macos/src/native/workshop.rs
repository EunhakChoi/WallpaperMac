//! Public Steam Web API transport. Subscription/download/upload remain in the
//! authenticated Steam client; public details never impersonate app credentials.
use super::*;
use dispatch2::DispatchQueue;
use objc2::ClassType;
use objc2_foundation::{
    NSData, NSHTTPURLResponse, NSMutableURLRequest, NSURLResponse, NSURLSession,
};
impl AppDelegate {
    pub(super) fn workshop_details(&self, id: u64, args: Value) {
        let response = (|| -> Result<wpe_platform::workshop::PublicRequest, String> {
            let ids = args["ids"]
                .as_array()
                .ok_or("Expected ids array")?
                .iter()
                .map(|v| {
                    v.as_u64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                        .ok_or_else(|| "Workshop ID must be an integer".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            wpe_platform::workshop::details_request(&ids).map_err(|e| e.to_string())
        })();
        let request = match response {
            Ok(request) => request,
            Err(error) => {
                self.reply(id, Err(error));
                return;
            }
        };
        let Some(url) = NSURL::URLWithString(&NSString::from_str(&request.url)) else {
            self.reply(id, Err("Invalid Steam API URL".into()));
            return;
        };
        let native = NSMutableURLRequest::requestWithURL(&url);
        native.setHTTPMethod(&NSString::from_str(&request.method));
        native.setTimeoutInterval(20.);
        native.setValue_forHTTPHeaderField(
            Some(&NSString::from_str(&request.content_type)),
            ns_string!("Content-Type"),
        );
        native.setHTTPBody(Some(&NSData::with_bytes(&request.body)));
        // Only a retained pointer is moved through the foreign completion. All access
        // to the main-thread delegate happens after DispatchQueue::main().
        let retained =
            unsafe { Retained::retain(self as *const Self as *mut Self) }.expect("live delegate");
        let address = Retained::into_raw(retained) as usize;
        let completion = block2::RcBlock::new(
            move |data: *mut NSData, response: *mut NSURLResponse, error: *mut NSError| {
                let result = (|| -> Result<Value, String> {
                    if let Some(error) = unsafe { error.as_ref() } {
                        return Err(error.localizedDescription().to_string());
                    }
                    let response =
                        unsafe { response.as_ref() }.ok_or("Steam API response is missing")?;
                    if !response.isKindOfClass(NSHTTPURLResponse::class()) {
                        return Err("Steam API response is not HTTP".into());
                    }
                    let status: isize = unsafe { msg_send![response, statusCode] };
                    if !(200..300).contains(&status) {
                        return Err(format!("Steam API HTTP{status}"));
                    }
                    let data = unsafe { data.as_ref() }.ok_or("Steam API body is missing")?;
                    if data.length() > 16 * 1024 * 1024 {
                        return Err("Steam API response exceeds16MiB".into());
                    }
                    let details = wpe_platform::workshop::parse_details(&data.to_vec())
                        .map_err(|e| e.to_string())?;
                    serde_json::to_value(details).map_err(|e| e.to_string())
                })();
                DispatchQueue::main().exec_async(move || {
                    let Some(delegate) =
                        (unsafe { Retained::<AppDelegate>::from_raw(address as *mut AppDelegate) })
                    else {
                        return;
                    };
                    delegate.reply(id, result);
                });
            },
        );
        let task = unsafe {
            NSURLSession::sharedSession()
                .dataTaskWithRequest_completionHandler(&native, &completion)
        };
        task.resume();
    }
    fn reply(&self, id: u64, result: Result<Value, String>) {
        if let Some(ui) = &self.ivars().borrow().management {
            ui.reply(id, result);
        }
    }
}

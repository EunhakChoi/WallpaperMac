//! Public SMAppService main-application login registration (macOS13+).
use objc2::rc::Retained;
use objc2::{extern_class, extern_methods};
use objc2_foundation::{NSError, NSObject};
use serde_json::{Value, json};
#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}
extern_class!(
    #[unsafe(super(NSObject))]
    #[derive(Debug, PartialEq, Eq, Hash)]
    struct SMAppService;
);
impl SMAppService {
    extern_methods!(
        #[unsafe(method(mainAppService))]
        #[unsafe(method_family=none)]
        fn main_app() -> Retained<Self>;
        #[unsafe(method(status))]
        #[unsafe(method_family=none)]
        fn status(&self) -> isize;
        #[unsafe(method(registerAndReturnError:_))]
        #[unsafe(method_family=none)]
        fn register(&self) -> Result<(), Retained<NSError>>;
        #[unsafe(method(unregisterAndReturnError:_))]
        #[unsafe(method_family=none)]
        fn unregister(&self) -> Result<(), Retained<NSError>>;
        #[unsafe(method(openSystemSettingsLoginItems))]
        #[unsafe(method_family=none)]
        fn open_settings();
    );
}
pub fn status() -> Value {
    let status = SMAppService::main_app().status();
    json!({"status":status,"enabled":status==1,"requires_approval":status==2,"description":match status{0=>"Not registered",1=>"Enabled",2=>"Requires approval in System Settings",3=>"Application service not found",_=>"Unknown ServiceManagement status"}})
}
pub fn set_enabled(enabled: bool) -> Result<Value, String> {
    let service = SMAppService::main_app();
    let status = service.status();
    if enabled && status != 1 && status != 2 {
        service
            .register()
            .map_err(|e| e.localizedDescription().to_string())?;
    } else if !enabled && (status == 1 || status == 2) {
        service
            .unregister()
            .map_err(|e| e.localizedDescription().to_string())?;
    }
    Ok(self::status())
}
pub fn open_settings() {
    SMAppService::open_settings();
}

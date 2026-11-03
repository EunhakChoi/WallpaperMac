//! Optional official Steamworks flat-API provider. No SDK binary is bundled.
//! ABI profile: generated UGC v020 / Utils v010, 64-bit Unix callback packing4.
//! `tools/verify-steam-abi.cpp` checks the profile against a supplied official SDK.
use crate::steam_apps::AppsApi;
pub use crate::steam_apps::DlcInfo;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CStr, CString, c_char, c_void},
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SteamConfig {
    pub library: PathBuf,
    pub app_id: u32,
}
#[derive(Clone, Debug, Serialize)]
pub struct InstalledItem {
    pub id: u64,
    pub folder: PathBuf,
    pub bytes: u64,
    pub updated: u32,
    pub state: u32,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ItemUpdate {
    pub id: u64,
    pub content: PathBuf,
    pub preview: Option<PathBuf>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub visibility: Option<i32>,
    pub change_note: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct SteamEvent {
    pub operation: u64,
    pub kind: String,
    pub ok: bool,
    pub value: Value,
}
#[repr(C)]
struct CallbackMsg {
    user: i32,
    callback: i32,
    data: *mut u8,
    bytes: i32,
}
#[repr(C)]
struct StringArray {
    strings: *const *const c_char,
    count: i32,
}
const _: () = assert!(
    std::mem::offset_of!(CallbackMsg, data) == 8 && std::mem::offset_of!(CallbackMsg, bytes) == 16
);
static ACTIVE: AtomicBool = AtomicBool::new(false);
struct ActiveGuard;
impl Drop for ActiveGuard {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::Release);
    }
}

#[cfg(unix)]
struct Module(*mut c_void);
#[cfg(unix)]
#[cfg_attr(not(target_os = "macos"), link(name = "dl"))]
unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    fn dlsym(module: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(module: *mut c_void) -> i32;
    fn dlerror() -> *const c_char;
}
#[cfg(unix)]
impl Module {
    fn open(path: &Path) -> Result<Self> {
        let path = path.canonicalize()?;
        let text = path
            .to_str()
            .ok_or_else(|| Error("Steam SDK library path is not UTF-8".into()))?;
        let path = CString::new(text).map_err(|_| Error("Steam SDK path contains NUL".into()))?;
        let pointer = unsafe { dlopen(path.as_ptr(), 2) };
        if pointer.is_null() {
            return Err(Error(format!(
                "Steam SDK could not load: {}",
                loader_error()
            )));
        }
        Ok(Self(pointer))
    }
    unsafe fn optional<T: Copy>(&self, name: &str) -> Option<T> {
        let name = CString::new(name).ok()?;
        let pointer = unsafe { dlsym(self.0, name.as_ptr()) };
        if pointer.is_null() {
            None
        } else {
            assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*mut c_void>());
            Some(unsafe { std::mem::transmute_copy(&pointer) })
        }
    }
    unsafe fn symbol<T: Copy>(&self, name: &str) -> Result<T> {
        unsafe { self.optional(name) }.ok_or_else(|| {
            Error(format!(
                "Configured Steam SDK lacks {name}; requires the pinned flat ABI profile"
            ))
        })
    }
}
#[cfg(unix)]
fn loader_error() -> String {
    let pointer = unsafe { dlerror() };
    if pointer.is_null() {
        "unknown loader error".into()
    } else {
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    }
}
#[cfg(unix)]
impl Drop for Module {
    fn drop(&mut self) {
        unsafe {
            dlclose(self.0);
        }
    }
}

macro_rules! functions{($( $field:ident:$ty:ty => $name:literal),*$(,)?)=>{
 struct Api{$($field:$ty),*}
 impl Api{#[cfg(unix)]unsafe fn load(module:&Module)->Result<Self>{Ok(Self{$($field:unsafe{module.symbol($name)?}),*})}}
}}
functions! {
 init:unsafe extern "C" fn()->bool=>"SteamAPI_Init",
 shutdown:unsafe extern "C" fn() -> () =>"SteamAPI_Shutdown",
 pipe:unsafe extern "C" fn()->i32=>"SteamAPI_GetHSteamPipe",
 manual_init:unsafe extern "C" fn() -> () =>"SteamAPI_ManualDispatch_Init",
 run:unsafe extern "C" fn(i32) -> () =>"SteamAPI_ManualDispatch_RunFrame",
 next:unsafe extern "C" fn(i32,*mut CallbackMsg)->bool=>"SteamAPI_ManualDispatch_GetNextCallback",
 free:unsafe extern "C" fn(i32) -> () =>"SteamAPI_ManualDispatch_FreeLastCallback",
 result:unsafe extern "C" fn(i32,u64,*mut c_void,i32,i32,*mut bool)->bool=>"SteamAPI_ManualDispatch_GetAPICallResult",
 ugc:unsafe extern "C" fn()->*mut c_void=>"SteamAPI_SteamUGC_v020",
 utils:unsafe extern "C" fn()->*mut c_void=>"SteamAPI_SteamUtils_v010",
 appid:unsafe extern "C" fn(*mut c_void)->u32=>"SteamAPI_ISteamUtils_GetAppID",
 details:unsafe extern "C" fn(*mut c_void,*mut u64,u32)->u64=>"SteamAPI_ISteamUGC_CreateQueryUGCDetailsRequest",
 all:unsafe extern "C" fn(*mut c_void,i32,i32,u32,u32,u32)->u64=>"SteamAPI_ISteamUGC_CreateQueryAllUGCRequestPage",
 send:unsafe extern "C" fn(*mut c_void,u64)->u64=>"SteamAPI_ISteamUGC_SendQueryUGCRequest",
 query_result:unsafe extern "C" fn(*mut c_void,u64,u32,*mut c_void)->bool=>"SteamAPI_ISteamUGC_GetQueryUGCResult",
 long_description:unsafe extern "C" fn(*mut c_void,u64,bool)->bool=>"SteamAPI_ISteamUGC_SetReturnLongDescription",
 release:unsafe extern "C" fn(*mut c_void,u64)->bool=>"SteamAPI_ISteamUGC_ReleaseQueryUGCRequest",
 num_subscribed:unsafe extern "C" fn(*mut c_void)->u32=>"SteamAPI_ISteamUGC_GetNumSubscribedItems",
 subscribed:unsafe extern "C" fn(*mut c_void,*mut u64,u32)->u32=>"SteamAPI_ISteamUGC_GetSubscribedItems",
 state:unsafe extern "C" fn(*mut c_void,u64)->u32=>"SteamAPI_ISteamUGC_GetItemState",
 install:unsafe extern "C" fn(*mut c_void,u64,*mut u64,*mut c_char,u32,*mut u32)->bool=>"SteamAPI_ISteamUGC_GetItemInstallInfo",
 download:unsafe extern "C" fn(*mut c_void,u64,bool)->bool=>"SteamAPI_ISteamUGC_DownloadItem",
 download_info:unsafe extern "C" fn(*mut c_void,u64,*mut u64,*mut u64)->bool=>"SteamAPI_ISteamUGC_GetItemDownloadInfo",
 subscribe:unsafe extern "C" fn(*mut c_void,u64)->u64=>"SteamAPI_ISteamUGC_SubscribeItem",
 unsubscribe:unsafe extern "C" fn(*mut c_void,u64)->u64=>"SteamAPI_ISteamUGC_UnsubscribeItem",
 create:unsafe extern "C" fn(*mut c_void,u32,i32)->u64=>"SteamAPI_ISteamUGC_CreateItem",
 start_update:unsafe extern "C" fn(*mut c_void,u32,u64)->u64=>"SteamAPI_ISteamUGC_StartItemUpdate",
 content:unsafe extern "C" fn(*mut c_void,u64,*const c_char)->bool=>"SteamAPI_ISteamUGC_SetItemContent",
 preview:unsafe extern "C" fn(*mut c_void,u64,*const c_char)->bool=>"SteamAPI_ISteamUGC_SetItemPreview",
 title:unsafe extern "C" fn(*mut c_void,u64,*const c_char)->bool=>"SteamAPI_ISteamUGC_SetItemTitle",
 description:unsafe extern "C" fn(*mut c_void,u64,*const c_char)->bool=>"SteamAPI_ISteamUGC_SetItemDescription",
 tags:unsafe extern "C" fn(*mut c_void,u64,*const StringArray)->bool=>"SteamAPI_ISteamUGC_SetItemTags",
 visibility:unsafe extern "C" fn(*mut c_void,u64,i32)->bool=>"SteamAPI_ISteamUGC_SetItemVisibility",
 submit:unsafe extern "C" fn(*mut c_void,u64,*const c_char)->u64=>"SteamAPI_ISteamUGC_SubmitItemUpdate",
 update_progress:unsafe extern "C" fn(*mut c_void,u64,*mut u64,*mut u64)->i32=>"SteamAPI_ISteamUGC_GetItemUpdateProgress",
}
#[derive(Clone, Debug)]
enum Pending {
    Query(u64),
    Subscribe(u64),
    Unsubscribe(u64),
    Delete(u64),
    Create,
    Update { item: u64, handle: u64 },
}
impl Pending {
    fn expected(&self) -> i32 {
        match self {
            Self::Query(_) => 3401,
            Self::Subscribe(_) => 1313,
            Self::Unsubscribe(_) => 1315,
            Self::Delete(_) => 3417,
            Self::Create => 3403,
            Self::Update { .. } => 3404,
        }
    }
    fn kind(&self) -> &str {
        match self {
            Self::Query(_) => "query",
            Self::Subscribe(_) => "subscribe",
            Self::Unsubscribe(_) => "unsubscribe",
            Self::Delete(_) => "delete",
            Self::Create => "create",
            Self::Update { .. } => "update",
        }
    }
}
pub struct SteamProvider {
    api: Api,
    ugc: *mut c_void,
    apps: Option<AppsApi>,
    stats: Option<StatsApi>,
    pipe: i32,
    app_id: u32,
    pending: BTreeMap<u64, Pending>,
    downloads: BTreeSet<u64>,
    #[cfg(unix)]
    _module: Module,
    _active: ActiveGuard,
    _main_thread: std::marker::PhantomData<Rc<()>>,
}
impl SteamProvider {
    pub fn load(config: &SteamConfig) -> Result<Self> {
        if config.app_id == 0 {
            return Err(Error(
                "Steam configuration requires a nonzero legitimate application ID".into(),
            ));
        }
        #[cfg(not(unix))]
        {
            let _ = config;
            return Err(Error(
                "This Steam dylib provider supports64-bit macOS/Unix hosts".into(),
            ));
        }
        #[cfg(unix)]
        {
            if ACTIVE
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return Err(Error(
                    "A Steamworks provider is already initialized in this process".into(),
                ));
            }
            let guard = ActiveGuard;
            let module = Module::open(&config.library)?;
            for version in 21..=32 {
                if unsafe {
                    module.optional::<unsafe extern "C" fn() -> *mut c_void>(&format!(
                        "SteamAPI_SteamUGC_v{version:03}"
                    ))
                }
                .is_some()
                {
                    return Err(Error("A newer generated Steam flat ABI is present; run the official-header ABI probe and add a matching profile before loading this library".into()));
                }
            }
            let api = unsafe { Api::load(&module) }?;
            if !unsafe { (api.init)() } {
                return Err(Error("SteamAPI_Init failed: Steam client/login/license/application environment or matching SDK is unavailable. The host does not rewrite global app-ID environment or source files.".into()));
            }
            let init = (|| -> Result<(*mut c_void, i32)> {
                let ugc = unsafe { (api.ugc)() };
                let utils = unsafe { (api.utils)() };
                if ugc.is_null() || utils.is_null() {
                    return Err(Error(
                        "Steam UGC_v020/Utils_v010 interface is unavailable".into(),
                    ));
                }
                let app_id = unsafe { (api.appid)(utils) };
                if app_id != config.app_id {
                    return Err(Error(format!(
                        "SDK authenticated AppID{app_id}, configured AppID{}; refusing cross-application operations",
                        config.app_id
                    )));
                }
                let pipe = unsafe { (api.pipe)() };
                if pipe == 0 {
                    return Err(Error("Steam client pipe is unavailable".into()));
                }
                unsafe {
                    (api.manual_init)();
                }
                Ok((ugc, pipe))
            })();
            let (ugc, pipe) = match init {
                Ok(v) => v,
                Err(e) => {
                    unsafe {
                        (api.shutdown)();
                    }
                    return Err(e);
                }
            };
            let apps = unsafe { load_apps(&module) };
            let stats = unsafe { load_stats(&module) };
            Ok(Self {
                api,
                ugc,
                apps,
                stats,
                pipe,
                app_id: config.app_id,
                pending: BTreeMap::new(),
                downloads: BTreeSet::new(),
                _module: module,
                _active: guard,
                _main_thread: std::marker::PhantomData,
            })
        }
    }
    /// The current authenticated user's published items, restricted to this app.
    pub fn query_published(&mut self, page: u32) -> Result<u64> {
        if page == 0 {
            return Err(Error("Steam query pages begin at1".into()));
        }
        #[cfg(unix)]
        unsafe {
            let user: unsafe extern "C" fn() -> *mut c_void =
                self._module.symbol("SteamAPI_SteamUser_v023")?;
            let steamid: unsafe extern "C" fn(*mut c_void) -> u64 =
                self._module.symbol("SteamAPI_ISteamUser_GetSteamID")?;
            let query: unsafe extern "C" fn(*mut c_void, u32, i32, i32, i32, u32, u32, u32) -> u64 =
                self._module
                    .symbol("SteamAPI_ISteamUGC_CreateQueryUserUGCRequest")?;
            let user = user();
            if user.is_null() {
                return Err(Error("Steam User_v023 is unavailable".into()));
            }
            let account = steamid(user) as u32;
            if account == 0 {
                return Err(Error("No authenticated Steam account".into()));
            }
            let handle = query(self.ugc, account, 0, -1, 0, self.app_id, self.app_id, page);
            self.query(handle)
        }
        #[cfg(not(unix))]
        {
            Err(Error("Steam dynamic provider requires Unix".into()))
        }
    }
    pub fn delete_item(&mut self, id: u64) -> Result<u64> {
        if id == 0 {
            return Err(Error("Positive Workshop item ID required".into()));
        }
        #[cfg(unix)]
        unsafe {
            let function: unsafe extern "C" fn(*mut c_void, u64) -> u64 =
                self._module.symbol("SteamAPI_ISteamUGC_DeleteItem")?;
            let call = function(self.ugc, id);
            self.operation(call, Pending::Delete(id))
        }
        #[cfg(not(unix))]
        {
            Err(Error("Steam dynamic provider requires Unix".into()))
        }
    }
    pub fn query_filtered(
        &mut self,
        page: u32,
        query_type: i32,
        matching_type: i32,
        text: Option<&str>,
        required: &[String],
        excluded: &[String],
    ) -> Result<u64> {
        if page == 0
            || required.len() + excluded.len() > 100
            || text.is_some_and(|v| v.len() > 4096)
        {
            return Err(Error("Invalid bounded Steam query".into()));
        }
        let handle = unsafe {
            (self.api.all)(
                self.ugc,
                query_type,
                matching_type,
                self.app_id,
                self.app_id,
                page,
            )
        };
        if handle == 0 || handle == u64::MAX {
            return Err(Error("Steam query creation failed".into()));
        }
        let result = (|| -> Result<()> {
            #[cfg(unix)]
            unsafe {
                if let Some(text) = text.filter(|s| !s.is_empty()) {
                    let text = c_string(text)?;
                    let set: unsafe extern "C" fn(*mut c_void, u64, *const c_char) -> bool =
                        self._module.symbol("SteamAPI_ISteamUGC_SetSearchText")?;
                    check(set(self.ugc, handle, text.as_ptr()), "query text")?;
                }
                for (tags, name) in [
                    (required, "SteamAPI_ISteamUGC_AddRequiredTag"),
                    (excluded, "SteamAPI_ISteamUGC_AddExcludedTag"),
                ] {
                    if !tags.is_empty() {
                        let set: unsafe extern "C" fn(*mut c_void, u64, *const c_char) -> bool =
                            self._module.symbol(name)?;
                        for tag in tags {
                            if tag.len() > 255 {
                                return Err(Error("Steam query tag exceeds255bytes".into()));
                            }
                            let tag = c_string(tag)?;
                            check(set(self.ugc, handle, tag.as_ptr()), "query tag")?;
                        }
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            unsafe {
                (self.api.release)(self.ugc, handle);
            }
            return Err(e);
        }
        self.query(handle)
    }
    pub fn unlock_publish_achievement(&mut self, kind: &str) -> Result<Value> {
        if self.app_id != 431960 {
            return Err(Error(
                "Original publication achievements are restricted to authenticated AppID431960"
                    .into(),
            ));
        }
        let name = match kind.to_ascii_lowercase().as_str() {
            "scene" => "POST_SCENE_WALLPAPER",
            "web" => "POST_WEB_WALLPAPER",
            "application" => "POST_APP_WALLPAPER",
            "video" => "POST_VIDEO_WALLPAPER",
            _ => return Err(Error("Unknown publication wallpaper type".into())),
        };
        self.stats
            .as_mut()
            .ok_or_else(|| {
                Error("Optional UserStats_v012/User_v023 flat APIs are unavailable".into())
            })?
            .request(name)?;
        Ok(json!({"pending":true,"achievement":name,"stored":false}))
    }
    pub fn achievement_status(&self) -> Value {
        self.stats.as_ref().map(|s|json!({"ready":s.ready,"loading":s.requested,"waiting":s.pending,"storing":s.storing})).unwrap_or(json!({"available":false}))
    }
    pub fn dlcs(&self) -> Result<Vec<DlcInfo>> {
        self.apps
            .as_ref()
            .ok_or_else(|| {
                Error(
                    "The configured SDK does not supply optional ISteamApps_v008/DLC flat APIs"
                        .into(),
                )
            })?
            .inventory()
    }
    pub fn dlc(&self, id: u32) -> Result<DlcInfo> {
        self.apps
            .as_ref()
            .ok_or_else(|| Error("Steam Apps/DLC interface unavailable".into()))?
            .lookup(id)
    }
    /// A successful call requests Steam client work; it does not prove installation.
    pub fn install_dlc(&self, id: u32, install: bool) -> Result<DlcInfo> {
        self.apps
            .as_ref()
            .ok_or_else(|| Error("Steam Apps/DLC interface unavailable".into()))?
            .set_installed(id, install)
    }
    fn operation(&mut self, call: u64, pending: Pending) -> Result<u64> {
        if call == 0 {
            return Err(Error(format!(
                "Steam {} request returned an invalid API call",
                pending.kind()
            )));
        }
        if self.pending.contains_key(&call) {
            return Err(Error(
                "Steam returned a duplicate pending call handle".into(),
            ));
        }
        self.pending.insert(call, pending);
        Ok(call)
    }
    fn query(&mut self, handle: u64) -> Result<u64> {
        if handle == u64::MAX {
            return Err(Error("Steam query creation failed".into()));
        }
        unsafe {
            (self.api.long_description)(self.ugc, handle, true);
        }
        let call = unsafe { (self.api.send)(self.ugc, handle) };
        match self.operation(call, Pending::Query(handle)) {
            Ok(call) => Ok(call),
            Err(e) => {
                unsafe {
                    (self.api.release)(self.ugc, handle);
                }
                Err(e)
            }
        }
    }
    pub fn query_details(&mut self, ids: &[u64]) -> Result<u64> {
        if ids.is_empty() || ids.len() > 100 || ids.contains(&0) {
            return Err(Error(
                "Steam details request requires1..100 positive item IDs".into(),
            ));
        }
        let mut ids = ids.to_vec();
        let query = unsafe { (self.api.details)(self.ugc, ids.as_mut_ptr(), ids.len() as u32) };
        self.query(query)
    }
    pub fn query_page(&mut self, page: u32, query_type: i32, matching_type: i32) -> Result<u64> {
        if page == 0 {
            return Err(Error("Steam query pages begin at1".into()));
        }
        let query = unsafe {
            (self.api.all)(
                self.ugc,
                query_type,
                matching_type,
                self.app_id,
                self.app_id,
                page,
            )
        };
        self.query(query)
    }
    pub fn subscribed(&self) -> Result<Vec<u64>> {
        let count = unsafe { (self.api.num_subscribed)(self.ugc) } as usize;
        if count > 100000 {
            return Err(Error("Steam subscription inventory exceeds100000".into()));
        }
        let mut items = vec![0; count];
        let returned =
            unsafe { (self.api.subscribed)(self.ugc, items.as_mut_ptr(), count as u32) } as usize;
        if returned > items.len() {
            return Err(Error(
                "Steam returned more subscriptions than its buffer".into(),
            ));
        }
        items.truncate(returned);
        Ok(items)
    }
    pub fn install_info(&self, id: u64) -> Result<Option<InstalledItem>> {
        if id == 0 {
            return Err(Error("Item ID must be positive".into()));
        }
        let mut bytes = 0;
        let mut updated = 0;
        let mut folder = vec![0i8; 32768];
        let ok = unsafe {
            (self.api.install)(
                self.ugc,
                id,
                &mut bytes,
                folder.as_mut_ptr(),
                folder.len() as u32,
                &mut updated,
            )
        };
        if !ok {
            return Ok(None);
        }
        let folder = c_text(unsafe {
            std::slice::from_raw_parts(folder.as_ptr().cast::<u8>(), folder.len())
        });
        Ok(Some(InstalledItem {
            id,
            folder: folder.into(),
            bytes,
            updated,
            state: unsafe { (self.api.state)(self.ugc, id) },
        }))
    }
    pub fn download(&mut self, id: u64, high_priority: bool) -> Result<()> {
        if id == 0 || !unsafe { (self.api.download)(self.ugc, id, high_priority) } {
            return Err(Error("Steam refused the download request".into()));
        }
        self.downloads.insert(id);
        Ok(())
    }
    pub fn download_progress(&self, id: u64) -> Option<(u64, u64)> {
        let (mut downloaded, mut total) = (0, 0);
        if unsafe { (self.api.download_info)(self.ugc, id, &mut downloaded, &mut total) } {
            Some((downloaded, total))
        } else {
            None
        }
    }
    pub fn subscribe(&mut self, id: u64, subscribe: bool) -> Result<u64> {
        if id == 0 {
            return Err(Error("Item ID must be positive".into()));
        }
        let call = unsafe {
            if subscribe {
                (self.api.subscribe)(self.ugc, id)
            } else {
                (self.api.unsubscribe)(self.ugc, id)
            }
        };
        self.operation(
            call,
            if subscribe {
                Pending::Subscribe(id)
            } else {
                Pending::Unsubscribe(id)
            },
        )
    }
    pub fn create(&mut self) -> Result<u64> {
        let call = unsafe { (self.api.create)(self.ugc, self.app_id, 0) };
        self.operation(call, Pending::Create)
    }
    pub fn update(&mut self, update: &ItemUpdate) -> Result<u64> {
        if update.id == 0 || !update.content.is_dir() {
            return Err(Error(
                "Update requires an existing item ID and content directory".into(),
            ));
        }
        if update.tags.len() > 100 {
            return Err(Error("At most100 update tags are supported".into()));
        }
        let handle = unsafe { (self.api.start_update)(self.ugc, self.app_id, update.id) };
        if handle == u64::MAX {
            return Err(Error("Steam update handle is invalid".into()));
        }
        let content = c_path(&update.content)?;
        check(
            unsafe { (self.api.content)(self.ugc, handle, content.as_ptr()) },
            "content",
        )?;
        for (value, set, name) in [
            (&update.title, self.api.title, "title"),
            (&update.description, self.api.description, "description"),
        ] {
            if let Some(value) = value {
                let value = c_string(value)?;
                check(unsafe { set(self.ugc, handle, value.as_ptr()) }, name)?;
            }
        }
        if let Some(preview) = &update.preview {
            if !preview.is_file() {
                return Err(Error("Preview file does not exist".into()));
            }
            let preview = c_path(preview)?;
            check(
                unsafe { (self.api.preview)(self.ugc, handle, preview.as_ptr()) },
                "preview",
            )?;
        }
        let strings = update
            .tags
            .iter()
            .map(|s| c_string(s))
            .collect::<Result<Vec<_>>>()?;
        let pointers = strings.iter().map(|s| s.as_ptr()).collect::<Vec<_>>();
        let tags = StringArray {
            strings: pointers.as_ptr(),
            count: pointers.len() as i32,
        };
        check(unsafe { (self.api.tags)(self.ugc, handle, &tags) }, "tags")?;
        if let Some(v) = update.visibility {
            if !(0..=3).contains(&v) {
                return Err(Error("Visibility must be0..3".into()));
            }
            check(
                unsafe { (self.api.visibility)(self.ugc, handle, v) },
                "visibility",
            )?;
        }
        let note = c_string(&update.change_note)?;
        let call = unsafe { (self.api.submit)(self.ugc, handle, note.as_ptr()) };
        self.operation(
            call,
            Pending::Update {
                item: update.id,
                handle,
            },
        )
    }
    pub fn pending_progress(&self) -> Value {
        json!(self.pending.iter().filter_map(|(call,p)|if let Pending::Update{item,handle}=p{let(mut done,mut total)=(0,0);let phase=unsafe{(self.api.update_progress)(self.ugc,*handle,&mut done,&mut total)};Some(json!({"operation":call,"item":item,"phase":phase,"done":done,"total":total}))}else{None}).collect::<Vec<_>>())
    }
    pub fn pump(&mut self) -> Vec<SteamEvent> {
        unsafe {
            (self.api.run)(self.pipe);
        }
        let mut events = Vec::new();
        for _ in 0..1024 {
            let mut message = CallbackMsg {
                user: 0,
                callback: 0,
                data: std::ptr::null_mut(),
                bytes: 0,
            };
            if !unsafe { (self.api.next)(self.pipe, &mut message) } {
                break;
            }
            let _release = CallbackRelease {
                free: self.api.free,
                pipe: self.pipe,
            };
            if message.bytes < 0 || message.bytes > 1024 * 1024 || message.data.is_null() {
                events.push(event(
                    0,
                    "protocol",
                    false,
                    json!({"error":"Invalid Steam callback buffer"}),
                ));
                continue;
            }
            let bytes = unsafe { std::slice::from_raw_parts(message.data, message.bytes as usize) };
            if message.callback == 703 && bytes.len() >= 16 {
                let call = u64_at(bytes, 0);
                let callback = i32_at(bytes, 8);
                let size = u32_at(bytes, 12) as usize;
                let Some(pending) = self.pending.remove(&call) else {
                    continue;
                };
                let result = (|| -> Result<Value> {
                    if callback != pending.expected() || size == 0 || size > 1024 * 1024 {
                        return Err(Error("Steam API call-result type/size mismatch".into()));
                    }
                    let mut payload = vec![0u64; size.div_ceil(8)];
                    let mut failed = false;
                    if !unsafe {
                        (self.api.result)(
                            self.pipe,
                            call,
                            payload.as_mut_ptr().cast(),
                            size as i32,
                            callback,
                            &mut failed,
                        )
                    } || failed
                    {
                        return Err(Error("Steam API call-result retrieval/I/O failed".into()));
                    }
                    let bytes =
                        unsafe { std::slice::from_raw_parts(payload.as_ptr().cast::<u8>(), size) };
                    self.finish(&pending, bytes)
                })();
                if let Pending::Query(handle) = &pending {
                    unsafe {
                        (self.api.release)(self.ugc, *handle);
                    }
                }
                events.push(match result {
                    Ok(v) => event(call, pending.kind(), true, v),
                    Err(e) => event(call, pending.kind(), false, json!({"error":e.to_string()})),
                });
            } else if matches!(message.callback, 1101 | 1102) {
                if let Some(stats) = &mut self.stats {
                    events.extend(stats.callback(message.callback, bytes, self.app_id));
                }
            } else if message.callback == 3406 && bytes.len() >= 16 {
                let app = u32_at(bytes, 0);
                let id = u64_at(bytes, 4);
                if app == self.app_id && self.downloads.remove(&id) {
                    let result = i32_at(bytes, 12);
                    events.push(event(id,"download",result==1,json!({"id":id,"result":result,"install":self.install_info(id).ok().flatten()})));
                }
            }
        }
        events
    }
    fn finish(&self, pending: &Pending, bytes: &[u8]) -> Result<Value> {
        match pending {
            Pending::Query(handle) => {
                if bytes.len() < 21 || u64_at(bytes, 0) != *handle {
                    return Err(Error("Steam query completion prefix is invalid".into()));
                }
                let result = i32_at(bytes, 8);
                if result != 1 {
                    return Err(Error(format!("Steam query EResult{result}")));
                }
                let count = u32_at(bytes, 12);
                if count > 100 {
                    return Err(Error("Steam query result count exceeds100".into()));
                }
                let mut rows = vec![];
                for index in 0..count {
                    let mut raw = vec![0u64; 4096];
                    if !unsafe {
                        (self.api.query_result)(self.ugc, *handle, index, raw.as_mut_ptr().cast())
                    } {
                        return Err(Error("GetQueryUGCResult failed".into()));
                    }
                    let b = unsafe {
                        std::slice::from_raw_parts(raw.as_ptr().cast::<u8>(), raw.len() * 8)
                    };
                    if i32_at(b, 8) == 1 && u32_at(b, 20) == self.app_id {
                        rows.push(json!({"id":u64_at(b,0),"title":c_text(&b[24..153]),"description":c_text(&b[153..8153]),"creator_app_id":u32_at(b,16),"consumer_app_id":u32_at(b,20)}));
                    }
                }
                Ok(json!({"items":rows,"total":u32_at(bytes,16),"cached":bytes[20]!=0}))
            }
            Pending::Subscribe(id) | Pending::Unsubscribe(id) | Pending::Delete(id) => {
                if bytes.len() < 12 || u64_at(bytes, 4) != *id {
                    return Err(Error(
                        "Steam subscription result does not match its item".into(),
                    ));
                }
                result_ok(i32_at(bytes, 0))?;
                Ok(json!({"id":id}))
            }
            Pending::Create => {
                if bytes.len() < 13 {
                    return Err(Error("Truncated Steam create result".into()));
                }
                result_ok(i32_at(bytes, 0))?;
                Ok(json!({"id":u64_at(bytes,4),"needs_workshop_agreement":bytes[12]!=0}))
            }
            Pending::Update { item, .. } => {
                if bytes.len() < 5 {
                    return Err(Error("Truncated Steam update result".into()));
                }
                result_ok(i32_at(bytes, 0))?;
                Ok(json!({"id":item,"needs_workshop_agreement":bytes[4]!=0}))
            }
        }
    }
}
impl Drop for SteamProvider {
    fn drop(&mut self) {
        for pending in self.pending.values() {
            if let Pending::Query(handle) = pending {
                unsafe {
                    (self.api.release)(self.ugc, *handle);
                }
            }
        }
        self.pending.clear();
        unsafe {
            (self.api.shutdown)();
        }
    }
}
struct CallbackRelease {
    free: unsafe extern "C" fn(i32),
    pipe: i32,
}
impl Drop for CallbackRelease {
    fn drop(&mut self) {
        unsafe {
            (self.free)(self.pipe);
        }
    }
}
fn event(operation: u64, kind: &str, ok: bool, value: Value) -> SteamEvent {
    SteamEvent {
        operation,
        kind: kind.into(),
        ok,
        value,
    }
}
fn result_ok(result: i32) -> Result<()> {
    if result == 1 {
        Ok(())
    } else {
        Err(Error(format!("Steam operation EResult{result}")))
    }
}
fn check(ok: bool, name: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error(format!("Steam rejected update {name}")))
    }
}
fn c_string(value: &str) -> Result<CString> {
    CString::new(value).map_err(|_| Error("Steam string contains NUL".into()))
}
fn c_path(path: &Path) -> Result<CString> {
    c_string(
        path.canonicalize()?
            .to_str()
            .ok_or_else(|| Error("Steam path is not UTF-8".into()))?,
    )
}
fn c_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len())])
        .into_owned()
}
fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn i32_at(bytes: &[u8], offset: usize) -> i32 {
    u32_at(bytes, offset) as i32
}

#[cfg(unix)]
unsafe fn load_apps(module: &Module) -> Option<AppsApi> {
    unsafe {
        let interface =
            module.optional::<unsafe extern "C" fn() -> *mut c_void>("SteamAPI_SteamApps_v008")?();
        if interface.is_null() {
            return None;
        }
        Some(AppsApi {
            interface,
            count: module.optional("SteamAPI_ISteamApps_GetDLCCount")?,
            data: module.optional("SteamAPI_ISteamApps_BGetDLCDataByIndex")?,
            installed: module.optional("SteamAPI_ISteamApps_BIsDlcInstalled")?,
            install: module.optional("SteamAPI_ISteamApps_InstallDLC")?,
            uninstall: module.optional("SteamAPI_ISteamApps_UninstallDLC")?,
            progress: module.optional("SteamAPI_ISteamApps_GetDlcDownloadProgress")?,
        })
    }
}

// Optional achievement APIs: only the original application's four recovered
// publication identifiers are requested. Schema membership is checked before set.
struct StatsApi {
    pointer: *mut c_void,
    user_id: u64,
    request: unsafe extern "C" fn(*mut c_void) -> bool,
    count: unsafe extern "C" fn(*mut c_void) -> u32,
    name: unsafe extern "C" fn(*mut c_void, u32) -> *const c_char,
    set: unsafe extern "C" fn(*mut c_void, *const c_char) -> bool,
    store: unsafe extern "C" fn(*mut c_void) -> bool,
    ready: bool,
    requested: bool,
    pending: BTreeSet<String>,
    storing: BTreeSet<String>,
}
impl StatsApi {
    fn request(&mut self, name: &str) -> Result<()> {
        self.pending.insert(name.into());
        if !self.ready {
            if !self.requested {
                if !unsafe { (self.request)(self.pointer) } {
                    self.pending.remove(name);
                    return Err(Error("Steam RequestCurrentStats was rejected".into()));
                }
                self.requested = true;
            }
            return Ok(());
        }
        self.flush()
    }
    fn flush(&mut self) -> Result<()> {
        if !self.ready || !self.storing.is_empty() || self.pending.is_empty() {
            return Ok(());
        }
        let count = unsafe { (self.count)(self.pointer) };
        if count > 10000 {
            return Err(Error(
                "Steam achievement schema exceeds10000 entries".into(),
            ));
        }
        let mut schema = BTreeSet::<String>::new();
        for index in 0..count {
            let pointer = unsafe { (self.name)(self.pointer, index) };
            if !pointer.is_null() {
                schema.insert(
                    unsafe { CStr::from_ptr(pointer) }
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
        let names = std::mem::take(&mut self.pending);
        for name in &names {
            if !schema.contains(name) {
                return Err(Error(format!(
                    "Authenticated Steam app schema does not define {name}"
                )));
            }
        }
        for name in &names {
            let name = c_string(name)?;
            if !unsafe { (self.set)(self.pointer, name.as_ptr()) } {
                return Err(Error(
                    "Steam SetAchievement rejected an identifier; no store was confirmed".into(),
                ));
            }
        }
        if !unsafe { (self.store)(self.pointer) } {
            return Err(Error(
                "Steam StoreStats rejected the update; server completion is unconfirmed".into(),
            ));
        }
        self.storing = names;
        Ok(())
    }
    fn callback(&mut self, id: i32, bytes: &[u8], app: u32) -> Vec<SteamEvent> {
        let mut events = vec![];
        if bytes.len() < 12 || u64_at(bytes, 0) != app as u64 {
            return events;
        }
        let result = i32_at(bytes, 8);
        if id == 1101 {
            if bytes.len() < 20 || u64_at(bytes, 12) != self.user_id {
                return events;
            }
            self.requested = false;
            self.ready = result == 1;
            if !self.ready {
                for name in std::mem::take(&mut self.pending) {
                    events.push(event(
                        0,
                        "achievement",
                        false,
                        json!({"achievement":name,"result":result,"stored":false}),
                    ));
                }
                return events;
            }
        } else if id == 1102 {
            for name in std::mem::take(&mut self.storing) {
                events.push(event(
                    0,
                    "achievement",
                    result == 1,
                    json!({"achievement":name,"result":result,"stored":result==1}),
                ));
            }
            if result != 1 {
                self.ready = false;
            }
        }
        let pending = self.pending.clone();
        if let Err(error) = self.flush() {
            self.pending.clear();
            for name in pending {
                events.push(event(
                    0,
                    "achievement",
                    false,
                    json!({"achievement":name,"error":error.to_string(),"stored":false}),
                ));
            }
        }
        events
    }
}
#[cfg(unix)]
unsafe fn load_stats(module: &Module) -> Option<StatsApi> {
    unsafe {
        let interface: unsafe extern "C" fn() -> *mut c_void =
            module.optional("SteamAPI_SteamUserStats_v012")?;
        let pointer = interface();
        if pointer.is_null() {
            return None;
        }
        let user: unsafe extern "C" fn() -> *mut c_void =
            module.optional("SteamAPI_SteamUser_v023")?;
        let steamid: unsafe extern "C" fn(*mut c_void) -> u64 =
            module.optional("SteamAPI_ISteamUser_GetSteamID")?;
        let user = user();
        if user.is_null() {
            return None;
        }
        let user_id = steamid(user);
        if user_id == 0 {
            return None;
        }
        Some(StatsApi {
            pointer,
            user_id,
            request: module.optional("SteamAPI_ISteamUserStats_RequestCurrentStats")?,
            count: module.optional("SteamAPI_ISteamUserStats_GetNumAchievements")?,
            name: module.optional("SteamAPI_ISteamUserStats_GetAchievementName")?,
            set: module.optional("SteamAPI_ISteamUserStats_SetAchievement")?,
            store: module.optional("SteamAPI_ISteamUserStats_StoreStats")?,
            ready: false,
            requested: false,
            pending: BTreeSet::new(),
            storing: BTreeSet::new(),
        })
    }
}

//! Optional ISteamApps_v008 flat functions, loaded from the same authenticated
//! official Steamworks module as UGC. No SDK binary is redistributed.
use crate::{Error, Result};
use serde::Serialize;
use std::ffi::{c_char, c_void};
pub(super) struct AppsApi {
    pub interface: *mut c_void,
    pub count: unsafe extern "C" fn(*mut c_void) -> i32,
    pub data: unsafe extern "C" fn(*mut c_void, i32, *mut u32, *mut bool, *mut c_char, i32) -> bool,
    pub installed: unsafe extern "C" fn(*mut c_void, u32) -> bool,
    pub install: unsafe extern "C" fn(*mut c_void, u32),
    pub uninstall: unsafe extern "C" fn(*mut c_void, u32),
    pub progress: unsafe extern "C" fn(*mut c_void, u32, *mut u64, *mut u64) -> bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct DlcInfo {
    pub id: u32,
    pub name: String,
    pub available: bool,
    pub installed: bool,
    pub progress: Option<(u64, u64)>,
}
impl AppsApi {
    pub fn inventory(&self) -> Result<Vec<DlcInfo>> {
        let count = unsafe { (self.count)(self.interface) };
        if !(0..=4096).contains(&count) {
            return Err(Error("Invalid Steam DLC count".into()));
        }
        let mut result = vec![];
        for index in 0..count {
            let (mut id, mut available) = (0, false);
            let mut name = [0i8; 1024];
            if !unsafe {
                (self.data)(
                    self.interface,
                    index,
                    &mut id,
                    &mut available,
                    name.as_mut_ptr(),
                    name.len() as i32,
                )
            } {
                return Err(Error(format!("Steam DLC index{index} lookup failed")));
            }
            if id == 0 {
                return Err(Error("Steam DLC entry has zero AppID".into()));
            }
            let bytes = name.map(|c| c as u8);
            let name = String::from_utf8_lossy(
                &bytes[..bytes
                    .iter()
                    .position(|b| *b == 0)
                    .ok_or_else(|| Error("Unterminated Steam DLC name".into()))?],
            )
            .into_owned();
            let (mut done, mut total) = (0, 0);
            let progress = unsafe { (self.progress)(self.interface, id, &mut done, &mut total) }
                .then_some((done, total));
            result.push(DlcInfo {
                id,
                name,
                available,
                installed: unsafe { (self.installed)(self.interface, id) },
                progress,
            });
        }
        Ok(result)
    }
    pub fn lookup(&self, id: u32) -> Result<DlcInfo> {
        self.inventory()?
            .into_iter()
            .find(|item| item.id == id)
            .ok_or_else(|| {
                Error(format!(
                    "DLC{id} is not enumerated for the authenticated parent application"
                ))
            })
    }
    pub fn set_installed(&self, id: u32, install: bool) -> Result<DlcInfo> {
        self.lookup(id)?;
        unsafe {
            if install {
                (self.install)(self.interface, id)
            } else {
                (self.uninstall)(self.interface, id)
            }
        }
        self.lookup(id)
    }
}

//! Public macOS display/window/power/input facts. No private Spaces or MediaRemote API.
use objc2::msg_send;
use objc2_app_kit::*;
use objc2_core_foundation::{
    CFDictionary, CFNumber, CFNumberType, CFRetained, CFString, CFType, CFUUID,
};
use objc2_core_graphics::*;
use objc2_foundation::{MainThreadMarker, NSCalendar, NSCalendarUnit, NSDate, NSRect, NSString};
use serde_json::{Value, json};
use std::{collections::BTreeMap, ffi::c_void, ptr::NonNull};
use wpe_platform::playback::{ApplicationFact, Calendar, PlaybackFacts};

#[link(name = "ColorSync", kind = "framework")]
unsafe extern "C-unwind" {
    fn CGDisplayCreateUUIDFromDisplayID(id: u32) -> Option<NonNull<CFUUID>>;
}
#[link(name = "IOKit", kind = "framework")]
unsafe extern "C-unwind" {
    fn IOPSCopyPowerSourcesInfo() -> Option<NonNull<CFType>>;
    fn IOPSGetProvidingPowerSourceType(info: &CFType) -> *const CFString;
}
#[derive(Clone, Debug)]
pub struct Display {
    pub id: String,
    pub name: String,
    pub index: usize,
    pub rect: NSRect,
    pub cg: [f64; 4],
    pub mask: u32,
}
impl Display {
    pub fn json(&self) -> Value {
        json!({"id":self.id,"name":self.name,"index":self.index,"x":self.rect.origin.x,"y":self.rect.origin.y,"width":self.rect.size.width,"height":self.rect.size.height})
    }
}
pub fn displays(mtm: MainThreadMarker) -> Vec<Display> {
    let screens = NSScreen::screens(mtm);
    let mut output = Vec::new();
    for (index, screen) in screens.iter().take(32).enumerate() {
        let number = screen
            .deviceDescription()
            .objectForKey(&NSString::from_str("NSScreenNumber"));
        let Some(number) = number else {
            continue;
        };
        let id: u32 = unsafe { msg_send![&*number, unsignedIntValue] };
        let uuid = unsafe { CGDisplayCreateUUIDFromDisplayID(id) }
            .map(|uuid| unsafe { CFRetained::from_raw(uuid) });
        let identifier = uuid
            .and_then(|uuid| CFUUID::new_string(None, Some(&uuid)))
            .map(|uuid| uuid.to_string())
            .unwrap_or_else(|| format!("display-{id}"));
        let bounds = CGDisplayBounds(id);
        output.push(Display {
            id: identifier,
            name: screen.localizedName().to_string(),
            index,
            rect: screen.frame(),
            cg: [
                bounds.origin.x,
                bounds.origin.y,
                bounds.size.width,
                bounds.size.height,
            ],
            mask: 1u32 << index,
        });
    }
    output
}
pub fn calendar() -> (Calendar, f32) {
    let now = NSDate::date();
    let c = NSCalendar::currentCalendar().components_fromDate(
        NSCalendarUnit::Hour
            | NSCalendarUnit::Minute
            | NSCalendarUnit::Second
            | NSCalendarUnit::Weekday,
        &now,
    );
    let hour = c.hour().clamp(0, 23) as u16;
    let minute = c.minute().clamp(0, 59) as u16;
    let second = c.second().clamp(0, 59) as u16;
    (
        Calendar {
            minute: hour * 60 + minute,
            weekday: (c.weekday().clamp(1, 7) - 1) as u8,
            day: (NSCalendar::currentCalendar()
                .startOfDayForDate(&now)
                .timeIntervalSince1970()
                .max(0.)
                / 86400.) as u64,
        },
        ((hour as f32 * 3600.) + minute as f32 * 60. + second as f32) / 86400.,
    )
}
pub fn on_battery() -> bool {
    let Some(info) = (unsafe { IOPSCopyPowerSourcesInfo() }) else {
        return false;
    };
    let info = unsafe { CFRetained::from_raw(info) };
    unsafe { IOPSGetProvidingPowerSourceType(&info).as_ref() }
        .is_some_and(|s| s.to_string() == "Battery Power")
}
unsafe fn number(dict: &CFDictionary, key: &CFString) -> Option<f64> {
    let value = unsafe { dict.value((key as *const CFString).cast()) };
    let number = unsafe { value.cast::<CFNumber>().as_ref() }?;
    let mut result = 0.0f64;
    if unsafe {
        number.value(
            CFNumberType::Float64Type,
            (&mut result as *mut f64).cast::<c_void>(),
        )
    } {
        Some(result)
    } else {
        None
    }
}
unsafe fn field<'a>(dict: &'a CFDictionary, key: &CFString) -> Option<&'a CFDictionary> {
    unsafe {
        dict.value((key as *const CFString).cast())
            .cast::<CFDictionary>()
            .as_ref()
    }
}
/// Fullscreen is exact bounds coverage; maximized is visible-work-area coverage.
/// macOS does not expose Windows IsZoomed/Cloak/session-peak equivalence.
pub fn playback(displays: &[Display], sleeping: bool, audio_active: bool) -> PlaybackFacts {
    let workspace = NSWorkspace::sharedWorkspace();
    let front = workspace
        .frontmostApplication()
        .map(|app| app.processIdentifier());
    let own = std::process::id() as i32;
    let mut apps = BTreeMap::<i32, ApplicationFact>::new();
    for app in workspace.runningApplications() {
        let pid = app.processIdentifier();
        if pid == own {
            continue;
        }
        let executable = app
            .executableURL()
            .and_then(|url| url.path())
            .map(|p| p.to_string())
            .or_else(|| app.bundleIdentifier().map(|p| p.to_string()))
            .unwrap_or_else(|| format!("pid-{pid}"));
        apps.insert(
            pid,
            ApplicationFact {
                executable,
                ..ApplicationFact::default()
            },
        );
    }
    let mut facts = PlaybackFacts {
        active_displays: displays.iter().fold(0, |mask, d| mask | d.mask),
        on_battery: on_battery(),
        sleeping,
        audio_active,
        ..Default::default()
    };
    let windows = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        0,
    );
    if let Some(windows) = windows {
        for i in 0..windows.count() {
            let Some(dict) = (unsafe { windows.value_at_index(i).cast::<CFDictionary>().as_ref() })
            else {
                continue;
            };
            let Some(pid) = (unsafe { number(dict, kCGWindowOwnerPID) }) else {
                continue;
            };
            if pid as i32 == own || unsafe { number(dict, kCGWindowLayer) }.unwrap_or(1.) != 0. {
                continue;
            }
            let Some(bounds) = (unsafe { field(dict, kCGWindowBounds) }) else {
                continue;
            };
            let keys = ["X", "Y", "Width", "Height"].map(CFString::from_str);
            let mut r = [0.; 4];
            let mut valid = true;
            for (j, key) in keys.iter().enumerate() {
                if let Some(n) = unsafe { number(bounds, key) } {
                    r[j] = n;
                } else {
                    valid = false;
                }
            }
            if !valid || r[2] <= 1. || r[3] <= 1. {
                continue;
            }
            let Some(app) = apps.get_mut(&(pid as i32)) else {
                continue;
            };
            for d in displays {
                let [x, y, w, h] = d.cg;
                let intersection = (r[0] + r[2]).min(x + w) - r[0].max(x);
                let vertical = (r[1] + r[3]).min(y + h) - r[1].max(y);
                if intersection <= 0. || vertical <= 0. {
                    continue;
                }
                if Some(pid as i32) == front {
                    app.focused |= d.mask;
                    facts.focused |= d.mask;
                }
                if (r[0] - x).abs() <= 2.
                    && (r[1] - y).abs() <= 2.
                    && (r[2] - w).abs() <= 2.
                    && (r[3] - h).abs() <= 2.
                {
                    app.fullscreen |= d.mask;
                    facts.fullscreen |= d.mask;
                }
                // AppKit's visibleFrame excludes menu bar/dock. Convert Cocoa y-up to CG y-down.
                let screens = NSScreen::screens(
                    MainThreadMarker::new().expect("window facts run on main thread"),
                );
                let screen = screens.objectAtIndex(d.index);
                let visible = screen.visibleFrame();
                let left = visible.origin.x;
                let top = y
                    + (d.rect.origin.y + d.rect.size.height
                        - visible.origin.y
                        - visible.size.height);
                if (r[0] - left).abs() <= 2.
                    && (r[1] - top).abs() <= 2.
                    && (r[2] - visible.size.width).abs() <= 2.
                    && (r[3] - visible.size.height).abs() <= 2.
                {
                    app.maximized |= d.mask;
                    facts.maximized |= d.mask;
                }
            }
        }
    }
    facts.applications = apps.into_values().collect();
    facts
}
pub fn cursor(rect: NSRect) -> ([f32; 2], u32) {
    let p = NSEvent::mouseLocation();
    let uv = [
        ((p.x - rect.origin.x) / rect.size.width.max(1.)) as f32,
        (1. - (p.y - rect.origin.y) / rect.size.height.max(1.)) as f32,
    ];
    (uv, NSEvent::pressedMouseButtons() as u32)
}

/// Global pointer position is useful for parallax. Button delivery is separately
/// authorized by the topmost public WindowServer window under that pointer.
pub fn pointer_for_view(view: &NSView, desktop: bool) -> ([f32; 2], u32, bool) {
    let Some(window) = view.window() else {
        let (uv, buttons) = cursor(view.bounds());
        return (uv, buttons, false);
    };
    let rect = window.convertRectToScreen(view.bounds());
    let (uv, buttons) = cursor(rect);
    let inside = (0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1]);
    if !inside || !window.isVisible() {
        return (uv, buttons, false);
    }
    let point = NSEvent::mouseLocation();
    let main = NSScreen::screens(MainThreadMarker::new().expect("main-thread pointer"))
        .firstObject()
        .map(|s| s.frame().size.height)
        .unwrap_or(0.);
    let p = [point.x, main - point.y];
    let target = window.windowNumber() as u32;
    let finder = NSRunningApplication::runningApplicationsWithBundleIdentifier(
        &NSString::from_str("com.apple.finder"),
    )
    .firstObject()
    .map(|app| app.processIdentifier());
    let Some(windows) = CGWindowListCopyWindowInfo(CGWindowListOption::OptionOnScreenOnly, 0)
    else {
        return (uv, buttons, false);
    };
    for i in 0..windows.count() {
        let Some(dict) = (unsafe { windows.value_at_index(i).cast::<CFDictionary>().as_ref() })
        else {
            continue;
        };
        if unsafe { number(dict, kCGWindowAlpha) }.is_some_and(|a| a <= 0.) {
            continue;
        }
        let Some(bounds) = (unsafe { field(dict, kCGWindowBounds) }) else {
            continue;
        };
        let names = ["X", "Y", "Width", "Height"].map(CFString::from_str);
        let coordinates = names
            .iter()
            .map(|name| unsafe { number(bounds, name) })
            .collect::<Option<Vec<_>>>();
        let Some(r) = coordinates else {
            continue;
        };
        if p[0] < r[0] || p[1] < r[1] || p[0] >= r[0] + r[2] || p[1] >= r[1] + r[3] {
            continue;
        }
        let id = unsafe { number(dict, kCGWindowNumber) }.unwrap_or(0.) as u32;
        if id == target {
            return (uv, buttons, true);
        }
        let pid = unsafe { number(dict, kCGWindowOwnerPID) }.map(|n| n as i32);
        let level = unsafe { number(dict, kCGWindowLayer) }.unwrap_or(0.) as i32;
        let icon = CGWindowLevelForKey(CGWindowLevelKey::DesktopIconWindowLevelKey);
        let background = CGWindowLevelForKey(CGWindowLevelKey::DesktopWindowLevelKey);
        return (
            uv,
            buttons,
            desktop && pid == finder && (level == icon || level == background),
        );
    }
    (uv, buttons, false)
}

pub fn screen_identity(screen: &NSScreen) -> String {
    let number = screen
        .deviceDescription()
        .objectForKey(&NSString::from_str("NSScreenNumber"));
    let Some(number) = number else {
        return "unknown-screen".into();
    };
    let id: u32 = unsafe { msg_send![&*number, unsignedIntValue] };
    unsafe { CGDisplayCreateUUIDFromDisplayID(id) }
        .map(|p| unsafe { CFRetained::from_raw(p) })
        .and_then(|uuid| CFUUID::new_string(None, Some(&uuid)))
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("display-{id}"))
}

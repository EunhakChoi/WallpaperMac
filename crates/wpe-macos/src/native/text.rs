//! TextKit/CoreText raster output, including system cascade fonts and color emoji.
//! Pixels are supplied to the shared renderer; simulation runs only once per frame.
use objc2::{AnyThread, rc::Retained, runtime::AnyObject};
use objc2_app_kit::*;
use objc2_core_foundation::{CFArray, CFData, CFRetained, CFType};
use objc2_foundation::{
    NSAttributedString, NSDictionary, NSNumber, NSPoint, NSRect, NSSize, NSString,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::c_void,
    path::Path,
    ptr::NonNull,
};
use wpe_core::{Project, ScenePlayer};
use wpe_render::{RenderFrame, RgbaImage};
#[link(name = "CoreText", kind = "framework")]
unsafe extern "C-unwind" {
    fn CTFontManagerCreateFontDescriptorsFromData(
        data: &CFData,
    ) -> Option<NonNull<CFArray<CFType>>>;
    // CTFont and NSFont are toll-free bridged on macOS. The returned object is +1.
    fn CTFontCreateWithFontDescriptor(
        descriptor: &CFType,
        size: f64,
        matrix: *const c_void,
    ) -> Option<NonNull<NSFont>>;
}
#[derive(Default)]
pub struct NativeText {
    states: BTreeMap<String, String>,
    fonts: BTreeMap<String, Retained<NSFont>>,
}
impl NativeText {
    pub fn update(
        &mut self,
        player: &mut ScenePlayer,
        frame: &mut RenderFrame,
    ) -> Result<(), String> {
        let objects = player.scene()["objects"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut active = BTreeSet::new();
        for object in objects.iter().filter(|o| o.get("text").is_some()) {
            let id = object
                .get("id")
                .cloned()
                .ok_or("A native text layer requires an id")?;
            let key = id.to_string();
            active.insert(key.clone());
            let state = state_key(object)?;
            if self.states.get(&key) == Some(&state) {
                continue;
            }
            let image = self.raster(player.project(), object)?;
            player
                .provide_text_raster(&id, image)
                .map_err(|e| e.to_string())?;
            self.states.insert(key, state);
        }
        self.states.retain(|key, _| active.contains(key));
        player
            .apply_text_rasters(frame)
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    fn raster(&mut self, project: &Project, object: &Value) -> Result<RgbaImage, String> {
        let text = literal(&object["text"]).as_str().unwrap_or("");
        if text.len() > 65536 {
            return Err("Text exceeds64KiB".into());
        }
        if text.is_empty() {
            return Ok(RgbaImage {
                width: 1,
                height: 1,
                rgba: vec![0; 4],
            });
        }
        let size = number(object, "pointsize", 32.);
        let outline = number(object, "outline", 0.);
        let blur = number(object, "shadowblur", 0.);
        let shadow_offset = vector(object.get("shadowoffset"), [0.; 4]);
        let padding = number(object, "padding", 0.);
        if !(0. ..=4096.).contains(&size)
            || size == 0.
            || !(0. ..=256.).contains(&outline)
            || !(0. ..=85.).contains(&blur)
            || !(0. ..=4096.).contains(&padding)
            || shadow_offset[..2]
                .iter()
                .any(|v| !v.is_finite() || v.abs() > 4096.)
        {
            return Err("Invalid native text dimensions/effects".into());
        }
        let name = literal(&object["font"])
            .as_str()
            .unwrap_or("fonts/arial.ttf");
        let fontkey = format!("{name}:{size}");
        let font = if let Some(font) = self.fonts.get(&fontkey) {
            font.clone()
        } else {
            let font = if let Ok(resource) = project.read_resource(name) {
                if resource.bytes.len() > 64 * 1024 * 1024 {
                    return Err("Font exceeds64MiB".into());
                }
                let data = CFData::from_bytes(&resource.bytes);
                let array = unsafe { CTFontManagerCreateFontDescriptorsFromData(&data) }
                    .map(|p| unsafe { CFRetained::from_raw(p) })
                    .ok_or_else(|| format!("CoreText rejected font {name}"))?;
                let descriptor = array.get(0).ok_or("Font has no face descriptors")?;
                let pointer =
                    unsafe { CTFontCreateWithFontDescriptor(&descriptor, size, std::ptr::null()) }
                        .ok_or("CoreText could not create font")?;
                unsafe { Retained::from_raw(pointer.as_ptr()) }.ok_or("Null bridged font")?
            } else {
                let family = Path::new(name)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(name);
                let family = family.strip_prefix("systemfont_").unwrap_or(family);
                NSFont::fontWithName_size(&NSString::from_str(family), size)
                    .unwrap_or_else(|| NSFont::systemFontOfSize(size))
            };
            if self.fonts.len() >= 128 {
                self.fonts.clear();
            }
            self.fonts.insert(fontkey, font.clone());
            font
        };
        let paragraph = NSMutableParagraphStyle::new();
        paragraph.setAlignment(match literal(&object["horizontalalign"]).as_str() {
            Some("left") => NSTextAlignment::Left,
            Some("right") => NSTextAlignment::Right,
            Some("center") => NSTextAlignment::Center,
            _ => NSTextAlignment::Natural,
        });
        paragraph.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
        let lineheight = number(object, "lineheight", 0.);
        if lineheight > 0. {
            paragraph.setMinimumLineHeight(lineheight);
            paragraph.setMaximumLineHeight(lineheight);
        }
        let mut tint = vector(object.get("color"), [1.; 4]);
        let brightness = number(object, "brightness", 1.) as f32;
        for v in &mut tint[..3] {
            *v *= brightness;
        }
        tint[3] *= number(object, "alpha", 1.) as f32;
        let color = nscolor(tint);
        let kern = NSNumber::new_f64(number(object, "spacing", 0.));
        let stroke = NSNumber::new_f64(-outline * 100. / size);
        let outlinecolor = nscolor(vector(object.get("outlinecolor"), [0., 0., 0., 1.]));
        let shadow = NSShadow::new();
        shadow.setShadowOffset(NSSize::new(
            shadow_offset[0] as f64,
            -shadow_offset[1] as f64,
        ));
        shadow.setShadowBlurRadius(blur);
        let shadowcolor = nscolor(vector(
            object.get("shadowcolor"),
            [
                0.,
                0.,
                0.,
                if blur > 0. || shadow_offset[0] != 0. || shadow_offset[1] != 0. {
                    1.
                } else {
                    0.
                },
            ],
        ));
        shadow.setShadowColor(Some(&shadowcolor));
        let keys = unsafe {
            [
                NSFontAttributeName,
                NSForegroundColorAttributeName,
                NSParagraphStyleAttributeName,
                NSKernAttributeName,
                NSStrokeWidthAttributeName,
                NSStrokeColorAttributeName,
                NSShadowAttributeName,
            ]
        };
        let values: [&AnyObject; 7] = [
            &font,
            &color,
            &paragraph,
            &kern,
            &stroke,
            &outlinecolor,
            &shadow,
        ];
        let attributes = NSDictionary::from_slices(&keys, &values);
        let attributed = unsafe {
            NSAttributedString::initWithString_attributes(
                NSAttributedString::alloc(),
                &NSString::from_str(text),
                Some(&attributes),
            )
        };
        let wrap = boolean(object, "limitwidth", false);
        let width = if wrap {
            number(object, "maxwidth", 500.)
        } else {
            8192.
        };
        if width <= 0. || width > 8192. {
            return Err("Text wrap width must be1..8192".into());
        }
        let mut options = NSStringDrawingOptions::UsesLineFragmentOrigin
            | NSStringDrawingOptions::UsesFontLeading;
        if boolean(object, "limituseellipsis", false) {
            options |= NSStringDrawingOptions::TruncatesLastVisibleLine;
        }
        let bounds = attributed.boundingRectWithSize_options_context(
            NSSize::new(width, 8192.),
            options,
            None,
        );
        let pad = (padding
            + outline
            + blur * 3.
            + shadow_offset[0].abs().max(shadow_offset[1].abs()) as f64)
            .ceil();
        let content_width = if wrap {
            width
        } else {
            bounds.size.width.ceil().max(1.)
        };
        let rowheight = if lineheight > 0. {
            lineheight
        } else {
            size * 1.3
        };
        let content_height = if boolean(object, "limitrows", false) {
            bounds
                .size
                .height
                .min(number(object, "maxrows", 1.).max(1.) * rowheight)
        } else {
            bounds.size.height
        };
        let w = (content_width + pad * 2.).ceil().max(1.);
        let h = (content_height + pad * 2.).ceil().max(1.);
        if !w.is_finite() || !h.is_finite() || w > 16384. || h > 16384. || w * h > 16_777_216. {
            return Err("Text raster exceeds16M pixels".into());
        }
        let (w, h) = (w as u32, h as u32);
        let bitmap=unsafe{NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(NSBitmapImageRep::alloc(),std::ptr::null_mut(),w as isize,h as isize,8,4,true,false,NSDeviceRGBColorSpace,NSBitmapFormat::empty(),w as isize*4,32)}.ok_or("Could not allocate native text bitmap")?;
        let data = NonNull::new(bitmap.bitmapData()).ok_or("Text bitmap has no memory")?;
        let stride = bitmap.bytesPerRow() as usize;
        if stride < (w as usize) * 4 {
            return Err("Invalid text bitmap stride".into());
        }
        unsafe {
            std::ptr::write_bytes(data.as_ptr(), 0, stride * h as usize);
        }
        let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap)
            .ok_or("Could not create native text graphics context")?;
        NSGraphicsContext::saveGraphicsState_class();
        struct Restore;
        impl Drop for Restore {
            fn drop(&mut self) {
                NSGraphicsContext::restoreGraphicsState_class();
            }
        }
        let _restore = Restore;
        NSGraphicsContext::setCurrentContext(Some(&context));
        if boolean(object, "opaquebackground", false) {
            let mut c = vector(object.get("backgroundcolor"), [0., 0., 0., 1.]);
            for v in &mut c[..3] {
                *v *= number(object, "backgroundbrightness", 1.) as f32;
            }
            c[3] = 1.;
            nscolor(c).setFill();
            NSRectFill(NSRect::new(
                NSPoint::new(0., 0.),
                NSSize::new(w as f64, h as f64),
            ));
        }
        attributed.drawWithRect_options_context(
            NSRect::new(
                NSPoint::new(pad, pad),
                NSSize::new(content_width, content_height.ceil().max(1.)),
            ),
            options,
            None,
        );
        context.flushGraphics();
        let source = unsafe { std::slice::from_raw_parts(data.as_ptr(), stride * h as usize) };
        let mut rgba = vec![0; w as usize * h as usize * 4];
        // NSBitmapImageRep's packed RGBA rows are copied with its explicit stride;
        // AppKit draws with its own text cascade rather than a single outline font.
        for y in 0..h as usize {
            let row = &source[y * stride..y * stride + w as usize * 4];
            let dest = &mut rgba[y * w as usize * 4..(y + 1) * w as usize * 4];
            for (p, q) in row.chunks_exact(4).zip(dest.chunks_exact_mut(4)) {
                q[3] = p[3];
                for c in 0..3 {
                    q[c] = if p[3] == 0 {
                        0
                    } else {
                        ((p[c] as u32 * 255 + p[3] as u32 / 2) / p[3] as u32).min(255) as u8
                    };
                }
            }
        }
        Ok(RgbaImage {
            width: w,
            height: h,
            rgba,
        })
    }
}
fn literal(v: &Value) -> &Value {
    v.get("value").unwrap_or(v)
}
fn number(v: &Value, key: &str, default: f64) -> f64 {
    literal(&v[key])
        .as_f64()
        .filter(|v| v.is_finite())
        .unwrap_or(default)
}
fn boolean(v: &Value, key: &str, default: bool) -> bool {
    literal(&v[key]).as_bool().unwrap_or(default)
}
fn vector(v: Option<&Value>, default: [f32; 4]) -> [f32; 4] {
    let Some(v) = v.map(literal) else {
        return default;
    };
    let values = if let Some(s) = v.as_str() {
        s.split_whitespace()
            .filter_map(|s| s.parse::<f32>().ok())
            .collect::<Vec<_>>()
    } else {
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_f64().map(|v| v as f32))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut out = default;
    for (i, v) in values.into_iter().take(4).enumerate() {
        if v.is_finite() {
            out[i] = v;
        }
    }
    out
}
fn nscolor(c: [f32; 4]) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        c[0].clamp(0., 1.) as f64,
        c[1].clamp(0., 1.) as f64,
        c[2].clamp(0., 1.) as f64,
        c[3].clamp(0., 1.) as f64,
    )
}
fn state_key(object: &Value) -> Result<String, String> {
    let keys = [
        "text",
        "font",
        "pointsize",
        "lineheight",
        "spacing",
        "limitwidth",
        "maxwidth",
        "limitrows",
        "maxrows",
        "limituseellipsis",
        "horizontalalign",
        "verticalalign",
        "blockalign",
        "padding",
        "outline",
        "outlinecolor",
        "shadowoffset",
        "shadowblur",
        "shadowcolor",
        "opaquebackground",
        "backgroundcolor",
        "backgroundbrightness",
        "alpha",
        "color",
        "brightness",
    ];
    let values = keys
        .into_iter()
        .map(|key| (key, literal(&object[key])))
        .collect::<BTreeMap<_, _>>();
    serde_json::to_string(&values).map_err(|e| e.to_string())
}

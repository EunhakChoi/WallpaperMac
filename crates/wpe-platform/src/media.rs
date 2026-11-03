//! Public opt-in media metadata adapters and bounded album artwork conversion.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Cursor};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaSource {
    #[default]
    Disabled,
    Owned,
    Music,
    Spotify,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaSettings {
    pub source: MediaSource,
    pub poll_seconds: u32,
    pub artwork: bool,
}
impl Default for MediaSettings {
    fn default() -> Self {
        Self {
            source: MediaSource::Disabled,
            poll_seconds: 1,
            artwork: true,
        }
    }
}
impl MediaSettings {
    pub fn validate(&self) -> Result<()> {
        if !(1..=60).contains(&self.poll_seconds) {
            return Err(Error("Media poll interval must be1..60seconds".into()));
        }
        Ok(())
    }
}
pub fn unavailable(enabled: bool) -> Value {
    json!({"status":{"enabled":enabled},"playback":{"state":0},"properties":{"title":"","artist":"","subTitle":"","albumTitle":"","albumArtist":"","genres":"","contentType":"music"},"timeline":{"position":0.,"duration":0.},"thumbnail":{"hasThumbnail":false,"thumbnail":""}})
}
pub fn from_transport(v: &Value, enabled: bool) -> Result<Value> {
    if let Some(error) = v["error"].as_str() {
        return Err(Error(error.chars().take(4096).collect()));
    }
    let mut result = unavailable(enabled);
    if !v["available"].as_bool().unwrap_or(false) {
        return Ok(result);
    }
    let state = match v["state"].as_str().unwrap_or("stopped") {
        "playing" => 1,
        "paused" => 2,
        _ => 0,
    };
    result["playback"] = json!({"state":state});
    for (dst, src) in [
        ("title", "title"),
        ("artist", "artist"),
        ("albumTitle", "album"),
        ("albumArtist", "album_artist"),
        ("genres", "genre"),
    ] {
        if let Some(s) = v[src].as_str() {
            result["properties"][dst] = json!(s.chars().take(16384).collect::<String>());
        }
    }
    let position = v["position"]
        .as_f64()
        .filter(|v| v.is_finite() && *v >= 0.)
        .unwrap_or(0.);
    let duration = v["duration"]
        .as_f64()
        .filter(|v| v.is_finite() && *v >= 0.)
        .unwrap_or(0.);
    result["timeline"] = json!({"position":position,"duration":duration});
    Ok(result)
}
pub fn thumbnail(bytes: &[u8]) -> Result<Value> {
    if bytes.len() > 5 * 1024 * 1024 {
        return Err(Error("Media artwork exceeds5MiB".into()));
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| Error(e.to_string()))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(|e| Error(e.to_string()))?;
    let image = if image.width() > 256 || image.height() > 256 {
        image.thumbnail(256, 256)
    } else {
        image
    }
    .to_rgba8();
    let mut bins = BTreeMap::<u16, (u64, [u64; 3])>::new();
    for p in image.pixels() {
        if p[3] < 32 {
            continue;
        }
        let key = ((p[0] as u16 >> 4) << 8) | ((p[1] as u16 >> 4) << 4) | (p[2] as u16 >> 4);
        let entry = bins.entry(key).or_default();
        entry.0 += p[3] as u64;
        for i in 0..3 {
            entry.1[i] += p[i] as u64 * p[3] as u64;
        }
    }
    let mut palette = bins.values().copied().collect::<Vec<_>>();
    palette.sort_by_key(|v| std::cmp::Reverse(v.0));
    let colors = palette
        .iter()
        .take(3)
        .map(|(weight, sum)| sum.map(|v| v as f32 / (*weight).max(1) as f32 / 255.))
        .collect::<Vec<_>>();
    let primary = colors.first().copied().unwrap_or([0.; 3]);
    let secondary = colors.get(1).copied().unwrap_or(primary);
    let tertiary = colors.get(2).copied().unwrap_or(secondary);
    let luminance = primary.map(|v| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    });
    let luminance = luminance[0] * 0.2126 + luminance[1] * 0.7152 + luminance[2] * 0.0722;
    let high = if (1.05 / (luminance + 0.05)) >= ((luminance + 0.05) / 0.05) {
        [1.; 3]
    } else {
        [0.; 3]
    };
    let mut png = Cursor::new(vec![]);
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| Error(e.to_string()))?;
    Ok(
        json!({"hasThumbnail":true,"thumbnail":format!("data:image/png;base64,{}",base64(png.get_ref())),"primaryColor":primary,"secondaryColor":secondary,"tertiaryColor":tertiary,"textColor":high,"highContrastColor":high}),
    )
}
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let bits = (chunk[0] as u32) << 16
            | (chunk.get(1).copied().unwrap_or(0) as u32) << 8
            | chunk.get(2).copied().unwrap_or(0) as u32;
        for i in 0..4 {
            output.push(if i > chunk.len() {
                '='
            } else {
                TABLE[((bits >> (18 - i * 6)) & 63) as usize] as char
            });
        }
    }
    output
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_image_pixels_make_png_thumbnail_and_contrast_palette() {
        let image = image::RgbaImage::from_pixel(3, 2, image::Rgba([255, 0, 0, 255]));
        let mut data = Cursor::new(vec![]);
        image.write_to(&mut data, image::ImageFormat::Png).unwrap();
        let t = thumbnail(data.get_ref()).unwrap();
        assert_eq!(t["primaryColor"], json!([1., 0., 0.]));
        assert_eq!(t["highContrastColor"], json!([0., 0., 0.]));
        assert!(
            t["thumbnail"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,iVBOR")
        );
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert!(thumbnail(b"not image").is_err());
    }
    #[test]
    fn transport_preserves_actual_states_and_rejects_permission_error() {
        let v=from_transport(&json!({"available":true,"state":"paused","title":"Song","position":2.25,"duration":10.}),true).unwrap();
        assert_eq!(v["playback"]["state"], 2);
        assert_eq!(v["timeline"]["position"], 2.25);
        assert_eq!(v["properties"]["title"], "Song");
        assert!(from_transport(&json!({"error":"Automation permission denied"}), true).is_err());
        assert_eq!(unavailable(false)["status"]["enabled"], false);
    }
}

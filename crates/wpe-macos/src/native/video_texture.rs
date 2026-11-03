//! Real AVFoundation decoded video frames for scene textures.
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_av_foundation::{
    AVMediaTypeVideo, AVPlayer, AVPlayerItem, AVPlayerItemStatus, AVPlayerItemVideoOutput,
};
use objc2_core_media::CMTime;
use objc2_core_video::*;
use objc2_foundation::{MainThreadMarker, NSDictionary, NSNumber, NSString, NSURL};
use serde_json::{Value, json};
use std::{
    cell::Cell,
    collections::BTreeMap,
    io::Write,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use wpe_core::ScenePlayer;
use wpe_formats::RgbaImage;
struct VideoTexture {
    player: Retained<AVPlayer>,
    item: Retained<AVPlayerItem>,
    output: Retained<AVPlayerItemVideoOutput>,
    path: PathBuf,
    playing: bool,
    looped: bool,
    rate: f32,
    last_feedback: f64,
}
impl VideoTexture {
    fn new(name: &str, bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 12 || bytes.len() > 256 * 1024 * 1024 {
            return Err("Video payload is empty/truncated or exceeds256MiB".into());
        }
        let extension = if matches!(
            &bytes[4..8],
            b"ftyp" | b"moov" | b"mdat" | b"wide" | b"free"
        ) {
            "mov"
        } else if bytes[..4] == [0x1a, 0x45, 0xdf, 0xa3] {
            "webm"
        } else if &bytes[..4] == b"RIFF" && &bytes[8..12] == b"AVI " {
            "avi"
        } else {
            return Err(format!(
                "{name}: embedded payload has no recognized video container signature"
            ));
        };
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory =
            std::env::temp_dir().join(format!("WallpaperMac-Video-{}", std::process::id()));
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let path = directory.join(format!(
            "{}.{}",
            NEXT.fetch_add(1, Ordering::Relaxed),
            extension
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        if let Err(e) = file.write_all(bytes) {
            let _ = std::fs::remove_file(&path);
            return Err(e.to_string());
        }
        drop(file);
        let mtm =
            MainThreadMarker::new().ok_or("Video texture must be created on the main thread")?;
        let url = NSURL::fileURLWithPath_isDirectory(
            &NSString::from_str(path.to_str().ok_or("Video cache path is not UTF-8")?),
            false,
        );
        unsafe {
            let key = NSString::from_str(&kCVPixelBufferPixelFormatTypeKey.to_string());
            let format = NSNumber::numberWithUnsignedInt(kCVPixelFormatType_32BGRA);
            let attributes =
                NSDictionary::<NSString, AnyObject>::from_slices(&[&*key], &[&*format]);
            let output = AVPlayerItemVideoOutput::initWithPixelBufferAttributes(
                AVPlayerItemVideoOutput::alloc(),
                Some(&attributes),
            );
            output.setSuppressesPlayerRendering(true);
            let item = AVPlayerItem::playerItemWithURL(&url, mtm);
            item.addOutput(&output);
            let player = AVPlayer::playerWithPlayerItem(Some(&item), mtm);
            player.setMuted(true);
            Ok(Self {
                player,
                item,
                output,
                path,
                playing: true,
                looped: true,
                rate: 1.,
                last_feedback: 0.,
            })
        }
    }
    fn state(&mut self, metadata: &Value, paused: bool) {
        self.playing = metadata["__playing"].as_bool().unwrap_or(true);
        self.looped = metadata["loop"].as_bool().unwrap_or(true);
        self.rate = metadata["rate"]
            .as_f64()
            .map(|v| v as f32)
            .filter(|v| v.is_finite() && *v >= 0.)
            .unwrap_or(1.);
        unsafe {
            if let Some(time) = metadata["__time"]
                .as_f64()
                .filter(|t| t.is_finite() && *t >= 0. && (*t - self.last_feedback).abs() > 1e-6)
            {
                let zero = CMTime::with_seconds(0., 60000);
                self.player.seekToTime_toleranceBefore_toleranceAfter(
                    CMTime::with_seconds(time, 60000),
                    zero,
                    zero,
                );
            }
            if paused || !self.playing {
                self.player.pause();
            } else {
                self.player.setRate(self.rate);
            }
        }
    }
    #[allow(deprecated)]
    fn metadata(&mut self) -> Value {
        unsafe {
            let duration = self.item.duration().seconds();
            let fps = AVMediaTypeVideo
                .and_then(|kind| self.item.asset().tracksWithMediaType(kind).firstObject())
                .map(|track| track.nominalFrameRate())
                .filter(|fps| fps.is_finite() && *fps > 0.);
            self.last_feedback = self.player.currentTime().seconds().max(0.);
            json!({"duration":if duration.is_finite(){Some(duration)}else{None},"fps":fps,"frameCount":fps.filter(|_|duration.is_finite()).map(|fps|(duration*fps as f64).round()as u64),"frameCountIsNominalEstimate":true,"__time":self.last_feedback,"__playing":self.playing,"loop":self.looped,"rate":self.rate})
        }
    }
    fn pull(&mut self) -> Result<Option<RgbaImage>, String> {
        unsafe {
            if self.item.status() == AVPlayerItemStatus::Failed {
                return Err(self
                    .item
                    .error()
                    .map(|e| e.localizedDescription().to_string())
                    .unwrap_or_else(|| "AVFoundation video decode failed".into()));
            }
            let mut time = self.player.currentTime();
            let duration = self.item.duration().seconds();
            if self.looped
                && self.playing
                && duration.is_finite()
                && duration > 0.
                && time.seconds() >= duration - 0.002
            {
                self.player.seekToTime(CMTime::with_seconds(0., 60000));
                time = CMTime::with_seconds(0., 60000);
                self.player.setRate(self.rate);
            }
            if !self.output.hasNewPixelBufferForItemTime(time) {
                return Ok(None);
            }
            let Some(buffer) = self
                .output
                .copyPixelBufferForItemTime_itemTimeForDisplay(time, std::ptr::null_mut())
            else {
                return Ok(None);
            };
            let flags = CVPixelBufferLockFlags::ReadOnly;
            if CVPixelBufferLockBaseAddress(&buffer, flags) != 0 {
                return Err("Video pixel buffer could not be locked".into());
            }
            struct Unlock<'a>(&'a CVPixelBuffer);
            impl Drop for Unlock<'_> {
                fn drop(&mut self) {
                    unsafe {
                        CVPixelBufferUnlockBaseAddress(self.0, CVPixelBufferLockFlags::ReadOnly);
                    }
                }
            }
            let _unlock = Unlock(&buffer);
            let (w, h, stride) = (
                CVPixelBufferGetWidth(&buffer),
                CVPixelBufferGetHeight(&buffer),
                CVPixelBufferGetBytesPerRow(&buffer),
            );
            let bytes = w
                .checked_mul(h)
                .and_then(|n| n.checked_mul(4))
                .filter(|n| *n <= wpe_formats::MAX_ALLOCATION)
                .ok_or("Video frame exceeds allocation policy")?;
            if w == 0
                || h == 0
                || stride < w * 4
                || CVPixelBufferGetPixelFormatType(&buffer) != kCVPixelFormatType_32BGRA
            {
                return Err("Video output has invalid dimensions or non-BGRA pixel layout".into());
            }
            let base = CVPixelBufferGetBaseAddress(&buffer).cast::<u8>();
            if base.is_null() {
                return Err("Video output has no CPU base address".into());
            }
            let mut rgba = vec![0; bytes];
            for y in 0..h {
                let row = std::slice::from_raw_parts(base.add(y * stride), w * 4);
                for (x, p) in row.chunks_exact(4).enumerate() {
                    rgba[(y * w + x) * 4..(y * w + x + 1) * 4]
                        .copy_from_slice(&[p[2], p[1], p[0], p[3]]);
                }
            }
            Ok(Some(
                RgbaImage::new(w as u32, h as u32, rgba).map_err(|e| e.to_string())?,
            ))
        }
    }
}
impl Drop for VideoTexture {
    fn drop(&mut self) {
        unsafe {
            self.player.pause();
            self.item.removeOutput(&self.output);
            self.player.replaceCurrentItemWithPlayerItem(None);
        }
        let _ = std::fs::remove_file(&self.path);
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}
#[derive(Default)]
pub struct VideoTextures {
    videos: BTreeMap<String, VideoTexture>,
    failed: BTreeMap<String, String>,
    paused: Cell<bool>,
}
impl VideoTextures {
    pub fn set_paused(&self, paused: bool) {
        self.paused.set(paused);
        for video in self.videos.values() {
            unsafe {
                if paused {
                    video.player.pause();
                } else if video.playing {
                    video.player.setRate(video.rate);
                }
            }
        }
    }
    pub fn update(&mut self, player: &mut ScenePlayer) -> Vec<String> {
        let mut errors = vec![];
        for (name, bytes) in player.drain_video_requests() {
            if self.videos.contains_key(&name) || self.failed.contains_key(&name) {
                continue;
            }
            match VideoTexture::new(&name, &bytes) {
                Ok(mut video) => {
                    video.state(&json!({"__playing":true}), self.paused.get());
                    self.videos.insert(name, video);
                }
                Err(error) => {
                    self.failed.insert(name, error.clone());
                    errors.push(error);
                }
            }
        }
        let objects = player.scene()["objects"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut feedback = vec![];
        for object in objects {
            let Some(metadata) = object.get("__videoTexture") else {
                continue;
            };
            let Some(name) = metadata["resource"].as_str() else {
                continue;
            };
            if let Some(video) = self.videos.get_mut(name) {
                video.state(metadata, self.paused.get());
                let mut value = video.metadata();
                value["resource"] = json!(name);
                feedback.push((object["id"].clone(), json!({"__videoTexture":value})));
            }
        }
        let mut failed = vec![];
        for (name, video) in &mut self.videos {
            match video.pull() {
                Ok(Some(frame)) => {
                    if let Err(e) = player.provide_texture_frame(name, frame) {
                        errors.push(e.to_string());
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    errors.push(format!("{name}: {error}"));
                    failed.push(name.clone());
                }
            }
        }
        for name in failed {
            self.videos.remove(&name);
            self.failed.insert(name, "decoder failed".into());
        }
        for (id, value) in feedback {
            if let Err(error) = player.apply_provider_state(&id, &value) {
                errors.push(error.to_string());
            }
        }
        errors
    }
}
